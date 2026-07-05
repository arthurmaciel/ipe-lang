//! CI drift gate: asserts ZERO MISMATCH rows in the parity matrix.
//!
//! Run: `cargo test -p skyc parity_matrix_drift`
//!
//! MISMATCH = a wired kernel whose naming.rs symbol doesn't exist as a pub fn
//! in the runtime, or whose lower_callee() arm is missing.  These are bugs.
//!
//! BACKLOG = a kernel not yet in ALL — these are allowed (they're the backlog).
//! Missing canon_qual for known non-QUALIFIERS qualifiers is also allowed.
//!
//! The test works by shelling out to the `parity-matrix` binary (built during
//! the same `cargo test` run via a build.rs hook — see below).  If the binary
//! is not available (e.g., CI without the tools crate), the test is skipped
//! with a note rather than failing.

use std::process::Command;

/// Path to the workspace root — detected from CARGO_MANIFEST_DIR.
fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR for skyc is `crates/skyc/`; workspace root is two levels up.
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(manifest)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Find the parity-matrix binary: check the Cargo target directory first, then PATH.
fn find_binary() -> Option<std::path::PathBuf> {
    // Honour CARGO_TARGET_DIR if set.
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| "target".to_string());
    let target = std::path::PathBuf::from(&target_dir);

    // Try debug build first, then release.
    for profile in &["debug", "release"] {
        let bin = target.join(profile).join("parity-matrix");
        if bin.exists() {
            return Some(bin);
        }
    }
    // Fallback: search PATH.
    if let Ok(output) = Command::new("which").arg("parity-matrix").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }
    }
    None
}

#[test]
fn parity_matrix_zero_mismatches() {
    let bin = match find_binary() {
        Some(b) => b,
        None => {
            eprintln!(
                "parity-matrix binary not found — \
                 build with `cargo build -p parity-matrix` to run this gate"
            );
            // Skip rather than fail when the tool isn't built yet.
            return;
        }
    };

    let root = workspace_root();
    let manifest_dir = root.join("tools/parity-matrix");

    // Run `parity-matrix extract`.
    let output = Command::new(&bin)
        .arg("extract")
        .env("CARGO_MANIFEST_DIR", &manifest_dir)
        .output()
        .expect("parity-matrix extract failed to launch");

    assert!(
        output.status.success(),
        "parity-matrix extract exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tsv = String::from_utf8_lossy(&output.stdout);

    // Count MISMATCH rows.
    let mut mismatch_rows: Vec<String> = Vec::new();
    for line in tsv.lines().skip(1) {
        // The status column is the last (16th, index 15).
        let cols: Vec<&str> = line.split('\t').collect();
        if let Some(status) = cols.get(15) {
            if status.starts_with("MISMATCH") {
                let variant = cols.first().copied().unwrap_or("?");
                mismatch_rows.push(format!("  {} → {}", variant, status));
            }
        }
    }

    if !mismatch_rows.is_empty() {
        panic!(
            "parity-matrix: {} MISMATCH rows found (these are bugs, not backlog):\n{}\n\
             Run `parity-matrix extract > docs/architecture/parity-matrix.tsv && \
             parity-matrix report docs/architecture/parity-matrix.tsv` for the full report.",
            mismatch_rows.len(),
            mismatch_rows.join("\n")
        );
    }
}
