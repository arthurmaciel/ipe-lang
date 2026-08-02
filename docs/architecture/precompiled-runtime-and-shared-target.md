# Precompiled runtime crate and shared build-once target

The design for strategies S3 (runtime as a real dependency crate) and S2
(ipe-managed shared `CARGO_TARGET_DIR`) from
`compilation-performance.md` — the pair that fixes the **warm** dev loop.
S1 gated the dependency floor and cut the cold build; the warm rebuild is
unchanged at ~2.4 s because every edit recompiles the ~2.6 MB runtime
vendored as source *inside* the user crate. S3 moves the runtime out of the
user crate; S2 makes its compiled artifacts (and the whole dependency
closure) build once per machine.

Scored against the strict precedence Security > Correctness > Soundness >
Efficiency: both strategies are Efficiency-only changes that must not weaken
anything above them. The reuse argument leans entirely on cargo's own
fingerprinting — everything still compiles from source, locally, which is
what distinguishes this pair from the rejected S4 prebuilt-binary route.

## What the warm 2.4 s is made of

`ipe run` on an already-built project, after one edit to `main`:

1. re-emit (milliseconds — the emit cache in `src/ipe-cli/src/cache.rs`
   skips the compiler pipeline; `reconcile_emitted_project` is content-gated,
   so only the changed file is rewritten),
2. `cargo build`: re-fingerprint the dep closure (all cached), then **rustc
   over the one emitted crate** — the user's generated `main.rs` *plus* the
   entire vendored runtime tree under `out/rust/src/ipe_runtime/` (~65
   modules, ~3.4 MB source, trimmed per program), because they are one
   compilation unit,
3. link.

The measured floor row in `compilation-performance.md` (hand-trimmed
runtime, 1 crate) puts the same edit at ~0.5 s — the difference is entirely
the vendored-runtime recompile. That is the cost S3 removes: once the
runtime is a dependency crate, cargo fingerprints it like any other dep and
an edit to user code recompiles user code only.

## S3 — the runtime as a dependency crate

### The emit model, before and after

Today (`src/ipe-cli/src/lib.rs`):

- `resolve_runtime()` (`lib.rs:1653`) locates the runtime *source tree*
  (`$IPE_RUNTIME_DIR`, else an upward walk to
  `src/runtime/rust/src`).
- `build_emit_manifest` (`lib.rs:1415`) copies that whole tree into the
  project manifest under `src/ipe_runtime/` (`collect_dir_text`,
  `lib.rs:1420`), then lets three backend-generated overlays win over the
  copies: `ipe_runtime/mod.rs` (the trimmed module set),
  `ipe_runtime/config.rs` (DB driver bindings), and
  `ipe_runtime/env_public.rs` (the `[wasm] publicEnv` allowlist match).
- The emitted `Cargo.toml` (base `src/compiler/backend/rust/templates/Cargo.toml`
  plus the `*_cargo_toml` surgery functions in
  `src/compiler/backend/rust/src/project.rs`) carries the third-party
  dependency list directly.

After S3, the emitted project contains **no runtime source**. Its manifest
gains one dependency entry — illustrative shape (the emitter does not
produce this yet; the path is filled in from runtime resolution):

```toml
[dependencies]
ipe_runtime = { package = "ipe-runtime-rust", path = "<resolved runtime crate root>", default-features = false, features = ["json", "async", "db", "db-sqlite"] }
```

and the surgery functions collapse into feature selection. The `package`
rename makes the extern crate visible as `ipe_runtime`, and the extern
prelude (edition 2018+) resolves `ipe_runtime::…` from *any* module without
imports — which is why the generated-code churn is tiny:

- `templates/main.rs:12` drops `pub mod ipe_runtime;`; the following
  `pub use ipe_runtime::*;` line and every `ipe_runtime::…` path in
  generated code keep working unchanged.
- The three emitter sites that spell `crate::ipe_runtime::…`
  (`project.rs:1716`, `project.rs:1723`, `emit_expr.rs` — the
  `IpeStringify` derive path) become `ipe_runtime::…`.
