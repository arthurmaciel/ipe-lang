//! Emit the `ipe_asan` cfg when the crate is built under AddressSanitizer.
//!
//! Cargo exposes each `--cfg` value it passes to the compiler as a
//! `CARGO_CFG_<NAME>` environment variable to the build script, so a sanitizer
//! build (`-Zsanitizer=address`) sets `CARGO_CFG_SANITIZE=address` here. Reading
//! that env var is stable — unlike reading `cfg(sanitize)` from source, which is
//! a nightly-gated feature. The recursion guard keys its instrumentation-off
//! attribute (and the nightly features that attribute needs) on `ipe_asan`, so a
//! stable build of the emitted runtime never touches an unstable feature while an
//! ASAN build still gets a sound remaining-stack probe.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(ipe_asan)");
    if let Ok(sanitizers) = std::env::var("CARGO_CFG_SANITIZE")
        && sanitizers.split(',').any(|s| s == "address")
    {
        println!("cargo::rustc-cfg=ipe_asan");
    }
}
