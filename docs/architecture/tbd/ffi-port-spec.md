# FFI Port Spec — automatic shim-free binding of Rust crates in Ipê

> **Status:** committed port plan. Doc-only. Supersedes the forward-looking
> `docs/architecture/ffi-design.md` (kept as the M0-era design-with-it-in-mind
> note) with a concrete, milestone-sequenced, guardian-gated port.
>
> **Goal:** `ipe add <crate>` binds an arbitrary **Rust** crate with *zero
> hand-written shims* for the pure/sync class, mirroring the behaviour of the
> Haskell generator in upstream Sky — not its bytes.
>
> **Scope split.** The inspector (`tools/ipe-ffi-inspector/src/main.rs`, 18.5k
> LOC) is **vendored and works** as a `PkgInfo` JSON producer (post-macro-expansion
> rustdoc-JSON; fail-closed over-drop). The **generator** — Haskell in upstream Sky,
> `src/Ipê/Build/Rust/{Ffi.hs (1616), FfiInstance.hs (952), FfiCall.hs (820)}` +
> `NumCoerce.hs` + naming, with `src/Ipê/Build/FfiGen.hs (1996)` as the shared
> `.ipei`/`kernel.json` emission reference — is **absent** and must be ported to a
> new `ipe_ffi` crate. The whole consumer side (kernel-registry M4, `.ipei`
> type-env seeding, dynamic `Cargo.toml` injection, `ipe add/install/remove`
> driver, `~/.cache/ipe/ffi/rust` cache, drift fence) is **absent**.

**Priority order for every decision below:** security > correctness > soundness >
efficiency > completeness > readability.

**Two fundamental rules.**
1. **Parse, don't validate.** The `.ipei` / `kernel.json` contract is the SINGLE
   typed parse point where a foreign value enters Ipê. Opaque foreign types unify
   nominally as `Ty::Con { module, name }`. No downstream re-coercion to `any`.
2. **Make invalid states unrepresentable.** Kernel origin is a sum type
   (`Origin::Stdlib | Origin::Ffi { crate }`). A foreign-call AST that cannot be
   rendered is rejected **at decode** with a `IPE-F4400` diagnostic — never
   emit-and-cargo-fail. Diagnostic codes follow the house scheme `IPE-X####`
   (`IPE-I`/`IPE-L`/`IPE-N`/`IPE-P`/`IPE-T` already in use; there are no bare
   `E####` codes). This port **reserves the `IPE-F` block for FFI diagnostics** —
   `IPE-F4400` is its first member.

---

## A. Sandbox threat-model — the #1 blocking gate

