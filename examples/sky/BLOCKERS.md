# Blockers surfaced by the Sky-example mirror + CI-prep

Honest ledger (PRINCIPLES.md §0) of every defect the mirror + sweep + CI-prep
surfaced. Two kinds: **fixed here** (root-caused, in-boundary) and **filed**
(a tracked blocker the owning lane must close). None is papered over.

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
migrated to the bare form. CLAUDE.md Active Limitation #9 dropped.
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

### Full-mirror build tally (build-only, IPE_SWEEP_MIRROR_SKY=1)
Of the 34 in-scope examples: **D1 (09/10/12) + D2 (06) now build + SEAL** (was 25
green → **29 green**). Remaining reds: **5 out-of-scope Go-FFI** (03-tea-external,
05-mux-server, 08-notes-app, 11-fyne-stopwatch, 13-skyshop — `is_out_of_scope`) +
**5 filed D3 gaps** (16/17: project-wide types; 18: DCE; 26: LiveReq hardening;
31: parametric-annotation arity-fill).

## Equivalence design — Go reference builds from PRISTINE upstream, not the patch

The Go-oracle reference (Haskell `sky`) does NOT understand the `Ipe.*`
namespaces the patch introduces — it wants `Sky.Core.*` / `Std.*` and `.sky`
sources. So the equivalence path must build the Go binary from the ORIGINAL
upstream Sky source, while the Rust build uses the patched Ipê mirror.
`sky_mirror_one` now preserves the pristine tree under
`examples/sky/<name>/.sky-original/` for exactly this. (The sweep's existing
`build_go` builds `src/Main.ipe` from the same dir as the Rust build — correct
for the first-party `examples/NN-*` set, which has no separate Go source, but for
the sky-mirror set the Go build should target `.sky-original/`. Wiring that
switch is a follow-up; the pristine source is now captured so it is a small
change, not a re-architecture.)

## Host / toolchain constraints (visual Go-vs-Rust comparison)

Two independent constraints gate the visual pixel-diff:

1. **Go-oracle version skew.** The `sky` binary on PATH is **v0.16.29**; the
   mirrored examples are **v0.17.9**. The v0.16.29 oracle builds only the subset
   of examples that avoid v0.16↔v0.17 stdlib skew (01-hello-world, 15-http-server
   build; the mirrored 26/31 fail the OTHER D3 gaps above). Full visual coverage needs
   a v0.17.x `sky` oracle pinned under `tools/oracle/bin/sky` (the sweep's
   documented resolution slot).

2. **Server-lifecycle vs harness-timeout interaction (this host).** The
   `screenshot-compare.mjs` driver is implemented (boots both binaries
   sequentially on the announced port, screenshots each in headless Chromium via
   `--no-sandbox`, pixel-diffs with `pixelmatch`/`pngjs`, exits 3=SKIP when the Go
   reference is absent). Chromium is installed and launches. But the emitted
   server binaries here do not exit on `SIGTERM`, so a single foreground
   invocation that boots one hangs until an outer `timeout` SIGTERMs the whole
   group — which lands before node flushes, so the run could not be captured
   live in this shell. The sweep's own `lib/checks.sh` solves the identical
   problem with process-group SIGKILL + `reap`; the driver uses
   `child.kill('SIGKILL')` and is expected to work under that harness. Recorded
   honestly as a not-live-proven-on-this-host constraint rather than a doctored
   pass.

`screenshot-compare.mjs` degrades safely on BOTH: it captures the Rust
screenshot always and exits 3 (SKIP, "Go reference unavailable") when the Go
binary is empty/version-skewed — never a silent pass.
