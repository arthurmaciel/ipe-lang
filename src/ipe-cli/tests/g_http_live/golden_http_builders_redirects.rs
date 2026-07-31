//! Go-parity `Http` builders: `withUrl` / `withFollowRedirects` /
//! `withMaxRedirects`.
//!
//! The Go reference's `Ipe.Http` exposes these three on top of the M5b
//! builder set; before this fix they did not exist anywhere in the Ipê kernel
//! registry or `Http.ipe`. Each is a pure single-field record update on
//! `HttpRequest`, emitted through `emit_http_builder_call`'s clone-and-reassign
//! block like its siblings (`withMethod` / `withTimeout` / `withBody`).
//!
//! Compile-tier assertion runs always; the run-tier requires `IPE_E2E=1`:
//!
//! ```text
//! IPE_E2E=1 cargo test -p ipe --test golden_m5b_http_builders_redirects
//! ```

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn redirect_builders_compile_and_run() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("http_builders_redirects")
        .join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_m5b_http_builders_redirects_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    // ipe-0: the three builders must resolve through the kernel path.
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "ipe build must succeed for http_builders_redirects: {:?}",
        built.err()
    );

    // Emission regression: each builder must emit its clone-and-reassign
    // block targeting the right field (not fall through to an undefined
    // `http_with_url(...)` call, which would be a SEAL breach at cargo).
    let emitted = std::fs::read_to_string(out.join("src").join("main.rs")).unwrap_or_default();
    for needle in [
        "__ipe_rec.url = ",
        "__ipe_rec.followRedirects = ",
        "__ipe_rec.maxRedirects = ",
    ] {
        assert!(
            emitted.contains(needle),
            "emitted Rust must contain `{needle}` (builder record update).\n\
             Relevant lines:\n{}",
            emitted
                .lines()
                .filter(|l| l.contains("__ipe_rec"))
                .take(10)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    // cargo-0 + run-0 with the exact expected chain result.
    let outcome = crate::support::build_and_run_emitted("http_builders_redirects", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "binary must exit 0; stdout:\n{}",
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "http://example.org\nnoredirect\n3",
        "builder chain must override url/followRedirects/maxRedirects"
    );
}
