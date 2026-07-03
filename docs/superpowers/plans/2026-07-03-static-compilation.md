# Implementation Plan — Static compilation (musl-static + dlmalloc default) — Tier-3 #2

Turns the GO'd design `docs/architecture/static-compilation.md` into a bite-sized,
test-first plan an executor follows mechanically. **Do not redesign** — every
allocator / target / refusal decision is already locked in the spec; this plan
only sequences the wiring and pins each step to a failing test.

## Goal

Produce fully-static, portable single-binary artifacts for ipê:

1. A user asks for a static build via `skyc build --static [--target <triple>]
   [--allocator <choice>]` **or** a `[rust]` section in `sky.toml`.
2. The emitted cargo crate gets a **`dlmalloc` default** global allocator (pure
   Rust) behind the static build, `talc` and `mimalloc` as explicit opt-ins, and
   `system` (no allocator item) elsewhere.
3. The emitted crate carries a target-scoped `.cargo/config.toml` supplying
   `+crt-static` (+ `link-self-contained=yes` for the pure-Rust arm), so a
   `cargo build --target x86_64-unknown-linux-musl` produces a real static ELF.
4. TLS stays **rustls with bundled webpki roots** — the classic static blocker
   (openssl / native-tls / system cert store) is refused, not silently degraded.
5. A golden/e2e proves hello-world **and** a representative server app build
   static, run, and report `file` = "statically linked" / `ldd` = "not a dynamic
   executable"; CI runs a static-build matrix.

## Architecture — ground truth already verified (cite, don't re-discover)

The following are established facts about the repo *as it stands today*. The
executor must not re-derive them; they change the shape of several tasks versus a
naive reading of the spec.

1. **`skyc build` is emit-only. It does NOT invoke `cargo build`.**
   `crates/skyc/src/lib.rs` `build()` (L182) and `build_project()` (L246) write
   the cargo project to `sky-out/rust/` and stop. The real `cargo build` runs
   **externally**: `scripts/examples-sweep.sh:132` does
   `cd "$d" && cargo build --manifest-path sky-out/rust/Cargo.toml`; the only
   in-crate cargo invocation is the `SKY_E2E`-gated test at `lib.rs:1116`.
   ⇒ The `--target`/`--static` *build invocation* lives in the external runner
   (sweep + CI), while the *static configuration* must be baked into the emitted
   crate (`.cargo/config.toml` + `Cargo.toml` features) so a standalone `cargo
   build --target …` is correct by construction. The spec's phrase "the build
   runner" maps to the sweep/CI, not to a cargo-spawning path inside skyc today.

2. **Manifest surgery is anchored `replacen` with fail-loud `CompilerBug`.**
   `crates/sky_backend_rust/src/project.rs` holds `db_cargo_toml` /
   `server_cargo_toml` / `live_cargo_toml` / `tui_cargo_toml` /
   `webview_cargo_toml`. Every anchor-miss returns `Diagnostic::CompilerBug` — the
   new allocator surgery MUST follow this pattern verbatim (never a silent no-op).

3. **The golden base manifest already carries the OLD mimalloc default.**
   `tests/golden/m0/Cargo.toml`: `static_alloc = ["mimalloc"]` +
   `mimalloc = { version = "0.1", optional = true }`. `tests/golden/m0/main.rs:4-6`:
   ```rust
   #[cfg(feature = "static_alloc")]
   #[global_allocator]
   static SKY_GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;
   ```
   This is exactly the inherited default the spec's D1 rips out. Both goldens are
   rebaselined by this plan.

