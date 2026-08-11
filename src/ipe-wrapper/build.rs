//! Build script for the deploy wrapper.
//!
//! When `IPE_EMBED_APP` and `IPE_EMBED_PROFILE` are set (by `ipe deploy
//! --embed`), copies those files into `OUT_DIR` as `embedded-app` and
//! `embedded-profile`, and emits `cargo::rustc-cfg=embed_mode` so `main.rs`
//! compiles the embed-mode path. When the vars are absent the standard
//! bundle-mode path (fixed-relative-path lookup) is compiled instead.
//!
//! No external tools, no network: the only I/O is a file copy in `OUT_DIR`.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Declare the custom cfg so rustc's check-cfg lint does not warn on
    // `#[cfg(embed_mode)]` in main.rs.
    println!("cargo::rustc-check-cfg=cfg(embed_mode)");

    let app_path = std::env::var_os("IPE_EMBED_APP");
    let profile_path = std::env::var_os("IPE_EMBED_PROFILE");

    match (app_path, profile_path) {
        (Some(app), Some(profile)) => {
            let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR not set by cargo")?);
            let app_dest = out.join("embedded-app");
            let profile_dest = out.join("embedded-profile");
            std::fs::copy(&app, &app_dest)?;
            std::fs::copy(&profile, &profile_dest)?;
            // Signal embed mode to the Rust compilation.
            println!("cargo::rustc-cfg=embed_mode");
            // Re-run if the source files change.
            println!("cargo::rerun-if-env-changed=IPE_EMBED_APP");
            println!("cargo::rerun-if-env-changed=IPE_EMBED_PROFILE");
            println!("cargo::rerun-if-changed={}", app.to_string_lossy());
            println!("cargo::rerun-if-changed={}", profile.to_string_lossy());
        }
        (None, None) => {
            // Bundle mode: no files to embed; re-run only when the env vars
            // are set (so a switch from bundle to embed forces a rebuild).
            println!("cargo::rerun-if-env-changed=IPE_EMBED_APP");
            println!("cargo::rerun-if-env-changed=IPE_EMBED_PROFILE");
        }
        _ => {
            // One set and one absent: a misconfigured embed invocation. Fail
            // at build time — never silently fall back to bundle mode.
            return Err(
                "IPE_EMBED_APP and IPE_EMBED_PROFILE must BOTH be set for embed mode, \
                 or NEITHER for bundle mode; got a partial set"
                    .into(),
            );
        }
    }

    Ok(())
}