- The path is emitted absolute, canonicalized, and TOML-escaped (forward
  slashes / literal string) — never interpolated from project-controlled
  input.

The two per-project overlays that generate *code* move into the user crate,
where per-project code belongs:

- `env_public.rs` (`render_env_public_rs`, `project.rs`): emitted as a user-
  crate module (`src/ipe_env_public.rs`, `pub use`d from `main.rs`); its one
  `use super::core::IpeMaybe` becomes `use ipe_runtime::core::IpeMaybe`. It
  is std-only, so nothing else changes.
- `config.rs` (DB driver aliases + `ipe_db_url`): becomes driver *features*
  on the runtime crate — see the trimming map below.

### Module trimming becomes feature selection

Two mechanisms express today's trimming:

1. **Manifest surgery** — `db_cargo_toml`, `server_cargo_toml`,
   `web_cargo_toml`, `tui_cargo_toml`, `webview_cargo_toml`,
   `websocket_cargo_toml`, `tea_cargo_toml`, `http_client_cargo_toml`,
   `email_cargo_toml`, `url_cargo_toml`, `config_cargo_toml`,
   `compression_cargo_toml`, `crypto_cargo_toml`, `jwt_cargo_toml`,
   `crypto_core_heavy_cargo_toml`, `async_runtime_cargo_toml`, … in
   `project.rs`.
2. **Module-set surgery** — the base `templates/ipe_runtime/mod.rs` plus the
   `RUNTIME_MOD_RS_*_APPEND` constants, driven by the IR's `uses_*` flags.

The runtime crate (`src/runtime/rust/Cargo.toml`, package
`ipe-runtime-rust`) already carries most of the target state: a `[features]`
table (`async`, `json`, `db`, `redis_store`, `crypto`, `compression`,
`csv_kernel`, `cache_kernel`, `tui`, `config`, `server`, `http_client`,
`email`, `websocket_client`, `web`, `locale`, `wasm-client`, `full`) with
optional deps, and `#[cfg(feature = …)]` gates on most heavy modules in
`src/runtime/rust/src/mod.rs`. The usage-driven gating was built in the
emitted manifest *and* largely mirrored here — so the mapping is mostly a
transcription, not an invention.

The gaps — where the crate's feature graph is coarser than the emitted
trimming and must be extended before the switch:

| Emitted trimming today | Crate state today | S3 feature |
|---|---|---|
| `url` crate gated on `uses_url` (`url_cargo_toml`); `url`/`ssrf` modules appended | `url = { version = "2" }` **non-optional**; `pub mod url;` ungated | new `url` feature; dep becomes optional. Without this, S3 silently reintroduces the idna/ICU4X subtree the usage-driven floor removed |
| `crypto_core` (sha2 floor) always emitted; `rsa` gated off the floor; heavy crypto appended on use | `crypto_core` is `cfg(feature = "crypto")` — the *heavy* feature | split: `crypto_core` (sha2/hmac floor) vs `crypto` (rsa, bcrypt, AEAD, pbkdf2); `crypto` implies `crypto_core` |
| DB driver chosen per project (`db_cargo_toml(base, driver)` → sqlx feature + the generated `config.rs` aliases) | one `db` feature, sqlite+postgres both in the sqlx dep | `db-sqlite` / `db-postgres` selecting the alias set and the sqlx driver feature; `compile_error!` if both are enabled (only the emitter ever selects one — SEAL-checked — so the error is a fail-closed guard, not a user surface) |
| sync `fn main` programs shed tokio/bcrypt (`async_runtime_cargo_toml`) | `tokio` optional, `async` feature exists | already expressible; the emitter selects `async` iff the program is async |
| base template ships `sha2`, `bcrypt`, `chrono-tz` unconditionally (pre-gating floor residue) | `sha1`/`md-5` non-optional, `bcrypt` optional, `chrono-tz` non-optional | align during the parity work: whatever the emit floor gates, the crate gates identically. Any future floor gating (e.g. `chrono-tz`) lands as a feature on the crate first |
| `jsonwebtoken` appended with `jwt` module | folded into `json = ["serde_json", "jsonwebtoken"]` | acceptable initially (coarse); an optional later `jwt` split is an Efficiency refinement, not a correctness issue |
| pure std modules trimmed by mod.rs omission (`html`, `dom`, `css`, `ui`, `tea` types, `telemetry`, …) | always compiled | **deliberately not featured.** They cost one-time runtime-crate compile, never the warm loop; the linker GCs unused code. Feature count stays proportional to *dependency-bearing* surfaces, not modules |
| `alloc_dlmalloc` / `alloc_mimalloc` (global allocator in `main.rs`) | n/a | stay user-crate features, unchanged |
| `uses_ffi` — `mod ffi;` in `main.rs` + project-specific crate deps (`ffi_cargo_toml`) | n/a | unchanged: FFI wrappers and their deps are user-project code and remain in the emitted crate |

