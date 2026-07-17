# Implementation Plan — Static compilation (musl-static + dlmalloc default)

Executes the authoritative design `docs/architecture/static-compilation.md`
(see its "Implementation amendments" section — this plan follows the amended
design). **Do not redesign** — every allocator / target / refusal decision is
locked there; this plan sequences the wiring, pinned to the repo as it stands
after the Sky→Ipê rename/relayout.

## Goal

Produce fully-static, portable single-binary artifacts for ipê:

1. A user asks for a static build via `ipe build … --static
   [--target <triple>] [--allocator <choice>] [--allow-slow-allocator]`,
   via env (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`), or via a `[rust]`
   section in `sky.toml`. Precedence: CLI > env > `sky.toml` > AUTO.
2. The emitted cargo crate activates a **`dlmalloc` default** global
   allocator (pure Rust) for the static build, `mimalloc` as an explicit
   opt-in, `system` (no allocator item compiled) elsewhere. `talc` parses but
   is refused with a typed refusal (amendment A1).
3. The emitted crate carries a marker-headed `.cargo/config.toml` supplying
   `+crt-static` for the musl target, so a standalone
   `cargo build --target x86_64-unknown-linux-musl` run from the emitted
   crate dir produces a real static ELF — correct by construction.
4. TLS stays rustls with bundled webpki roots (already true of every emitted
   manifest; guarded, not migrated).
5. Static-ness is asserted, never assumed: `ldd` = "not a dynamic executable"
   / `file` = "statically linked", and the binary runs.

## Ground truth — the repo as it stands (verified; cite, don't re-discover)

1. **Paths after the rename/relayout.**
   - CLI crate `ipe` (binary `ipe`): `src/ipe-cli/` — driver `src/ipe-cli/src/lib.rs`,
     `sky.toml` parsing `src/ipe-cli/src/project.rs`.
   - Backend crate `ipe_backend_rust`: `src/compiler/backend/rust/` —
     manifest surgery + emitted-project assembly `…/src/project.rs`,
     entry/preamble slicing `…/src/preamble.rs`, version SSOT
     `…/src/crate_specs.rs`.
   - Runtime crate `ipe_runtime_rust` (flat): `src/runtime/rust/` — vendored
     into every emitted project from `src/runtime/rust/src/`.
   - Golden base fixtures: `tests/golden/basics/{Cargo.toml,main.rs}`; the
     emit output dir is `sky-out/rust/`.
2. **`ipe build` is emit-only; cargo runs externally.** `ipe run` invokes
   `cargo build` with **CWD = the emitted crate dir** (so an emitted
   `.cargo/config.toml` IS discovered by `ipe run`). The examples sweep
   (`scripts/equivalence-checks/examples-sweep.sh`) invokes
   `cargo build --manifest-path <example>/sky-out/rust/Cargo.toml` from the
   *example* dir — cargo discovers config from CWD ancestors, not from
   `--manifest-path`, so the sweep's future `--static` variant MUST
   `cd sky-out/rust` first (the CWD trap).
3. **Manifest surgery is anchored `replacen` with fail-loud
   `Diagnostic::CompilerBug`** (`db_cargo_toml` / `server_cargo_toml` / … in
   `project.rs`). The static splice follows this pattern verbatim.
4. **The old `static_alloc = ["mimalloc"]` default is manifest-wide.** The
   golden base `Cargo.toml` declares it and **every** golden `main.rs`
   (~80 fixtures) carries the 3-line mimalloc `#[global_allocator]` block in
   its preamble (the preamble is sliced from `tests/golden/basics/main.rs`,
   whose line numbers are asserted in `preamble.rs` tests — rebaselining
   shifts those). No emitter code path ever *activated* `static_alloc`; it
   was inert manifest surface.
5. **TLS is already rustls-only** (`reqwest` `default-features = false` +
   `rustls-tls` in both `src/runtime/rust/Cargo.toml` and the golden).
6. **The default graph is NOT C-free**: `zstd` (C) and `ring` (C/asm, via
   reqwest→rustls) are unconditional in the emitted manifest. Empirically
   verified (amendment A2): the full dep set builds static for
   `x86_64-unknown-linux-musl` with `+crt-static` only, on a host with a
   musl-capable C compiler (`musl-gcc` / `x86_64-linux-musl-gcc`); the
   binary is static-pie, `ldd`-clean, and runs — for system, dlmalloc, and
   mimalloc selections alike. The preflight therefore checks target
   installation AND the musl C compiler, refusing with actionable hints.
7. **There is no repo-root `.cargo/config.toml`** (D2 resolved). The
   shared-target pin lives in `~/.cargo/config.toml` / `CARGO_TARGET_DIR`.
   The emitted config never writes `target-dir`.
8. **The driver has an on-disk build cache** (EmittedProject tier + lowered-IR
   tier, `src/ipe-cli/src/cache.rs`) whose keys do NOT include the static
   plan. The static transform is therefore applied **post-emit at write
   time** on every path (cache hit or miss) as a deterministic function of
   the plan — cache keys stay untouched and a cached dynamic emit can never
   masquerade as a static one or vice versa.
9. **`ipe::build` / `build_project` / `build_with_sibling_discovery` have
   ~200 test call sites.** Static wiring is ADDITIVE (`*_with_options`
   variants + a `BuildOptions` default); existing signatures stay.
10. **The reconciler's prune pass is scoped to `out_dir/src`** and never
    touches the project root — the root-level `.cargo/config.toml` needs its
    own marker-guarded cleanup on non-static rebuilds (amendment A5).

Data flow:

```
ipe build --static --target … --allocator … --allow-slow-allocator   ┐
env IPE_STATIC / IPE_TARGET / IPE_ALLOC                              ├─▶ build_plan::resolve()
sky.toml [rust] static / target / allocator / allowSlowAllocator     ┘        │
                                                                              ▼  Result<Option<StaticPlan>, Refusal>
                               ┌── Refusal ──▶ CliError::StaticRefusal, no artifact
                               │
                        StaticPlan { triple, allocator }          (parse, don't validate)
                               │  + preflight (rustup target, musl C compiler)
                               ▼
      compile pipeline (unchanged, cache-key-untouched) ─▶ EmittedProject
                               ▼
      write path: webview-shape gate ─▶ staticize (feature splice + .cargo/config.toml)
                               ▼
      external cargo:  cd sky-out/rust && cargo build --target x86_64-unknown-linux-musl
                               ▼
      ldd = "not a dynamic executable" ∧ runs        ← asserted, not assumed
```

## Global constraints

- Principle order **security > correctness > soundness > efficiency >
  completeness > readability**; the two rules (*parse, don't validate*,
  *make invalid states unrepresentable*); THE SEAL (ipe exit-0 ⇒ emitted
  cargo-builds). A build asked to be static that cannot be static is
  **refused**, never silently degraded.
- No new `unwrap`/`expect`/`panic!`/raw indexing in shipping code; typed
  errors only (`Refusal` enum through `CliError::StaticRefusal`).
- A default (non-static) `ipe build` emits byte-identical output to the
  rebaselined goldens: no allocator feature active, no config file, both
  allocator arms inert behind `#[cfg(feature = …)]`.

## Milestones

### M1 — Backend: `alloc_*` feature family + cfg-gated arms + `static_build` module  [LANDED]

**Files:** `tests/golden/basics/Cargo.toml`, every `tests/golden/*/main.rs`,
`tests/golden/{mm_diamond,mm_local_pkg,multi_mod_split_pilot}/Cargo.toml`,
`src/compiler/backend/rust/src/preamble.rs` (line-anchor tests),
new `src/compiler/backend/rust/src/static_build.rs` (+ `lib.rs` export).

- Golden base manifest: replace `static_alloc = ["mimalloc"]` +
  `mimalloc = { version = "0.1", optional = true }` with
  `alloc_dlmalloc = ["dep:dlmalloc"]` / `alloc_mimalloc = ["dep:mimalloc"]`
  and `dlmalloc = { version = "0.2", features = ["global"], optional = true }`
  / `mimalloc = { version = "0.1", optional = true, default-features = false }`.
  `default` list unchanged ⇒ system allocator by default.
- Every golden `main.rs`: replace the mimalloc block with the two cfg-gated
  arms (dlmalloc + mimalloc); update `preamble.rs` line-anchored tests.
- `static_build.rs`: closed `StaticTriple` (x86_64 musl only — A3), closed
  `StaticAllocator { Dlmalloc, Mimalloc, System }`, `StaticPlan`,
  `staticize_manifest` (anchored default-list splice, `CompilerBug` on
  drift), `cargo_config` (marker + `+crt-static`), `manifest_is_webview`
  (typed shape probe over the emitted default-feature list).
- **Gate:** `cargo test -p ipe_backend_rust` + `-p ipe` green; probe crate
  builds+runs static under all three selections (A2).

### M2 — CLI: typed plan resolution + flags/env/`sky.toml` + preflight  [LANDED]

**Files:** new `src/ipe-cli/src/build_plan.rs`; `src/ipe-cli/src/lib.rs`
(`CliError::StaticRefusal`, `run_build` flags, USAGE);
`src/ipe-cli/src/project.rs` (`[rust]` section → `StaticManifestSection`).

- `AllocatorChoice::parse` closed over `{auto,system,dlmalloc,talc,mimalloc}`
  (unknown → `Refusal::UnknownAllocator`, incl. `jemalloc`/`snmalloc`).
- `resolve(request) -> Result<Option<StaticPlan>, Refusal>` encoding: AUTO
  (musl→Dlmalloc; non-static→None), `MuslMallocCliff` (system+musl without
  ack — CLI flag or `allowSlowAllocator` toml key both satisfy the gate),
  `TalcRequiresArenaDesign` (A1), `UnknownStaticTarget` (A3),
  `TargetRequiresStatic` (`--target` without `--static`).
- Precedence merge CLI > env > toml as a pure function (env injected as
  data, testable without env mutation).
- Preflight: `rustup target list --installed` (fail-soft when rustup absent)
  + musl C compiler presence (`x86_64-linux-musl-gcc` / `musl-gcc` /
  `CC_x86_64_unknown_linux_musl` / `TARGET_CC`), each with an actionable
  install hint. Runs before the compile pipeline.
- **Gate:** unit tests per refusal + per AUTO row + precedence rows.

### M3 — Write-path wiring: options threading + staticize + hygiene  [LANDED]

**Files:** `src/ipe-cli/src/lib.rs`.

- `BuildOptions { static_plan }` + additive `build_project_with_options` /
  `build_with_sibling_discovery_with_options`; existing entry points delegate
  with the default (no churn across ~200 call sites).
- `write_emitted_project` gains the plan: webview manifests under a static
  plan are refused (`Refusal::WebviewStatic`) before ANY file is written;
  static plans splice the manifest + add the marker-headed
  `.cargo/config.toml`; non-static writes remove a stale marker-carrying
  config (A5). Applied identically on cache-hit and cache-miss paths.
- **Gate:** integration tests — static emit contains `alloc_dlmalloc` in the
  default list + the config file; re-emitting non-static removes the config
  and restores the baseline manifest byte-identically.

### M4 — End-to-end static proof  [LANDED]

**Files:** `src/ipe-cli/tests/static_e2e.rs` (gated `IPE_E2E_STATIC=1`).

- Emit `examples/01-hello-world` with the static plan → standalone
  `cargo build --target x86_64-unknown-linux-musl` with CWD = emitted crate
  dir → assert `ldd` says "not a dynamic executable" (or "statically
  linked") → run the binary → assert stdout. THE SEAL end-to-end.
- **Gate:** `IPE_E2E_STATIC=1 cargo test -p ipe --test static_e2e` green;
  manual `file`/`ldd`/run transcript recorded in the landing report.

### M5 — Follow-ups (ordered, not yet started)

1. **TLS/static-compat regression guard** — manifest-scan test freezing
   rustls-only + bundled webpki roots (no `rustls-tls-native-roots`).
2. **Sweep `--static` variant** — `scripts/equivalence-checks/examples-sweep.sh`
   builds every non-webview example static (CWD = crate dir per ground
   truth 2) and `ldd`-asserts each; server-shape example serves a request.
3. **`ipe run --static`** — same flags on `run`; cargo already runs with
   CWD = crate dir there.
4. **CI matrix** (spec §4.3, trimmed): linux-static-x64 (build+run in a
   `scratch` container), linux-static-mimalloc, refusal negative tests,
   supply-chain (`cargo audit`/`deny` over the emitted lock + SBOM).
5. **`aarch64-unknown-linux-musl`** once a verifying host/CI lane exists (D6).
6. **Windows `+crt-static` / wasm / `--macos-portable`** per the §2 matrix.
7. **talc arena design** (A1) — requires a no-unsafe arena story first.
8. **D4 `zstd`→`ruzstd` + `ring` policy** — makes the pure-Rust
   `link-self-contained` no-C-toolchain arm real.
9. **§4.5 measure-before-finalize bench** — sizes the mimalloc opt-in
   recommendation and fills the divergence `<X>`; does not flip the default.
10. **Divergence record** — sanctioned `oracle_divergence` for the allocator
    default per `docs/architecture/divergence-policy.md`.