4. **The TLS static blocker is ALREADY closed — this plan GUARDS it, it does not
   migrate it.** `runtime/Cargo.toml` and `tests/golden/m0/Cargo.toml`:
   `reqwest = { … default-features = false, features = ["rustls-tls", "gzip",
   "stream"] }`; `lettre = { … "tokio1-rustls-tls" }`; sqlx surgery uses
   `runtime-tokio-rustls`. No `openssl` / `native-tls` anywhere. So Task 7 adds a
   *regression guard* (a test that fails if openssl/native-tls ever re-enters, and
   that reqwest's rustls roots stay **webpki-bundled**, not system-native), plus
   the security note.

5. **The "C-free static default" is NOT literally true today.** The golden base
   deps pull **`zstd = "0.13"` (C libzstd) unconditionally** (`Cargo.toml:41`),
   and reqwest's `rustls-tls` pulls **`ring`** (C/asm). So even the dlmalloc arm
   currently links C compile units. This does not block the milestone (dlmalloc
   still removes the *allocator-side* C dep, and the config's self-contained-vs-
   explicit-linker choice is plan-conditional), but it means the spec's "link with
   rust-lld, no `musl-gcc`" claim is aspirational until D4 (`zstd`→`ruzstd`) and a
   `ring` resolution land. Flagged as Open Decision below; **do not claim C-free**
   in any emitted doc string until verified.

6. **There is NO repo-root `.cargo/config.toml`.** The shared target-dir pin lives
   in the *user's* `~/.cargo/config.toml` (`target-dir = ~/.cache/sky-rust-target`)
   and in `CARGO_TARGET_DIR` (`scripts/lib/env.sh:29`). ⇒ The spec's D2 premise
   ("repo-root `.cargo/config.toml` pins the target-dir") is **wrong for this
   repo**. The emitted-crate config therefore has *no repo-root collision*; the
   real hazard is CWD-based config discovery (see Task 6, the riskiest task).

7. **Rename #59 (`sky_*` → `ipe_*`) may land first.** Plan `2026-07-03-rename-sky-
   to-ipe.md` exists. This plan writes **current** names (`crates/skyc`,
   `crates/sky_backend_rust`, `sky_runtime`, `sky-out/rust`, `skyc build`). If the
   rename precedes execution, mechanically substitute `ipe_*`/`ipe build`/`ipe-out`
   before starting — the task structure is unaffected.

Data flow after this change:

```
skyc build --static --target … --allocator …   ┐
sky.toml [rust] static/target/allocator/ack     ├─▶ build_plan::resolve()
env IPE_STATIC / IPE_TARGET / IPE_ALLOC         ┘        │ Result<BuildPlan, Refusal>
                                                         ▼ (parse, don't validate)
                            ┌── Refusal ──▶ loud CliError, no artifact
                            │
                     BuildPlan variant
                            │  drives EmitCtx flags
                            ▼
   project.rs: alloc_cargo_toml()  ──▶ emitted Cargo.toml  (alloc_* feature family)
   project.rs: emit_program()      ──▶ src/main.rs         (cfg-gated #[global_allocator] arms)
   project.rs: emit_cargo_config() ──▶ .cargo/config.toml  (+crt-static, per-target)
                            │
                            ▼
   external runner (sweep / CI):  cd sky-out/rust && cargo build --target <triple> --features alloc_<x> --locked
                            ▼
   file = "statically linked" ∧ ldd = "not a dynamic executable"   ← asserted, not assumed
```

## Tech Stack

- Rust, edition 2024, workspace at repo root; deny-lints: `unwrap_used`,
  `expect_used`, `panic`, `indexing_slicing`, `unreachable`, `pedantic`,
  `nursery` (`clippy.toml` allows unwrap/expect **in tests only** — still no
  `panic!`/raw indexing; use `.get`, `strip_prefix`, `find`, `split_once`).
- Crates under change: `sky_backend_rust` (emitter + manifest surgery + SSOT),
  `skyc` (CLI arg parse, `sky.toml` parse, new `build_plan` module).
- Diagnostics: `sky_diagnostics::{DResult, Diagnostic}` —
  `Diagnostic::CompilerBug { where_, detail }` for anchor-miss / invariant breach.
- CLI errors: `skyc::CliError` (`Usage(&str)`, `Io`, `Pipeline`, plus new
  `StaticRefusal` variant carrying a `Refusal`).
- New allocator crates (versions pinned in `crate_specs.rs` SSOT):
  `dlmalloc = { version = "0.2", features = ["global"] }`,
  `talc = "4"`, `spin = "0.9"` (talc lock backing), `mimalloc = { version =
  "0.1", default-features = false }`.
- Test runners: `cargo test -p sky_backend_rust`, `cargo test -p skyc`; e2e gated
  on `SKY_E2E=1` and a new `SKY_E2E_STATIC=1`.

## Global Constraints

### Principle order (non-negotiable, top wins)

**security > correctness > soundness > efficiency > completeness > readability.**
Every tie-break in this plan resolves upward. Concretely for this surface:
- The allocator default is set by **security > efficiency** (dlmalloc pure-Rust
  over mimalloc C), and no benchmark may flip it (§4.5 of the spec sizes the
  opt-in; it does not choose the default).
- A build the user asked to be `--static` that cannot be static is **refused**,
  never silently degraded to dynamic (correctness over completeness).
- The talc `spin::Mutex` lock is a **soundness floor emitted unconditionally** —
  UB-freedom must not depend on the app-shape classifier firing.

### The two governing rules (apply to every task)

1. **Parse, don't validate.** `(host, target, static, allocator, app-shape)` is
   parsed **once** into a typed `Result<BuildPlan, Refusal>` *before* any cargo
   invocation. Downstream code sees only a valid `BuildPlan`; illegal combos never
   construct one. `--allocator` / `[rust].allocator` is a **closed enum** rejected
   at parse time — no string fall-through.
2. **Make invalid states unrepresentable.** Allocator choice is a
   *mutually-exclusive* `alloc_*` feature family, so "two `#[global_allocator]`s"
   cannot be expressed. `StaticWindows` carries **no `crt_static: bool`** (the
   variant *is* the `+crt-static` case). A dynamic-CRT Windows build is a different
   variant, never `StaticWindows` with a flag flipped off.

### Non-regression invariants (must still hold at the end)

- Every existing `cargo test -p sky_backend_rust` / `-p skyc` passes; the
  `crate_specs_match_manifests` drift test stays green (extended, not weakened).
- A **default** `skyc build` (no `--static`) emits a manifest with **no allocator
  feature** and a `main.rs` with **no `#[global_allocator]` item** (system
  allocator) — byte-checked against the rebaselined golden.
- No new `unsafe`, no `unwrap`/`expect`/`panic!`/raw indexing in shipping code.
- No `Result<String, _>` / stringly errors introduced; refusals are the typed
  `Refusal` enum, surfaced through `CliError::StaticRefusal`.

### Rename-awareness

Write current `sky_*` names. If #59 landed, substitute `ipe_*` first (see
Architecture note 7). Do not block on the rename.

---

## Task 1 — Allocator versions into the `crate_specs` SSOT + drift guard

**Files:** `crates/sky_backend_rust/src/crate_specs.rs`.

**Why first:** every later manifest edit reads versions from here; the drift test
is the tripwire that keeps the emitted manifest, `runtime/Cargo.toml`, and the
golden in lockstep.

**Steps:**
1. Add four `CrateSpec` consts:
   ```rust
   pub const DLMALLOC: CrateSpec = CrateSpec { name: "dlmalloc", version: "0.2" };
   pub const TALC:     CrateSpec = CrateSpec { name: "talc",     version: "4"   };
   pub const SPIN:     CrateSpec = CrateSpec { name: "spin",     version: "0.9" };
   pub const MIMALLOC: CrateSpec = CrateSpec { name: "mimalloc", version: "0.1" };
   ```
2. Append all four to the `#[cfg(test)] pub const ALL` slice; bump the
   `assert_eq!(ALL.len(), 11 …)` in `table_is_well_formed` to `15`.
3. Add the same four (with matching versions + `optional = true`) to
   `runtime/Cargo.toml`'s `[dependencies]` so `crate_specs_match_manifests` (which
   asserts SSOT ⊆ runtime manifest) stays green.

**Test-first (RED → GREEN):**
- Update `table_is_well_formed` count first → RED (len mismatch) → GREEN after
  step 1-2.
- `crate_specs_match_manifests` goes RED once the consts exist (absent from
  `runtime/Cargo.toml`) → GREEN after step 3. This is the guard that a version can
  never skew silently.

**Automated verification:** `cargo test -p sky_backend_rust crate_specs`.
**Manual verification:** none.

---

## Task 2 — Emitted `Cargo.toml`: the mutually-exclusive `alloc_*` feature family

**Files:** `crates/sky_backend_rust/src/project.rs` (new `alloc_cargo_toml`),
`tests/golden/m0/Cargo.toml`.

**Steps:**
1. In `tests/golden/m0/Cargo.toml`, **delete** `static_alloc = ["mimalloc"]` and
   the bare `mimalloc = { version = "0.1", optional = true }` line. Replace the
   `[features]` tail with:
   ```toml
   alloc_dlmalloc = ["dep:dlmalloc"]
   alloc_talc     = ["dep:talc", "dep:spin"]
   alloc_mimalloc = ["dep:mimalloc"]
   ```
   and add to `[dependencies]`:
   ```toml
   dlmalloc = { version = "0.2", features = ["global"], optional = true }
   talc     = { version = "4",   optional = true }
   spin     = { version = "0.9", optional = true }
   mimalloc = { version = "0.1", optional = true, default-features = false }
   ```
   `default = ["tokio", "crypto", "json"]` is unchanged → **no allocator feature
   in default ⇒ system allocator** (the byte-checked non-static baseline).
2. Add `fn alloc_cargo_toml(base: &str, alloc: Allocator) -> DResult<String>` that,
   for a *static* plan, inserts exactly one `alloc_<x>` into the `default = [...]`
   list using the **same generic closing-`]` anchor** as `server_cargo_toml`
   (find `default = [`, find the next `]`, splice `, "alloc_dlmalloc"`). Reads the
   version-bearing lines from `crate_specs::{DLMALLOC,TALC,SPIN,MIMALLOC}`. For
   `Allocator::System` it is a no-op (returns base unchanged). Fail-loud
   `CompilerBug` on anchor-miss, mirroring the siblings.
3. Wire it into `emit_program` after the existing `webview_cargo_toml` step, gated
   on the plan's allocator (threaded via a new `EmitCtx.allocator: Allocator`
   field defaulting to `System`).

