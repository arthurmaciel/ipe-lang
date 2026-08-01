# Ipê Playground — local setup and `/run` security model

The playground is a split-pane browser UI (Ipê source left, emitted Rust
right). The Ipê → Rust pipeline runs **in the browser** as a WebAssembly
module (`ipe-wasm`); clicking **Run** (or `Ctrl+Enter` / `Cmd+Enter`) posts
the emitted Rust to a local Ipê server, which builds and executes it inside a
bubblewrap jail and returns the real stdout/stderr/exit — no page reload.

## Prerequisites

| Requirement | Why |
|---|---|
| `ipe` compiler binary | the build program and the server run as Ipê programs (`cargo build -p ipe` builds it) |
| `bwrap`, `timeout`, `prlimit` | the `/run` jail primitives (Linux) |
| `wasm32-unknown-unknown` target + `wasm-bindgen` CLI | only when rebuilding the browser bundle (`build/`) |

## Build the browser bundle

```sh
cd examples/wasm/language-playground/build
ipe run
```

`build/src/Main.ipe` probes `git`/`cargo`/`rustup`/`wasm-bindgen`, adds the
wasm target, builds the `ipe-wasm` crate (resolving the target dir via cargo
metadata so `CARGO_TARGET_DIR` and cargo config are honoured), runs
`wasm-bindgen` into `../pkg/`, and prints the server run hint.

## Run the server

```sh
cd examples/wasm/language-playground/server
ipe run
```

`server/src/Main.ipe` is an `Ipe.Http.Server` app on port **8000** serving the
playground root and `/pkg` statically, plus `POST /run`. Open
http://localhost:8000.

The first `/run` needs a warm cargo cache (see below).

## `POST /run` — sandboxed native build + execute

`/run` accepts the **emitted Rust** (not Ipê source — the in-browser WASM
compiler produced it, and the wire includes the whole stdlib closure under
`src/ipe_runtime/*`, ~2.6 MiB for a typical program; 16 MiB is the payload
cap). The server stages it into a fresh project dir under
`~/.cache/ipe/playground-runs/<token>/` and hands it to the **jail-runner**
workspace member (`examples/wasm/language-playground/jail-runner`), a
`argv in, JSON out` harness:

```
jail-runner run <project-dir> [--wall N] [--warm <dir>]
jail-runner prewarm [--warm <dir>]
```

Exit codes: `0` whenever a JSON document was printed, `1` on a crash (no
JSON), `2` on usage errors or harness wall-clock expiry.

### Pipeline

1. **Stage** (trusted) — `Runner.ipe` splits the banner-delimited emitted
   Rust into `Cargo.toml` + `src/main.rs` under the token project dir and
   execs `jail-runner run <dir> --wall 300 --warm <warm>` via `Process.run`
   (a direct argv vector, no shell). The project dir is removed after the
   run (also from the harness watchdog).
2. **Build** (jailed) — `cargo build --offline` of the emitted crate inside
   the jail. Building runs dependency build scripts + proc-macros, so it is
   confined; subprocess creation is *allowed* here (rustc + the linker
   legitimately spawn), but network/filesystem/resource confinement apply.
3. **Run** (jailed, hardened) — the emitted `ipe-app` binary runs inside the
   jail **plus** a seccomp filter that denies subprocess creation. This is
   where the user's own program executes.

The build is fully offline. `jail-runner prewarm` builds the fixed dependency
closure (identical for every program — the runtime fixes the manifest) once
with network on; each request seeds the prebuilt registry + target into its
project dir and compiles only the user's own crate. **User-derived code never
has network access, at build time or run time.**

### Fail-closed

If the host lacks a jail primitive (`bwrap`, `timeout`, or `prlimit`),
`probe_or_refuse()` returns a refusal that **names the sandbox** and `/run`
fails closed — it never falls back to an unsandboxed build or run. The only
explicit escape hatch is `IPE_FFI_ALLOW_UNSANDBOXED=1`, which the harness
honours only after printing a loud trust-boundary warning (output capped at
64 KiB, the harness wall-clock still enforced); the wire marks such runs
`"unsandboxed": true`.

### Threat model — which knob enforces each control

Every build and run of user-derived code is confined by `ipe_sandbox` (the
same crate the compiler SEAL uses). The enforcing knob for each control
(proven by the tests in
`examples/wasm/language-playground/jail-runner/tests/sandbox_security.rs`):

