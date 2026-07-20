# Blockers surfaced by the upstream Sky-example mirror sweep

Honest ledger (PRINCIPLES.md §0) of every defect the mirror + sweep surfaced.
Two kinds: **fixed here** (root-caused, in-boundary) and **filed** (a tracked
blocker the owning lane must close). None is papered over.

## Fixed here

### 1. SEAL violation — `dom` in the base runtime module set without `html`
`ipe build` exit-0 then `cargo build` fail (E0432 `unresolved import crate::html`)
for every NON-render program (plain CLI / headless server). Root cause:
`src/compiler/backend/rust/src/project.rs` declared `pub mod dom;` unconditionally
in the base `ipe_runtime/mod.rs` (`tests/golden/basics/ipe_runtime/mod.rs`), but
`dom` is mutually referential with `html` (`html.rs` calls
`crate::dom::form::decode_form_or_warn`; `dom/{diff,dispatch,form}.rs` import
`crate::html::*`), and `html` is only appended for render programs
(`RUNTIME_MOD_RS_UI_APPEND`). A non-render program got `dom` with an unsatisfied
`crate::html`. Fix: moved `pub mod dom;` out of the base set into
`RUNTIME_MOD_RS_UI_APPEND`, so `dom` and `html` always appear together. Proven:
01-hello-world / 07-todo-cli (non-UI) now cargo-build-0; 09-live-counter (UI) +
server_e2e (11/11) + wasm_target_gate stay green.

### 2. `scripts/lib/env.sh` — wrong `IPE_RUNTIME_DIR` default
Exported `$REPO/src/runtime/rust/src/sky_runtime`, a path that does not exist
(the runtime `.rs` files sit directly under `$REPO/src/runtime/rust/src`, no
`sky_runtime` subdir). A sweep that sourced env.sh got a bad runtime dir and
`ipe build` mis-vendored → the emitted crate cargo-failed. Fixed to the real
path.

### 3. CI — miri job referenced non-existent crates (the confirmed CI-fail cause)
`.github/workflows/ci.yml` miri job ran
`-p sky_intern -p sky_ir -p sky_types -p sky_lower -p sky_diagnostics -p sky_backend_rust`
— all renamed to `ipe_*`. `cargo miri` errors immediately on an unknown package.
Renamed to `ipe_intern ipe_ir ipe_types ipe_lower ipe_diagnostics ipe_backend_rust`
(all verified to resolve via `cargo pkgid`), plus the stale crate names in the
surrounding comment.

