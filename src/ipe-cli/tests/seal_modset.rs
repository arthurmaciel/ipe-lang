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
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Io as Io\n\
    main = Io.println \"bare\"\n";

/// `Cmd.publish` with NO Web/server/TEA-app kernel. `cmd_publish` lives in
/// `live::pubsub`; the shape must pull in the `live` module (and, transitively,
/// `tea`). Was E0425 `cmd_publish` before the fix.
const CMD_PUBLISH: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Cmd as Cmd\n\
    import Ipe.Io as Io\n\
    pubCmd : Cmd msg\n\
    pubCmd = Cmd.publish \"topic\" \"hello\"\n\
    main = Io.println \"cmdpublish\"\n";

/// `Sub.subscribeTopic` with no Web kernel. `sub_subscribe_topic` lives in
/// `live::pubsub`. Was E0425 `sub_subscribe_topic` before the fix.
const SUB_SUBSCRIBE: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Sub as Sub\n\
    import Ipe.Io as Io\n\
    type Msg = Got String\n\
    subFor : Sub Msg\n\
    subFor = Sub.subscribeTopic \"topic\" Got\n\
    main = Io.println \"subtopic\"\n";

/// `Web.renderStatic` from a CLI `main` (web WITHOUT any TEA/server kernel).
/// The `web` module's `use crate::tea::{IpeCmd, IpeSub}` is unconditional, so
/// `tea` must be declared even though no `Cmd`/`Sub` kernel is named. Was E0432
/// `crate::tea` before the fix.
const LIVE_RENDER_STATIC: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Tea.Web as Web\n\
    import Ipe.Html as Html\n\
    import Ipe.Task as Task\n\
    type alias Model = { title : String }\n\
    viewStatic : Model -> Html msg\n\
    viewStatic model =\n    \
        Html.node \"div\" [] [ Html.text model.title ]\n\
    main =\n    \
        Web.renderStatic viewStatic { title = \"hi\" } |> Task.run\n";

/// `HttpStream.chunks` where the `StreamId` arrives as a parameter (no `open`
/// in the module set). `sub_subscribe_stream` + `IpeStreamId` live in
/// `http_stream`, declared by the server append. Was E0412 `IpeStreamId` +
/// E0425 `sub_subscribe_stream` before the fix.
const HTTP_STREAM_CHUNKS: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Http.Stream as HttpStream exposing (StreamId, ChunkEvent(..))\n\
    import Ipe.Io as Io\n\
    type Msg = Chunked ChunkEvent\n\
    subFor : StreamId -> Sub Msg\n\
    subFor sid = HttpStream.chunks sid Chunked\n\
    main = Io.println \"streamchunks\"\n";

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
        .expect("Web.renderStatic must declare tea (was E0432 crate::tea)");
}

#[test]
fn http_stream_chunks_no_open_builds() {
    if skip() {
        return;
    }
    emit_and_build("http_stream_chunks", HTTP_STREAM_CHUNKS)
        .expect("HttpStream.chunks must pull in the server/http_stream module (was E0412/E0425)");
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