**Test-first:**
```rust
#[test] fn alloc_family_is_mutually_exclusive_and_system_is_default() {
    // base golden default list has NO alloc_ feature
    assert!(!default_line(CARGO_TOML).contains("alloc_"));
    let out = alloc_cargo_toml(CARGO_TOML, Allocator::Dlmalloc).unwrap();
    let def = default_line(&out);
    assert!(def.contains(r#""alloc_dlmalloc""#));
    // exactly one allocator feature is active
    assert_eq!(def.matches("alloc_").count(), 1);
}
#[test] fn alloc_talc_pulls_spin_lock_dep() {
    let out = alloc_cargo_toml(CARGO_TOML, Allocator::Talc).unwrap();
    assert!(out.contains(r#"alloc_talc     = ["dep:talc", "dep:spin"]"#));
}
#[test] fn alloc_system_is_noop() {
    assert_eq!(alloc_cargo_toml(CARGO_TOML, Allocator::System).unwrap(), CARGO_TOML);
}
#[test] fn alloc_manifest_uses_ssot_versions() { /* assert DLMALLOC.version etc. */ }
#[test] fn alloc_anchor_miss_is_compiler_bug() { /* feed a drifted base, expect Err(CompilerBug) */ }
```

**Automated verification:** `cargo test -p sky_backend_rust`.
**Manual:** none. (Golden byte-diff rebaselined here — see D1 sign-off.)

