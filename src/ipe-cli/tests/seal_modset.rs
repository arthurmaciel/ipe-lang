//! Ground-truth SEAL gate for the emitted runtime-module set.
//!
//! The backend trims `ipe_runtime/mod.rs` to a base set and appends
//! feature-modules per program shape (`uses_*` flags). When a shape emits code
//! that references a symbol whose vendored module was NOT appended, `ipe` exits
//! 0 but the emitted crate fails `cargo build` (E0425/E0412) — the module-set
//! SEAL breach class. A curated golden cannot catch it (goldens are hand-picked
//! and the byte-compare default never runs cargo).
//!
//! This test emits a minimal program for each reachable program shape and runs
//! an EXPLICIT `cargo build` of the emitted crate, asserting exit 0. It is the
//! authoritative gate: a passing byte-golden is not sufficient proof of the
//! SEAL.
//!
//! Gated on `IPE_E2E=1` (each shape does a full `cargo build`); without it the
//! test returns early so the default `cargo test` stays fast.
//!
//! The `*_vendored_*` tests additionally force the vendored emit model
//! (equivalent to `IPE_RUNTIME_VENDORED=1`), which was historically never run
//! in CI and concealed a class of SEAL breaks where the vendored manifest was
//! missing a Cargo feature flag required by `#[cfg(feature = "...")]` guards in
//! the vendored runtime source.
//!
//! ```text
//! IPE_E2E=1 cargo test -p ipe --test seal_modset
//! ```

use std::path::Path;

