# FFI-to-Rust Subsystem — Design + Implementation Plan (Tier 2)

**Status:** design-ahead. Tier-2 FFI is implemented LAST per the roadmap
(`ROADMAP.md` §B); nothing in this doc executes now.

> **GOAL CORRECTION (supersedes this doc's original scope + P-ordering).** The goal is fully-automatic, UNIVERSAL (every crate) Ipê→Rust
> FFI. **The acceptance metric is `../sky/examples/rust/skyshop-rs` running with
> ZERO manual shims for firestore, firebase, and stripe, plus DCE of unused FFI
> functions/values** (bind/emit only the used subset; stripe visible surface =
> 20,358 symbols post-#89, 3,534 bound). CORRECTED PREMISE: the reference did
> NOT give up on async — it binds foreign `async fn` as `Task Error a` natively
> binds firestore 0.49 direct and shim-free (fixture
> 104), and proved every stripe mechanism on fixtures 93-96 (WALL-I/J/K); the
> skyshop shims are a pre-#44 fossil. We PORT that campaign and FINISH its two
> open ends — the never-run real async-stripe E2E build, and the never-done
> skyshop de-shim. "Exceed the reference" means exactly the acceptance metric
> above, not out-inventing its async design. Consequence: §6's "P7 async bridge
> LAST" ordering is WRONG — async emission is part of the base generator (P1-P3)
> per the authoritative conciliated design, **`async-ffi-bridge-design.md`**,
> which re-slots the P-phases (its §9) and supersedes P7/M-G. The
> reference-validated architecture in §1-§5 (inspector, generator, sandbox,
> no-fallback wall) stands unchanged.
**Relationship to prior docs:** this doc RECONCILES and PARTIALLY SUPERSEDES
the banked #39 suite — `ffi-design.md`, `ffi-port-spec.md`,
`ffi-subsystem-design.md`, `ffi-sandbox-and-generator-impl-ready.md` — now
that the reference (`/home/arthur/Documentos/comp/sky`, branch
`feat/runtime-rust` @ v0.17.2-1204) has a COMPLETE, shipping Rust-crate FFI
story. §7 states exactly what is confirmed, what changed, and what is
superseded. The four banked docs remain the implementation-level detail
source for the sandbox argv and the `ipe_ffi` module internals; this doc is
the architectural umbrella and the executable milestone plan.

---

## 0. Executive summary

The single most important learning: **the reference already does exactly what
our premise predicted — it binds RUST CRATES directly, shim-free, via a
rustdoc-JSON inspector + Haskell generator, with a hard no-fallback wall
between the Go-FFI and Rust-FFI worlds.** Our banked spec was written against
the reference's state at the time of writing and is remarkably current; the
reference completing does not invalidate it — it CONFIRMS its architecture
and sharpens four things (§7):

1. The sandbox (#41) gates **shipping `ipe add`** (the driver, milestone
   M-F), not **building the generator** (M-A..M-E, a pure JSON→files
   function testable on committed fixture JSON). The build order in the
   original #39 suite (`#40 → #41 → #42`) is refined to
   `#40 ∥ #42-generator → #41 → #42-driver → E2E`.
2. Our vendored inspector is NOT stale — it is a superset of the local
   reference checkout (18,500 vs 16,853 lines, vendored 2026-06-29 from the
   v0.17.3 line, both carrying the #109/#110/WALL-G markers). The sync
   direction in any future refresh is upstream-tag → us, not "catch up now".
3. Our runtime's refusal of dynamic FFI dispatch
   (`runtime/tests/ffi_call_task_divergence.rs`) is **parity with the
   reference's Rust backend**, not a divergence from the reference — the
   reference's own `runtime-rust/src/sky_runtime/ffi_polyfills.rs` contains
   the same two panicking polyfills with the same rationale. The only thing
   it diverges from is the Go runtime's `%v`-string registry.
4. The reference binds async natively — firestore 0.49 direct +
   shim-free (fixture 104, `IPE_DCE=0` residual 124 → 10), all stripe
   mechanisms proven on fixtures 93-96. `examples/rust/skyshop-rs` still
   *ships* its three shim crates, but that is an unfinished migration
   (pre-#44 fossil), not a capability boundary. The honest scope statement
   is now: 10 sync crates + firestore proven; real async-stripe E2E and the
   skyshop de-shim are the two open ends we finish (P6/P7 of the
   conciliated plan).

---

## 1. What the reference actually does (the learning)

All citations are against `/home/arthur/Documentos/comp/sky` @
feat/runtime-rust v0.17.2-1204 (`c818e081`).

### 1.1 The inspector: `sky-ffi-inspect-rs` (rustdoc JSON, nightly)

`tools/sky-ffi-inspect-rs/src/main.rs` (16,853 lines in the local checkout).

- **Mechanism:** `cargo +nightly rustdoc --output-format json`
  (`main.rs:1169` introspect; `main.rs:1118` `cargo check`; `main.rs:1284`
  `fetch_dep`). Rustdoc runs **after macro expansion**, so proc-macro/derive
  generated items invisible to syn are fully visible (`main.rs:3-6` states
  this rationale explicitly). Nightly is pinned only by channel
  (`tools/sky-ffi-inspect-rs/rust-toolchain.toml:1-3`, `channel = "nightly"`,
  no version pin — a reproducibility gap our #40 closes).
- **Security posture:** it EXECUTES untrusted code — every `build.rs` in the
  dep closure runs under `cargo check`, and proc-macros execute during both
  compile and rustdoc phases. The reference runs this **unsandboxed**, via
  `sh -c` + shell-quoting from Haskell
  (`src/Ipê/Build/Rust/Ffi.hs:216-222`,
  `readProcessWithExitCode "sh" ["-c", cmd']`). Crate names are gated by
  `safe_crate_name` (`main.rs:3756`, `[A-Za-z0-9_-]+`), git URLs are only
  shell-quoted. This is the gap our sandbox (#41) is for — a **sanctioned
  divergence where we are strictly better** (§5).
- **Output:** a `PkgInfo` JSON (`main.rs:432-470`): `pkg`, `name`,
  `version`, `functions: Vec<Function>`, `modules`, `errors`, `notes`,
  `transitive_deps` (locked versions, WALL-B #75), `features` (the effective
  feature set rustdoc actually succeeded with, #100 Part B —
  `main.rs:982-986` in our vendored copy). Each `Function` carries `name`,
  typed `params`/`results` (Ipê type + Rust type per param), **`effect`
  (`main.rs:50` — pure/fallible/effectful; present in BOTH Go and Rust
  inspectors)**, receiver/method metadata, field/enum-binding flags
  (`is_enum_ctor`/`is_enum_tag`/`is_enum_extract`), `self_returning`
  (owned-threading setters), `call_path` (#109 submodule free fns), and an
  optional `generic` block: type params, per-param trait bounds, and a typed
  **Call AST** (`kind`, `path`, `typeArgs`/`argTypes`/`ret` as a `TypeRef`
  tree, receiver `ref|refmut|value`, `iterAdapters`, `borrowAsRefArgs`,
  UFCS `traitQualifier`, `isAsync`, `methodTurbofish`).
- **Type mapping (kept):** primitives; `String`/`&str` (via `AsRef`
  coercion, WALL-3b); `&[u8]`/`Vec<u8>` → Bytes-like; `Vec<T>` → `List`;
  `Option<T>` → `Maybe`; `Result<T,E>` with error stringified; serde-bounded
  generics → JSON-text `String` (WALL 3a); Clone-opaque nominal handles for
  everything else reachable at a public path.
- **Type mapping (rejected, fail-closed "over-drop is sound"):**
  non-elidable lifetimes, raw pointers, trait objects (`dyn`), `impl Trait`
  returns, closures as anything but a direct arg (Phase 6.2 future),
  ambiguous trait-bounded generics (0 or >1 unique impls →
  `GenericDrop::TraitBoundedParamAmbiguous`, `main.rs:~10263`), non-Clone
  opaques in sequences, async methods on receivers without a Send proof
  (`recv_provably_async_send`, `main.rs:4923`). Trait bounds are modellable
  only within the `MODELLABLE_5` set {Hash, Eq, Ord, Clone, Default} with a
  compile-time fence (`main.rs:~12962`).
- **Classification:** free fn / constructor (`new`, `from_str`, `parse`,
  `builder`, … prefixes) / accessor / synthetic field getter+setter /
  pkg-level const / enum-variant ctor+tag+extract — exactly the taxonomy our
  `sky-rust-backend:ffi-audit` skill counts
  (`runtime-rust/plugins/sky-rust-backend/skills/ffi-audit/ffi_audit.py`,
  verdict heuristic at lines 140-155: `rich ≥ 10` constructables, `usable
  3-10`, `thin 1-3`, `peripheral 0`).

### 1.2 The generator + driver (Haskell): `sky add <crate> --backend rust`

- **CLI:** `app/Main.hs:878-996` (`addHandler`). For Rust it discovers the
  Cargo package name (Main.hs:919) and invokes the inspector — which ships
  **embedded in the sky binary** (`Sky.Build.EmbeddedInspectorRust`,
  imported at `src/Ipê/Build/Rust/Ffi.hs:44`; materialized to
  `<dir>/bin/sky-ffi-inspect-rs`, `Ffi.hs:319`). Git sources are supported
  (`--git URL --rev|--branch|--tag`, `Ffi.hs:122-125`) and a manifest mode
  inspects multiple crates in one process (WALL-G #84, `Ffi.hs:175`).
- **Artifacts** (`generateRustBindings`, `Ffi.hs:195-219`), all under
  `.ipe-cache/ffi/rust/`:
  - `<slug>_bindings.rs` — the wrapper crate source (direct `::crate::fn`
    calls, sync `SkyResult` wrappers);
  - `<slug>.skyi` — the Ipê-typed catalogue seeding the HM env
    (`FfiGen.hs:1924-2022` for the Go twin; comment-based signatures with
    `[pure|fallible|effectful]` effect annotations);
  - `<slug>.kernel.json` — the registry entry consumed at every warm build.
- **Registry:** `src/Ipê/Build/FfiRegistry.hs:273-284` —
  `loadRegistry BackendRust` reads ONLY `.ipe-cache/ffi/rust/`; if empty it
  warns about stale legacy layout and returns an **empty registry — there is
  NO Go-FFI fallback**. `[go.dependencies]` is inert under
  `backend = "rust"` (`app/Main.hs:1087-1095`, "the Rust codegen can't link
  Go packages"). Kernel names derive as `Rust_<Crate>`
  (`FfiGen.hs:419-433`).
- **DCE:** FFI refs are tracked as `FfiRef kernelName fnName`
  (`src/Ipê/Build/Dce.hs:19-22, 64-66`); unreached bindings never emit.
  This is the 76k-symbol scale answer, together with per-crate
  `.kernel.json` caching (warm builds never re-run the inspector) and
  bounded-parallel inspection (`app/Main.hs:1013`, QSem +
  `IPE_INSTALL_PARALLEL`).
- **Generics — the (A)-model** (`src/Ipê/Build/Rust/FfiInstance.hs:1-30`):
  ONE `<T: bounds>` generic Rust wrapper per generic FFI fn; **rustc
  monomorphises the call sites** (unlike the Go model's N per-instance
  wrappers). Separately, a per-used-instantiation bindability check proves
  every concrete type-arg lies in the closed Ipê↔Rust set and satisfies the
  declared bounds via a static closed-set × trait table; violations are
  first-class E4400 diagnostics keyed to the call-site region — never a
  silent drop.
- **Cargo wiring:** the emitted project's `Cargo.toml` gets one
  `[dependencies]` line per FFI crate at the **exact locked version** from
  `transitive_deps`, plus the **effective feature set** the inspector
  succeeded with (`FfiGen.hs:186-201`; crate-version SSOT at
  `src/Ipê/Generate/Rust/Builder/crate-specs.toml`).

### 1.3 The Rust runtime's FFI stance (identical to ours)

- `runtime-rust/src/sky_runtime/ffi_polyfills.rs:1-63` — static dispatch
  only. The codegen peephole rewrites literal-shaped
  `Ffi.callPure "<Kernel>" [args]` to direct monomorphic kernel calls; the
  two dynamic-dispatch polyfills **panic by design** ("exactly the dynamism
  this backend exists to refuse", lines 53-61). The crate-wide lint wall
  (`runtime-rust/src/lib.rs:19-34`) denies `clippy::panic` everywhere except
  these two sites.
- Task boundary: `SkyTask<E, A> = Pin<Box<dyn Future<Output = SkyResult<E,
  A>> + Send + 'static>>` (`runtime-rust/src/sky_runtime/core.rs:17`).
  Effectful kernels reach Rust via `Ffi.kernel` Stage-4 rewrite, never via
  `Ffi.callTask`.
- Capability gating: `runtime-rust/Cargo.toml:74-119` defines the feature
  lattice (`db`, `crypto`, `server`, `http_client`, `websocket_client`,
  `live`, `webview`, …); the compiler's AST walker
  (`src/Ipê/Generate/Rust/Builder/Walker.hs:72-203`) detects kernel usage
  and the emitter (`Emitter.hs:~1121-1131`) enables only the needed
  features per program.

### 1.4 Honest scope in the completed reference

- The Rust-backend example sweep **excludes** every Go-FFI example:
  `runtime-rust/scripts/lib/examples.sh:47-128` (`is_out_of_scope`) admits
  only `Ipê.*`/`Ipe.*`, **`Rust.*` FFI wrapper imports**, and local
  modules; `13-skyshop` (Go Stripe/Firestore) is out of scope by
  construction (`examples.sh:53`).
- The Rust-native skyshop, `examples/rust/skyshop-rs/sky.toml`, binds
  **hand-written shim crates** (`sky-firestore-shim`, `sky-stripe-shim`) —
  the async SDK surface is NOT auto-bound.
- Auto-bound-and-proven: **10 shim-free pure/sync crates** — the fixture
  ladder our port-spec already mirrors (fixtures 107-114: semver, multi,
  multi2, toml, serde-json, regex, bytes, jiff; the 9th and 10th landed
  upstream 2026-06-28 as #109/#110).

---

## 2. Our current assets

| Asset | State |
|---|---|
| Banked #39 suite (`ffi-design.md` 121 L, `ffi-port-spec.md` 459 L, `ffi-subsystem-design.md` 685 L, `ffi-sandbox-and-generator-impl-ready.md` 618 L) | Design-locked, implementation-ready; already cites reference `main.rs`/`Ffi.hs` line numbers; sandbox host probe verified 2026-07-02 (bwrap ✓, unshare ✓, prlimit ✓, userns ✓) |
| `tools/sky-ffi-inspect-rs` (vendored, commit `8e549ca` 2026-06-29) | 18,500-line superset of the local reference checkout (16,853 @ 2026-06-28); both carry `#109`/`#110`/`WALL-G` markers and the `effect` field (`src/main.rs:50`). NOT stale. Missing: `rust-toolchain.toml` version pin, committed `Cargo.lock` (→ #40 B0.1) |
| `tools/oracle/bin/sky-ffi-inspect` | Go ELF binary, runs (verified: `--help` probe returns a well-formed error `PkgInfo` JSON). Oracle-only — Ipê ports from the Rust inspector, not this |
| `runtime/src/sky_runtime/ffi_polyfills.rs` + `runtime/tests/ffi_call_task_divergence.rs` | Vendored parity with the reference's runtime; the divergence test asserts dynamic `Ffi.callTask`/`callPure` panic with actionable messages and `Ffi.toAny` is identity (no erasure) |
| `docs/architecture/kernel-registry-design.md` | Two-tier `KernelId = Stdlib(StdlibKernel) \| Ffi(FfiKernelId)` — closed enum for stdlib exhaustiveness (F1), opaque index for open FFI (R0.2). This is the M4 dependency the consumer side blocks on |
| `sky-rust-backend:ffi-audit` skill | Already carried in-repo; wraps the inspector for the ~50-crate bindability audit |
| Compiler `Ffi.kernel` handling | Stage-4 kernel aliasing works for stdlib (`crates/ipe/stdlib/Ipê/Core/Dict.ipe:42` etc.); no `ipe add`-style consumer exists yet — that is #42 |

---

## 3. Architecture

```
 ipe add <crate> [--git URL --tag T] [--features f1,f2]
        │
        ▼
 ┌─────────────────────────────── SANDBOX (#41, ships before ipe add) ────┐
 │ Phase 1 FETCH   (network ON, scoped registry cache)   cargo fetch     │
 │ Phase 2 COMPILE (network OFF, --frozen --locked)      cargo check     │
 │ Phase 3 INSPECT (network OFF)                rustdoc --output-format  │
 │        bwrap --unshare-net … -- prlimit … -- sky-ffi-inspect-rs argv  │
 └───────────────────────────┬────────────────────────────────────────---┘
                             │  PkgInfo JSON (stdout, size-capped)
                             ▼
 ipe_ffi crate (#42 — pure function: JSON → files; no registry, no net)
   pkginfo.rs  wire DTO → TryFrom → domain PkgInfo   (M-A)
   num_coerce.rs  saturating scalar coercion, leaf   (M-C)
   call.rs     Call AST decode, 7 checks, IPE-F4400  (M-B, keystone)
   emit/       <crate>_bindings.rs  +  <crate>.ipei  +  <crate>.kernel.json  (M-D)
   instance.rs (A)-model generic wrapper + per-instantiation gate (M-E)
   async_bridge.rs  async fn → Task Error a          (M-G, last)
                             │
                             ▼   .ipecache/ffi/rust/<slug>.{ipei,kernel.json,_bindings.rs}
 consumer wiring (blocks on M4 kernel registry)
   canon:   Rust.<Crate>.<fn> → KernelId::Ffi(fid)   (sky_kernels::resolve)
   types:   .ipei → Scheme seeding of the HM env     (FfiRegistry, origin=Ffi)
   lower:   Call { callee: Kernel(Ffi fid), args }   (zero new match arms)
   backend: wrapper call emission + Cargo.toml dep line (locked version
            + effective features) + S4 sentinel-sliced wrapper DCE
```

### 3.1 Inspector (Phase 0, #40)

Keep the vendored rustdoc-JSON inspector as-is architecturally — it is the
proven design (proc-macro-generated items visible; syn-based parsing
rejected upstream for exactly that reason, `main.rs:3-6`). Harden per the
banked port-spec §B: de-workspace (own `target/`, own `Cargo.lock`, own
nightly `rust-toolchain.toml` with a pinned version, not just a channel);
flip `unwrap/expect/panic` lints allow→deny (≈130 sites → error-`PkgInfo` +
non-zero exit); adversarial-JSON fuzz corpus. Preserve the seven "over-drop
is sound" keystone comments verbatim. Distribution decision (diverges from
the reference's Haskell-TH embedding): ship the inspector as a **separate
binary distributed with the toolchain and invoked by argv** — the sandbox
requires an exec boundary anyway, and `include_bytes!`-embedding buys
nothing but binary bloat once the jail must materialize a file to exec.

### 3.2 Typed surface: `.ipei`

Rename of `.skyi`, same role: the HM-typed catalogue for the type-checker
and `ipe doc`. Two decode sites, one domain type (banked R0.1): add-time
decodes inspector stdout; warm-build-time decodes the cached
`<crate>.kernel.json` through the SAME `TryFrom` validators, so a
hand-corrupted cache is re-rejected. The `.ipei → Ty` decoding must be
proven **structurally identical** to the stdlib's exhaustive-projection `Ty`
for the same logical signature before the first `Ffi` entry lands (M-D
acceptance gate, banked D5). Type mapping is the banked D2 table —
notably `Result<T,E> → Result Error a` (E erased to `Error`, never
`Task String`), integer carriers `i64` with saturating `NumCoerce`
(sanctioned divergence: values above `i64::MAX` saturate, logged
`oracle_divergence = true`), and **no `Ty::Any` arm anywhere**.

### 3.3 Generator: the `ipe_ffi` crate

Topology and build order exactly as banked (`ffi-subsystem-design.md`
§crate topology; leaf-first DAG M-A → M-C → M-B → M-D → M-E → M-F → M-G per
R0.5, mirroring the Haskell `FfiCall`/`NumCoerce` cycle-break). Every
public fallible function returns `Result<T, Diagnostic>` with an
`IPE-F####` code. The keystone is `call.rs`: the `Call` AST is
unconstructible without passing the seven structural checks ported verbatim
from the reference's `FfiCall.hs:256-333` (param-index bounds, method ⇔
receiver, gap-free unique arg indices, arity match, no nested closures,
iter-adapter targets are `Vec`), failing as a closed `CallDefect` enum →
`IPE-F4400`.

### 3.4 Kernel-dispatch integration (no hand-registration)

FFI symbols enter the pipeline as **data, not code**: `.ipei` seeding
registers `(Rust.<Crate>.<fn>) → (KernelId::Ffi(fid), Scheme)` with
`origin = Ffi { crate }`; lowering emits
`Call { callee: Kernel(Ffi fid), args }` identically to stdlib kernels;
the backend resolves `fid → wrapper_ref_name` from the registry and emits a
direct call into `<slug>_bindings.rs`. Zero new match arms per binding —
the two-tier `KernelId` (kernel-registry-design.md) makes the stdlib side
stay closed/exhaustive (F1) while the FFI side stays open (R0.2). FFI ids
resolve to a total none-of-these for every classifier (`is_tea`, `is_db`,
…) so an FFI kernel can never be mis-routed into a stdlib fast-path.
Adopt the reference's kernel naming (`Rust_<Crate>`, import surface
`Rust.<Crate>`) and its (A)-model for generics: one `<T: bounds>` wrapper,
rustc monomorphises, per-instantiation bindability gate as a region-keyed
diagnostic (reference E4400 → ours `IPE-F44xx`).

### 3.5 Effect + Task boundary

The inspector's `effect` field (pure/fallible/effectful) drives wrapper
shape: pure → bare value; fallible → `SkyResult`; effectful/async →
`Task Error a` via M-G's bridge (`futures::FutureExt::catch_unwind` on the
pinned future, spawn onto the executor's reactor, never `block_on` inside
it). Every foreign call — sync or async — is wrapped in a catch-unwind
boundary so a foreign panic becomes a Ipê `Err`, preserving "well-typed Ipê
never panics". The wrapper crate carries the `compile_error!` fence
`#[cfg(panic = "abort")]` so a `panic=abort` profile cannot silently void
the boundary. Dynamic dispatch stays refused: the two panicking polyfills
and `ffi_call_task_divergence.rs` remain the guarded boundary — this is now
documented as **reference parity** (the reference's Rust runtime does the
same), not as a unilateral divergence.

---

## 4. Sandbox threat model + gate (#41)

Unchanged from the banked docs in substance; restated because it is the
blocking ship-gate and the one place we are deliberately STRICTER than the
completed reference (which still runs the inspector unsandboxed via
`sh -c`, `Ffi.hs:216-222` — recorded as a divergence where Rust/our-side is
strictly better, §5).

- **RCE surface:** `ipe add` executes untrusted `build.rs` + proc-macros
  twice (cargo check at reference `main.rs:1118`; rustdoc at `main.rs:1169`)
  and lets cargo fetch a full dep closure (`main.rs:1284`).
- **Critical-path rule:** untrusted code runs ONLY inside an explicit,
  interactive `ipe add`/`ipe install`, ONLY inside the jail. Warm
  `ipe build` reads cached `.ipei`/`kernel.json` and never re-invokes the
  inspector. CI never runs the inspector on untrusted crates.
- **Jail:** bwrap primary (`--unshare-net --unshare-pid --clearenv
  --ro-bind / / --tmpfs /home` + single writable scoped tempdir +
  `CARGO_NET_OFFLINE=1` + `prlimit` RSS/CPU/fd/proc/fsize caps + `timeout`
  wall clock; full argv in `ffi-sandbox-and-generator-impl-ready.md`
  lines 91-121). Fallback `unshare` MUST pass the post-spawn isolation
  proof (pid==1, ns ids differ from parent, no non-loopback iface) before
  exec'ing any payload; failure → hard refusal `IPE-F4410`. Escape hatch
  `IPE_FFI_ALLOW_UNSANDBOXED=1` prints a red trust warning; CI never sets
  it.
- **Phase separation:** fetch with network ON into a scoped registry cache;
  compile + introspect strictly `--frozen --locked --offline`.
- **Trust gate:** print crate, version, git URL, transitive-dep count;
  require confirmation (`--yes` for scripted use). `--git` restricted to
  `https://` + hostname charset + optional `IPE_FFI_GIT_HOSTS` allowlist.
- **No shell anywhere:** the driver uses argv `std::process::Command`,
  killing the reference's `sh -c` + quoteShell construction structurally.

---

## 5. Adopt vs diverge (sanctioned-divergence discipline)

Default is reference parity; every divergence has a reason and is recorded
here + in `docs/divergences-from-sky.md` when implemented.

| Concern | Reference does | We do | Verdict |
|---|---|---|---|
| Introspection mechanism | rustdoc JSON, nightly (`main.rs:3-6`) | same (vendored inspector) | **adopt** |
| PkgInfo/Call-AST wire schema | `main.rs:432-470` + Call AST | same schema, decoded via wire→domain `TryFrom` | **adopt** (schema) + hardened decode |
| Inspector distribution | TH-embedded in sky binary (`EmbeddedInspectorRust`, `Ffi.hs:44,319`) | separate toolchain binary, argv-exec'd | **diverge** — jail needs an exec boundary; TH embedding is a Haskell-ism |
| Inspector invocation | `sh -c` + quoteShell, unsandboxed (`Ffi.hs:216-222`) | argv exec inside bwrap/unshare jail | **diverge — strictly better** (RCE gate; Principle 1) |
| Toolchain pin | nightly channel only, no `Cargo.lock` | pinned nightly version + committed lock (#40 B0.1) | **diverge — strictly better** (reproducibility) |
| Cache layout | `.ipe-cache/ffi/rust/<slug>.{skyi,kernel.json,_bindings.rs}`, warm builds never re-inspect (`FfiRegistry.hs:273-284`) | `.ipecache/ffi/rust/`, same artifacts, `.ipei` extension; cache key includes nightly pin | **adopt** (+ key hardening) |
| Cross-backend fallback | none — Rust registry empty ⇒ empty, Go deps inert (`Main.hs:1087-1095`) | N/A — we have no Go backend at all | **adopt** (simpler: only one registry dir) |
| Kernel naming / import surface | `Rust_<Crate>` / `Rust.<Crate>` (`FfiGen.hs:419-433`) | same (post-rename: surface stays `Rust.<Crate>`) | **adopt** |
| Registry integration | data-driven `.kernel.json` registry beside closed stdlib dispatch | two-tier `KernelId = Stdlib(enum) \| Ffi(u32)` | **adopt** semantics; our shape is stronger (F1 exhaustiveness kept) |
| Generics | (A)-model: one generic wrapper, rustc monomorphises; E4400 per-instantiation gate (`FfiInstance.hs:1-30`) | same; diagnostic code `IPE-F44xx` | **adopt** |
| Effect classification | inspector emits `effect` per fn (`main.rs:50`) | same field, decoded into closed `Effect` enum, unknown string → hard error | **adopt** + fail-closed decode |
| Error type at boundary | foreign `Result<T,E>` → error stringified | `Result Error a` — E erased to Ipê `Error`, never `Result String` | **diverge — strictly better** (repo non-regression rule: no `Result String`) |
| Scalar coercion | Go-parity casts | saturating `NumCoerce` leaf, grep-fenced; > `i64::MAX` saturates | **diverge — recorded** (`oracle_divergence = true`; total + documented beats silent wrap) |
| Dynamic `Ffi.callTask`/`callPure` | Rust runtime panics by design (`ffi_polyfills.rs:53-61`) | identical (vendored) | **adopt** — reclassified from "our divergence" to reference parity |
| DCE of unused bindings | `FfiRef` reachability (`Dce.hs:19-22`) + sentinel-sliced `_bindings.rs` | same (S4, conservative-keep text slicing on BEGIN/END sentinels) | **adopt** |
| Cargo dep wiring | exact locked version + effective features from PkgInfo (`FfiGen.hs:186-201`) | same; resolve `(ident, canonical_name, version)` triple, never guess `_`→`-`, never `"*"` | **adopt** |
| Async SDKs | binds natively (firestore direct, fixture 104; stripe mechanisms proven on fixtures 93-96); skyshop-rs shims are an unfinished migration | port the async emission + finish the two open ends (real stripe E2E, skyshop de-shim) — see `async-ffi-bridge-design.md` | **adopt + finish** |

## 6. Milestone plan

Dependency DAG (refines the banked linear ordering — see §7 item 2):

```
#40 Phase 0 (inspector)────────────┐
                                   ├──► #42d M-F driver (ipe add/install/remove)──► E2E ladder
#42g generator M-A→M-C→M-B→M-D→M-E─┤                       ▲
   (pure JSON→files; parallel-safe)│                       │
#41 sandbox (ipe_sandbox crate)────┘        M4 kernel registry (consumer wiring only)
                                                            │
#42g M-G async bridge (last, after E2E rung 2)──────────────┘
```

`#42g` (generator library) needs neither the sandbox nor the registry: it
is a pure function from committed fixture JSON to files, byte-diffable
against the reference's artifacts. `#41` gates only `#42d` (the driver that
actually runs untrusted code). `M4` gates only the consumer wiring
(`.ipei` seeding + lowering). This maximizes design-ahead parallelism while
keeping the security gate ahead of anything a user can invoke.

| Phase | Deliverables | Verify gate | Est. |
|---|---|---|---|
| **P0 — #40 inspector hardening** | De-workspace `tools/sky-ffi-inspect-rs` (own target/lock/toolchain); pinned nightly `rust-toolchain.toml` + committed `Cargo.lock`; lints allow→deny (~130 unwrap/expect/panic sites → error-`PkgInfo` + exit≠0); adversarial-JSON fuzz corpus; keystone comments preserved | Inspector self-tests green; fuzz corpus: zero panics, bounded RSS, error-`PkgInfo` out; `--help`/probe parity with vendored oracle binary | 2-3 sessions |
| **P1 — #42g decode + coerce (M-A, M-C, M-B)** | `ipe_ffi` crate: wire→domain `PkgInfo` decode (identifier newtypes, closed enums, `FnShape`); `num_coerce.rs` leaf + grep-fence test (no bare `as i64/u64` outside it); `call.rs` seven-check `TryFrom` → `IPE-F4400` with accept/reject corpus mirroring `FfiCallSpec.hs` | Corpus green; injection strings cannot construct `RustIdent`; warm-cache re-validation test (hand-corrupted kernel.json rejected) | 3-4 sessions |
| **P2 — #42g emitters (M-D)** | `emit/{ipei,kernel,bindings}.rs` with BEGIN/END sentinels; fallibility as single stored bit read by both emitters (R0.4); `.ipei → Ty` ≡ stdlib-projection structural-identity test | **Byte-diff vs reference artifacts for semver (fixture 107)**; golden fallibility diff-test | 2-3 sessions |
| **P3 — #42g generics (M-E)** | `instance.rs`: (A)-model wrapper synth + per-instantiation bindability gate (closed-set × MODELLABLE_5 table) as region-keyed diagnostics | Byte-diff vs reference for the generic fixtures (multi/multi2); rejection cases produce `IPE-F44xx`, never silent drops | 2-3 sessions |
| **P4 — #41 sandbox** | `ipe_sandbox` crate: bwrap argv builder; unshare fallback + post-spawn isolation proof; refusal `IPE-F4410` + `IPE_FFI_ALLOW_UNSANDBOXED=1`; phase-separated fetch/compile/inspect; resource caps; env scrub | Isolation proof tests (ns-id comparison, no-egress probe from inside jail); refusal path test; caps enforced (OOM-crate fixture killed at RSS cap) | 2-3 sessions |
| **P5 — #42d driver + consumer wiring** | `ipe add/install/remove` (argv-exec, trust gate, `--git` gating, dynamic `Cargo.toml` with locked versions + effective features, S4 sentinel DCE); `.ipei` HM seeding via `KernelId::Ffi` (needs M4); lowering + backend emission | **E2E rung 1:** `ipe add semver` → inspect-in-jail → emit → type-check → cargo build → run, matching reference behavior | 3-4 sessions (after M4) |
| **P6 — E2E ladder** | All 10 shim-free crates (fixtures 107-114 + regressions 73/76/92/97/105/106) through the full pipeline; `ffi-audit` skill re-pointed at our inspector | **E2E rung 2:** 10/10 byte-diff (or recorded-divergence-diff) vs reference; audit-skill sweep produces verdict table | 2 sessions |
| **P7 — M-G async bridge** | **SUPERSEDED 2026-07-04** — async emission is part of the base emitters (P2/P3) per `async-ffi-bridge-design.md` §9; M-G dissolves into M-D/M-E. New P6 = 10 sync crates + firestore parity + the real async-stripe E2E; new P7 = skyshop-rs zero-shim + used-set DCE (acceptance) | see `async-ffi-bridge-design.md` §9-§10 | — |

Total: ~16-22 sessions, of which P1-P3 (≈8-10) are parallel-safe with the
critical-path compiler work (design-ahead lane discipline applies: they are
a new leaf crate with no workspace-target contention beyond the shared
`target/`).

## 7. Reconciliation with the banked #39 suite

The suite was banked against the reference state after the 10th shim-free crate landed. It is
therefore mostly CONFIRMED, not obsoleted. Explicit dispositions:

1. **Confirmed, now with completed-reference evidence:** direct Rust-crate
   binding (no Go-package pathway) is the shipped reference architecture
   (`FfiRegistry.hs:273-284`, `Main.hs:1087-1095`); rustdoc-JSON
   introspection; PkgInfo/Call-AST schema; (A)-model generics; per-crate
   cache + warm-build no-reinspect; DCE via `FfiRef`; `Rust_<Crate>`
   naming; 10-crate shim-free acceptance ladder; async SDKs hand-shimmed.
   The banked premise "auto shim-free RUST-crate binding" is validated
   end-to-end.
2. **Refined (supersedes the banked Part-3 ordering):** the linear
   `#40 → #41 → #42` gate chain becomes the DAG of §6 — the sandbox is the
   blocking gate for the DRIVER (`ipe add`, P5), not for the generator
   library (P1-P3), which is a pure function over committed fixture JSON
   and executes no untrusted code. The banked docs' own R3 already implied
   this; this doc makes it the plan of record.
3. **Reversed (supersedes agent-era assumption of staleness):** our
   vendored inspector (18,500 lines, vendored 2026-06-29 off the v0.17.3
   line) is a SUPERSET of the local reference checkout (16,853 @
   v0.17.2-1204); both carry #106/#109/#110/WALL-G. No catch-up sync is
   needed now; future syncs follow the release-tag-only upstream policy.
4. **Reclassified:** `ffi_call_task_divergence.rs` asserts reference
   PARITY (the reference's Rust runtime has byte-similar polyfills with the
   same panic rationale), diverging only from the Go runtime's reflection
   registry. Update the test's framing comment when touched next (no code
   change).
5. **New decisions this doc adds:** (a) inspector ships as a separate
   argv-exec'd toolchain binary, NOT embedded (reference embeds via
   Haskell TH — §3.1, §5); (b) the M-D acceptance byte-diff target is the
   reference's `.ipe-cache/ffi/rust/` artifacts for fixtures 107-114, with
   every intentional difference (`.ipei` extension, `Result Error`
   erasure, NumCoerce saturation) enumerated in the diff filter and in
   `docs/divergences-from-sky.md`; (c) `effect` decodes into a closed enum
   with unknown-string → hard error (the reference tolerates it as a bare
   String).
6. **Unchanged and still authoritative in the banked docs:** the bwrap
   argv (impl-ready doc lines 91-121), the unshare isolation-proof
   procedure, the resource-cap defaults, the seven `validate_call` checks,
   the D1-D8 module contracts, and the crate topology of `ipe_ffi`.

## 8. Open questions (to resolve at implementation time, not now)

- **M4 registry timing:** P5 blocks on it; if M4 slips, P1-P4 + P6's
  byte-diff harness can still complete (the harness drives `ipe_ffi` as a
  library, no compiler wiring needed).
- **`.ipei` naming vs rename sweep:** the Ipê→Ipê rename (task #59) will
  hit `.skyi`→`.ipei` and `Rust.<Crate>` import prose; keep the extension
  decision here so the rename doesn't fork it.
- **Nightly-pin lifecycle:** rustdoc JSON format is unstable; the pin bump
  procedure (bump → fuzz corpus → fixture re-diff) needs a small runbook
  when P0 lands. Cache keys already include the nightly channel.
- **Windows/macOS jails:** bwrap/unshare are Linux-only; `ipe add` on other
  hosts refuses (IPE-F4410) until a per-OS jail (sandbox-exec /
  AppContainer) is designed. The reference offers no precedent (it is
  unsandboxed everywhere).