### 4. CI — E2E job self-skipped, so THE SEAL was never enforced
The `e2e` job gated on a `vendor/upstream-sky/runtime-rust/src/sky_runtime` path
that never landed; detection always failed → E2E always skipped → an
`ipe`-exit-0-then-cargo-fail regression (exactly blocker #1) could ship green.
Pointed the job at the in-repo runtime (`src/runtime/rust/src`). Left it
`continue-on-error: true` (non-gating) for now because turning it on surfaces
the pre-existing oracle-staleness backlog (filed #A) — flip to gating once that
backlog is regenerated.

## Filed (tracked blockers — NOT fixed here)

### A. ~101 of 450 goldens are oracle-stale
`tests/golden/<name>/oracle.meta`'s cached `Main.ipe` sha256 no longer matches
the on-disk `Main.ipe` for ~101 goldens — the `.sky`→`.ipe` (commit 8446a500) +
namespace-flatten (62d9eefa) renames rewrote the fixtures without a
`refresh-oracle` pass. The E2E oracle guard (`support/mod.rs`) correctly fails on
the mismatch. Regeneration is `cargo run -p refresh-oracle -- <golden>` (rebuilds
the Go reference) — owner + Go-toolchain territory, out of CI-prep scope. Blocks
flipping the E2E job to gating.

### B. `adts` backend test — enum-stringify emit red (pre-existing)
`cargo test -p ipe_backend_rust --test adts` fails 3/5
(`concrete_multi_field_enum_emits`, `generic_enum_def_construction_and_pattern_emit`,
`recursive_enum_boxes_self_edges`) — "payload stringify arm missing or wrong".
Confirmed identical on clean master HEAD (797ac11b) with my changes stashed — a
pre-existing enum-emit defect, unrelated to the mirror. Filed for the backend
lane.

## Sky→Ipê language divergences (a mirrored example needs a BEHAVIOURAL change to build)

These are NOT syntactic patches — the mirrored upstream source is faithful and
the Ipê compiler rejected a shape Sky accepted. Four are now ROOT-CAUSE FIXED
(D1, D2); the rest stay FILED (§0) with the exact blocker.

### D1. `Css.zero` / `auto` / `none` / `systemFont` — bare zero-arg keyword constants — FIXED
Upstream v0.17.9 declares these as BARE values (`zero : Length`, `none : String`)
and writes `margin zero`; the Ipê stdlib had shipped them as unit-arg
(`zero : () -> Length`), so `margin zero` hit `IPE-T0001: expected
Ipe.Css.Length, found () -> Ipe.Css.Length`.
**Root cause + fix:** the stdlib SHAPE was wrong, not the compiler. `src/stdlib/Ipe/Css.ipe`
`zero`/`auto`/`none`/`systemFont` are now bare values matching v0.17 Sky (`monoFont`
stays `() -> String`, matching upstream). The four first-party examples that used the
old `(zero ())` call form (`examples/{09,10,12}-*`, `examples/16-skychess`) were
migrated to the bare form. AGENTS.md Active Limitation #9 dropped.
**SEAL:** mirrored 09/10/12 → `ipe build` exit 0 → emitted `cargo build` exit 0.

### D2. Value binding shares a name with a record-alias auto-ctor — FIXED
06-json declares `type alias Profile = { … }` AND an explicit
`Profile name age active = { … }` record-constructor helper. Ipê synthesised the
record-alias auto-ctor unconditionally, seeded its name into `seen_values`, then
rejected the user's explicit binding as `IPE-N0010: value defined more than once`.
**Root cause + fix:** the explicit user binding IS the constructor — it must
SUPPRESS synthesis, exactly as the upstream Rust emitter's `existingNames` guard
(`Sky.Generate.Rust.Builder.ModuleEmitter`, `synCtor … if Set.member ctorName
existingNames then []`). `synthesize_record_alias_ctors`
(`src/compiler/canon/src/resolve.rs`) now declines synthesis when a top-level
value of the same name exists. Test updated:
`explicit_binding_suppresses_record_alias_ctor_synthesis`.
**SEAL:** mirrored 06-json → `ipe build` exit 0 → `cargo build` exit 0 → runs,
emits correct JSON incl. the decoded `Profile` record.

### D3. Remaining v0.17.9 parity gaps

**18-job-queue — type layer FIXED, lowering FILED.**
`viewJob job = … job.running …` is an UNCALLED, un-annotated helper (dead code).
Ipê deferred the `job.running` field access and, because no call site ever pinned
`job` to a concrete record, reported `IPE-T0012: type a has no field running`.
**Type-layer root cause + fix:** the reference constrains a field access as an
open row on the spot (`Sky.Type.Constrain.Expression` `Access` →
`{ field : ρ | ext }`); Ipê's deferred pass never grew the record. The deferred
resolver (`src/compiler/types/src/lib.rs` `resolve_deferred`) now settles a stuck
`Flex` base to a singleton open record and GROWS the open row for sibling accesses
(`job.result`/`job.id`/`job.name`) — matching Sky's row-polymorphic inference (106
`ipe_types` tests + 17 record goldens stay green). This advances 18 from the WRONG
`IPE-T0012` to the honest **`IPE-L0102` (polymorphism)**: the lowerer emits every
top-level def and cannot monomorphise a fully-polymorphic (open-record) value that
no call site instantiates.
**Remaining blocker (FILED):** Ipê has no whole-program DCE, so it lowers the
unreachable `viewJob`; Sky tree-shakes it (`SKY_DCE`, `Dce.Ref`,
`Mono.ReachableSet`). Closing 18 needs reachability-based dead-def elimination
before lowering — a backend feature, out of this cycle's boundary. (An
un-annotated open-record helper that IS reachable would additionally need generic
row-poly Rust emission; DCE removes the dead case Sky relies on here.)