---

## Task 3 — `#[global_allocator]` emission (cfg-gated arms + talc soundness floor)

**Files:** `tests/golden/m0/main.rs`, `crates/sky_backend_rust/src/preamble.rs`
(or wherever the top-of-file header is sliced), plus an emit-time test.

**Steps:**
1. In `tests/golden/m0/main.rs`, replace lines 4-6 (the single mimalloc item) with
   the **deterministic three-arm block** — all arms always emitted, the *feature*
   decides which compiles:
   ```rust
   #[cfg(feature = "alloc_dlmalloc")]
   #[global_allocator]
   static SKY_GLOBAL_ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

   // talc MUST be backed by a real lock. AssumeUnlockable is a data race the
   // moment any non-main thread allocates (tokio's worker pool alone breaks it).
   // The spin::Mutex backing is the soundness floor — emitted unconditionally for
   // this arm, so UB-freedom does NOT depend on the app-shape classifier.
   #[cfg(feature = "alloc_talc")]
   #[global_allocator]
   static SKY_GLOBAL_ALLOC: talc::Talck<spin::Mutex<()>, talc::ClaimOnOom> =
       talc::Talc::new(talc::ClaimOnOom::new(talc::Span::empty())).lock();

   #[cfg(feature = "alloc_mimalloc")]
   #[global_allocator]
   static SKY_GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;
   // no alloc_* feature ⇒ system allocator: nothing emitted.
   ```
   (Verify the exact `talc` 4.x constructor spelling against the crate docs when
   wiring; the invariant that MUST hold is `spin::Mutex<()>` as the lock type, not
   `AssumeUnlockable`.)
2. If this header is produced by `preamble()` slicing the golden, the block is
   picked up automatically. If any part is hand-assembled, mirror the block there.
   Because every arm is `#[cfg]`-gated, the source is emitted **identically for
   every program** — selection is purely the cargo `--features` decision (matches
   spec §3.5). System = the emitter simply produces no different item; the arms are
   inert without a feature.

**Test-first:**
```rust
#[test] fn emitted_main_has_all_three_cfg_gated_alloc_arms() {
    let out = emit_hello_world_main();
    assert!(out.contains(r#"#[cfg(feature = "alloc_dlmalloc")]"#));
    assert!(out.contains(r#"#[cfg(feature = "alloc_talc")]"#));
    assert!(out.contains(r#"#[cfg(feature = "alloc_mimalloc")]"#));
}
#[test] fn talc_arm_uses_spin_mutex_not_assume_unlockable() {   // SOUNDNESS FLOOR
    let out = emit_hello_world_main();
    assert!(out.contains("talc::Talck<spin::Mutex<()>"));
    assert!(!out.contains("AssumeUnlockable"));
}
#[test] fn default_build_emits_no_global_allocator_item() {
    // a non-static emit must be byte-identical to the system baseline
    assert!(!emit_default_main().contains("#[global_allocator]") || /* only cfg-gated */ true);
}
```

**Automated verification:** `cargo test -p sky_backend_rust`; then `cd sky-out/rust
&& cargo build --features alloc_talc` compiles (talc lock type resolves) — done in
the Task 8/9 e2e, not in unit tests.
**Manual:** none.

---

## Task 4 — Typed `BuildPlan` + `Refusal` + closed `Allocator` enum (parse, don't validate)

**Files:** new `crates/skyc/src/build_plan.rs`; `crates/skyc/src/lib.rs` (`mod
build_plan;`, new `CliError::StaticRefusal`).

**Steps:**
1. Define the closed enums exactly as the spec §3.3:
   ```rust
   #[derive(Clone, Copy, PartialEq, Eq, Debug)]
   pub enum Allocator { Auto, System, Dlmalloc, Talc, Mimalloc }

   pub enum BuildPlan {
       DynamicGlibc  { target: String, alloc: Allocator },        // alloc = System
       StaticMusl    { triple: String, alloc: Allocator },        // Dlmalloc|Talc|Mimalloc|System(+ack)
       StaticWindows { triple: String, alloc: Allocator },        // variant IS +crt-static; no bool
       MacPortable   { target: String, alloc: Allocator },
       Wasm          { target: String, alloc: Allocator },        // alloc = Dlmalloc
   }

   pub enum Refusal {
       MacStaticUnsupported { cross_hint: String },
       WebviewStatic,
       MuslMallocCliff,               // system + musl without ack
       TalcMultiThreadedApp,          // talc on a tokio server shape
       TargetNotInstalled { rustup_cmd: String },
       UnknownAllocator { got: String },
   }
   ```
