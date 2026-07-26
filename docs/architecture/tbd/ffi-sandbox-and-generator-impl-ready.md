# FFI sandbox (#41) + consumer generator (#42) — implementation-ready

> **Status:** implementation-ready deepening. Doc-only. Does **not** redesign the
> threat model or the milestone DAG — those are settled in
> [`ffi-port-spec.md`](./ffi-port-spec.md) (§A sandbox, §C M-A..M-G) and
> [`ffi-subsystem-design.md`](./ffi-subsystem-design.md) (D1..D8, R0.1..R0.5).
> This document lowers the two security-critical slices to the level a Sonnet
> executor can build against: exact isolation argv, the post-spawn isolation
> proof, the decode-boundary type signatures, the seven `validate_call` checks as
> Rust, the `NumCoerce` port table, and the `.ipei ≡ stdlib_scheme` gate procedure.
>
> **Priority order:** security > correctness > soundness > efficiency >
> completeness > readability.
>
> **Public-artifact rule.** The Haskell generator in upstream Sky
> (`src/Ipê/Build/Rust/{Ffi,FfiCall,FfiInstance,NumCoerce}.hs`) is the
> **capability oracle** — it defines *what* can be bound shim-free. It is **not**
> the security oracle: it runs the inspector unsandboxed via `sh -c` and quotes
> args with `quoteShell` only. Everywhere below, Ipê is **stricter by design**;
> each such point is marked **[STRICTER]** with the reason. No disparagement, no
> upstream-contribution note — this is a from-scratch security posture for a
> language that fetches and compiles untrusted crates on `ipe add`.

Two fundamental rules drive every type below:

1. **Parse, don't validate.** Untrusted rustdoc JSON crosses into Ipê at exactly
   two `TryFrom<wire> → Result<Domain, Diagnostic>` boundaries (`PkgInfo`,
   `Call`). After decode, an ill-formed foreign binding is *unrepresentable*.
2. **Make invalid states unrepresentable.** A `Call` that has not passed
   `validate_call` is **unconstructible** (private fields, one fallible
   constructor). An `FfiKernelId` is validated at its parse boundary. Kernel
   origin is a closed sum. No `Ty::Any` arm exists anywhere in the mapper.

---

## Part 1 — SANDBOX (#41): the blocking gate

### 1.1 What runs untrusted, and exactly when

`ipe add <crate>` is remote code execution gated only by a crate name. Three
distinct untrusted-code events happen, in order. The vendored inspector
(`tools/sky-ffi-inspect-rs/src/main.rs`) drives all three:

