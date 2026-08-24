//! Acceptance: a real `package.ipe` fixture carrying dependencies, rust
//! dependencies, and declared capabilities parses into the typed
//! `ProjectManifest`, exercised through the public `parse_manifest` API.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ipe::project::{IpeDep, parse_manifest};
use ipe_ir::Capability;

/// The bundled multi-section manifest fixture.
fn fixture_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2_manifest/package.ipe")
}

#[test]
fn full_manifest_round_trips_every_section() {
    let m = parse_manifest(&fixture_manifest()).expect("the fixture manifest must parse");

    assert_eq!(m.name, "sp2-manifest-fixture");

    // [dependencies]: an index dep, a git escape, and a path escape.
    let http = m.dependencies.get("http");
    assert!(
        matches!(http, Some(IpeDep::Index(req)) if req.matches(&semver::Version::new(1, 4, 0))),
        "http should be an Index dep admitting 1.4, got {http:?}"
    );
    assert_eq!(
        m.dependencies.get("mylib"),
        Some(&IpeDep::Git {
            url: "https://example.com/mylib.git".to_owned(),
            rev: Some("abc123".to_owned()),
        }),
        "mylib should be the git escape",
    );
    assert_eq!(
        m.dependencies.get("local"),
        Some(&IpeDep::Path(PathBuf::from("../local"))),
        "local should be the path escape",
    );

    // [rust.dependencies]: a bare version and an inline table with features.
    let uuid = m.rust_dependencies.get("uuid").expect("uuid present");
    assert_eq!(uuid.version, "1.10");
    let stripe = m.rust_dependencies.get("stripe").expect("stripe present");
    assert_eq!(stripe.version, "=1.0.0");
    assert_eq!(stripe.features, vec!["blocking", "webhooks"]);

    // [capabilities]: parsed into the same typed set the compiler infers.
    assert_eq!(
        m.capabilities,
        BTreeSet::from([Capability::Network, Capability::Clock])
    );
}
