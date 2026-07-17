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
the Ipê compiler rejects a shape Sky accepted. Per §0 they are FILED gaps, not
patched. The affected examples show `ipe-fail` in the mirror sweep (honestly),
not a doctored green.

### D1. `Css.zero` (and the other zero-arg `Css.*` keyword constants)
Upstream Sky writes `margin zero`; Ipê requires `Css.zero ()` (CLAUDE.md Active
Limitation #9). The mirrored 09-live-counter / 26-ui-showcase /
31-webview-stopwatch-ui hit `IPE-T0001: expected Ipe.Css.Length, found () ->
Ipe.Css.Length`. NOTE the pinned Go oracle (`sky` v0.16.29) ALSO rejects this
against the v0.17.9 sources (`Foreign 'Std.Css.zero': () -> Length vs Length`),
so it is a v0.16↔v0.17 stdlib-shape skew, not an Ipê-only gap — the fix is to
teach the emitter/stdlib to accept the bare zero-arg form (matching v0.17 Sky),
tracked as a completeness gap.

### D2. Value binding shares a name with a type alias
06-json defines `type alias Profile` AND a function `Profile name age active`
(a record-constructor helper). Sky allows the shadow; Ipê rejects it
(`IPE-N0010: value defined more than once`). A completeness/parity gap to close
in name resolution.

### D3. Other v0.17.9 parity gaps surfaced by the mirror
The remaining in-scope reds (below) are distinct compiler/stdlib parity gaps,
each honestly surfaced as `ipe-fail` — none patched:

| Example | Diagnostic | Gap |
|---|---|---|
| 16-skychess, 17-skymon | `IPE-N0002: cannot find this type in scope` | a type the upstream module defines that Ipê name resolution can't find (v0.17 module/type shape). |
| 18-job-queue | `IPE-T0012: this record has no such field` | record-field surface skew vs the v0.17 stdlib record. |
| 26-ui-showcase | `IPE-T0001: expected LiveReq, found {}` | the `init req` LiveReq shape — the empty-record init the example uses no longer unifies with `LiveReq`. |
| 31-webview-stopwatch-ui | `IPE-T0001: expected Html, found Html Main.Msg` | `Html` vs `Html msg` type-arity / alias divergence in the webview view path. |

### Full-mirror build tally (build-only, IPE_SWEEP_MIRROR_SKY=1)
39 upstream examples → **25 build green**; 14 red = **5 out-of-scope Go-FFI**
(03-tea-external, 05-mux-server, 08-notes-app, 11-fyne-stopwatch, 13-skyshop —
excluded by the normal sweep's `is_out_of_scope` filter) + **9 in-scope
divergences** (D1: 09/10/12; D2: 06; D3: 16/17/18/26/31). So of the 34 in-scope
examples, 25 build and 9 are the filed parity gaps above.

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
   build; 09/26/31 fail the SAME `Css.zero` skew as D1 — confirmed:
   `Foreign 'Std.Css.zero': () -> Length vs Length`). Full visual coverage needs
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
