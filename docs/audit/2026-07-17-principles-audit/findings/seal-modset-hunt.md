# SEAL hunt — curated runtime-module set vs. program shape (dom-class breaches)

Hunt for `ipe` exit-0 → emitted-crate `cargo build` failures caused by the
per-shape curation of `ipe_runtime/mod.rs` (base set + `uses_*`-gated appends
in `src/compiler/backend/rust/src/project.rs`), the class the `pub mod dom;`
base-set bug belongs to.

Method: extracted the base module set (`tests/golden/basics/ipe_runtime/mod.rs`)
and every append + its guard; built the full sibling-dependency graph of
`src/runtime/rust/src/**/*.rs` (`use crate::X` / `use super::X`); cross-checked
every kernel descriptor's `rust_name` against the runtime module that defines
it and the `is_*` family predicate that would pull that module in; then
confirmed the top candidates empirically (fresh `ipe` build, `ipe build` each
witness, explicit `cargo build` of the emitted crate).

Baseline: bare hello-world (`w0`) emits and cargo-builds green — the dom fix is
landed on master.

## CONFIRMED breaches (ipe exit 0, cargo build fails)

All three share one root cause: **the `is_tea` kernel family contains kernels
whose runtime symbols live in conditionally-declared modules (`live`,
`http_stream`), and conversely the `live` module unconditionally imports
`crate::tea` while the TEA append guard (`uses_tea || uses_server ||
uses_websocket`, project.rs:1154) does not include `uses_live`.** The family
predicates and the module-dependency closure have drifted apart.

### 1. `Cmd.publish` without any Live/server kernel → E0425

- Program shape: any program whose funcs mention `Cmd.publish` (or
  `Cmd.publishNoEcho`) but no Live/server/websocket kernel.
- Kernel: `CmdPublish` / `CmdPublishNoEcho` — in `is_tea` only
  (kernels/src/lib.rs:3929–3930), NOT in `is_live` (lib.rs:4717).
- Runtime symbol: `cmd_publish` / `cmd_publish_no_echo` defined ONLY in
  `src/runtime/rust/src/live/pubsub.rs`, surfaced by
  `RUNTIME_MOD_RS_LIVE_APPEND` — which only fires on `uses_live || uses_webview`.
- Witness `w2-cmdpublish` (`pubCmd = Cmd.publish "topic" "hello"`, main =
  println): `ipe build` exit 0; cargo:
  `error[E0425]: cannot find function `cmd_publish` in this scope` (src/main.rs).
- Status: **empirically confirmed**.

### 2. `Sub.subscribeTopic` without any Live/server kernel → E0425

- Program shape: same as #1 with `Sub.subscribeTopic` (e.g. a Tui app that
  subscribes to a pub/sub topic — cross-backend view sharing makes this a
  realistic shape).
- Kernel: `SubSubscribeTopic` — `is_tea` only (lib.rs:3931), not `is_live`.
- Runtime symbol: `sub_subscribe_topic`, ONLY in `live/pubsub.rs` /
  LIVE_APPEND.
- Witness `w3-subtopic`: `ipe build` exit 0; cargo:
  `error[E0425]: cannot find function `sub_subscribe_topic` in this scope`.
- Status: **empirically confirmed**.

### 3. `Live.renderStatic` (live-without-TEA) → E0432 `crate::tea`

- Program shape: `uses_live` set with NO TEA/server/websocket kernel. Concrete
  witness: static-site generation via `Live.renderStatic view model` from a
  CLI main (renderStatic's type `(model -> Html msg) -> model -> Task ()`
  requires no `Cmd`/`Sub` value anywhere).
- Guard gap: LIVE_APPEND fires (`uses_live`), `live` Cargo feature promoted,
  but TEA append guard omits `uses_live` — `pub mod tea;` absent while
  `live/mod.rs:253` and `live/pubsub.rs:17` do
  `use crate::tea::{IpeCmd, IpeSub};` unconditionally.