| Phase | Inspector call site | Untrusted code that runs | Network? |
|---|---|---|---|
| **Fetch** | `fetch_dep` (`main.rs:1284`); `cargo metadata` for transitive closure | none *executes* yet, but cargo resolves + downloads the full dep closure | **yes — required** |
| **Compile** | `cargo check --quiet` (`main.rs:1118`) | **every `build.rs` in the closure runs; every proc-macro compiles + executes** | must be **no** |
| **Introspect** | `rustdoc --output-format=json` (`main.rs:1169`) | **proc-macros execute again** during macro expansion (that is the inspector's whole value proposition — post-expansion surface) | must be **no** |

**The critical-path rule (never violate).** None of Fetch / Compile / Introspect
may run in the **PR path, CI default path, or any `ipe build` of a project that
did not itself run `ipe add`.** Untrusted crate code runs **only** inside an
explicit, interactive (or `--yes`) `ipe add`/`ipe install`, and **only** inside
the jail below. A warm `ipe build` reads the cached `.ipei`/`kernel.json` and
**never re-invokes the inspector** (D8 warm-cache rule). This is what keeps a
malicious transitive dep from executing when a teammate merely builds the repo.

### 1.2 Confirmed weaknesses being closed (verified on the vendored inspector)

- **Offline-then-networked silent fallback.** `fetch_dep` runs
  `cargo fetch --offline` (`main.rs:1287`) then, on failure, **falls back to a
  networked `cargo fetch`** (`main.rs:1296`) with no gate — uncontrolled egress
  during "just inspecting."
- **Compile + introspect are not network-isolated.** `cargo check` / `rustdoc`
  run with the user's full ambient network + filesystem + env.
- **`sh -c` string construction.** The Haskell driver builds one command string
  and runs `readProcessWithExitCode "sh" ["-c", cmd']` (`Ffi.hs:131,182`),
  relying entirely on `quoteShell` (single-quote wrap + `'\''` escape,
  `Ffi.hs:133`). **[STRICTER]** the Rust port drops the shell entirely (§1.6).
- **`--git` unconstrained.** `runRustInspectorWith` passes `--git <url>` with only
  `quoteShell` (`Ffi.hs:118-131`); no scheme/host/charset gate.
- **No toolchain/lock pin.** The vendored inspector ships **no
  `rust-toolchain.toml`, no `Cargo.lock`** (verified) — its own build is
  non-reproducible and network-dependent.

### 1.3 Host capability probe (verified on this host, 2026-07-02)

```
bwrap    /usr/bin/bwrap        unshare  /usr/bin/unshare
prlimit  /usr/bin/prlimit      timeout  /usr/bin/timeout
setpriv  /usr/bin/setpriv      kernel.unprivileged_userns_clone = 1
docker/podman/firejail/nsjail  ABSENT
```

Therefore: **bubblewrap is the primary jail**, `unshare` the fallback (with the
§1.5 post-spawn proof), and a documented **refusal** when neither can prove
isolation. `prlimit` supplies rlimits; `timeout` the wall clock. These live in a
**separate `ipe_sandbox` crate** so the RCE surface is confined and the
`ipe_ffi` decode/emit core stays capability-free and unit-testable.

### 1.4 The bubblewrap jail (primary) — concrete argv

The sandbox wraps every Compile and Introspect invocation. It is fail-closed:
network denied, filesystem read-only except one per-invocation tempdir, env
scrubbed, PID/IPC/UTS namespaces fresh, resources bounded.

```
bwrap
  --unshare-net                      # NEW empty net namespace — no egress
  --unshare-pid --unshare-uts --unshare-ipc --unshare-cgroup
  --die-with-parent                  # child dies if the driver dies
  --new-session                      # detach controlling tty (no TIOCSTI injection)
  --clearenv                         # scrub ALL env; re-add only the allowlist
  --ro-bind / /                      # toolchain, std, rustc, cargo: read-only
  --tmpfs /home  --tmpfs /root  --tmpfs /tmp
  --ro-bind <REGISTRY_CACHE> <REGISTRY_CACHE>   # pre-fetched crate sources, RO
  --bind    <SCOPED_TMP> <SCOPED_TMP>           # the ONLY writable mount
  --chdir   <SCOPED_TMP>
  --setenv CARGO_NET_OFFLINE 1
  --setenv CARGO_HOME <SCOPED_TMP>/cargo-home
  --setenv PATH /usr/bin:/bin
  --setenv RUSTUP_TOOLCHAIN <pinned-nightly>    # from rust-toolchain.toml
  --                                            # end of bwrap flags
  prlimit --as=<RSS_CAP> --cpu=<CPU_SECS> --nofile=<FD_CAP> --nproc=<PROC_CAP> --fsize=<OUT_CAP>
    -- <inspector-bin> <argv...>                # NO shell anywhere
```

Wrapped by `timeout --kill-after=5s <WALL_SECS> bwrap …` for a hard wall clock,
and the driver caps stdout at `<OUT_CAP>` bytes with a bounded read (a 76k-symbol
crate must not OOM the host — the inspector's over-drop bounds *what* is bound;
this bounds *what it costs*).

**Env allowlist (the only vars that enter):** `CARGO_NET_OFFLINE`, `CARGO_HOME`,
`PATH`, `RUSTUP_TOOLCHAIN`, `RUSTC`, `TMPDIR=<SCOPED_TMP>`. **Never** `IPE_*`,
`IPE_*`, `AWS_*`, `GH_*`, `*_TOKEN`, `SSH_*`, `HOME` (real).

**Resource caps (default, all env-overridable with a printed warning):**
`RSS_CAP` 4 GiB, `CPU_SECS` 300, `WALL_SECS` 420, `FD_CAP` 256, `PROC_CAP` 512,
`OUT_CAP` 256 MiB JSON.

**[STRICTER]** the reference runs the inspector with the user's real `$HOME`,
`~/.cargo` writable, full network, full env. Ipê denies all of that by default.

### 1.5 The `unshare` fallback MUST PROVE isolation — never assume

`bwrap` fails closed itself. `unshare` does **not**: `unshare --net --pid --mount`
for a non-root user needs an unprivileged userns to succeed, and on a host where
that is disabled (`kernel.unprivileged_userns_clone=0`, seccomp/LSM policy, some
container hosts) the call can **partially fail or silently no-op yet still return
exit 0** — leaving a process with full host networking. So the fallback is sound
**only** if, as the first action inside the unshared child and **before any
untrusted code runs**, it *proves* every namespace it claimed:

```
prove_isolation():                 # runs as pid-namespace init inside the child
  assert getpid() == 1             # PID namespace took effect (else HARD-FAIL)
  assert readlink(/proc/self/ns/net)  != parent_ns.net    # net ns differs
  assert readlink(/proc/self/ns/mnt)  != parent_ns.mnt    # mount ns differs
  assert readlink(/proc/self/ns/uts)  != parent_ns.uts
  assert readlink(/proc/self/ns/ipc)  != parent_ns.ipc
  assert no non-loopback iface up  AND  no default route  # net truly empty
  # any failure ⇒ do NOT exec the payload; exit to the refusal path
```

The parent passes its own `/proc/self/ns/*` ids in via the scrubbed env or a
pipe so the child can compare. **Any proof failure ⇒ HARD-FAIL to refusal — never
proceed to compile on the assumption `unshare` worked.** This is the single most
important soundness property of the fallback.

### 1.6 Refusal + override (fail-closed default)

If neither `bwrap` nor a *proven* `unshare` jail is available, `ipe add`
**refuses** with `IPE-F4410` ("cannot establish an isolation jail; refusing to
compile an untrusted crate unsandboxed"). The sole override is
`IPE_FFI_ALLOW_UNSANDBOXED=1`, which prints a red multi-line trust warning naming
the crate and the fact that its `build.rs`/proc-macros will run with full
privileges. CI never sets it.

### 1.7 The trust-decision gate (before any fetch)

`ipe add <crate>` is a trust decision and must surface it, **before** the network
is ever touched:

1. Print: crate, resolved version, git URL (if any), and the **count of
   transitive deps that will be compiled** (from `cargo metadata`, itself run in
   a network-*fetch* jail — see phase separation below).
2. Require interactive confirm, or `--yes` for CI.
3. **Phase separation is mandatory.** Fetch is its own **network-enabled** jail
   (net *on*, but still `--ro-bind /`, tmpfs `$HOME`, scrubbed env, RSS/CPU/wall
   caps, writable only the registry cache). Compile + Introspect run in the
   **network-denied** jail (§1.4). Never one un-gated networked step.
4. After fetch, **all** cargo invocations run `--frozen --locked --offline`
   (`CARGO_NET_OFFLINE=1` already set) so no implicit re-resolution can reach the
   network during compile/introspect. This also closes the `fetch_dep`
   offline-then-networked fallback: in the compile jail there is no network to
   fall back to, and `--frozen` makes a cache miss a hard error, not a silent
   re-fetch.

### 1.8 `--git` + crate-name gating (parse, don't validate — at the driver)

The driver validates the git source into a typed value **before** it can reach
any `Command`. Make the bad state unconstructible:

```rust
/// Constructed ONLY via `parse`. A `GitSource` value is, by existence, https,
/// host-charset-clean, host-allowlisted, and rev/branch/tag mutually exclusive.
pub struct GitSource { url: Url, pin: GitPin }        // private fields
pub enum GitPin { Rev(GitRev), Branch(GitRef), Tag(GitRef), Default }

impl GitSource {
    pub fn parse(raw: &str, pin: RawPin, hosts: &HostAllowlist)
        -> Result<GitSource, Diagnostic>            // IPE-F4411 on any failure
    {
        // scheme: https only (ssh behind a flag); reject file://, http://, data://…
        // host:  ^[A-Za-z0-9._-]+$  AND  host ∈ hosts (default github.com,
        //        gitlab.com, codeberg.org; override via IPE_FFI_GIT_HOSTS)
        // pin:   at most one of rev/branch/tag  (Haskell TODO Ffi.hs:94 → hard check)
    }
}
```

Crate name is validated by the inspector's `safe_crate_name` (`main.rs:3756`,
`^[A-Za-z0-9_-]+$`, non-empty) **before** it reaches any argv. **[STRICTER]** the
reference applies only `quoteShell` to the git URL and leaves crate-name/host
ungated at the driver.

**[STRICTER] — kill the shell class structurally.** The Rust driver builds an
argv `Vec<OsString>` and spawns `std::process::Command` directly — **no `sh -c`,
no `quoteShell`.** `quoteShell` is a mitigation for a shell that Ipê never
invokes; removing the shell removes the entire injection class rather than
escaping around it. (`quoteShell` is ported only as a doc reference of the class
being eliminated, not as live code.)

### 1.9 Inspector reproducibility pin (B0.1 — a restored regression)

The jail is only deterministic if the inspector is. The vendored inspector must
regain: a dir-scoped `rust-toolchain.toml` (**nightly pin** — rustdoc JSON is
nightly-only, and the exact channel is the drift-fence + byte-diff anchor), a
committed `Cargo.lock`, and exact-version `serde`/`serde_json`/`tempfile`. The
FFI cache key **includes the nightly channel** so a pin bump forces re-inspect.
Prerequisite **B0.0** de-workspaces the inspector first (a Cargo workspace has one
root lockfile, so the inspector cannot own its own `Cargo.lock` while it is a
`[workspace]` member) — B0.0 edits the shared root `Cargo.toml`, so run it when
the build lane is idle.

---

## Part 2 — GENERATOR (#42): the Haskell → `ipe_ffi` port

Target crate `ipe_ffi`, leaf-first module DAG mirroring the Haskell cycle-break
`Ffi → FfiInstance → FfiCall → NumCoerce`. Build order (R0.5, leaf-first):
**M-A → M-C → M-B → M-D → M-E → M-F → M-G**.

### 2.1 Decode boundary 1 — `PkgInfo`/`FnInfo` (M-A): validated newtypes, closed sums

Two layers. A permissive **wire** layer (`#[derive(Deserialize)]`, every field
`Option`/`#[serde(default)]`, byte-mirroring the inspector; no
`deny_unknown_fields` — the wire deliberately omits absent optional keys for
back-compat). A **domain** layer with **private fields, constructed only via
`TryFrom<wire> → Result<_, Vec<Diagnostic>>`** — the sole constructor, so no
unvalidated `FnInfo` can exist.

**Identifiers are validated newtypes at the boundary (the injection firewall):**

```rust
pub struct RustIdent(String);   // private; parse enforces ^[A-Za-z_][A-Za-z0-9_]*$
pub struct ModulePath(Vec<RustIdent>);
pub struct FieldName(RustIdent);
```

A crate that names a symbol `"; std::process::Command::new(…)"` can **never
construct a `RustIdent`** → the injection class dies at the trusted surface, and
the emit-side `rust_str_lit`/`absolutize_crate` become belt-and-suspenders rather
than the sole defense. **[STRICTER]** the reference emits foreign names into Rust
source guarded only by emit-time string-literal escaping; Ipê refuses the bad
name at *decode*, one layer earlier.

**Closed sums, no catch-all** — an unknown discriminator string is a hard
`IPE-F4401`, never a defaulted variant:

```rust
enum FnShape { Free, Method, Field{fallible: Fallibility}, FieldSet, EnumCtor }  // flag-soup collapse; two-flags-set ⇒ IPE-F4402
enum CallKind { Method, Function }        enum ByKind { Ref, RefMut, Value }
enum ClosureKind { Fn, FnMut, FnOnce }    enum Effect { Pure, Fallible, Effectful }
enum Fallibility { Infallible, TaskError }   // R0.4: ONE stored bit, both emitters read it
```

`Fallibility` being a single decoded bit closes risk #4 at the type level: the
Haskell computes fallibility twice (`emitRustKernelJson` `Ffi.hs:1495`,
`emitSkyiRustFn` `Ffi.hs:1560`) and agreement is human-maintained; here both
emitters read the same field, and a diff-golden byte-checks the two artifacts.

### 2.2 `TypeRef → Ty` mapper (M-A/M-B): four directions, never conflated, **no `Ty::Any`**

`TypeRef` deserializes via a **hand-written single-key `Visitor`**, not
`#[serde(untagged)]` — untagged swallows *which* variant failed and can backtrack
on an adversarial map. It rejects the multi-discriminator case (`param`+`prim`
both present).

| Direction | Function | Totality |
|---|---|---|
| `TypeRef` AST → Rust source | `render_type_ref` | total, **no `"F?"` fallback** — the only unrepresentable case (non-direct closure) is refused by `validate_call` check 6 |
| `FnInfo` → Ipê `Ty` (seed `.ipei`) | `sky_type_of` | total |
| Ipê `Ty` → Rust type (concrete) | `ty_to_rust` | total on closed set; emit-only `→ String` fallback tolerated only off the `call` path (unknown ⇒ over-drop already happened) |
| Ipê `Ty` → Rust type (generic slot) | `ty_to_rust_closed` | **fallible, no fallback** — record/tuple/fn/bare-TVar/opaque → `Err` → `IPE-F4400` |

**Foreign → Ipê mapping** (`sky_type_of`): int widths → `Int` (carrier `i64`);
`f32/f64` → `Float`; `bool/char/()` direct; `String/&str/&Path/&OsStr` → `String`;
`Option<T>` → `Maybe`; `Result<T,E>` → **`Result Error a`** (`E` erased to Ipê
`Error` — **never a type param, never `Result String`/`Task String`**); `Vec<T>` →
`List`; serde-bound generic → `String` (JSON text); anything else → nominal
`Ty::Con { module: "Rust.<Crate>", name }` interned as `Symbol` so the same
opaque type unifies across `.ipei` files. **There is no `Ty::Any` arm anywhere** —
that absence is the eval-hole foreclosure.

### 2.3 Decode boundary 2 — `Call` AST (M-B): the KEYSTONE

`Call` is the render-totality keystone. Its **only** constructor is
`TryFrom<(wire::Call, n_params)> → Result<Call, Diagnostic>`, running the seven
structural checks **inside decode**. A `Call` that has not passed them is
**unconstructible**. The error is typed — a closed `CallDefect`, **never a bare
`String`** (the Haskell `validateCall :: Either String Call` and `parseCall`'s
`fail String` are stringly-typed carriers that violate the no-`Result String`
non-regression rule; they must **not** port verbatim):

```rust
pub struct Call { /* private */ }          // no public constructor

pub enum CallDefect {                        // closed — one per check
    ParamRefOutOfRange { idx: i64, n_params: usize },   // check 1
    ReceiverKindMismatch,                                // check 2
    ArgIndexNegative { idx: i64 },                       // check 3a
    ArgIndexGap { missing: usize },                      // check 3b
    ArgIndexDuplicated { idx: usize },                   // check 4  (use-after-move)
    ArgTypeArityMismatch { got: usize, want: usize },    // check 5
    ClosureNestedOrNonDirect,                            // check 6  (Vec<closure> E0412)
    IterAdapterTargetNotVec { idx: usize },              // check 7
}

impl Call {
    fn try_new(w: wire::Call, n_params: usize) -> Result<Call, Diagnostic> {
        // → Diagnostic{ code: IPE-F4400, reason: CallDefect, span }
    }
}
```

The seven checks, ported verbatim from `validateCall` (`FfiCall.hs:256-333`):

1. every `TRParam i` (in `typeArgs`/`ret`) has `0 ≤ i < n_params`;
2. `Method ⇔ receiver present`, `Function ⇔ receiver absent`;
3. every value-arg ref (receiver arg + each `args`) is `≥ 0` and the referenced
   set is **gap-free from 0**;
4. arg indices are **unique** (a repeated index is a use-after-move in Rust);
5. `arg_types.len() == arity` where `arity = max(referenced idx) + 1` (0 if none);
6. **no closure nested** in ctor/ret/typeArgs/turbofish — a closure is valid
   *only* as a direct `arg_types` element (else it reaches the non-direct path and
   would emit `Vec<F?>` → cargo E0412);
7. every `iter_adapters` index targets a slot whose `argType` is a `Vec<_>` ctor
   (accept `::Vec` and bare `Vec`) — `argJ.into_iter()` is sound only on a Vec.

Once decode passes, `render_call` over an `Ok(Call)` is **total** and cannot
emit-and-cargo-fail. A non-direct closure reaching `render_type_ref` is a
`CompilerBug`-class diagnostic (unreachable after check 6), **never** a leaked
`"F?"` string. **Drift fence:** an accept/reject corpus (mirror `FfiCallSpec.hs`)
asserting each check rejects with `IPE-F4400` and each well-formed call renders
byte-stable.

**Warm-cache re-validation (R0.1).** The `Call` written into `kernel.json` is a
re-serialization of an already-validated domain `Call`; a warm `ipe build`
re-runs the identical `try_new` on read, so a hand-corrupted cache is re-rejected.
Drift is impossible by code structure — one domain type, one fallible constructor,
both entry points construct through it.

### 2.4 `NumCoerce` (M-C): the saturating leaf — port table

`num_coerce.rs` is the DAG leaf (no deps). **Every** scalar cast in `emit` and
`instance` delegates here; a grep-fence test asserts **no bare `as i64`/`as u64`
outside `num_coerce`** — this is the "one saturating helper" invariant
(`NumCoerce.hs:8-11`). Port verbatim (all total, panic-free, clippy-clean):

**PARAM (Ipê carrier → foreign width), `num_saturate(raw, e)`** — `e` must be a
side-effect-free bound local (`isize` arm evaluates twice):

| target | emitted Rust | note |
|---|---|---|
| `f64`/`i64` | `e` | identity |
| `f32` | `(e) as f32` | precision-lossy, total |
| `i8/i16/i32` | `(e).clamp(T::MIN as i64, T::MAX as i64) as T` | signed narrow |
| `u8/u16/u32` | `(e).clamp(0, T::MAX as i64) as T` | unsigned narrow |
| `u64` | `(e).max(0) as u64` | negatives → 0 |
| `i128` | `(e) as i128` | sign-preserving widen |
| `u128` | `(e).max(0) as u128` | avoids `-1 → ~3.4e38` sign-extend |
| `usize` | `usize::try_from((e).max(0)).unwrap_or(usize::MAX)` | **32-bit-correct** |
| `isize` | `isize::try_from(e).unwrap_or_else(|_| if (e) < 0 { isize::MIN } else { isize::MAX })` | **32-bit-correct** |

**RETURN (foreign width → Ipê carrier), `num_widen_scalar(raw)`:**

| source | carrier | coerce |
|---|---|---|
| `i8/i16/i32/i64/u8/u16/u32/isize` | `i64` | `(e) as i64` (lossless) |
| `u64/usize/u128` | `i64` | `(e).min(i64::MAX as raw) as i64` (avoids `u64::MAX → -1`) |
| `i128` | `i64` | `i64::try_from(e).unwrap_or(if (e)<0 {i64::MIN} else {i64::MAX})` |
| `f32/f64` | `f64` | `(e) as f64` |

**Sanctioned divergence (record `oracle_divergence = true` + reason).** A value
above `i64::MAX` **saturates** — not wraps, not errors. This satisfies "no silent
numeric coercion" (AGENTS.md §8) because the clamp is *total and documented*, not
a `-1 → 3.4e38` sign-flip. `usize`/`isize` routing through `try_from` is
32-bit-correct **by construction** — a bare `as` truncates on 32-bit, which an
all-64-bit CI can never catch.

### 2.5 Three emitters, one naming SSOT (M-D)

`naming.rs` is the single source of truth: `wrapper_ref_name`
(`lower_first(name)` + `_from_<lower_first(recv)>` for accessors, returning a
validated `RustIdent`), `rust_module_name` (`uuid → Rust.Uuid`),
`rust_kernel_name` (`Rust_<CapBase>`, version-suffix aware), and the BEGIN/END
sentinel constants. **No emitter constructs a name independently** — so the
`.ipei` binding name, the `kernel.json` `"name"`, the `_bindings.rs`
`// IPE-FFI-WRAPPER BEGIN <ref>` sentinel, and the S4 DCE reachability key are
**byte-equal by construction**; three-way name skew (an under-bind that
link-fails) is structurally impossible.

- **`emit/ipei.rs`** — `module Rust.<Crate> exposing (..)`, one HM signature per
  fn from `sky_type_of`, fallibility from the single `Fallibility` bit. **Also
  emits one nominal opaque-type declaration** (`type Version` — no constructors)
  per referenced foreign type, so the `.ipei` is a *complete* type-env seed (a
  `Ty::Con` no module declares is a dangling seed reference — a consumer under-bind).
- **`emit/kernel.json`** — per fn: `wrapper_ref_name`, `sky_signature`, the
  round-tripped `Call` AST, `origin: Ffi { crate, version }`, the raw `generic`
  block, plus `transitive_deps`/`features` for the driver.
- **`emit/bindings.rs`** — per fn a wrapper bracketed by BEGIN/END sentinels
  (everything outside a pair is preamble, kept unconditionally); body =
  `render_call` wrapped in `catch_unwind → Err` + scalar coercions. **Module top
  carries the panic-profile fence (§2.8).**

**No injection from crate metadata at emit.** Foreign crate/type/fn names are
already `RustIdent`s (§2.1); string literals go through `rust_str_lit`
(`Ffi.hs:412`); `absolutize_crate` (`Ffi.hs:381`) keeps extern refs unambiguous.

### 2.6 The `.ipei ≡ stdlib_scheme` structural-equivalence gate (M-D acceptance, ties to registry)

This is the FFI re-review gate and the kernel-registry **OPEN DECISION 1**
imported as a **blocking check**. The stdlib scheme and the FFI scheme are built
by **two independent `Ty`-constructors**: the stdlib's hand-`match KernelId →
Scheme` projection vs the `.ipei` decoder. They agree today only *by test*.

**Gate procedure — run before the first `Ffi` entry lands:**

1. Pick logical signatures that exist in **both** worlds (e.g. FFI `fun(int,
   string) → string` vs stdlib `String.fromInt`-shaped).
2. Decode the `.ipei` scheme and project the stdlib scheme independently.
3. Assert the two `Ty` trees are **structurally identical** (same constructors,
   same arity, same nominal `Con` keys, same `Result Error a` erasure) after
   canonical alpha-renaming.
4. **If any divergence:** promote to a single shared descriptor both paths build
   from (do not keep two constructors that can drift). **If identical:** keep the
   hand-`match`, and land a **permanent golden** that re-asserts the structural
   identity so a future stdlib-projection edit can't silently desync the FFI path.

This closes the risk that an FFI `fun(int,string)` type-checks *differently* from
the structurally-equal stdlib kernel — a correctness hole that would surface only
as a mysterious FFI-call type error. It is an **M-D acceptance criterion**, not a
deferred nicety.

`FfiKernelId` validation (registry G4) belongs to the same boundary: a
`kernel.json` resolves a call to a `KernelId`; the FFI id is parsed/validated at
decode into `KernelId::Ffi(fid)` (an unknown `Rust.*` qualifier is an
unknown-qualifier error, not a fallback), and the classification predicates
(`is_db`/`is_tea`/`is_server`) return a **total none-of-these** default for FFI
ids (R0.2) — an FFI kernel is never mis-routed into a stdlib fast-path.

### 2.7 Generic monomorphisation + closure gate + MODELLABLE_5 (M-E)

Instance collection is demand-driven from reachable call sites (bounded by
program size, not the crate's 76k symbols), deduped by `(callee, types)`, gated by
`check_instance` **before** any Rust is emitted. Per type-param, in order:

1. **Closed-set** (`ty_to_rust_closed`): non-nameable Rust type → `IPE-F4400`
   (`mk_closed_set_error`).
2. **Trait-bound** (only on args past 1): bound ∉ `MODELLABLE_5` →
   `mk_unmodellable_bound_error` (names the bound); bound ∈ set but concrete lacks
   it → `mk_trait_bound_error` (e.g. `f64` at a `Hash`/`Eq`/`Ord` slot).

**MODELLABLE_5 two-way drift fence.** `{Hash, Eq, Ord, Clone, Default}` lives on
both the inspector (`main.rs:411`, asserted the exact modellable subset with
`MARKER_TRAITS.len() > MODELLABLE_5.len()` at `main.rs:12962`) and the generator
(`modellableTrait`, `FfiInstance.hs:292`). A cross-crate test asserts the two sets
are byte-identical — either side changing without the other fails CI, never a
user's cargo build. A bound outside the set on an *unused* symbol is over-drop
(silent, sound); on a *reached* call site it is a loud `IPE-F4400`.

**Closure-capture Clone gate.** `Fn`/`FnMut` slots re-clone every capture per call,
so every capture must be **positively `Clone`** via a closed **allowlist**
(`rust_type_is_clone`, never a denylist); first non-Clone → `IPE-F4400`. `FnOnce`
is moved once, never gated. **Named M-E acceptance item:** re-verify
`traits_of_rust_type` cell-by-cell against Ipê's actual runtime derives — notably
`SkyMaybe` derives *no* `Default`/`Hash`/`Eq`, and `f64`/`f32` are `Clone`+`Default`
only (the IEEE-754 security-critical cell). Port on evidence, not on faith.

**Polymorphic-passthrough is LOCKED as reject (not open):** a generic FFI slot
instantiated at a bare tyvar fails `ty_to_rust_closed` → `IPE-F4400` at the call
site. No erased `func(any) any` — that is the eval-hole the design forbids; the
user monomorphises at the boundary. Stated expressiveness limitation, only sound
answer.

### 2.8 async → `Task Error a` + `catch_unwind` + panic-profile fence (M-G)

- Every foreign call becomes an Ipê `Err` on panic: sync via
  `std::panic::catch_unwind`; **async via `futures::FutureExt::catch_unwind` on
  the pinned future** (you cannot `.await` inside the closure
  `std::panic::catch_unwind` takes).
- **Panic-profile gate = a `compile_error!` fence emitted IN the wrapper crate**,
  once per `<crate>_bindings.rs` top:

  ```rust
  #[cfg(panic = "abort")]
  compile_error!("ipe_ffi catch_unwind boundary requires panic=unwind");
  ```

  `cfg(panic)` (stable since Rust 1.60) sees the **effective** strategy — catching
  a workspace-root `[profile.*] panic="abort"`, `RUSTFLAGS=-Cpanic=abort`, or a
  `-Zbuild-std` rebuild — which a `Cargo.toml` text-scan **cannot**. This is the
  sound gate. `cargoProfilePanicIsUnwind` (`Ffi.hs:66-82`) is ported but **demoted
  to an advisory fast pre-check** (a friendly early error when the emitted manifest
  itself declares abort). Under `panic="abort"` a foreign panic aborts the process
  before the boundary can convert it — the keystone-forbidden emit-then-abort
  class; the fence makes the requirement *enforced*, not assumed.
- A foreign `async fn -> Result<T,E>` maps to **`Task Error T`** (never
  `Task String`): the wrapper `spawn`s onto **the single reactor the Task executor
  owns** and bridges the handle into the Task completion channel — **it never calls
  `block_on`** (that panics inside a running reactor). The foreign `E` folds into a
  typed `Error` at the boundary.
- **Send discipline:** async combinator closure args need
  `Box<dyn Fn + Send + 'static>`; the **inspector** supplies the Send verdict
  (`recv_provably_async_send`, `PROVABLY_SEND_OPAQUE_NAMES`, `main.rs:4923`) — an
  unprovable concrete keeps dropping (over-drop). The generator never re-derives
  Send-ness.
- **Honest scope:** M-G proves the *bridge* on a small async crate. Large async
  SDKs (firestore/firebase/stripe) stay hand-shimmed even in upstream Sky and are
  **not** marketed shim-free.

### 2.9 Driver + dynamic Cargo.toml + sentinel DCE (M-F)

- **`ipe add`** — trust-gate (§1.7) → fetch jail → sandboxed inspect (§1.4) →
  decode (M-A/M-B) → gate (M-E) → emit (M-D) → write four cache artifacts.
- **`ipe install`** — from-scratch regenerate each recorded dep; idempotent warm.
- **`ipe remove`** — delete the four cache files + the manifest line + re-seed the
  type-env.
- **Cache:** `.ipe/cache/ffi/rust/<crate>.{ipei, kernel.json, _bindings.rs,
  coverage.md}` (`coverage.md` reports what was over-dropped — the keystone made
  visible). **Never regenerated on a warm cache**; the key includes the nightly
  channel. (Locality — project-local vs a global introspection cache — is OPEN-1.)
- **Dynamic `Cargo.toml`:** base manifest + one `[dependencies]` line per FFI crate
  the program uses, each with the **exact pinned version** from
  `PkgInfo.transitive_deps` (resolve via the `(ident, canonical_name, version)`
  triple — **never guess `_`→`-`, never emit `"*"`**) and the **effective feature
  set** rustdoc succeeded with (under-including makes feature-gated types vanish —
  the firestore #73/#100 class). **No `panic=` key** (cargo default unwind — the
  fence enforces it).
- **S4 sentinel DCE:** whole-program reachability yields the reached
  `wrapper_ref_name` set; the driver **text-slices** `_bindings.rs` on BEGIN/END
  sentinels, keeping only reached regions + preamble, **without parsing Rust**.
  Conservative-keep (keep if reached OR referenced by a kept preamble) so it can
  never under-keep — this compiles only the ~dozen wrappers a 76k-symbol crate's
  caller actually uses.

---

## Part 3 — Implementation ordering + the security gate before each ships

```
#40 inspector-harden  ──►  #41 sandbox (BLOCKING)  ──►  #42 generator
      (parallel-safe)          (ship-gate)                (behind M4 for consumer wiring)
```

| Slice | Ships only when… (the gate that must pass) |
|---|---|
| **#40 B0.0** de-workspace | inspector has own `target/` + own `Cargo.lock` + dir-scoped `rust-toolchain.toml`; run when build lane idle (edits root `Cargo.toml`). |
| **#40 B0.1** repro pin | nightly pinned, `Cargo.lock` committed, deps exact-versioned, nightly CI job green. |
| **#40 B0.2** fail-closed parse | flip inspector `unwrap_used`/`expect_used`/`panic` from `allow` → **deny** (a *reversal* of the deliberate `Cargo.toml:12-19` decision; ~130 call sites: 42 `unwrap`/57 `expect`/31 `panic`); every adversarial-JSON path returns error-`PkgInfo` + non-zero exit, **never aborts**; **over-drop keystone comments survive verbatim** (`main.rs:812,1667,1965,2950,4578,4634,4670`); **no change perturbs a well-formed crate's `PkgInfo`** (would desync the byte-diff). |
| **#40 B0.3** fuzz | adversarial rustdoc-JSON fuzz target proves: no panic, bounded memory, error-`PkgInfo` out. Acceptance test for B0.2. |
| **#41 sandbox** (BLOCKING) | **bwrap jail (§1.4) denies net + scrubs env + RO-binds `/` + bounds RSS/CPU/wall**; **`unshare` fallback proves every namespace post-spawn (§1.5) or HARD-FAILS**; **refusal is the default when neither proves (§1.6)**; **trust-gate + phase separation + `--frozen --locked --offline` (§1.7)**; **`GitSource`/crate-name parse-not-validate + no `sh -c` (§1.8)**. *No untrusted crate may be introspected/compiled until every one of these passes.* This is the gate that unblocks the rest of #42's driver. |
| **#42 M-A/M-C/M-B** decode+coerce | fixture 107 (`semver`) artifact byte-diff green vs sky; `IPE-F4400` reject corpus green; grep-fence "no bare `as i64/u64` outside `num_coerce`" green. **No M4 needed** (generator is a pure JSON→files function). |
| **#42 M-D** emit | tri-artifact name-SSOT byte-equal by construction; **`.ipei ≡ stdlib_scheme` structural gate (§2.6) green**; fallibility diff-golden green; `compile_error!` fence present at every wrapper-module top. |
| **#42 M-E** generics | MODELLABLE_5 two-way fence green; `traits_of_rust_type` re-verified cell-by-cell against Ipê runtime derives; per-instance `IPE-F4400` corpus green. (Prereq OPEN-2: confirm per-region solved `Ty` reaches the FFI-callee region.) |
| **#42 M-F** driver | consumer wiring blocks on **M4 kernel registry**; then the 10 shim-free sync crates (107-114 + regressions 73/76/92/97/105/106) byte-diff green → shim-free claim proven; malicious crate-name + bad git-URL refused before any compile. |
| **#42 M-G** async | bridge proven on a small async crate (FutureExt::catch_unwind, single Task reactor, typed `Error`, never `block_on`); large async SDKs explicitly NOT claimed shim-free. |

---

## Part 4 — Open decisions (genuine forks, deferred with trade-offs)

- **OPEN-1 — cache locality.** Project-local `.ipe/cache/ffi/rust/` (default;
  per-project trust consent; artifacts travel/wipe with the project; a
  cross-project shared *artifact* cache is a trust-laundering surface) **vs** a
  global content-addressed **introspection-only** cache (`~/.cache/ipe/…` keyed by
  `crate+version+features+toolchain+inspector-rev`; introspect-once win).
  **Recommendation:** ship project-local as the source of truth; add a global
  cache only for the raw `PkgInfo` introspection, gated by **re-consent on first
  use in each new project**. Decide at M-F.
- **OPEN-2 — per-region solved `Ty` at FFI callees.** Demand-driven instance
  collection assumes lowering exposes a per-call-site region→concrete-`Ty` map at
  FFI callees. The capability appears present (`sky_lower/src/lower.rs:26` already
  imports `SolvedTypes, Ty`); the narrow confirmation is that the map reaches the
  **FFI-callee region specifically**. Confirm before scheduling M-E — likely a
  targeted check, not new lowering capability.
- **OPEN — seccomp profile.** §1.4 lists `bwrap --seccomp <fd>` as a stretch/v2
  hardening (deny obvious escape/persist syscalls). Not required for the blocking
  gate (network + fs + resource isolation carry it); decide whether to land a
  minimal profile in v1 or defer.
- **OPEN — `ssh://`/`git@` git sources.** §1.8 allows them only behind a flag.
  Whether to support private-repo FFI crates in v1 (and how to scope the SSH agent
  into the fetch jail without leaking keys) is unresolved.

---

## Reference source map (capability oracle, upstream Sky)

`Ffi.hs`: `runRustInspectorWith` shell driver `:87-131`, `quoteShell` `:133`,
`cargoProfilePanicIsUnwind` `:66-82`, `generateRustBindings` `:195`,
`emitRustFile` `:658`, naming `:222-278`, sentinels `:247-258`. `FfiCall.hs`:
`validateCall` seven checks `:256-333`, `parseCall` `:764-820`, `TypeRef`/render
`:200-224,382-756`. `FfiInstance.hs`: `checkInstances`/closure-Clone gate
`:137-419`. `NumCoerce.hs`: whole file (`numSaturate`/`numWidenScalar`/`numCarrier`
one-helper invariant `:8-11`). Inspector: `fetch_dep` `:1284`, `cargo check`
`:1118`, `rustdoc` `:1169`, `safe_crate_name` `:3756`, `MODELLABLE_5` `:411`,
fence `:12962`, `errors` channel `:451`.