| Control | Enforcer | Proving test |
|---|---|---|
| **Network** off | `NetworkPolicy::Denied` → bwrap `--unshare-net` (a fresh empty net namespace — no route exists), plus `cargo --offline` at build | `network_access_is_denied` |
| **Filesystem** jailed | bwrap `--ro-bind / /` + `--tmpfs /home /root /tmp` + a single writable `--bind` (the project dir); the read-only toolchain binds (`~/.cargo/bin`, `~/.rustup`) are re-exposed past the tmpfs masks — never the `~/.cargo` parent, which holds `credentials.toml` | `out_of_jail_filesystem_read_is_denied` |
| **Memory** cap | `prlimit --as` (run phase: 512 MiB; build: 6 GiB) | resource-cap defaults |
| **CPU** cap | `prlimit --cpu` (run: 5 s; build: 900 s) | resource-cap defaults |
| **File descriptors** | `prlimit --nofile` (run: 64; build: 512) | resource-cap defaults |
| **Subprocess** confined (run) | seccomp `subprocess_deny_program` (denies `fork`/`vfork` and non-thread `clone`; see the `clone3` note below), bwrap `--unshare-pid`, and `prlimit --nproc` as a fork-bomb ceiling (run: 32 — threads count, so >1 lets the tokio runtime start) | `a_spawned_subprocess_cannot_escape_the_jail`, `a_fork_bomb_is_bounded_not_unbounded` |
| **Wall time** kill | `timeout --kill-after=5s <wall>` (SIGTERM then SIGKILL), plus a harness watchdog (`--die-with-parent` reaps the bwrap tree) | `infinite_loop_is_killed_by_the_time_limit` |
| **Output** bound | the jail's bounded stdout/stderr read (`out_cap_bytes`, also `prlimit --fsize`: run 8 MiB, build 512 MiB) | — |
| **Sandbox absent** | refuse — never an unsandboxed run | `probe_refusal_names_the_sandbox` |

A benign program (`main = Io.println "hello"`) builds, runs sandboxed, and
returns its real stdout — proven by `hello_world_runs_and_returns_stdout`.

### The subprocess control is confinement, not absolute denial

The run jail's seccomp filter (hand-assembled in
`src/compiler/sandbox/src/seccomp.rs`) denies `fork`/`vfork` and legacy
`clone` unless the flags are exactly a thread create (`CLONE_VM | CLONE_THREAD`),
but **not `clone3`** — which modern glibc's `posix_spawn` uses. `clone3` is
allowed unconditionally because the emitted program's tokio runtime creates
its threads through `clone3`, and a seccomp classifier cannot inspect
`clone3`'s pointer-borne flags to tell a thread-create from a process-create.
So a run *can* start a subprocess.

This is sound because a spawned child inherits the exact same bubblewrap
confinement — the fresh empty net namespace (no egress), the read-only root
with the project dir as the only writable mount, the same seccomp filter, and
the `prlimit` caps — so it gains **no capability the parent lacked**. A fork
bomb is bounded by `--nproc` + the wall clock; a child cannot reach the
network or read a host file outside the jail. The security boundary is the
bubblewrap namespace + resource caps, not the seccomp fork-deny (which is a
best-effort narrowing of the common paths). Both properties are proven by the
tests named above.

### The warm cache

The warm cache (default `$IPE_PLAYGROUND_WARM_DIR` or
`~/.cache/ipe/playground-warm`) is a dedicated playground cache — never the
operator's `~/.cargo`, so it carries no credentials:

- `jail-runner prewarm` builds the crate-template hello project with
  `CARGO_HOME=<warm>/cargo-home` and target `<warm>/crate-target`, and saves
  the resolved `Cargo.lock` next to them.
- Each `run` seeds the project dir from it: the registry index is *copied*,
  the dep artifacts are *hard-linked* (same filesystem — cheap), and the
  warm `Cargo.lock` is provisioned into the project. With the lock present,
  `cargo build --offline` never re-resolves from the registry index, so the
  jailed cargo cannot prune the hard-linked sparse-index entries (observed
  failure without it: "no matching package named `X` found" on later runs).

### Running the security suite

The load-bearing proofs run under `IPE_PLAYGROUND_E2E=1` (Linux — the bwrap
jail is proven there). They drive the *same* `run_jailed` helpers the server
uses, with real Ipê programs emitted by the trusted `ipe` binary
(`IPE_BIN`, else `<CARGO_TARGET_DIR>/debug/ipe`) and hand-authored
adversarial Rust (the stronger probe — it actively attempts to escape):

```sh
IPE_PLAYGROUND_E2E=1 cargo test -p playground-jail-runner --test sandbox_security -- --test-threads 1
```

The suite reuses a warm build cache (`IPE_PLAYGROUND_WARM_TARGET`, else
`<tmp>/ipe-playground-test-warm`) and takes a few minutes cold; later runs
are much faster.

## Wire shapes

- `POST /run` body: `{ "rust": "<emitted Rust project text>" }`.
- `jail-runner` JSON out:
  `{ ok, unsandboxed, build: {status,stdout,stderr,killed}|null, run: …, exit, error }`.
- Server response: `{ ok, unsandboxed, output }` — `output` is the formatted
  `── Build ──` / `── Run ──` / `── Error ──` transcript.