**26-ui-showcase — FILED (sanctioned divergence).**
`init : {  } -> ( Model, Cmd Msg )` annotates the per-session request as an empty
record. Sky types `Live.app`'s `init` field with a FREE polymorphic `req` var
(`Sky.Type.Constrain.Expression`, `("Live","app")` → `init : req -> …`), so `{}`
unifies trivially. Ipê DELIBERATELY types it as an opaque `LiveReq` `Con`
(`src/compiler/types/src/lib.rs` `LiveReqFields` — "no bare record literal can
masquerade as the runtime struct"), so `{}` fails `IPE-T0001: expected LiveReq,
found {}`. This is a sanctioned security divergence (Parse-don't-validate), not a
bug. Closing 26 without reverting the hardening needs a NARROW bridge: an EMPTY
record annotation (no spoofable fields) may unify with the opaque request `Con`.
Deferred — the bridge lives in the hot `unify` path and warrants its own soundness
review + tests.

**31-webview-stopwatch-ui — FILED.**
`view : Model -> Html` annotates the view with a BARE `Html` (0 type args). Sky's
`Ui.layout` returns the wildcard `any` (`sky-stdlib/Std/Ui.sky:1697` —
`layout : … -> any`), which unifies with any `Html` arity, so the bare annotation
holds. Ipê's `Ui.layout` scheme returns the fully-parameterised `Html msg`
(`src/compiler/types/src/constrain.rs` `K::UiLayout → html_t(var(0))`), and the
bare-`Html` annotation stays 0-arg, so the body's `Html Main.Msg` clashes:
`IPE-T0001: expected Html, found Html Main.Msg`. The principled fix is arity-fill:
an under-applied parametric type in an annotation fills its missing trailing args
with fresh vars up to the type's declared arity (strictly better than Sky's `any`
return). Deferred — canon has no builtin-type arity table at the `TType` arm, so
the fill needs that table threaded in first.

**16-skychess / 17-skymon — FILED (project-wide type resolution).**
`Chess/Move.ipe` writes `Model` (and 17 writes `Html` in a `(Html Msg)`
annotation) in signatures WITHOUT importing the module that defines it (`State.ipe`
for `Model`). Sky resolves an unqualified unknown type to the empty-home sentinel
(`Sky.Canonicalise.Type` `resolveTypeName` → `Map.findWithDefault (Canonical "")`),
which then unifies by name via the empty-home bridge. Ipê deliberately FAILS CLOSED
(`src/compiler/canon/src/resolve.rs` `resolve_unqualified_type_home` → `IPE-N0002`)
to avoid the former empty-home-Con ICE (`IPE-I0001`). Matching Sky SOUNDLY (not its
lax empty-home fallback) requires resolving the unqualified name against a
PROJECT-WIDE type index (name → real home) so it unifies with the genuine
`State.Model`. That index must be threaded through the salsa module-resolution
firewall (`src/compiler/db`), which is invasive and risks the incremental
early-cut invariant — out of this cycle's boundary.

### 00-standard-libs — Money `Result` type-mismatch (BUILD fixed via patch)
`Money.format (Money.add a b)` and `Money.format (Money.sumOf USD parts)` passed
the result of a checked-arithmetic op straight into `Money.format`. Upstream
Sky's `Money.add` / `Money.sumOf` return a bare `Money`; Ipê's return
`Result Error Money` (a fail-closed divergence — a currency mismatch is a typed
error, not a silent wrong sum), so the example hit `IPE-T0001: expected
Ipe.Money.Money, found Result Error Ipe.Money.Money`. This is a sanctioned
security divergence, not a bug: the checked result MUST be unwrapped before
formatting. Fixed by `ipe-patches/00-standard-libs.patch`, which threads both
sums through `Result.withDefault (Money.zero USD)` (the `Result` module is
already imported; both sums are same-currency, hence always `Ok`). Verified:
`ipe build` exit 0 and the emitted crate `cargo build` exit 0 (THE SEAL).

## Filed

### 00-standard-libs — 4 RUN-time test failures (unmasked by the BUILD fix)
With the type-mismatch above resolved, the example compiles and its `Test`
suite now RUNS for the first time: `127 passed, 4 failed (131 total)`. The four
failures are runtime behaviour mismatches, NOT build/type errors, and were
previously masked because the example never compiled. The stdlib `Ipe.Test`
runner prints only the pass/fail COUNT (`src/stdlib/Ipe/Test.ipe` collects
failing test names but the summary line does not emit them), so the specific
four are not yet identified. Closing this needs either a `Test`-runner change to
print each failing test's name + expected/actual, or a bisect of the suite to
name the four — then a root-cause per failure. Recorded honestly: the row is
BUILD-green, RUN-red pending that diagnosis, never papered over.

### New-model sweep tally (scripts/examples-sweep.sh, BUILD + RUN)

Of the **28 in-scope sky-mirror examples** (the 41 upstream minus 13 Go-FFI
excluded by the manifest's `go_ffi = true` classification): **25 build + run
green**; **3 red**, each a filed compiler/stdlib gap above — 00-standard-libs
(Money `Result` divergence), 26-ui-showcase (LiveReq hardening), 31-webview-
stopwatch-ui (parametric-annotation arity-fill). 39-hub-demo is a multi-app
composite (billing-app + frontend-app) with no top-level `src/Main.ipe`, so the
per-dir sweep reports `no-source`; building a composite's sub-apps as separate
units is a sweep-structure follow-up, not a compiler gap.

The 16/17/18 project-wide-type / DCE gaps (D3 above) remain real, but those
examples are Go-FFI (`[go.dependencies]` in their `sky.toml`) and so are out of
the Rust build set by the manifest — their reds no longer appear in the sweep.

**Cargo-warning gate — FILED (pre-existing, repo-wide).** Every emitted crate
warns `unexpected cfg condition value: wasm-client` (6 per non-wasm example): the
runtime references `cfg(feature = "wasm-client")` but the emitted non-wasm
`Cargo.toml` never declares that feature. The sweep counts these via
`IPE_SWEEP_WARN_GATE` (default on), so the verdict is FAIL until they are fixed
even when every row is build-ok. This is a codegen/runtime defect independent of
the mirror model; the CI examples-sweep job is `continue-on-error` so it does not
block the workflow. Set `IPE_SWEEP_WARN_GATE=0` to score BUILD+RUN only while the
warning is open.

## CI-preparation backlog

### #310 — FFI dep `version` reaches the emitted Cargo.toml unvalidated (RESOLVED)
Status: RESOLVED. The FFI inspector's `PkgInfo` decode validates the crate
`name` (`PackageName::parse`, `[A-Za-z0-9_-]+`), `pkg_path`, function names, and
Rust types at the wire boundary — an injection-shaped value fails the whole
package (`WireDefect::InvalidPkgPath` / illegal-crate-name). The `version` field
was the one gap: `src/compiler/ffi/src/pkginfo.rs` stored it as a raw `String`
with no parse, and `render_dep_line` interpolated it into `<name> = "=<version>"`
(and the `features` branch) of the generated `Cargo.toml`, so a `version`
carrying a `"`-and-newline payload could break out of the TOML string and inject
a rogue `[dependencies.evil]` table.

Structural fix (mirrors the `PackageName`/`PkgPath` treatment, not a render-site
escape): a `CrateVersion` validated newtype in `pkginfo.rs` with a private
`parse` smart constructor gating the semver charset `[0-9A-Za-z.*=<>~^,+ -]`
(rejecting every TOML metacharacter — quote, brace, bracket, backslash, control),
returning `WireDefect::InvalidVersion`. Both `PkgInfo.version` and
`TransitiveDep.version` are now `CrateVersion`, parsed at the
`TryFrom<WirePkgInfo>` decode boundary — an injection-bearing version fails the
WHOLE package there. `render_dep_line` now takes `&CrateVersion`, whose only
constructor is the decode-boundary parse, so a raw unchecked string is
unrepresentable at emission (the type, not a runtime escape, closes the class).
The charset mirrors the driver's existing `VersionPin` (the CLI-supplied pin),
so the CLI and the resolved-dependency paths share one semver-value gate; an
empty version stays legal at decode (the unresolved-probe case) and is refused
loudly downstream by `cargo_dep_lines`. Regression tests:
`an_injection_bearing_version_fails_the_whole_package` +
`an_injection_bearing_transitive_version_fails_the_whole_package` +
`legal_versions_decode` (pkginfo) and
`an_injection_bearing_version_never_reaches_a_manifest_line` (driver). Proven:
`cargo build --workspace` green, `cargo nextest run -p ipe_ffi` 157/157,
`cargo clippy -p ipe_ffi --all-targets` clean.

### #313 — 00-standard-libs Money.add Result divergence (RESOLVED)
Previously BUILD-green/RUN-red (the `Ipe.Money.add` `Result` return divergence
left 4 runtime test failures). Now GREEN in the CI examples sweep
(`00-standard-libs  ok  ok`) — the `ipe-patches/00-standard-libs.patch` covers
the divergence. No open work.

### #314 — `unexpected cfg condition value: wasm-client` warning (RESOLVED)
The emitted non-wasm `Cargo.toml` now DECLARES the `wasm-client` feature in its
`[features]` table (`wasm-client = []`), so the runtime's
`cfg(all(target_arch = "wasm32", feature = "wasm-client"))` reference no longer
raises `unexpected cfg`. The CI examples sweep reports `cargo warnings (past
#![allow]): 0 total`, and a local emitted-crate build of 01-hello-world shows 0
warnings. No open work.

### FFI dep `features`/transitive `name` reach the emitted Cargo.toml ungated (RESOLVED)
Status: RESOLVED. A sibling of the dep-`version` injection fix. The version
splice was already closed by `CrateVersion`, but two other values on the SAME
`render_dep_line` splice were ungated: `PkgInfo.features` was stored raw
(`features: w.features`), and a transitive `TransitiveDep.name` was stored raw
(`name: dep.name`) — only its `ident` went through `RustIdent`. `render_dep_line`
(`src/compiler/ffi/src/driver.rs`) splices each feature into a `features = [ … ]`
array and the name as the `[dependencies]` key of the emitted `Cargo.toml`, so a
feature or name carrying a quote, bracket, brace and a newline could break out of
the array / inline table and inject a rogue `[dependencies.evil]` table (with a
`path`/`git` override and a `build.rs`) that runs at the user's next
`cargo build`. A `<slug>.pkg.json` is the sole source of record re-decoded on
every load, and a tampered/planted one is explicitly in scope.

Structural fix (parse-don't-validate at the decode boundary, mirroring
`CrateVersion`/`PackageName`): a `FeatureName` validated newtype in
`pkginfo.rs` gating the feature charset `[A-Za-z0-9_+./?:-]` (excludes every
TOML metacharacter), returning `WireDefect::InvalidFeature`; `PkgInfo.features`
is now `Vec<FeatureName>`, parsed at the `TryFrom<WirePkgInfo>` boundary.
`TransitiveDep.name` is now the existing `PackageName` (`[A-Za-z0-9_-]+`,
alphabetic-first), parsed at the same boundary. The inspector's own probe
scaffold (`_ipe_ffi_probe_…`, not a legal `PackageName`) is DROPPED at decode
rather than rejected — a synthetic non-registry package is never a typed
`TransitiveDep`, so the post-decode skip in `cargo_dep_lines` is gone.
`render_dep_line` now takes `&[FeatureName]`; the CLI `ipe add --features`
argument gate routes through the same `FeatureName::parse`, so the wire and CLI
surfaces share one feature gate (as version does via `CrateVersion`/`VersionPin`).
An injection-bearing feature or name now fails the WHOLE package at decode,
never reaching the emitter. Regression tests:
`an_injection_bearing_feature_fails_the_whole_package`,
`an_injection_bearing_transitive_name_fails_the_whole_package`,
`legal_features_decode`, `the_probe_scaffold_is_dropped_at_decode` (pkginfo) and
`an_injection_bearing_feature_never_reaches_a_manifest_line` +
`a_legal_feature_set_reaches_a_pinned_manifest_line` (driver). Proven:
`cargo build --workspace` green, `cargo test -p ipe_ffi` green, `cargo clippy
-p ipe_ffi --all-targets` clean.
