# Ipê Playground — local setup

The playground is a split-pane browser UI (editor left, live preview right).
Clicking **Run** (or pressing `Ctrl+Enter` / `Cmd+Enter`) sends the source text
to the server, which compiles it to WASM and streams the bundle back into the
preview iframe — no page reload needed.

## Prerequisites

| Requirement | Why |
|---|---|
| Rust toolchain (`stable`) | builds the playground server and `ipe` itself |
| `wasm-pack` + `wasm32-unknown-unknown` target | `ipe build --target wasm` calls `wasm-pack` internally |
| `musl-tools` (Linux) | only needed for `--static`; not required for WASM |

Install the WASM toolchain once:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

## Build

```sh
# 1. Build the ipe compiler binary.
cargo build --release -p ipe

# 2. Build the playground server.
cargo build --release -p ipe-playground
```

Both binaries land in `target/release/`.

## Run

The server needs three environment variables:

| Variable | What it points to |
|---|---|
| `IPE_BIN` | absolute path to the `ipe` binary |
| `IPE_RUNTIME_DIR` | absolute path to `src/runtime/rust/src` |
| `IPE_PLAYGROUND_STATIC_DIR` | directory that holds `index.html` (the playground UI) |

```sh
export IPE_BIN="$(pwd)/target/release/ipe"
export IPE_RUNTIME_DIR="$(pwd)/src/runtime/rust/src"
export IPE_PLAYGROUND_STATIC_DIR="$(pwd)/examples/wasm/language-playground/server/www"

./target/release/ipe-playground
# Listening on 0.0.0.0:3000
```

Open `http://localhost:3000` in a browser.

### Optional tuning

| Variable | Default | Effect |
|---|---|---|
| `IPE_PLAYGROUND_PORT` | `3000` | server port |
| `IPE_PLAYGROUND_TARGET_DIR` | `/tmp/ipe-playground-target` | shared warm cargo target (keep it across restarts for fast recompiles) |
| `IPE_PLAYGROUND_TIMEOUT_SECS` | `120` | per-compile subprocess timeout |

Set `IPE_PLAYGROUND_TARGET_DIR` to a persistent path to avoid cold recompiles
on every restart:

```sh
export IPE_PLAYGROUND_TARGET_DIR="$HOME/.cache/ipe/playground-target"
```

## Edit-compile-preview loop

1. Edit Ipê source in the left pane (or paste any `.ipe` program).
2. Press **Run** (or `Ctrl+Enter` / `Cmd+Enter`).
3. The server compiles the source to WASM and injects the bundle into the
   preview iframe on the right.
4. Compile errors appear in the status bar; the preview shows the formatted
   diagnostics.

The first compile is slow (cargo builds all dependencies). Subsequent compiles
reuse the warm target directory and are significantly faster.

## Architecture note

Each compile runs `ipe build --target wasm` as an isolated subprocess.
CPU/memory limits are the responsibility of the operating environment
(cgroups / container runtime). The server itself imposes no kernel-level
sandboxing on the WASM compile — the WASM runs in the browser's own sandbox, not
on the server. Source payloads larger than 1 MiB are rejected before a subprocess
is spawned.

## `POST /run` — sandboxed native build + execute

`/run` is the study-tool endpoint: it takes untrusted Ipê source, compiles it to a
native Rust crate, **builds that crate and runs the resulting binary on the
server**, and returns the real stdout/stderr/exit/timing. Building and running
attacker-derived code is a remote-code-execution surface, so every build and every
run happens inside the `ipe_sandbox` bubblewrap jail. This endpoint is
**Linux/x86_64 only** (the jail is proven there); on any other host it fails
closed.

### Pipeline

1. **Emit** (trusted) — `ipe build <src>` runs the project's own compiler over the
   source. This is deterministic codegen, not execution of the user's program, so
   it is a plain timeout-bounded subprocess, not jailed.
2. **Build** (jailed) — `cargo build --offline` of the emitted crate inside the
   jail. Building runs dependency build scripts + proc-macros, so it is confined.
   Subprocess creation is *allowed* here (rustc + the linker legitimately spawn),
   but network/filesystem/resource confinement all apply.
3. **Run** (jailed, hardened) — the emitted `ipe-app` binary runs inside the jail
   **plus** a seccomp filter that denies subprocess creation. This is where the
   user's own program executes.