- Witness `w4-renderstatic`: `ipe build` exit 0; cargo:
  `error[E0432]: unresolved import `crate::tea`` at
  `src/ipe_runtime/live/pubsub.rs:17` and `src/ipe_runtime/live/mod.rs:253`
  (the trailing E0282/E0599 in live/mod.rs are cascades of the missing
  `IpeCmd`/`IpeSub` types, same root cause).
- Status: **empirically confirmed**.
- Same-class latent shapes (structural, no non-degenerate witness): `uses_tui`
  and `uses_webview` without `uses_tea` — `tui/app.rs:14` and `webview.rs:33`
  import `tea` unconditionally inside their promoted-feature gates. Every
  practical Tui/Webview app names `Cmd.none`/`Sub.none` somewhere, so
  `uses_tea` rides along **by accident, not by construction** (a diverging
  binding inhabiting `Cmd msg` type-checks without any TEA kernel). The
  structural fix for #3 should include these two flags in the same guard (or
  force `uses_tea` from live/tui/webview at the lowerer).

### 4. `HttpStream.chunks` without any server kernel → E0412 + E0425

- Program shape: a func using `HttpStream.chunks` where the `StreamId` arrives
  as an (HM-inferred) parameter rather than from `HttpStream.open` — e.g. a
  helper `subFor sid = HttpStream.chunks sid (\s -> s)` in a program that
  never calls `open` in its own module set.
- Kernel: `HttpStreamChunks` — `is_tea` only (lib.rs:3934); its siblings
  `HttpStreamOpen/ForEachChunk/Close` are `is_server` (lib.rs:3980–3982), but
  `chunks` was left out of `is_server`.
- Runtime symbol: `sub_subscribe_stream` (+ the `IpeStreamId` alias) live in
  `http_stream.rs`, declared only by `RUNTIME_MOD_RS_SERVER_APPEND`
  (`uses_server`).
- Witness `w5-streamchunks`: `ipe build` exit 0; cargo:
  `error[E0412]: cannot find type `IpeStreamId`` and
  `error[E0425]: cannot find function `sub_subscribe_stream`` (src/main.rs:243–244).
- Status: **empirically confirmed**. Masking note: any program that also calls
  `HttpStream.open` gets `uses_server` and is fine — which is why the sweep
  never sees this.

## Checked and NOT breaches (negative results, for the record)

- **Base-set closure (bare shape)**: `telemetry.rs`'s `crate::live::*` and
  `crate::telemetry_spill::*` refs are `#[cfg(feature = "live")]` /
  `#[cfg(feature = "db")]`-gated and the feature promotion co-fires with the
  corresponding mod.rs append — aligned. `ssrf`/`task`/`stringify`/`error`
  closures all inside base. w0 bare witness builds.
- **`uses_db`-only**: `telemetry_spill.rs`'s `live::hub` reference is inside
  `#[cfg(test)]` only; db append + sqlx/bincode manifest surgery closed.
- **live-without-db**: `live/mod.rs:1856` and `live/console_proxy.rs`
  telemetry_spill refs are `#[cfg(feature = "db")]`-gated — closed.
- **`uses_websocket`-only**: ws_client's `tea` need is force-appended
  (guard includes `uses_websocket`); `ssrf` in base; `websocket_client = []`
  IS declared in the base golden Cargo.toml (checked — the promote-only
  surgery is sound).
- **`PubSub.publish`-only CLI** (`w1-pubsub`): builds green — `PubSubPublish`
  is in BOTH `is_tea` and `is_live`, so both appends fire. This dual
  membership is exactly what `CmdPublish`/`SubSubscribeTopic` (finding 1–2)
  are missing.
- **server type-only programs**: `ir_type_mentions_server` (lower.rs:7283)
  covers Request/Response-typed funcs with no server kernel call.
- **css/ui/tui closure**: css append ordered before UI append; `uses_ui`
  forced by `uses_tui` in the lowerer; `dom`/`html` co-declared (the fixed
  dom bug); `ui` submodule deps (element/render/helpers) internal.