**This gate blocks `ipe add` from shipping (task #41). Nothing in Milestones
M-C onward may run against an untrusted crate until this lands.**

### A.1 The attack surface

`ipe add <crate>` does not merely *read* a crate — it **compiles** it. The
inspector shells out to `cargo fetch`, `cargo metadata`, and `rustdoc
--output-format=json`. Each of these runs, at build time, arbitrary attacker code:

- **`build.rs`** — runs as a normal process during `cargo fetch`/build with the
  invoking user's full privileges (network, filesystem, env, exec).
- **proc-macros** — compiled and *executed* in-process by `rustdoc`/`rustc`
  during macro expansion. Post-macro-expansion introspection (the inspector's
  whole value proposition) *requires* running them.
- **transitive deps** — `cargo metadata`/`fetch` resolve and can build the full
  dependency closure; any transitive `build.rs`/proc-macro is equally live.

This is **remote code execution on `ipe add`**, gated only by the user typing a
crate name. It is the single highest-risk surface in the toolchain (risk #1).

### A.2 Confirmed weaknesses in the current path

- **Network egress with silent fallback.** `fetch_dep` (inspector
  `main.rs:1201`) tries `cargo fetch --offline`, then **falls back to a networked
  `cargo fetch`** with no gate. `collect_transitive_deps` likewise runs `cargo
  metadata` which resolves online when the cache is cold. → uncontrolled network
  during a "just inspecting" step (risk #2).
- **`--git` URL is unconstrained.** The Haskell driver
  (`Ffi.hs:runRustInspectorWith`, ~118-131) passes `--git <url>` with only
  `quoteShell` applied — no host allowlist, no scheme/charset gate. A crafted git
  URL reaches `cargo`'s git resolver (risk #2 / supply-chain).
- **No lock/toolchain pin.** `tools/ipe-ffi-inspector` ships **no
  `rust-toolchain.toml` and no `Cargo.lock`** (verified). Every inspect resolves
  "latest semver-compatible", so the *inspector's own* build is non-reproducible
  and network-dependent.
- **Shell-string construction.** The Haskell driver builds a single command
  string and runs it via `sh -c` (`Ffi.hs:131`), relying entirely on `quoteShell`
  for injection safety. The Rust port MUST drop `sh -c` (see A.5).

### A.3 Required isolation (Linux; our infra)

Verified available on this host: **`bwrap` (bubblewrap)** and **`unshare`**.
Absent: `docker`, `podman`, `firejail`, `nsjail`. Therefore the sandbox is built
on **bubblewrap as the primary mechanism**, `unshare` as the namespace fallback,
and a documented "unsandboxed refusal" when neither is present.

Every inspector invocation and every crate compile MUST run inside a jail with:

| Control | Requirement | Mechanism |
|---|---|---|
| **Network** | **Denied by default.** No egress during inspect/compile. Opt-in fetch is a *separate, explicit* phase (A.4). | `bwrap --unshare-net` (new empty net namespace, no loopback needed) |
| **Filesystem** | Read-only bind of the toolchain + a **scoped, per-invocation tempdir** as the only writable mount. No `$HOME`, no `~/.cargo` writable, no project tree. | `bwrap --ro-bind / /`, `--tmpfs /home`, `--bind <scoped-tmp> <scoped-tmp>`, `--bind <ro cargo registry cache>` |
| **Env** | Scrubbed. No secrets (`IPE_*`, `AWS_*`, tokens) pass in. `CARGO_NET_OFFLINE=1` set. | explicit env allowlist |
| **Process/PID** | New PID namespace; no ptrace of host. | `bwrap --unshare-pid --unshare-uts --unshare-ipc` |
| **Resources** | Wall-clock timeout, RSS cap, CPU cap, output-size cap on the JSON. | `timeout(1)` wrapper + rlimits (`--rlimit` via a prlimit pre-exec) + a max-bytes read on stdout |
| **Syscall** | Optional seccomp profile denying the obvious escape/persist syscalls. | `bwrap --seccomp <fd>` (stretch; document as v2) |

**`unshare` fallback MUST prove isolation post-spawn — never assume the flags
worked.** `bwrap` is the primary mechanism; when it is absent the `unshare`
fallback is only sound if the requested namespaces actually took effect.
`unshare --net` needs an unprivileged user namespace to succeed for a non-root
user; on a host where unprivileged userns is disabled (`kernel.unprivileged_userns_clone=0`,
seccomp/LSM policy, some container hosts) the call can **partially fail or
silently no-op yet still return exit 0**, leaving a process that runs with full
host networking. Therefore, as the first action *inside* the unshared child and
before any untrusted code runs, the jail MUST **assert every namespace it
claimed**:

- **Net:** assert the new net namespace is empty — no non-loopback interface, no
  default route, no reachable network (e.g. the child confirms its net-ns id
  differs from the host's and that no route to a public address exists). Any
  proof failure ⇒ hard-fail.
- **PID + mount + UTS/IPC:** assert the child is PID 1 in its namespace and that
  the mount/UTS/IPC namespaces differ from the host's (compare `/proc/self/ns/*`
  ids against the parent's).

If any assertion does not prove the namespace took effect, the fallback
**HARD-FAILS to the refusal path** (below) — it never proceeds to compile on the
assumption that `unshare` succeeded. bwrap remains the primary and is not subject
to this (it fails closed itself); this hardens only the fallback.

If neither `bwrap` nor `unshare` is available — or the `unshare` fallback cannot
*prove* its namespaces — `ipe add` **refuses** with a clear error (never silently
runs a raw compile). `IPE_FFI_ALLOW_UNSANDBOXED=1` is the only override and MUST
print a red trust warning.

### A.4 The explicit trust-decision gate

`ipe add <crate>` is a **trust decision** and MUST surface it:

1. Print the crate, resolved version, git URL (if any), and the transitive-dep
   count that will be *compiled*.
2. Require interactive confirmation (or `--yes` for CI) **before** any fetch.
3. **Fetch is its own network-enabled phase**, isolated from compile: fetch with
   network on into a scoped registry cache, then compile/inspect with
   `--unshare-net`. Never fetch and compile in one un-gated networked step.
4. **`--frozen --locked --offline` / `CARGO_NET_OFFLINE=1` enforcement.** After
   the fetch phase, all cargo invocations run `--frozen --locked` so no implicit
   re-resolution or network touch can happen during inspect/compile.

### A.5 `--git` URL gating (in the ported driver)

The Rust port replaces the `sh -c` string with a **direct `std::process::Command`
argv** (no shell — kills the injection class structurally). Additionally:

- **Scheme allowlist:** `https://` only (optionally `ssh://`/`git@` behind a
  flag). Reject `file://`, `http://`, and anything else.
- **Host charset + optional host allowlist:** URL host must match
  `[A-Za-z0-9._-]+`; an `IPE_FFI_GIT_HOSTS` allowlist (default: `github.com`,
  `gitlab.com`, `codeberg.org`) may further constrain it.
- **rev/branch/tag mutual-exclusion** enforced in the driver (the Haskell TODO at
  `Ffi.hs:94` is now a hard check).
- **crate name** validated via the inspector's `safe_crate_name`
  (`main.rs:3756`: `[A-Za-z0-9_-]+`, non-empty) *before* it reaches any command.

### A.6 Bounded introspection

A Stripe-SDK-scale crate (76k symbols in the Haskell benchmark) must not OOM the
host. Cap JSON output size, stream/DCE unused bindings before emission, and honour
the resource rlimits in A.3. The inspector's fail-closed over-drop already bounds
*what* is bound; the sandbox bounds *what it costs*.

---

## B. Phase 0 — inspector hardening (Milestone M-0, parallel-safe)

The inspector lives in a **disjoint `tools/` crate** with no dependency on the
Ipê workspace, so this milestone is doc/disjoint-parallel-safe (see §E) and can
run concurrently with the generator port's design milestones.

### B0.1 Pin reproducibility (blocks nondeterministic inspects)
- Add `tools/ipe-ffi-inspector/rust-toolchain.toml` with a **nightly pin**
  (rustdoc JSON is nightly-only; the exact channel is the drift-fence anchor).
- Vendor `tools/ipe-ffi-inspector/Cargo.lock` (commit it).
- Pin the three deps (`serde`, `serde_json`, `tempfile`) to exact versions.
- Restore these under a nightly CI job that rebuilds the inspector from the pin.

### B0.2 Fail-closed the internal parse paths (risk #3 — DoS on malformed JSON)
The inspector currently carries **42 `unwrap()` lines (45 occurrences), 57 `expect(...)`, 31 `panic!`**
(verified). Its `Cargo.toml` **already carries a `[lints.clippy]` block that
deliberately sets `unwrap_used` / `expect_used` / `panic = "allow"`, with a
justifying comment** (`tools/ipe-ffi-inspector/Cargo.toml:12-19`). So B0.2 is a
**REVERSAL of that deliberate decision** — flip those three to **deny** — not an
additive tightening on a clean slate; it exposes the ~130 call sites the `allow`
was chosen to avoid churning. rustdoc JSON is attacker-influenced (a malicious
crate shapes its own doc output). A panic here is a DoS and violates the "no
partial/guessed bindings" rule. Required:
- **Flip `clippy::unwrap_used`, `expect_used`, `panic` from `"allow"` to
  `"deny"`** on the crate (reverse the prior decision; record why the original
  `allow` no longer holds); drive the count to zero on every path that touches
  decoded rustdoc JSON.
- On any internal parse failure, **return an error-`PkgInfo`** (the
  `errors: Vec<String>` field already exists, `main.rs:451`) and exit non-zero —
  never abort. The generator side already treats a non-empty `errors` /
  empty-output as fail-closed (`Ffi.hs:133`).
- **Preserve the over-drop keystone verbatim:** absence ⇒ over-drop (sound); the
  binder must never *under-bind*. Every fail-closed drop point
  (`main.rs` "over-drop is sound" comments at 812, 1667, 1965, 2950, 4578, 4634,
  4670…) is load-bearing and must survive hardening unchanged. Over-dropping a
  bindable symbol is acceptable; emitting an unsound binding is not.

### B0.3 Adversarial-JSON fuzz
Add a fuzz/property target feeding malformed + adversarial rustdoc JSON
(truncated, wrong types, huge arrays, cyclic ids, non-UTF-8) and assert: no panic,
bounded memory, error-`PkgInfo` out. This is the acceptance test for B0.2.

---

## C. Milestone-sequenced generator port

Each milestone is a guardian-gated Workflow: **Opus design → Sonnet impl → Haiku
mechcheck → Opus review** (per the backend-wiring protocol). Target: a new
`ipe_ffi` crate in the Ipê workspace. Haskell source ranges below are the port
inputs; we mirror behaviour, not bytes.

### C.0 Dependency ordering (read first)
```
M-0  inspector hardening ────────── parallel-safe (disjoint tools/ crate)
M4   kernel registry ───────────── BLOCKS the consumer side entirely
        │
M-A  PkgInfo decode (parse-don't-validate)
M-B  Call AST decode + IPE-F4400 gate  (FfiCall)  ← the render-totality keystone
M-C  scalar coercion (NumCoerce)   ← leaf, no deps beyond types
M-D  sync wrapper emit (Ffi.emitRustFile) + .ipei + kernel.json
M-E  generic monomorphisation (FfiInstance) + closure gates + MODELLABLE_5 fence
M-F  driver: ipe add/install/remove + sandbox (A) + dynamic Cargo.toml
M-G  async → Task Error a bridge + catch_unwind + panic-profile gate
```
**The consumer port BLOCKS on the M4 kernel registry.** An FFI binding is a
kernel whose signature came from introspection instead of the stdlib; both must
share one `KernelEntry { sky_signature, per-backend emission, origin }` with
`origin: Stdlib | Ffi { crate }`. No FFI milestone past M-A can wire into
canon/lower until M4's registry exists. This is called out here so the schedule
does not start M-D/M-E against a `KernelFn` enum that has to be re-keyed later.

### M-A — `PkgInfo` decode as the single typed parse point
- **Port from:** inspector `PkgInfo`/`Function`/`Param`/`Generic`/`Call`/
  `Receiver`/`TypeRef`/`TransitiveDep` structs (`main.rs:26-475`); Haskell
  `FnInfo`/`PkgInfo` (`FfiGen.hs:70-249`).
- **Shape:** `serde` structs in `ipe_ffi::pkginfo`, deriving `Deserialize`, with
  **validating decoders** — not bare `Deserialize` — for anything the renderer
  depends on. This is rule (1): the JSON is parsed **once** here into types that
  make the downstream render total.
- **Parse-don't-validate invariants enforced at decode** (mirror
  `FfiCall.validateCall`, §M-B): receiver-iff-method, arg-index bounds +
  gap-free-from-0 + uniqueness, `argTypes` arity match, closure-only-as-direct-arg,
  `iterAdapters` target-is-Vec.
- **Opaque foreign types → `Ty::Con { module, name }`.** The `.ipei` catalogue is
  the type-env seed; opaque types unify nominally, never re-coerce to `any`.

### M-B — `Call` AST decode + the `IPE-F4400` reject-at-decode gate (keystone)
- **Port from:** `FfiCall.hs` in full — `data Call/CallKind/Receiver/ByKind/
  ClosureKind/TypeRef` (73-226), `validateCall` (256-333), `parseCall` (764-820),
  `renderCall`/`renderRetType`/`renderArgType`/`renderTypeRef` (382-756).
- **This is the "invalid states unrepresentable" core.** `parseCall` runs
  `validateCall` inside the `serde` decode; a malformed `Call` is a **hard decode
  error**, surfaced as diagnostic **`IPE-F4400`** ("foreign-call AST cannot be
  rendered"). It never reaches emission. The seven structural checks
  (`FfiCall.hs:246-333`, the seventh being the `iterAdapters` target-is-`Vec`
  check dispatched at `FfiCall.hs:299`) guarantee `renderCall` is **total** once decode
  passes — every `TypeRef` constructor maps to valid Rust, no `F?` fallback
  escapes.
  - `unknown call kind ⇒ reject` (`parseCall:767`).
  - closure only as a *direct* `argTypes` element; `Vec<closure>` / closure in
    `ret`/`typeArgs`/turbofish ⇒ reject (`validateCall:280-290`,
    `nestedClosure`/`hasClosure:357-367`).
- **The ported decoder's error type is a typed `Diagnostic`, NOT
  `Result<_, String>`.** The Haskell `validateCall :: Either String …` and
  `parseCall`'s `fail String` are stringly-typed error carriers that MUST NOT
  port verbatim — a `Result String a` in a public surface violates the
  no-`Result String a` non-regression rule (AGENTS.md §8). The Rust decoder
  returns `Result<Call, Diagnostic>` where `Diagnostic` carries the `IPE-F4400`
  code + structured span/reason; the serde error path is adapted into that typed
  `Diagnostic`, never surfaced as a bare `String`.
- **Port `NumCoerce` first as a leaf dep** (`FfiCall`/`FfiInstance` import it;
  `Ffi → FfiInstance → FfiCall` import cycle means `NumCoerce` must be the leaf) —
  see M-C.
- **Drift-fence test:** a decode corpus of accept/reject `Call` JSON (mirror
  `FfiCallSpec.hs`) asserting each of the seven checks rejects with `IPE-F4400`
  and each well-formed call renders byte-stable.

### M-C — scalar numeric coercion (`NumCoerce`), correctness gate (risk #4)
- **Port from:** `NumCoerce.hs` in full (`numSaturate`, `numWidenScalar`,
  `numCarrier`, `isNumericRust`).
- **Correctness ruling: saturating coercion is the SANCTIONED policy, not a
  silent-coercion violation.** Every width cast is **total + saturating**, never
  wrapping: `u64` param ← `i64` is `.max(0) as u64`; `u64`/`usize`/`u128` return →
  `i64` is `.min(i64::MAX …) as i64` (`NumCoerce.hs:70,101`); platform widths
  (`usize`/`isize`) route through `try_from` so they are 32-bit-correct by
  construction. The "no silent numeric coercion" rule (AGENTS.md) is satisfied
  because the clamp is *documented and total*, not a `-1 → 3.4e38` sign-flip.
  **Record this as a sanctioned divergence** (`oracle_divergence` + reason) in the
  port: a value above `i64::MAX` saturates rather than wraps or errors. Preserve
  the "one saturating helper" invariant (`NumCoerce.hs:8-11`) — exactly one source
  of scalar widening; `translateRustRet`'s scalar tail must delegate here.

### M-D — sync wrapper emit + `.ipei` + `kernel.json`
- **Port from:** `Ffi.hs:emitRustFile` (658-1426), `emitRustSkyi` (285),
  `emitRustKernelJson` (1488), `skyTypeToRust` (331), sequence classifier
  `seqKind` (467), `translateRustRet` (577), naming (`wrapperRefName:236`,
  `rustModuleName:262`, `rustKernelName:270`), plus the shared `.ipei`/`kernel.json`
  shape from `FfiGen.hs:emitKernelJson`/`emitSkyi` (441/1834).
- **No injection from crate metadata (A.2 discipline at emit).** Foreign
  crate/type/fn names flow into generated Rust source; validate every one as a
  Rust identifier and render string literals via `rustStrLit` (`Ffi.hs:412`).
  `absolutizeCrate` (`Ffi.hs:381`) keeps extern refs unambiguous.
- **`.ipei` is the type-env seed** (rule 1). `sky_types` loads it; FFI call sites
  type-check against it. `kernel.json` resolves the call to a `KernelId` in the M4
  registry. **Divergence to close (risk #4):** the Haskell has a
  `kernel.json`-vs-`.ipei` getter-fallibility mismatch — reconcile so a field
  getter's fallibility (`Maybe`/`Result` vs infallible) is identical in both
  artifacts (`infallibleFfiFn:1573`). File a golden that diffs the two.
- **`emitRustFile` uses BEGIN/END wrapper sentinels** (`wrapperBeginSentinel:252`,
  `wrapperEndSentinel:257`) for per-fn region slicing — port the sentinel protocol
  so DCE can drop unused wrappers.
- **Acceptance:** fixture **107** (`semver`, shim-free) round-trips: inspect →
  `.ipei` + `kernel.json` + `<crate>_bindings.rs` → Ipê type-checks a call → cargo
  builds → runs. Byte-diff the emitted `.ipei`/`kernel.json`/wrapper against the
  sky fixture (`upstream:runtime-rust/tests/sky/107-ffi-shimfree-semver`).

### M-E — generic monomorphisation + closure gates + MODELLABLE_5 drift fence
- **Port from:** `FfiInstance.hs` in full — `FfiInstance`/`GenericFn` (108-135),
  `checkInstances`/`checkInstance` (140-172), `skyTypeToRustClosed` (263),
  `modellableTrait` (292), `rustTypeHasTrait` (297), `traitsOfRustType` (408),
  the closure-capture gate `closureCaptureGate`/`skyCaptureIsClone`/
  `rustTypeIsClone` (308-390), `synthesiseGenericWrapper` (518), `closureDropReason`
  (897), and the `IPE-F4400`-family diagnostics (`mkClosedSetError:184`,
  `mkTraitBoundError:200`, `mkUnmodellableBoundError:227`, `mkUnmodellableFnError`).
- **MODELLABLE_5 drift fence (explicit deliverable).** The inspector's
  `MODELLABLE_5 = {Hash, Eq, Ord, Clone, Default}` (`main.rs:411`; doc comment
  `:409`) is the exact set of trait bounds the parametric-stub monomorphiser can
  model. The Haskell `modellableTrait` must agree. Port **both** sides and add
  the two-way fence test (mirror inspector `main.rs:12962-12971` which asserts
  `MODELLABLE_5` is EXACTLY the modellable subset and
  `MARKER_TRAITS.len() > MODELLABLE_5.len()`): if either
  side's set changes without the other, the fence fails. A bound outside the set ⇒
  **over-drop** (sound), never guess.
- **Demand-driven monomorphisation:** the type-directed lowering already threads
  concrete instantiations (`SolvedTypes.regions → IrType`); lower passes the
  concrete instance to the registry entry, and `synthesiseGenericWrapper` emits
  the monomorphic wrapper. Keep foreign generic entries instantiable per call site.
- **Closure soundness:** a Ipê lambda captured into a multi-call `Fn`/`FnMut` slot
  must be `Clone` (`closureNeedsClone`, `FfiCall.hs:581`; capture gate
  `FfiInstance.hs:322-390`) — else `IPE-F4400`. This is the boundary that keeps
  "well-typed Ipê never panics" across FFI.
- **Acceptance:** fixtures **92** (generic-self open-T), **105** (generic struct
  accessor). Regression coverage: walls **60/66/73/76/92/105/106**.

### M-F — driver (`ipe add`/`install`/`remove`) + sandbox + dynamic Cargo.toml
- **Port from:** `Ffi.hs` driver front-doors `runRustInspector*` (87-191),
  `generateRustBindings` (195), `resolveRustInspector` (295), transitive-dep +
  feature handling read off `PkgInfo.transitive_deps`/`features`
  (`main.rs:428/473`, `build_dep_entry:1502`, `choose_visibility_features:1488`).
- **Sandbox (A) is wired here.** Replace the `sh -c` string (`Ffi.hs:131`) with a
  direct argv `Command`, wrapped in the bwrap/unshare jail. Fetch phase and
  inspect/compile phase are separated; `--frozen --locked --offline` enforced
  post-fetch; git URL gated per A.5.
- **Dynamic `Cargo.toml` injection.** The emitted manifest becomes a generator:
  base manifest + one `[dependencies]` line per FFI crate a program uses, with the
  **exact pinned version** and **effective feature set** from `PkgInfo`
  (`transitive_deps` → resolve `_`→`-` + version without guessing;
  `features` → merge the set rustdoc succeeded with, else feature-gated types
  vanish — the firestore #73/#100 class, `main.rs:464-474`). Never emit `"*"`.
- **`~/.cache/ipe/ffi/rust/` cache.** `<crate>.ipei`, `<crate>.kernel.json`,
  `<crate>_bindings.rs`, `<crate>.coverage.md` (never regenerated on warm cache;
  explicit `ipe add`/`install` step, mirroring the ipe watch-cache rule).
- **Acceptance:** `ipe add`/`remove` round-trip on a shim-free crate; a
  from-scratch `ipe install` regenerates the cache; malicious-crate-name and
  bad-git-URL are refused before any compile.

### M-G — async → `Task Error a` bridge + `catch_unwind` + panic-profile gate
- **Port from:** the async wrapper body path in `FfiCall.renderCall` gated by
  `_call_isAsync` (`FfiCall.hs:797`), the closure `catch_unwind` boundary
  (`Ffi.hs:53-65` doc contract), and `cargoProfilePanicIsUnwind` (`Ffi.hs:66-82`).
- **`catch_unwind` → `Err` (soundness bridge).** Every foreign call in the wrapper
  is wrapped so a panicking/aborting FFI fn becomes an Ipê `Err`, preserving
  guarantee (b) — well-typed Ipê never panics — across the boundary.
- **Panic-profile gate (soundness keystone).** `catch_unwind` is sound ONLY under
  `panic = "unwind"`; under `panic = "abort"` a foreign panic aborts the process
  before the boundary can convert it to `Err` — the keystone-forbidden
  emit-then-abort class (well-typed Ipê observes a hard process kill with no
  diagnostic). The gate MUST be a **compile-time fence emitted IN the wrapper
  crate**, not a text-scan of `Cargo.toml`. A text-scan cannot see the *effective*
  panic strategy: a workspace-root `[profile.*] panic = "abort"`, a
  `RUSTFLAGS`/`CARGO_BUILD_RUSTFLAGS=-Cpanic=abort`, or a `-Zbuild-std` rebuild all
  set abort without touching the emitted manifest. Mandate, at the top of every
  emitted `<crate>_bindings.rs` wrapper module:
  ```rust
  #[cfg(panic = "abort")]
  compile_error!("ipe_ffi catch_unwind boundary requires panic=unwind");
  ```
  `cfg(panic)` is stable since Rust 1.60, so this fires on the *actual* compile
  configuration regardless of how abort was selected — cargo fails loudly at
  build time instead of silently shipping a binary that aborts on a foreign panic.
  Port `cargoProfilePanicIsUnwind` and keep it, but **demote it to an advisory
  fast pre-check** (a friendly early error when the emitted manifest itself
  declares `panic = "abort"`); the `compile_error!` fence is the sound gate. The
  dynamic manifest (M-F) sets no `panic =` key (cargo default = unwind); the fence
  makes the requirement enforced rather than assumed.
- **Async result mapping (risk #4).** A foreign async fn returning
  `Result<_, E>` must map to **`Task Error a`**, never `Task String a` /
  `Result String a` (AGENTS.md non-regression). The wrapper's error arm constructs
  a typed `Error`, not a stringified one. `Send`-closure discipline: async
  combinator args need `Box<dyn Fn + Send + 'static>`; the inspector's
  `recv_provably_async_send` / `PROVABLY_SEND_OPAQUE_NAMES` machinery
  (`main.rs:4923`, XcImpl `send_ok:824`) supplies the Send verdict — an unprovable
  concrete keeps dropping (over-drop, sound).
- **Honest scope:** async is where shim-free is NOT yet proven. See §D.

---

## D. Acceptance bar + honest scope

**Proven shim-free (must pass, byte-diff vs sky output):** the 10 pure/sync
crates behind fixtures **107-114**
(`upstream:runtime-rust/tests/sky/{107 semver, 108 multi, 109 multi2, 110 toml,
111 serde-json, 112 regex, 113 bytes, 114 jiff}` + the numeric/borrow/serde
regressions **73/76/92/97/105/106**).

Acceptance ladder:
1. **One pure crate end-to-end** — `semver` (fixture 107): inspect → emit → Ipê
   type-check → cargo build → run, byte-diff `.ipei`/`kernel.json`/wrapper.
2. **The 10 shim-free crates** — all of 107-114 green, byte-diff vs sky.
3. **Async SDKs (firestore / firebase / stripe) are EXPLICITLY NOT claimed
   shim-free.** They are still hand-shimmed even in upstream Sky. M-G proves the async
   *bridge* on a small async crate; large async SDKs remain out of the shim-free
   claim until demonstrated. Do not market shim-free for them.

**Golden oracle = the sky fixtures.** Byte-diff is the pass/fail. A sanctioned
divergence (e.g. the saturating u64→i64 in M-C) is recorded as
`oracle_divergence = true` + reason, never a silent mismatch.

---

## E. Build-lane / parallelism

| Work | Lane |
|---|---|
| This spec (M-0 §A/§B docs) | doc-only, parallel-safe |
| **M-0 inspector hardening** | **disjoint `tools/ipe-ffi-inspector` crate — no Ipê-workspace dep; parallel-safe with the design milestones of C** |
| M-A/M-B/M-C (`ipe_ffi` decode + coercion, pure logic) | new-crate, mostly parallel; but M-B/M-C are leaves M-D/M-E depend on, so land them first |
| **M4 kernel registry** | **serializes the whole consumer side — M-D onward blocks on it** |
| M-D/M-E/M-F/M-G | serialize behind the **shared workspace `target/`** (any change to registry/canon/lower/backend forces a workspace rebuild) and behind M4 |

Doc + disjoint-tool work parallelises; anything touching the shared workspace
`target/` or the kernel registry serializes. Do not spawn M-D against a
`KernelFn` enum — wait for M4's registry so FFI drops onto the same rails.

---

## One-line summary

Sandbox first (bubblewrap, network-denied, tempdir-scoped, trust-gated `ipe add`)
because compiling an untrusted crate is RCE; harden the inspector to fail-closed
(no unwrap/panic on adversarial rustdoc JSON) while preserving the over-drop
keystone; then port the Haskell generator to `ipe_ffi` milestone-by-milestone
behind the M4 kernel registry, with the `Call`-AST decode gate (`IPE-F4400`,
parse-don't-validate) and the saturating `NumCoerce` as the correctness/soundness
keystones, proving shim-free on the 10 pure crates (107-114) by byte-diff and
explicitly deferring the async-SDK shim-free claim.
