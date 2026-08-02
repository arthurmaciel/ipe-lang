//! Single source of truth for the crate VERSIONS the Rust codegen emits into a
//! generated project's `Cargo.toml`. Edit a version HERE; `project.rs`'s
//! manifest-surgery functions read it, so a version can never drift between the
//! emitter, `runtime/Cargo.toml`, and `tests/golden/basics/Cargo.toml`.
//!
//! Feature lists + `optional` flags stay inline in `project.rs` (they depend on
//! usage). Only the version SPEC lives here. The `crate_specs_match_manifests`
//! test below is the drift tripwire.

/// One authoritative crate version. Feature lists + `optional` flags stay in the
/// emitter (`project.rs`); only the version SPEC lives here.
pub struct CrateSpec {
    pub name: &'static str,
    pub version: &'static str,
}

pub const TOKIO: CrateSpec = CrateSpec {
    name: "tokio",
    version: "1",
};
pub const SQLX: CrateSpec = CrateSpec {
    name: "sqlx",
    version: "0.8",
};
pub const BINCODE: CrateSpec = CrateSpec {
    name: "bincode",
    version: "1",
};
pub const AXUM: CrateSpec = CrateSpec {
    name: "axum",
    version: "0.7",
};
pub const TOWER_HTTP: CrateSpec = CrateSpec {
    name: "tower-http",
    version: "0.5",
};
pub const ASYNC_TRAIT: CrateSpec = CrateSpec {
    name: "async-trait",
    version: "0.1",
};
// `libc` is declared directly in the base `Cargo.toml` template (under
// `[target.'cfg(unix)'.dependencies]`, for the `Io.readSecret` termios path and
// the live console-proxy's `libc::prctl`), not by a surgery function — so this
// spec is consulted only by the drift guard, which walks `ALL` under `cfg(test)`.
#[cfg(test)]
pub const LIBC: CrateSpec = CrateSpec {
    name: "libc",
    version: "0.2",
};
pub const CROSSTERM: CrateSpec = CrateSpec {
    name: "crossterm",
    version: "0.28",
};
pub const UNICODE_WIDTH: CrateSpec = CrateSpec {
    name: "unicode-width",
    version: "0.1",
};
pub const WRY: CrateSpec = CrateSpec {
    name: "wry",
    version: "0.55",
};
pub const TAO: CrateSpec = CrateSpec {
    name: "tao",
    version: "0.35",
};
pub const TOKIO_TUNGSTENITE: CrateSpec = CrateSpec {
    name: "tokio-tungstenite",
    version: "0.24",
};
pub const LETTRE: CrateSpec = CrateSpec {
    name: "lettre",
    version: "0.11",
};
pub const REQWEST: CrateSpec = CrateSpec {
    name: "reqwest",
    version: "0.12",
};
pub const TOML: CrateSpec = CrateSpec {
    name: "toml",
    version: "0.8",
};
pub const SERDE_YAML: CrateSpec = CrateSpec {
    name: "serde_yaml",
    version: "0.9",
};
pub const FLATE2: CrateSpec = CrateSpec {
    name: "flate2",
    version: "1",
};
pub const ZSTD: CrateSpec = CrateSpec {
    name: "zstd",
    version: "0.13",
};
pub const CSV: CrateSpec = CrateSpec {
    name: "csv",
    version: "1",
};
pub const SHA1: CrateSpec = CrateSpec {
    name: "sha1",
    version: "0.10",
};
pub const MD5: CrateSpec = CrateSpec {
    name: "md-5",
    version: "0.10",
};
pub const AES_GCM: CrateSpec = CrateSpec {
    name: "aes-gcm",
    version: "0.10",
};
pub const CHACHA20POLY1305: CrateSpec = CrateSpec {
    name: "chacha20poly1305",
    version: "0.10",
};
pub const PBKDF2: CrateSpec = CrateSpec {
    name: "pbkdf2",
    version: "0.12",
};
pub const JSONWEBTOKEN: CrateSpec = CrateSpec {
    name: "jsonwebtoken",
    version: "9",
};
pub const URL: CrateSpec = CrateSpec {
    name: "url",
    version: "2",
};
pub const RSA: CrateSpec = CrateSpec {
    name: "rsa",
    version: "0.9",
};
pub const FUTURES_UTIL: CrateSpec = CrateSpec {
    name: "futures-util",
    version: "0.3",
};
pub const CHRONO_TZ: CrateSpec = CrateSpec {
    name: "chrono-tz",
    version: "0.10",
};