The authority for the mapping is a new single source of truth in the
backend: `runtime_features(&ir::Module) -> BTreeSet<&'static str>`, replacing
the append constants — one function the emitter, the SEAL, and the shared-
target tooling all read.

### Version pinning and the install story

The runtime crate version is the compiler version (they already release
together). Pinning is structural, not declarative: the emitted dependency is
a **path** into the same toolchain installation that ran the compiler, so an
`ipe` binary can never pair with a foreign runtime by accident. Two guards
make skew fail-closed anyway:

- a SEAL test asserting `src/runtime/rust/Cargo.toml`'s `version` equals the
  compiler's `CARGO_PKG_VERSION`;
- resolution returns a typed `RuntimeCrate` (parse, don't validate): the
  resolver reads the candidate's `Cargo.toml` and verifies package name
  `ipe-runtime-rust` and the exact expected version, refusing with a clear
  diagnostic otherwise.

`resolve_runtime` today only finds an in-repo/env source tree — an installed
`ipe` binary has no runtime. S3 closes that gap. The toolchain ships the
runtime **as a source crate** (Cargo.toml + src/), and resolution order
becomes:

1. `$IPE_RUNTIME_DIR` — now names a *crate root*. A path that is a bare
   `src/` tree (the old meaning) is refused with a message naming the new
   expectation — fail closed, no guessing.
2. Binary-relative: `<dir of current_exe>/../lib/ipe/runtime/` — the
   installed layout every packaging target (tarball, installer) provides.
3. Upward walk to `src/runtime/rust/` — the in-repo development case.
4. The legacy sibling paths, retired once nothing uses them.

Shipping source (not rlibs) keeps the S4 rejection intact: users compile
everything locally; there is no shipped object code, no signing surface, no
platform × toolchain artifact matrix.

### The SEAL adapts: module-set closure → feature-set closure

Fail-closed invariant, unchanged in spirit: **`ipe` exiting 0 implies the
emitted project `cargo build`s.** The breach class moves from "appended
module whose `use crate::…` closure is missing from the emitted `mod.rs`"
to "selected feature set under which the runtime crate does not compile or
does not export a referenced kernel".

- `src/compiler/backend/rust/tests/runtime_modset_closure.rs` (the fast
  static SEAL over all reachable `uses_*` masks, `FLAG_COUNT = 18`) becomes
  `runtime_featureset_closure`: for every reachable mask it computes
  `runtime_features(...)` and statically checks (i) every selected feature
  is declared in the runtime crate's `[features]` universe, (ii) every
  module the emitted code can reference under that mask is `cfg`-satisfied
  by the selected set (walking the crate's own `src/mod.rs` cfg attributes —
  the same source-parse discipline the modset test uses today), and (iii)
  the intra-crate `use crate::<dep>` closure holds under that cfg valuation.
- Emit-time guard: selecting a feature outside the declared universe is an
  internal error that refuses the build — never a cargo failure downstream.
- The ground-truth E2E gate (`src/ipe-cli/tests/seal_modset.rs`, real
  `cargo build`s across shapes) keeps its shape and now exercises the
  feature graph.
- `crate_specs.rs` + its `crate_specs_match_manifests` drift test shrink:
  the emitted native manifest no longer carries third-party versions (the
  runtime crate's own `Cargo.toml` is the single manifest declaring them),
  so the drift surface reduces to the runtime crate plus the WASM template,
  which still vendors (below).

### The emit cache and reconcile

The cache already stores `EmittedProject` (backend output only — no runtime
files), so keys and epochs are untouched. `build_emit_manifest` drops
`collect_dir_text` — the reconcile manifest becomes user files + manifest
only, the orphan-prune scope shrinks with it, and the "vendor first, emit
second" precedence rule retires. `out/rust/` becomes readable at a glance:
the user's generated code and one small `Cargo.toml`.

The trade: the emitted project is no longer self-contained — `cargo build`
inside `out/rust` works only where the toolchain (or repo) provides the
path target. That is acceptable for a build artifact; it is not a source
distribution. No `--vendor-runtime` escape hatch ships unless a concrete
need appears (and per policy it would not be advertised before it works).

### Out of scope: the WASM target

The `--target wasm` emit keeps its own manifest (`WASM_CARGO_TOML`,
`project.rs`) and vendored module set for now: it pins `wasm-bindgen`
exactly, has a distinct dependency floor, and its build is not the warm
loop this design targets. The runtime crate already compiles on wasm32
under `wasm-client`, so unifying it onto the dependency model is a natural
follow-up — tracked, not bundled.

## S2 — the shared build-once target

### Key derivation

`src/ipe-cli/src/cache.rs::derive_epoch` already computes exactly the right
identity: `sha256(len-prefixed: tag, compiler_revision_hash,
toolchain_fingerprint_hash)` — ipe version/revision × rustc fingerprint.
S2 reuses that derivation (with its own domain tag) to key a directory,
`<cache root>/target/<epoch>/`.

**The resolved feature set is deliberately NOT in the key.** All projects on
one (ipe, rustc) pair share one target directory. Cargo hashes each unit's
feature set (with profile, flags, and source identity) into its
`-C metadata`/fingerprint, so a dependency built under two different feature
unifications yields two artifacts that *coexist* in the same target dir —
both stay cached, neither thrashes the other. That is precisely the
mitigation for the heterogeneous-feature fragmentation the usage-driven
floor introduced: with a feature-set-keyed directory scheme, two projects
differing in one feature would duplicate the entire shared closure; with
one directory, they share every unit whose resolved features coincide (the
runtime's dependency closure for a given feature set — the expensive part)
and diverge only where they genuinely differ. Distinct projects' final
`ipe-app` crates also coexist: the workspace path differs per project and
lands in the metadata hash. The plan verifies this coexistence empirically
in the first shared-target step; if real-world thrash is ever observed, the
fallback is the finer key `(epoch, feature-set-hash)` — a one-line change
to the key function, at a disk cost.

### Placement, precedence, invasiveness

This is the one part of the pair that touches the user's environment, so it
is designed to be least-surprising and fully overridable — explicit
configuration, no magic, and **nothing of the user's own configuration is
ever written or changed**: not `~/.cargo/config.toml`, not global
environment, nothing. `ipe` sets `CARGO_TARGET_DIR` only in the environment
of the child `cargo` process it spawns, and stores artifacts only under its
own cache namespace.

- Location: `$XDG_CACHE_HOME/ipe/target/<epoch>/`, i.e. `~/.cache/ipe/…` on
  Linux, the platform cache-dir convention elsewhere. Same namespace
  discipline as the emit cache's `IPE_BUILD_CACHE_DIR` override precedent.
- Precedence, first match wins:
  1. an explicit user `CARGO_TARGET_DIR` in the environment — always
     respected, exactly as the CLI already honours it today
     (`lib.rs:2116` probes it to locate the built binary);
  2. `ipe.toml` `[build] target = "local" | "shared" | "<path>"` — the
     Elm-style explicit project config;
  3. `IPE_TARGET_DIR=<path>` / `IPE_TARGET=local` environment override;
  4. default: shared.
- **Default: shared (on), surfaced loudly.** The first build that creates
  the directory prints a one-line notice naming the path and the opt-out
  (`[build] target = "local"`). Rationale for default-on rather than
  opt-in: the strategy exists for the first-contact user on a cold machine —
  the person who will never discover an opt-in flag before the 40 s cold
  build has already made its impression; a per-project target defeats S2
  entirely. Precedent: Elm's shared `~/.elm` cache, default-on with an
  `ELM_HOME` override — the model this project already looks to first. The
  invasive surface is confined to one directory under the platform cache
  convention, discoverable via `ipe cache` and the first-use notice.
- `ipe cache` subcommand: `ipe cache` (print path, epochs, sizes),
  `ipe cache clean` (remove non-current epochs; `--all` for everything,
  with a warning). Ships together with the default flip — the default is
  never on before the discoverability tooling exists.
- Release builds share the same target directory (cargo separates profiles
  itself); no split needed.

### Correctness: cargo's fingerprinting, no new trust

Nothing changes about *what* is compiled — same sources, same lockfile-
resolved versions, same rustc, compiled locally. The only change is *where*
artifacts live. Deciding whether a cached artifact is reusable is cargo's
fingerprint (toolchain, profile, flags, features, dependency graph, source
hashes) — the identical mechanism that makes a cargo workspace's shared
target, or any user-set global `CARGO_TARGET_DIR`, sound today. No new
equivalence argument, no new trusted input. A corrupted cache degrades to a
rebuild (or a cargo error), never to silently wrong reuse beyond what any
cargo user already accepts. Contrast S4, which imports foreign object code
and therefore provenance, signing, and reproducibility obligations —
rejected on precedence; S2 has none of that surface.

One residual reuse-quality (not correctness) concern: each project keeps
its own `Cargo.lock`, so two projects can resolve different semver-
compatible dependency versions and share less than expected. The hardening
step ships a blessed `Cargo.lock` for the full-feature closure with the
toolchain and emits it into projects — improving both artifact reuse and
supply-chain reproducibility (exact pinned closure, reviewable once).

### Concurrency and reclaim

- Concurrent builds: cargo's own target-directory locking serializes
  conflicting writes — the same story as two terminals building one
  workspace. No ipe-side locking is added.
- `ipe cache clean` only removes epochs other than the current one by
  default, so it cannot race a live build in the current epoch; `--all`
  warns. Epochs go stale precisely on ipe/rustc upgrade, so "remove
  non-current" is the whole GC policy; an optional size budget with
  oldest-epoch-first removal covers long-lived machines.
- Honest disk cost: one epoch is roughly 1–3 GB once a complex app's
  full-feature closure is built (comparable to today's per-project
  `out/rust/target`, but paid once per machine-epoch instead of per
  project — for more than one project this is a net disk *reduction*).

### When S2 is not worth it

- Single-project machines: after the first build, a per-project target is
  already warm; S2 then only saves the `rm -rf out/` ritual and future
  projects. If S3 lands and feedback shows one-project usage dominates,
  shipping S2 opt-in first and flipping the default later is the cheap
  retreat — the design isolates the default choice to one function.
- Environments with unreliable file locking (NFS homes) or hermetic CI
  sandboxes: opt out (`target = "local"`), documented next to the notice.
- Disk-constrained machines: `ipe cache clean` plus the size budget; worst
  case, opt out.

## Predicted warm loop

Anchored two ways: the measured table in `compilation-performance.md`
(hello-world, warm dev box), and a direct prototype measurement of the
S3 emit model itself.

- Measured today: warm rebuild 2.4 s with the vendored runtime in-crate;
  0.5 s for the hand-trimmed 1-crate floor (a small runtime still compiled
  in-crate); cold 40 s after the usage-driven floor.
- **Prototype measurement of the dep model**: a scratch crate depending on
  the real `ipe-runtime-rust` via the exact dependency shape above
  (`package` rename, `default-features = false`, `features = ["json"]`,
  trivial `main` using `ipe_runtime::…` paths from a nested module) builds,
  and a `touch src/main.rs` rebuild takes **0.33–0.38 s** on the same class
  of warm dev box — default linker, default dev profile *with* debuginfo
  (the emitted profile's `debug = 0` can only be faster). The same run also
  confirmed the package-rename/extern-prelude mechanics and, incidentally,
  the `url` gap: the non-optional `url` dep compiled even under the minimal
  feature set.
- After S3, an edit recompiles only the generated `main.rs` and links
  against the prebuilt runtime rlib plus its dep closure. A real generated
  `main.rs` is larger than the prototype's, and heavier feature sets link
  more — the ranges below widen the measured point accordingly.

Predicted, warm edit→run:

| Configuration | Predicted |
|---|---|
| Hello-world-class, S3 (+S2), default linker | **~0.35–0.7 s** (measured 0.33–0.38 s for the prototype; real generated mains and the ~44-crate closure push the upper end) |
| + mold/lld (S8) | ~0.25–0.5 s |
| Complex app (server + Db, ~300-crate closure), default linker | ~1–2.5 s (link-dominated) |
| Complex app + mold | ~0.7–1.2 s |

S2 additionally collapses the *second and subsequent projects'* cold build
to roughly warm-plus-first-user-crate-compile (~1–2 s); the first-ever cold
build per machine-epoch is unchanged (the usage-driven floor's territory).

**Input to the S6 go/no-go:** S3+S2 credibly deliver "sub-second common
programs, few-seconds complex apps" — the second budget row — but a floor
of ~0.3 s remains (cargo + rustc invocation + link; the prototype's 0.33 s
is essentially that floor), one to two orders of magnitude above the
"milliseconds" requirement for common programs. S6 (the IR interpreter)
remains the only route to that top row; what changes is urgency, not
necessity: a ~0.35 s loop is a usable product while S6 is built. The
hello-world point is prototype-measured; the complex-app rows are
decomposition estimates — the plan's final step re-measures everything and
updates the performance doc before any S6 decision.

## Coupling and ordering

**S3 lands first.** It is the structural change (emit model), it delivers
the warm-loop win even with per-project targets (the runtime compiles once
per project at first build, then never again on edits), and it produces the
stable, feature-keyed crate S2 wants to share. S2 alone would help cold
cross-project builds but cannot touch the warm loop while the runtime lives
inside the user crate — and S2 after S3 is a small, self-contained CLI
change. Interaction with the shipped usage-driven gating: the per-kernel
attribution becomes the feature map (same flags, new output), and S2's
single shared directory neutralizes the feature-set fragmentation that
heterogeneous per-project manifests would otherwise cause across projects.

## Implementation plan

Ordered, test-first, each step independently landable and green. Golden
re-blessing is mechanical and automated; its size is noted, not weighed.

1. **Runtime feature parity** (runtime crate only; emit untouched, so the
   vendoring path is unaffected — the crate's own `mod.rs` and features are
   not consulted by today's emit). Tests first: a feature-universe test
   (every planned feature declared; `db-sqlite`/`db-postgres` mutual-
   exclusion `compile_error!`), plus a representative standalone
   `cargo check` feature matrix in CI. Then: make `url` optional behind a
   `url` feature; split `crypto_core` from `crypto`; add the driver
   features (absorbing the generated `config.rs` aliases as cfg'd code);
   align the floor optionals (`sha1`/`md-5`/`bcrypt`/`chrono-tz`) with
   whatever the emit floor gates.
2. **Feature-map single source of truth in the backend.** Test first:
   `runtime_featureset_closure` (all reachable `uses_*` masks → declared
   universe + cfg-satisfaction + `use crate::` closure, as specified above)
   written against the not-yet-existing `runtime_features(...)`; then
   implement the map alongside the existing append logic. Pure addition;
   the old modset test still guards the still-default vendoring path.
3. **Emit-model switch behind a flag** (`IPE_RUNTIME_DEP=1`, internal):
   dependency line + feature selection in `project.rs`; `main.rs` template
   drops `pub mod ipe_runtime;`; the three `crate::ipe_runtime::` sites;
   the `env_public` relocation into the user crate; `resolve_runtime`
   returns a typed `RuntimeCrate` (crate root) in this mode; the CLI skips
   the runtime copy. Tests: the seal E2E and the example sweep run under
   the flag; a unit test that the emitted manifest's path is absolute and
   escaped. Default path untouched — no golden churn yet.
4. **Flip the default; consolidate SEAL and goldens.** Retire the mod.rs
   template + `RUNTIME_MOD_RS_*_APPEND` constants and the modset closure
   test (superseded by the featureset closure); shrink `crate_specs.rs` to
   the WASM/vendored surface; re-bless goldens (~512 cases: a one-line
   region in every `main.rs`, the four golden `Cargo.toml`s become
   dep+features, the blessed `ipe_runtime/` overlay dirs retire);
   `build_emit_manifest` drops `collect_dir_text`. Binary-size and
   behavior checks ride the existing example sweep.
5. **Install resolution.** Tests first against a fake installed layout
   (binary-relative `../lib/ipe/runtime/`): resolution order, version-skew
   refusal, `IPE_RUNTIME_DIR`-as-crate-root including the fail-closed
   message for a bare src tree; the version-pin SEAL test. Then the
   resolver change + packaging-script updates.
6. **The shared target.** Tests first: key-derivation unit tests (reusing
   the `derive_epoch` components), the precedence chain (env pin >
   `ipe.toml` > `IPE_TARGET_DIR` > default), opt-out honored, first-use
   notice printed exactly once, artifact-coexistence smoke (two projects,
   two feature sets, one epoch dir — the second build compiles no shared
   dep twice; two *parallel* builds complete under cargo's lock). Then the
   target-dir module, `ipe cache` (path/status/clean), and the default
   flip — in that order, default last.
7. **Blessed `Cargo.lock`** shipped with the toolchain and emitted into
   projects (reuse + reproducibility hardening). Test: emitted project
   builds `--locked`; drift between the blessed lock and the runtime
   manifest fails a SEAL check.
8. **Re-measure** the table (cold/warm × hello/complex × linker) and update
   `compilation-performance.md`; this measurement is the S6 go/no-go input.

## Risks and honest costs

- **Feature-combination compile coverage.** The runtime crate must compile
  under every emitter-reachable feature set. The static featureset-closure
  test plus the E2E seal gate cover it, but cfg discipline inside the
  runtime (a module referencing a gated sibling without matching gates)
  becomes a permanent maintenance rule — exactly the class the closure test
  exists to catch without cargo.
- **Non-additive driver features.** `db-sqlite`/`db-postgres` are mutually
  exclusive, which cuts against cargo's additive-features convention. Only
  the emitter selects them and a `compile_error!` guards the union, but it
  is a wart to document, and it makes the runtime crate slightly hostile to
  hypothetical third-party direct use.
- **Emitted-project self-containment.** Net readability win (user code
  only), but the project references the toolchain by absolute path, is not
  relocatable, and `cargo build` outside a machine with the toolchain
  fails. Accepted; documented.
- **Version pinning.** Path-dependency into the same install makes skew
  structurally hard, and the typed resolver + version SEAL make the
  remaining cases fail closed. The `IPE_RUNTIME_DIR` meaning change (src
  tree → crate root) is a small breaking change for dev workflows.
- **Golden churn.** ~512 golden cases and the golden test binaries re-bless
  mechanically when the default flips.
- **Install-story work.** Every packaging path must ship the runtime source
  tree at the binary-relative location; until the install-resolution step
  lands, an installed binary still has no runtime (the pre-existing gap —
  S3 makes it visible and then closes it).
- **WASM divergence.** Native emits a dependency; wasm keeps vendoring —
  two emit models to maintain until the follow-up unification.
- **S2's environment footprint.** One directory under the platform cache
  convention, 1–3 GB per (ipe, rustc) epoch, default-on. Mitigations:
  first-use notice, `ipe cache`, three override layers, and the documented
  opt-in retreat if default-on proves the wrong call. S2 is *not* worth
  shipping default-on if usage is dominated by single-project machines or
  lock-hostile filesystems — evidence the re-measurement step and early
  feedback can supply before the default flips.
- **Binary size / always-compiled pure modules.** Dropping below-feature-
  granularity trimming leans on linker GC; the default-flip step measures
  the emitted binary against the current table to confirm no regression.