The build is fully offline. The fixed dependency closure (identical for every
program — the runtime fixes the manifest) is pre-warmed once at startup with a
network-on `cargo build`; each request seeds the prebuilt registry + target into
its jail and compiles only the user's own crate. **User-derived code never has
network access, at build time or run time.**

### Fail-closed

If the host lacks a jail primitive (`bwrap`, `timeout`, or `prlimit`), or if the
startup pre-warm fails, `/run` **refuses** with a clear message. It never falls
back to an unsandboxed build or run. The per-request scratch directory is the only
writable mount and is deleted after the request.

### Threat model — which knob enforces each control

Every build and run of user-derived code is confined by `ipe_sandbox`. The
enforcing knob for each control (proven by the tests in
`examples/wasm/language-playground/server/tests/sandbox_security.rs`):

| Control | Enforcer | Proving test |
|---|---|---|
| **Network** off | `NetworkPolicy::Denied` → bwrap `--unshare-net` (a fresh empty net namespace — no route exists), plus `cargo --offline` at build | `network_access_is_denied` |
| **Filesystem** jailed | bwrap `--ro-bind / /` + `--tmpfs /home /root /tmp` + a single `--bind <scratch>`; the only writable/visible mount is the per-request scratch | `out_of_jail_filesystem_read_is_denied` |
| **Memory** cap | `prlimit --as` (address space) | resource-cap defaults |
| **CPU** cap | `prlimit --cpu` | resource-cap defaults |
| **Subprocess** confined (run) | seccomp `subprocess_deny_program` (denies legacy `fork`/`vfork`/`clone`; see the `clone3` note below), bwrap `--unshare-pid`, and `prlimit --nproc` as a fork-bomb ceiling | `a_spawned_subprocess_cannot_escape_the_jail`, `a_fork_bomb_is_bounded_not_unbounded` |
| **Wall time** kill | `timeout --kill-after=5s <wall>` (SIGTERM then SIGKILL) | `infinite_loop_is_killed_by_the_time_limit` |
| **Output** bound | the jail's bounded stdout/stderr read (`out_cap_bytes`, also `prlimit --fsize`) | — |
| **Sandbox absent** | refuse — never an unsandboxed run | `probe_refusal_names_the_sandbox` |

A benign program (`main = Io.println "hello"`) builds, runs sandboxed, and returns
its real stdout — proven by `hello_world_runs_and_returns_stdout`.

### The subprocess control is confinement, not absolute denial

The run jail's seccomp filter denies the legacy `fork`/`vfork`/`clone` subprocess
syscalls, but **not `clone3`** — which modern glibc's `posix_spawn` uses. `clone3`
is allowed unconditionally because the emitted program's tokio runtime creates its
threads through `clone3`, and a seccomp classifier cannot inspect `clone3`'s
pointer-borne flags to tell a thread-create from a process-create. So a run *can*
start a subprocess.

This is sound because a spawned child inherits the exact same bubblewrap
confinement — the fresh empty net namespace (no egress), the read-only root with
the scratch as the only writable mount, and the `prlimit` caps — so it gains **no
capability the parent lacked**. A fork bomb is bounded by `--nproc` + the wall
clock; a child cannot reach the network or read a host file outside the jail. The
security boundary is the bubblewrap namespace + resource caps, not the seccomp
fork-deny (which is a best-effort narrowing of the common paths). Both properties
are proven by the tests named above.

### Running the study tool locally

Serve the merged client UI (`examples/wasm/language-playground`) from the run
server so Run posts to the same origin:

```sh
cargo build --release -p ipe
cargo build --release -p ipe-playground

export IPE_BIN="$(pwd)/target/release/ipe"
export IPE_RUNTIME_DIR="$(pwd)/src/runtime/rust/src"
export IPE_PLAYGROUND_STATIC_DIR="$(pwd)/examples/wasm/language-playground"

./target/release/ipe-playground   # first start pre-warms the dep closure (slow)
```

Open `http://localhost:3000`. The left pane shows the emitted Rust live (in-browser
WASM); pressing **Run** (or `Ctrl/Cmd+Enter`) builds and runs the program in the
sandbox and shows the real output. The security tests run under
`IPE_PLAYGROUND_E2E=1` (Linux/x86_64 only).
