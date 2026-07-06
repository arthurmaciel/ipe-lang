//! i148 regression lock — `HttpStream.open` must return `Task Error StreamId`,
//! not `Task Error Int`.
//!
//! Before this fix, `K::HttpStreamOpen`'s scheme was `fun(http_request(), task(int()))`.
//! User code that handled the result as `Result Error StreamId` triggered
//! `SKY-T0001: expected StreamId, found Int`.
//!
//! The fix changes the scheme to `fun(http_request(), task(stream_id()))` where
//! `stream_id()` resolves to the opaque `StreamId` builtin type, and introduces
//! the `SkyStreamId` Rust newtype in the runtime + routes it through `enum_name`
//! in the emitter.
//!
//! This test uses `skyc::emit_ir_text` (parse → canon → types → lower → IR text)
//! which stops before Rust emission — sufficient to verify the type scheme and
//! lowering without hitting unrelated Http-builder synthesizer limitations.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Parse + type-check + lower `tests/golden/i148_http_stream_id/Main.sky`.
/// Asserts the pipeline succeeds (no SKY-T0001 from mismatched `Int` vs
/// `StreamId`, and no `callee_arity` panic from a missing lower arm).
#[test]
fn http_stream_open_returns_stream_id_not_int() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i148_http_stream_id")
        .join("Main.sky");

    let result = skyc::emit_ir_text(&entry);
    assert!(
        result.is_ok(),
        "HttpStream.open typed as `Task Error StreamId` must pass type-check + lower \
         without SKY-T0001; got: {:?}",
        result.err()
    );
}