/// Shared error type for the emit-and-build helper.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Emit `ipe_source` for shape `name` and `cargo build` the emitted crate.
/// Returns `Ok(())` iff `ipe` exits 0 AND the emitted crate builds — THE SEAL.
fn emit_and_build(name: &str, ipe_source: &str) -> Result<(), BoxError> {
    let src_dir = std::env::temp_dir().join(format!("seal_modset_{name}_ipe"));
    let _ = std::fs::remove_dir_all(&src_dir);
    std::fs::create_dir_all(&src_dir)
        .map_err(|e| -> BoxError { format!("{name}: cannot create src dir: {e}").into() })?;

    let entry = src_dir.join("Main.ipe");
    std::fs::write(&entry, ipe_source)
        .map_err(|e| -> BoxError { format!("{name}: cannot write Main.ipe: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("seal_modset_{name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{name}: runtime unavailable: {e}").into() })?;

    // ipe accept — a codegen bug or a rejection surfaces here.
    ipe::build(&entry, &out_dir, &runtime)
        .map_err(|e| -> BoxError { format!("{name}: ipe build failed: {e}").into() })?;

    // THE real test: the emitted crate must `cargo build` (exit 0). A missing
    // runtime-module append reads as E0425/E0412 in this step.
    e2e_support::build_rust_binary(name, &out_dir)
        .map(|_| ())
        .map_err(|e| -> BoxError { e.into() })
}

/// Like `emit_and_build` but forces the VENDORED emit model (`runtime_dep:
/// false`), regardless of the `IPE_RUNTIME_VENDORED` environment variable.
///
/// The vendored model copies the runtime source into the emitted crate and
/// compiles it with the feature flags declared in the emitted `Cargo.toml`.
/// A missing feature flag silently compiles out `#[cfg(feature = "...")]`-gated
/// items, causing E0425/E0412 at `cargo build` despite `ipe` exit 0 — a SEAL
/// breach that the default dep-model tests cannot catch.
fn emit_and_build_vendored(name: &str, ipe_source: &str) -> Result<(), BoxError> {
    let src_dir = std::env::temp_dir().join(format!("seal_modset_{name}_ipe"));
    let _ = std::fs::remove_dir_all(&src_dir);
    std::fs::create_dir_all(&src_dir)
        .map_err(|e| -> BoxError { format!("{name}: cannot create src dir: {e}").into() })?;

    let entry = src_dir.join("Main.ipe");
    std::fs::write(&entry, ipe_source)
        .map_err(|e| -> BoxError { format!("{name}: cannot write Main.ipe: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("seal_modset_{name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{name}: runtime unavailable: {e}").into() })?;

    // Force the vendored emit model so `#[cfg(feature = "...")]` coverage in
    // the vendored runtime source is verified — the dep-model never exercises it.
    let options = ipe::BuildOptions {
        runtime_dep: false,
        ..ipe::BuildOptions::default()
    };
    ipe::build_with_options(&entry, &out_dir, &runtime, options)
        .map_err(|e| -> BoxError { format!("{name}: ipe build (vendored) failed: {e}").into() })?;

    e2e_support::build_rust_binary(name, &out_dir)
        .map(|_| ())
        .map_err(|e| -> BoxError { e.into() })
}

/// True unless `IPE_E2E` is set — the per-shape `cargo build`s are expensive.
fn skip() -> bool {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("seal_modset: set IPE_E2E=1 to run (each shape does a cargo build)");
        return true;
    }
    false
}

// ── Program shapes ──────────────────────────────────────────────────────────

/// Baseline: a bare `Io.println` program. Emits only the base module set.
const BARE: &str = "module Main exposing (main)\n\
    import Ipe.Io as Io\n\
    main = Io.println \"bare\"\n";

/// Minimal Web TEA app that fires `Cmd.publish` from `update`. `cmd_publish`
/// lives in `web::pubsub`; the Web shape must append the `web` runtime module
/// (and, transitively, `tea`). A missing append surfaces as E0425 `cmd_publish`
/// at `cargo build`.
const CMD_PUBLISH: &str = "module Main exposing (main)\n\
    import Ipe.Tea.Web as Web\n\
    import Ipe.Tea.Web.Cmd as Cmd\n\
    import Ipe.Tea.Web.Sub as Sub\n\
    import Ipe.PubSub as PubSub\n\
    import Ipe.Ui as Ui\n\
    type Msg = Publish | Ignored\n\
    type alias Model = {}\n\
    topic = PubSub.topic \"seal-cmd\"\n\
    init _req = ( {}, Cmd.none )\n\
    update msg model = case msg of\n\
    \x20   Publish -> ( model, Cmd.publish topic \"ping\" )\n\
    \x20   Ignored -> ( model, Cmd.none )\n\
    subscriptions _model = Sub.none\n\
    view _model = Ui.html (Ui.layout [] (Ui.text \"ok\"))\n\
    main = Web.app { init = init, update = update, view = view\n\
    \x20            , subscriptions = subscriptions, routes = [], notFound = Ignored }\n";

/// Minimal Web TEA app that registers `Sub.subscribeTopic` in `subscriptions`.
/// `sub_subscribe_topic` lives in `web::pubsub`; the Web shape must append the
/// `web` runtime module. A missing append surfaces as E0425 `sub_subscribe_topic`
/// at `cargo build`.
const SUB_SUBSCRIBE: &str = "module Main exposing (main)\n\
    import Ipe.Tea.Web as Web\n\
    import Ipe.Tea.Web.Cmd as Cmd\n\
    import Ipe.Tea.Web.Sub as Sub\n\
    import Ipe.PubSub as PubSub\n\
    import Ipe.Ui as Ui\n\
    type Msg = Got String | Ignored\n\
    type alias Model = {}\n\
    topic = PubSub.topic \"seal-sub\"\n\
    init _req = ( {}, Cmd.none )\n\
    update msg model = case msg of\n\
    \x20   Got _ -> ( model, Cmd.none )\n\
    \x20   Ignored -> ( model, Cmd.none )\n\
    subscriptions _model = Sub.subscribeTopic topic Got\n\
    view _model = Ui.html (Ui.layout [] (Ui.text \"ok\"))\n\
    main = Web.app { init = init, update = update, view = view\n\
    \x20            , subscriptions = subscriptions, routes = [], notFound = Ignored }\n";

/// `Html.renderStatic` from a CLI `main` (web WITHOUT any TEA/server kernel).
/// The `web` module's `use crate::tea::{IpeCmd, IpeSub}` is unconditional, so
/// `tea` must be declared even though no `Cmd`/`Sub` kernel is named. Was E0432
/// `crate::tea` before the fix.
///
/// `renderStatic` lives under the shape-neutral `Ipe.Html`, so this Program
/// imports NO `Ipe.Tea.*` shape and is not misclassified as a TEA app (ADR 0048).
const LIVE_RENDER_STATIC: &str = "module Main exposing (main)\n\
    import Ipe.Html as Html\n\
    type alias Model = { title : String }\n\
    viewStatic : Model -> Html msg\n\
    viewStatic model =\n    \
        Html.node \"div\" [] [ Html.text model.title ]\n\
    main =\n    \
        Html.renderStatic viewStatic { title = \"hi\" }\n";

/// `HttpStream.chunks` where the `StreamId` arrives as a parameter (no `open`
/// in the module set). `sub_subscribe_stream` + `IpeStreamId` live in
/// `http_stream`, declared by the server append. Was E0412 `IpeStreamId` +
/// E0425 `sub_subscribe_stream` before the fix.
const HTTP_STREAM_CHUNKS: &str = "module Main exposing (main)\n\
    import Ipe.Http.Stream as HttpStream exposing (StreamId, ChunkEvent(..))\n\
    import Ipe.Io as Io\n\
    type Msg = Chunked ChunkEvent\n\
    subFor : StreamId -> Sub Msg\n\
    subFor sid = HttpStream.chunks sid Chunked\n\
    main = Io.println \"streamchunks\"\n";

/// Authed-route program using `Server.getAuthed` and `Server.AuthConfig`.
///
/// Under the vendored emit model the runtime `auth.rs` and `server.rs` carry
/// `#[cfg(feature = "jwt")]` guards over `AuthConfig`, `server_get_authed`, and
/// the JWT sign/verify helpers. Without `"jwt"` in the emitted `Cargo.toml`'s
/// `default = [...]` those items are compiled out and `main.rs` references fail
/// with E0425 (`AuthConfig` not found) — the SEAL breach this test gates.
const AUTHED_ROUTE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/golden/authed_route_revocation_vendored_seal/Main.ipe"
));

/// Authed-route + Db-store program using `Store.allAs` behind `Server.getAuthed`.
///
/// Under the vendored emit model the emitted `ipe_runtime/mod.rs` is a trimmed
/// subset of the full runtime `mod.rs`. The runtime `db.rs` calls
/// `crate::ssrf::VettedDial::for_host` in its `build_pool` function
/// unconditionally (production code, not test-only), and `external_conn.rs` calls
/// `crate::dsn::{Dsn, DsnDriver}` and `crate::ssrf::VettedDial`. Without `ssrf`,
/// `dsn`, and `external_conn` appended to the vendored `mod.rs` whenever `uses_db`
/// is set, those references fail E0425/E0433 (ipe exit 0, cargo fails — SEAL
/// breach). This test gates the db-surface instance of the vendored-model SEAL
/// class.
const AUTHED_STORE_QUERY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/golden/authed_store_query_seal/Main.ipe"
));

// ── The gate: every shape emits AND cargo-builds ────────────────────────────

#[test]
fn bare_shape_builds() {
    if skip() {
        return;
    }
    emit_and_build("bare", BARE).expect("bare shape must emit and cargo-build");
}

#[test]
fn cmd_publish_no_live_builds() {
    if skip() {
        return;
    }
    emit_and_build("cmd_publish", CMD_PUBLISH)
        .expect("Cmd.publish must pull in the live module (was E0425 cmd_publish)");
}

#[test]
fn sub_subscribe_topic_no_live_builds() {
    if skip() {
        return;
    }
    emit_and_build("sub_subscribe", SUB_SUBSCRIBE)
        .expect("Sub.subscribeTopic must pull in the live module (was E0425 sub_subscribe_topic)");
}

#[test]
fn live_render_static_cli_builds() {
    if skip() {
        return;
    }
    emit_and_build("live_render_static", LIVE_RENDER_STATIC)
        .expect("Html.renderStatic must declare tea (was E0432 crate::tea)");
}

#[test]
fn http_stream_chunks_no_open_builds() {
    if skip() {
        return;
    }
    emit_and_build("http_stream_chunks", HTTP_STREAM_CHUNKS)
        .expect("HttpStream.chunks must pull in the server/http_stream module (was E0412/E0425)");
}

// ── Vendored-model SEAL: jwt feature must be in default = [...] ─────────────

/// Under the vendored emit model an authed-route program (`Server.getAuthed` +
/// `Server.AuthConfig`) must cargo-build. The runtime `auth.rs` and `server.rs`
/// gate `AuthConfig`, `server_get_authed`, and JWT helpers on
/// `#[cfg(feature = "jwt")]`; without `"jwt"` in the emitted `Cargo.toml`'s
/// `default` those items are compiled out and `main.rs` fails with E0425
/// (ipe exit 0, cargo fails — SEAL breach). This test catches that class under
/// the vendored emit path, which CI previously never ran.
#[test]
fn authed_route_vendored_builds() {
    if skip() {
        return;
    }
    emit_and_build_vendored("authed_route_vendored", AUTHED_ROUTE).expect(
        "authed route must cargo-build under the vendored emit model \
         (jwt feature must be in default = [...])",
    );
}

/// Under the vendored emit model an authed-route + Db-store program
/// (`Server.getAuthed` + `Store.allAs`) must cargo-build. The runtime `db.rs`
/// calls `crate::ssrf::VettedDial::for_host` in `build_pool` (production, not
/// test-only); `external_conn.rs` calls `crate::dsn::{Dsn, DsnDriver}` and
/// `crate::ssrf::VettedDial`. Without `ssrf`, `dsn`, and `external_conn`
/// appended to the vendored `mod.rs` under `uses_db`, the emitted crate fails
/// E0425/E0433 — ipe exit 0, cargo fails: the db-surface SEAL breach.
#[test]
fn authed_store_query_vendored_builds() {
    if skip() {
        return;
    }
    emit_and_build_vendored("authed_store_query_vendored", AUTHED_STORE_QUERY).expect(
        "authed store-query program must cargo-build under the vendored emit model \
         (ssrf + dsn + external_conn must be declared when uses_db)",
    );
}

/// The runtime source tree must resolve for every shape above — a smoke check
/// that fails loudly (rather than silently skipping) when the tree moved.
#[test]
fn runtime_tree_resolves() {
    let runtime = ipe::resolve_runtime().expect("runtime tree must resolve from the workspace");
    assert!(
        Path::new(&runtime).is_dir(),
        "resolved runtime path must be a directory: {}",
        runtime.display()
    );
}