/// Every spec emitted by the surgery functions, for drift-test iteration.
///
/// Test-only: the shipping emitter reads the individual `const`s by name; this
/// aggregate exists solely so the drift tripwire can walk the full set.
#[cfg(test)]
pub const ALL: &[CrateSpec] = &[
    TOKIO,
    SQLX,
    BINCODE,
    AXUM,
    TOWER_HTTP,
    ASYNC_TRAIT,
    LIBC,
    CROSSTERM,
    UNICODE_WIDTH,
    WRY,
    TAO,
    TOKIO_TUNGSTENITE,
    LETTRE,
    REQWEST,
    TOML,
    SERDE_YAML,
    FLATE2,
    ZSTD,
    CSV,
    SHA1,
    MD5,
    AES_GCM,
    CHACHA20POLY1305,
    PBKDF2,
    JSONWEBTOKEN,
    URL,
    RSA,
    FUTURES_UTIL,
    CHRONO_TZ,
];

#[cfg(test)]
mod tests {
    use super::ALL;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The table is non-empty and every entry has a non-empty name + version.
    #[test]
    fn table_is_well_formed() {
        assert!(!ALL.is_empty(), "crate spec table must not be empty");
        for spec in ALL {
            assert!(!spec.name.is_empty(), "empty crate name in ALL");
            assert!(!spec.version.is_empty(), "empty version for {}", spec.name);
        }
        assert_eq!(ALL.len(), 29, "expected 29 surgery-emitted crate specs");
    }

    /// Extract the version from a Cargo dependency value: `"0.4"` or
    /// `{ version = "0.4", ... }`. Parse-once into a bare version string; every
    /// step is total (`.get` / `strip_prefix` / `find`), never an unchecked
    /// index.
    fn version_of(value: &str) -> Option<String> {
        let v = value.trim();
        if let Some(rest) = v.strip_prefix('{') {
            let idx = rest.find("version")?;
            let after = rest.get(idx + "version".len()..)?.trim_start();
            let after = after.strip_prefix('=')?.trim_start();
            let after = after.strip_prefix('"')?;
            let end = after.find('"')?;
            after.get(..end).map(str::to_owned)
        } else if let Some(rest) = v.strip_prefix('"') {
            let end = rest.find('"')?;
            rest.get(..end).map(str::to_owned)
        } else {
            None
        }
    }

    /// Parse `name = <value>` dependency lines into name → version. When
    /// `only_dep_sections` is true, only lines under a `[...dependencies]`
    /// header are considered (skips `[features]`, `[profile.*]`, …). A crate is
    /// captured with first-insert-wins, so the primary `[dependencies]` table
    /// takes precedence over a later `[dev-dependencies]` entry.
    fn parse_deps(text: &str, only_dep_sections: bool) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut in_deps = !only_dep_sections;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_deps = line.contains("dependencies");
                continue;
            }
            if !in_deps {
                continue;
            }
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim();
                if name.is_empty() || name.contains(' ') || name.contains('"') {
                    continue;
                }
                if let Some(ver) = version_of(value) {
                    out.entry(name.to_owned()).or_insert(ver);
                }
            }
        }
        out
    }

    /// The SSOT versions MUST match `runtime/Cargo.toml` for every crate (the
    /// generated project vendors that runtime and must compile against the
    /// versions it was tested with), and the golden base manifest for every
    /// SSOT crate it declares (tokio). Bump a version in ONE place and this
    /// fails until the others are updated — the invalid "manifest requests a
    /// version the runtime was not built against" state is unrepresentable
    /// without going red here.
    #[test]
    fn crate_specs_match_manifests() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let runtime_path = manifest.join("../../../runtime/rust/Cargo.toml");
        let golden_path = manifest.join("../../../../tests/golden/basics/Cargo.toml");

        let runtime_txt = std::fs::read_to_string(&runtime_path)
            .expect("crate_specs drift guard: cannot read runtime/Cargo.toml");
        let golden_txt = std::fs::read_to_string(&golden_path)
            .expect("crate_specs drift guard: cannot read tests/golden/basics/Cargo.toml");

        let runtime = parse_deps(&runtime_txt, true);
        let golden = parse_deps(&golden_txt, true);

        let mut problems = Vec::new();
        for spec in ALL {
            match runtime.get(spec.name) {
                None => problems.push(format!(
                    "{}: in SSOT ({}) but absent from runtime/Cargo.toml",
                    spec.name, spec.version
                )),
                Some(rt_ver) if rt_ver != spec.version => problems.push(format!(
                    "{}: SSOT = {}, runtime/Cargo.toml = {rt_ver}",
                    spec.name, spec.version
                )),
                Some(_) => {}
            }
            // Golden check only where the base manifest declares the crate.
            if let Some(g_ver) = golden.get(spec.name)
                && g_ver != spec.version
            {
                problems.push(format!(
                    "{}: SSOT = {}, tests/golden/basics/Cargo.toml = {g_ver}",
                    spec.name, spec.version
                ));
            }
        }
        assert!(
            problems.is_empty(),
            "crate-version drift between the SSOT and the manifests:\n  {}",
            problems.join("\n  ")
        );
    }
}