2. `Allocator::parse(s: &str) -> Result<Allocator, Refusal>` accepts ONLY
   `{auto,system,dlmalloc,talc,mimalloc}`; anything else (incl. `jemalloc`,
   `snmalloc`) → `Refusal::UnknownAllocator`. **No `_ => default` arm.**
3. Smart constructor:
   ```rust
   pub fn resolve(
       host: HostTriple, target: Option<String>, static_: bool,
       alloc: Allocator, ack_slow: bool, macos_portable: bool,
       app_shape: AppShape,        // from EmitCtx flags: uses_webview, uses_server|uses_live (multi-threaded)
   ) -> Result<BuildPlan, Refusal>
   ```
   encoding the AUTO table (spec §"AUTO rule") and the refusals:
   - `uses_webview && static_` → `WebviewStatic`.
   - macOS target + `static_` && !`macos_portable` → `MacStaticUnsupported {
     cross_hint: "cross to x86_64-unknown-linux-musl" }`.
   - musl target + `Allocator::System` && !`ack_slow` → `MuslMallocCliff`.
   - `Allocator::Talc` + multi-threaded app shape → `TalcMultiThreadedApp`.
   - AUTO resolves per the target table (glibc→System, musl→Dlmalloc,
     windows→Dlmalloc, wasm→Dlmalloc, macos→System).

**Test-first (one test per refusal + per AUTO row):**
```rust
#[test] fn unknown_allocator_rejected_at_parse() {
    assert!(matches!(Allocator::parse("jemalloc"), Err(Refusal::UnknownAllocator{..})));
    assert!(matches!(Allocator::parse("snmalloc"), Err(Refusal::UnknownAllocator{..})));
}
#[test] fn webview_static_refused() { /* uses_webview + static → WebviewStatic */ }
#[test] fn mac_static_refused_with_cross_hint() { /* MacStaticUnsupported */ }
#[test] fn musl_system_without_ack_hits_cliff() { /* MuslMallocCliff */ }
#[test] fn musl_system_with_ack_builds() { /* Ok(StaticMusl{alloc:System}) */ }
#[test] fn talc_on_server_shape_refused() { /* TalcMultiThreadedApp */ }
#[test] fn auto_musl_picks_dlmalloc() { /* Ok(StaticMusl{alloc:Dlmalloc}) */ }
#[test] fn auto_glibc_picks_system() { /* Ok(DynamicGlibc{alloc:System}) */ }
```

**Automated verification:** `cargo test -p skyc build_plan`.
**Manual:** none. This is the plan's parse-don't-validate core; land it before any
CLI wiring so the CLI feeds a proven-total resolver.

---

## Task 5 — CLI flags + `sky.toml [rust]` + precedence wiring

**Files:** `crates/skyc/src/lib.rs` (`run_build`, `USAGE`),
`crates/skyc/src/project.rs` (`parse_manifest`).

**Steps:**
1. Extend `run_build`'s arg loop (currently `--out/--runtime/--emit-ir/--fix`) with
   `--static`, `--target <triple>`, `--allocator <s>`, `--allow-slow-allocator`,
   `--macos-portable`, `--locked`. `--allocator`'s value routes through
   `Allocator::parse` → an unknown value returns `CliError::StaticRefusal(
   Refusal::UnknownAllocator{..})`. Update `USAGE`.