- **Manifest surgeries**: composition order db→server→live→tui→webview→
  websocket→email→ffi checked for anchor validity in every reachable
  combination; the tokio-feature "sync" idempotence checks hold; sqlx
  `postgres` promotion for live+db present; `http_header` base-membership
  E0428 note holds.
- **Cosmetic, not SEAL**: native builds emit `unexpected_cfgs` warnings for
  `feature = "wasm-client"` in vendored `html.rs`/dom (the feature is only
  declared in the wasm manifest) — warning-only under the emitted crate's
  lint set.

## Root-cause pattern + suggested structural fix (for the fixing lane)

The recurring illegal state: a kernel's emitted `rust_name` resolves to a
runtime module that the kernel's family flags do not pull in (and dually, an
appended module whose `use crate::X` closure the guards do not cover). The
curated `is_*` lists and the hand-maintained append guards are two parallel
encodings of the same fact and have now drifted three separate times
(dom, tea↔live, http_stream). A structural fix would derive the required
module set per kernel from a single symbol→module table (or make the
transitive-closure check a build-time assertion over the real runtime source,
e.g. a test that parses `use crate::X` from every vendored module and verifies
closure for every reachable flag combination).

## Witnesses (all under /tmp/sealhunt, emitted crates in `<w>/sky-out/rust`)

| witness | shape | ipe exit | cargo | error |
|---|---|---|---|---|
| w0-bare | println only | 0 | **0 (green)** | — (dom fix landed) |
| w1-pubsub | `PubSub.publish` CLI | 0 | 0 (green) | — (dual family membership) |
| w2-cmdpublish | `Cmd.publish`, no live | 0 | **FAIL** | E0425 `cmd_publish` |
| w3-subtopic | `Sub.subscribeTopic`, no live | 0 | **FAIL** | E0425 `sub_subscribe_topic` |
| w4-renderstatic | `Live.renderStatic` CLI | 0 | **FAIL** | E0432 `crate::tea` ×2 |
| w5-streamchunks | `HttpStream.chunks`, no open | 0 | **FAIL** | E0412 `IpeStreamId` + E0425 `sub_subscribe_stream` |

Build environment: ipe debug binary + emitted crates built with dedicated
`CARGO_TARGET_DIR`s (`~/.cache/ipe/sealhunt-target-wt`,
`~/.cache/ipe/sealhunt-emitted-target`), `IPE_RUNTIME_DIR=src/runtime/rust/src`.

## SEAL-MODSET-005 · PubSub.publish server-path — doc/impl gap + latent SSOT trap
- severity: medium (latent) / documentation (immediate)
- axis: completeness + soundness(SEAL, latent)
- surfaced-by: user 2026-07-17 ("can't a server shape use publish?")
- problem: CLAUDE.md documents `PubSub.publish`/`publishNoEcho` as Task-shaped, callable from raw
  `api` handlers/post-init/scheduled jobs (the server-side publish path). But (a) the `PubSub`
  qualifier is NOT registered in the canon QUALIFIERS table (`src/compiler/canon/src/env.rs:~900`),
  so `PubSub.publish` fails name-resolution — documented-but-unwired; and (b) `PubSubPublish`/
  `PubSubPublishNoEcho` return `None` from `KernelFn::required_runtime_module()` while their runtime
  symbol `pubsub_publish` lives in `ipe_runtime::live::pubsub` (the `live` module). So the instant the
  qualifier is wired, a server-shape program (`uses_server`, no tea/live) calling `PubSub.publish`
  emits `pubsub_publish` without vendoring `live` → E0425 — the same module-set SEAL class as
  SEAL-MODSET-001..004, this time reachable from a headless server (not covered by the seal_modset
  matrix, which only tests Cmd.publish-no-Live).
- also: `PubSubPublish` is `class = Tea` despite being Task-shaped/handler-callable — separate
  dispatch-soundness question under investigation (pubsub-class-investigation.md).
- fix direction: pre-emptively map `PubSubPublish|PubSubPublishNoEcho -> Live` in the SSOT (in
  progress); wire the `PubSub` qualifier + resolve the class question as a separate change; add a
  server+PubSub.publish row to the seal_modset matrix once nameable.