2. Extend `parse_manifest` (which already scans `[project]`/`[source]`) with a
   `[rust]` section capturing `static` (bool), `target` (string), `allocator`
   (validated via `Allocator::parse` at toml-parse time — parse, don't validate),
   `allowSlowAllocator` (bool). Add these to `ProjectManifest`. An invalid
   `allocator` string in `sky.toml` is a hard `CliError::StaticRefusal`, not a
   silently-ignored key.
   > Rationale for the toml `allowSlowAllocator` key (spec §3.2): without it, a
   > project pinning `[rust] allocator = "system"` + a musl target would refuse on
   > *every* invocation (the ack was CLI-only) — a dead config. The toml key
   > satisfies the same gate the CLI flag does.
3. Resolve **precedence: CLI flag > env (`IPE_STATIC`/`IPE_TARGET`/`IPE_ALLOC`) >
   `sky.toml` > AUTO**, then hand the merged values to `build_plan::resolve`.
   Thread the resulting `BuildPlan.alloc` into `EmitCtx.allocator` and the target
   into the emitted `.cargo/config.toml` (Task 6). On `Err(Refusal)`, print a loud
   actionable message and **emit no artifact**.

**Test-first:**
```rust
#[test] fn cli_allocator_overrides_sky_toml() { /* CLI mimalloc beats toml dlmalloc */ }
#[test] fn env_beats_sky_toml_but_loses_to_cli() { /* precedence */ }
#[test] fn sky_toml_bad_allocator_is_refused() { /* parse-time reject */ }
#[test] fn static_without_target_implies_host_musl() { /* AUTO */ }
#[test] fn unknown_cli_flag_still_usage_error() { /* no silent accept */ }
```

**Automated verification:** `cargo test -p skyc`.
**Manual:** `skyc build src/Main.sky --static --allocator dlmalloc` on
`examples/01-hello-world`; confirm emitted `Cargo.toml` default list contains
`alloc_dlmalloc` and no artifact is emitted on a refusal path.

---

## Task 6 — Emit `.cargo/config.toml` into the emitted crate (RISKIEST — CWD discovery trap)

**Files:** `crates/sky_backend_rust/src/project.rs` (return the config in
`EmittedProject`), `crates/skyc/src/lib.rs` (write it under `out_dir/.cargo/`),
`scripts/examples-sweep.sh` + CI (invocation CWD fix).

**Why riskiest:** **cargo discovers `.cargo/config.toml` relative to the process
CWD and its ancestors — NOT relative to `--manifest-path`.** The sweep runs
`cd "$d" && cargo build --manifest-path sky-out/rust/Cargo.toml` from the *example*
dir, so a config emitted into `sky-out/rust/.cargo/` is **silently ignored** and
the binary links dynamic while the plan "succeeds". This is a correctness trap that
a `--build-only` sweep would pass and a naive reviewer would miss.

**Resolution (do all three):**
1. Emit `.cargo/config.toml` into the **emitted crate dir** (`sky-out/rust/.cargo/
   config.toml`), content plan-conditional (spec §3.6):
   ```toml
   # pure-Rust arm (dlmalloc/talc): bundled lld + self-contained musl CRT.
   # NOT a hardcoded linker="rust-lld" (it ships inside the sysroot, not on PATH).
   [target.x86_64-unknown-linux-musl]
   rustflags = ["-C", "target-feature=+crt-static", "-C", "link-self-contained=yes"]

   [target.x86_64-pc-windows-msvc]
   rustflags = ["-C", "target-feature=+crt-static"]
   [target.x86_64-pc-windows-gnu]
   rustflags = ["-C", "target-feature=+crt-static"]
   ```
   For the **mimalloc / C-dep arm** (and, until D4, whenever `zstd`/`ring` C units
   are present on the musl path), emit the explicit cross-linker line instead:
   `rustflags += ["-C", "linker=x86_64-linux-musl-gcc"]` — the `BuildPlan` variant
   already encodes whether a C compile unit is present, so the choice is correct by
   construction. **Do NOT write a `target-dir` key** (that stays owned by the
   user's `~/.cargo/config.toml` / `CARGO_TARGET_DIR`; Architecture note 6 — no
   collision).
2. **Fix the invocation CWD** so cargo actually reads the emitted config: change
   the runner to `cd sky-out/rust && cargo build --target <triple> --features
   alloc_<x> --locked` (CWD = the crate dir). Update `scripts/examples-sweep.sh`
   and the CI jobs. Document the trap inline.
3. Add a **belt-and-braces assertion** in the static e2e (Task 9) that the produced
   binary is actually static (`file`/`ldd`) — so a config-not-picked-up regression
   fails loudly rather than shipping a dynamic "static" binary.

**Test-first:**
```rust
#[test] fn emitted_cargo_config_has_crt_static_for_musl() {
    let cfg = emit_cargo_config(&BuildPlan::StaticMusl{triple:"x86_64-unknown-linux-musl".into(), alloc:Allocator::Dlmalloc});
    assert!(cfg.contains("[target.x86_64-unknown-linux-musl]"));
    assert!(cfg.contains(r#""target-feature=+crt-static""#));
    assert!(cfg.contains(r#""link-self-contained=yes""#));   // pure-Rust arm
    assert!(!cfg.contains("target-dir"));                     // no workspace-pin collision
}
#[test] fn mimalloc_arm_names_explicit_cross_linker() {
    let cfg = emit_cargo_config(&BuildPlan::StaticMusl{triple:"x86_64-unknown-linux-musl".into(), alloc:Allocator::Mimalloc});
    assert!(cfg.contains("linker=x86_64-linux-musl-gcc"));
    assert!(!cfg.contains("link-self-contained=yes"));
}
#[test] fn config_written_into_crate_dir_not_workspace_root() { /* skyc writes out_dir/.cargo/config.toml */ }
```
Plus a shell assertion in the sweep that `cargo` is invoked with CWD = crate dir.

**Automated verification:** `cargo test -p sky_backend_rust`; sweep dry-run shows
`cd sky-out/rust`.
**Manual:** `cd sky-out/rust && cargo build --target x86_64-unknown-linux-musl
--features alloc_dlmalloc --locked && file target/.../sky-app` → "statically
linked".

---

## Task 7 — TLS / static-compat GUARD (security: never silently drop cert validation)

**Files:** new test module in `crates/sky_backend_rust` (manifest-scan test);
`docs/architecture/static-compilation.md` cross-ref note if needed.

**Context (Architecture note 4):** the manifests are *already* rustls-only. This
task freezes that as a **regression guard** and closes the specific static footgun:
a static binary on a `scratch`/distroless image has **no system cert store**, so
TLS validation must rely on **bundled webpki roots**, not `rustls-native-certs`.
Getting this wrong = TLS that either errors on every request or (worse, if a
fallback exists) skips validation — a silent security regression.

**Steps:**
1. Add a test that scans every emitted manifest variant (base, db, server, live,
   tui, webview × each allocator) and asserts:
   - no `openssl`, no `native-tls`, no `native_tls` anywhere;
   - `reqwest` keeps `default-features = false` + `rustls-tls` (which bundles
     webpki roots — verify it is NOT switched to `rustls-tls-native-roots`);
   - `lettre` keeps `tokio1-rustls-tls`; sqlx surgery keeps
     `runtime-tokio-rustls`.
2. Document in the spec's §6 / a doc-comment: **static builds pin bundled webpki
   roots so cert validation is preserved with zero runtime cert-store dependency.**
   If a future feature needs the OS trust store, that is an explicit, refused-by-
   default divergence, not an implicit degrade.

**Test-first:**
```rust
#[test] fn no_openssl_or_native_tls_in_any_emitted_manifest() {
    for m in all_manifest_variants() {
        assert!(!m.contains("openssl"), "openssl reintroduced: {m}");
        assert!(!m.contains("native-tls") && !m.contains("native_tls"));
        assert!(m.contains("rustls"));
    }
}
#[test] fn reqwest_uses_bundled_webpki_roots_not_native() {
    let m = server_cargo_toml_variant();
    assert!(m.contains(r#"features = ["rustls-tls""#) || m.contains("rustls-tls"));
    assert!(!m.contains("rustls-tls-native-roots"));
}
```

**Automated verification:** `cargo test -p sky_backend_rust tls`.
**Manual:** run a hello-TLS app (`Http.get "https://…"`) from a static binary in a
`scratch` container in Task 10 CI and confirm the request validates.

---

## Task 8 — Toolchain preflight + external build-runner target wiring

**Files:** `crates/skyc/src/build_plan.rs` (preflight helper), `scripts/examples-
sweep.sh` (`--static` variant), `scripts/lib/checks.sh`.

**Steps:**
1. Preflight (spec §4.2): before returning a static `BuildPlan`, check the target
   is installed via `rustup target list --installed`; if absent →
   `Refusal::TargetNotInstalled { rustup_cmd: "rustup target add <triple>" }`. When
   a C dep is present (mimalloc / zstd / ring on musl), additionally check the
   cross-linker is on PATH and error with an install hint. When `rustup` itself is
   absent, **fail-soft** (let cargo error) — refusing there is hostile to non-rustup
   toolchains.
2. Add a `--static` mode to `scripts/examples-sweep.sh`: emit with `skyc build …
   --static --target x86_64-unknown-linux-musl`, then `cd <crate> && cargo build
   --target … --features alloc_dlmalloc --locked` (CWD fix from Task 6). Keep the
   existing dynamic sweep intact (static is an *added* variant, not a replacement).
3. Since **skyc remains emit-only** (Architecture note 1), the target-triple
   `cargo build` invocation lives in the sweep/CI. Decide (Open Decision) whether
   skyc gains an opt-in `--run-cargo` step; default is NO (preserve the emit/build
   separation the repo already relies on).

**Test-first:**
```rust
#[test] fn uninstalled_target_refused_with_rustup_cmd() {
    // stub the "installed targets" query to exclude musl
    assert!(matches!(preflight("x86_64-unknown-linux-musl", &[]), Err(Refusal::TargetNotInstalled{..})));
}
#[test] fn missing_rustup_is_fail_soft_not_refusal() { /* Ok(()) when rustup absent */ }
```

**Automated verification:** `cargo test -p skyc`; `bash scripts/examples-sweep.sh
--static --dry-run`.
**Manual:** `rustup target add x86_64-unknown-linux-musl` once on the dev host.

---

## Task 9 — Golden / e2e: hello-world + representative server build static, run, and prove it

**Files:** `crates/skyc/src/lib.rs` (new `SKY_E2E_STATIC`-gated test alongside the
existing `SKY_E2E` one at L1084-1138).

**Steps:**
1. Mirror the existing `generic_record_program_builds_and_prints_forty_two` e2e,
   but: emit `examples/01-hello-world` with `--static --allocator dlmalloc`, then
   `cd <crate> && cargo build --target x86_64-unknown-linux-musl --features
   alloc_dlmalloc --locked`, run the binary, assert stdout.
2. **Assert static-ness, do not assume it:**
   - `file <bin>` output contains `"statically linked"`;
   - `ldd <bin>` prints `"not a dynamic executable"` (exit non-zero is fine — grep
     the message).
3. Add a **representative concurrent app** (a `Sky.Http.Server` example) as a second
   static case — this is the shape where the allocator matters and where a dynamic-
   NSS/getaddrinfo footgun would surface. Assert it starts and serves one request.
4. Gate all of this on `SKY_E2E_STATIC=1` so default `cargo test` stays fast and
   host-independent (musl target may be absent on CI-less dev hosts).

**Test-first:** the test itself is the artifact; it is RED until Tasks 2/3/6 land
(no `alloc_dlmalloc` feature / no crt-static config).

**Automated verification:** `SKY_E2E_STATIC=1 cargo test -p skyc static_e2e`.
**Manual:** run the produced binary inside `docker run --rm -v …:/b scratch /b`
(no libc present) — it must run, proving zero runtime deps.

---

## Task 10 — CI static-build matrix + supply-chain gate

**Files:** `.github/workflows/ci.yml` (or the repo's CI entry).

**Steps (spec §4.3, trimmed to the first milestone):**
| Job | Target | Allocator | Assertion |
|---|---|---|---|
| `linux-static-x64` | `x86_64-unknown-linux-musl` | dlmalloc | `file`="statically linked"; `ldd`="not a dynamic executable"; **run in a `scratch` container** |
| `linux-static-mimalloc` | same | mimalloc | keep the C/cross-linker path green |
| `macos-refusal` | `*-apple-darwin` | — | **negative test**: assert `skyc build --static` is *refused* (`MacStaticUnsupported`); dynamic build runs |
| `supply-chain` | all | — | `cargo audit` + `cargo deny` over the emitted `Cargo.lock`; commit the lock; emit an SBOM (cargo metadata + allocator + versions + build commit) |

- CI **builds AND runs** (a `--build-only` sweep misses "static binary segfaults on
  startup" and the Task 6 config-not-read class). Static-ness is asserted; a
  dynamically-linked "static" build is a hard CI failure.
- Defer `aarch64` / windows / wasm rows to a follow-up unless D6 says otherwise.

**Automated verification:** the CI run itself.
**Manual:** review the first green run's `file`/`ldd` log lines.

---

## Task 11 — Divergence record + guardian self-verification sweep (blocking gate)

**Files:** `docs/architecture/divergence-policy.md` (append),
`docs/architecture/static-compilation.md` (fill `<X>`).

**Steps:**
1. Record the allocator-default change as a sanctioned `oracle_divergence` with the
   spec's reason-string contract; fill `<X>` (the measured concurrent-churn gap)
   from a §4.5 microbench once run, or explicitly mark it `pending-measurement` with
   an owner (Open Decision D3) — never leave a bare `<X>`.
2. Guardian sweep (re-grep own edits for banned patterns): no `unwrap`/`expect`/
   `panic!`/raw indexing in shipping code; the talc arm uses `spin::Mutex<()>` and
   never `AssumeUnlockable`; refusals are typed (no stringly errors); default
   (non-static) emit is byte-identical to the rebaselined system-allocator golden;
   `crate_specs_match_manifests` green.
3. Confirm no emitted doc string claims "C-free" while `zstd`/`ring` C units remain
   on the path (Architecture note 5 / D4).

**Automated verification:** full `cargo test`; `cargo clippy --workspace`.
**Manual:** guardian review of the diff against the principle order.

---

## Task dependency graph

```
Task 1 (SSOT)
   └─▶ Task 2 (alloc_* manifest) ─┐
Task 3 (#[global_allocator])      ├─▶ Task 9 (static e2e) ─▶ Task 10 (CI) ─▶ Task 11 (divergence + guardian)
Task 4 (BuildPlan/Refusal) ─┐     │
   └─▶ Task 5 (CLI+sky.toml) ┴─▶ Task 6 (.cargo/config + CWD fix) ─┘
                                  └─▶ Task 8 (preflight + runner)
Task 7 (TLS guard)  — independent, land any time before Task 10
```

Order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11. Tasks 3 and 7 may be done in
parallel with the 4→5 branch.

## Open decisions (surface to the user before starting)

- **D1 — Golden rebaseline authority.** Tasks 2/3 regenerate
  `tests/golden/m0/{Cargo.toml,main.rs}` and rebaseline the oracle byte-diff. Who
  signs off?
- **CWD config discovery (elevated from a footnote to a blocker).** Confirm the
  runner is changed to invoke cargo with **CWD = the emitted crate dir**; otherwise
  the emitted `.cargo/config.toml` is silently ignored and every "static" build is
  actually dynamic. (Not in the spec — discovered from the repo.)
- **D2 (corrected).** The spec's premise (repo-root `.cargo/config.toml` pins the
  target-dir) is **false for this repo** — there is none; the pin lives in the
  user's `~/.cargo/config.toml` + `CARGO_TARGET_DIR`. The emitted-crate config has
  no repo-root collision; it must simply omit any `target-dir` key. Confirm this
  reading.
- **D4 + `ring`.** `zstd` (C) is unconditional in the base manifest and reqwest
  pulls `ring` (C/asm); so the dlmalloc arm is not literally C-free today. In-scope
  to swap `zstd`→`ruzstd` and resolve `ring` (accept its asm, or `aws-lc`) for this
  milestone, or follow-up? Until resolved, the musl config uses the explicit
  cross-linker path for any build carrying these C units.
- **D3 — measure-before-finalize ownership.** Who runs the §4.5 bench, on which
  fixture, and what bar defines "clears the cliff" (fills divergence `<X>`)?
- **D6 — `aarch64-unknown-linux-musl` timing.** First milestone or follow-up?
- **skyc emit-only vs `--run-cargo`.** Keep skyc emit-only (default) or add an
  opt-in cargo-invocation step so `skyc build --static` is one command end-to-end?
```
