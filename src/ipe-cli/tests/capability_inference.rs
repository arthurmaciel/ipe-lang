//! `infer_package_capabilities` surfaces the real compiler diagnostic when a
//! package cannot be lowered, rather than a generic "nothing lowered" that hides
//! the actual cause (regression guard for the opaque failure that masked several
//! example-sweep reds). It also infers over ONE shared source graph: the result
//! equals the per-entry union, and each module is analyzed once per package.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use ipe::PackageSourceSet;
use ipe_ir::Capability;

/// A package whose only module fails to lower yields the module's real
/// diagnostic (`CliError::Pipeline`), naming the offending file — never the
/// generic `CliError::Usage` "no module could be lowered".
#[test]
fn a_package_that_cannot_lower_surfaces_the_real_diagnostic() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join("ipe_capinfer_bad_entry");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src"))?;
    fs::write(
        dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"badpkg\", version = \"0.1.0\" }\n",
    )?;
    // `Main` references a name that does not exist, so lowering fails.
    fs::write(
        dir.join("src/Main.ipe"),
        "module Main\n\nmain : Task ()\nmain = thisNameDoesNotExist\n",
    )?;

    let result = ipe::infer_package_capabilities(&dir.join("package.ipe"));

    // The entry's real diagnostic (Pipeline, naming Main.ipe) must surface —
    // never the generic Usage "no module could be lowered".
    let surfaced_entry_diagnostic = matches!(
        &result,
        Err(ipe::CliError::Pipeline { file, .. }) if file.ends_with("Main.ipe")
    );
    assert!(
        surfaced_entry_diagnostic,
        "expected the entry's real Pipeline diagnostic, got: {result:?}"
    );

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared-graph inference: equivalence with the per-entry union + once-guard
// ---------------------------------------------------------------------------

const MANIFEST: &str = "module Package exposing (package)\n\n\npackage =\n    { name = \"sharedpkg\", version = \"0.1.0\" }\n";

/// `Main` imports a helper that reaches `Ipe.Html.Unsafe`; the unimported
/// sibling `Extra` reaches the network.
const UNSAFE_AND_SIBLING_NETWORK: &[(&str, &str)] = &[
    (
        "Main.ipe",
        "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Lib.Page exposing (page)\n\n\
         main : Task ()\nmain =\n\x20   Io.println page\n",
    ),
    (
        "Lib/Page.ipe",
        "module Lib.Page exposing (page)\n\nimport Ipe.Html exposing (render, section)\n\
         import Ipe.Html.Unsafe exposing (unsafeScript)\n\n\
         page : String\npage =\n\x20   render (section [] [ unsafeScript \"console.log(1)\" ])\n",
    ),
    (
        "Extra.ipe",
        "module Extra exposing (fetch)\n\nimport Ipe.Http as Http\n\
         import Ipe.Task as Task\nimport Ipe.Io as Io\nimport Ipe.Url as Url\n\n\
         fetch : Task ()\nfetch =\n\
         \x20   case Url.fromString \"http://example.com\" of\n\
         \x20       Ok url ->\n\
         \x20           Http.get url\n\
         \x20               |> Task.andThen (\\_ -> Io.println \"done\")\n\n\
         \x20       Err e ->\n\
         \x20           Task.fail e\n",
    ),
];

/// `Main` reaches the network and the clock; `Util` is pure; `Broken` does not
/// lower and must be skipped without masking its siblings.
const NETWORK_CLOCK_WITH_BROKEN_SIBLING: &[(&str, &str)] = &[
    (
        "Main.ipe",
        include_str!("fixtures/capabilities/uses_http_and_clock.ipe"),
    ),
    (
        "Util.ipe",
        "module Util exposing (shout)\n\nimport Ipe.String as String\n\n\
         shout : String -> String\nshout s =\n\x20   String.toUpper s\n",
    ),
    (
        "Broken.ipe",
        "module Broken exposing (oops)\n\noops : Int\noops =\n\x20   thisNameDoesNotExist\n",
    ),
];

/// Materialise a package (`package.ipe` + `src/<files>`) under a unique temp dir.
fn scratch_package(tag: &str, files: &[(&str, &str)]) -> Result<PathBuf, Box<dyn Error>> {
    let dir =
        std::env::temp_dir().join(format!("ipe_capinfer_shared_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src"))?;
    fs::write(dir.join("package.ipe"), MANIFEST)?;
    for (rel, src) in files {
        let path = dir.join("src").join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, src)?;
    }
    Ok(dir)
}

/// The per-entry reference: each entry lowered alone in its own cold database,
/// skipped when it does not lower, and the results unioned.
fn per_entry_union(package: &PackageSourceSet) -> BTreeSet<Capability> {
    let mut union = BTreeSet::new();
    for entry in package.entry_module_paths() {
        if let Ok(caps) = ipe::infer_package_capabilities_in(
            &ipe_db::IpeDatabase::new(),
            &package.restricted_to_entry(entry),
        ) {
            union.extend(caps);
        }
    }
    union
}

fn assert_shared_equals_per_entry(
    tag: &str,
    files: &[(&str, &str)],
    must_contain: &[Capability],
) -> Result<(), Box<dyn Error>> {
    let dir = scratch_package(tag, files)?;
    let manifest = dir.join("package.ipe");
    let package = PackageSourceSet::read(&manifest)?;

    let shared = ipe::infer_package_capabilities_in(&ipe_db::IpeDatabase::new(), &package)?;
    assert_eq!(
        shared,
        per_entry_union(&package),
        "shared-graph inference must equal the per-entry union"
    );
    // The public entry point agrees, run after run.
    assert_eq!(ipe::infer_package_capabilities(&manifest)?, shared);
    assert_eq!(ipe::infer_package_capabilities(&manifest)?, shared);
    for cap in must_contain {
        assert!(
            shared.contains(cap),
            "fixture `{tag}` must lower and disclose {cap:?}, got {shared:?}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn shared_graph_equals_per_entry_union_with_an_unimported_sibling() -> Result<(), Box<dyn Error>> {
    assert_shared_equals_per_entry(
        "unsafe_sibling",
        UNSAFE_AND_SIBLING_NETWORK,
        &[Capability::Unsafe, Capability::Network],
    )
}

#[test]
fn shared_graph_equals_per_entry_union_with_a_broken_sibling() -> Result<(), Box<dyn Error>> {
    assert_shared_equals_per_entry(
        "broken_sibling",
        NETWORK_CLOCK_WITH_BROKEN_SIBLING,
        &[Capability::Network],
    )
}

/// A poison-safe log of the debug key of every executed salsa query.
#[derive(Clone, Default)]
struct ExecutionLog(Arc<Mutex<Vec<String>>>);

impl ExecutionLog {
    fn push(&self, key: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(key);
    }

    fn keys(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn executions_of(&self, query: &str) -> usize {
        let needle = format!("{query}(");
        self.keys().iter().filter(|k| k.contains(&needle)).count()
    }
}

fn logged_db() -> (ipe_db::IpeDatabase, ExecutionLog) {
    let log = ExecutionLog::default();
    let sink = log.clone();
    let db = ipe_db::IpeDatabase::with_event_callback(Box::new(move |event: salsa::Event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            sink.push(format!("{database_key:?}"));
        }
    }));
    (db, log)
}

/// Each module (package and stdlib alike) is parsed and canonicalized at most
/// once for the whole package, each entry is lowered exactly once on the
/// caller's database, and no query instance ever executes twice.
fn assert_each_module_analyzed_once(
    tag: &str,
    files: &[(&str, &str)],
) -> Result<(), Box<dyn Error>> {
    let dir = scratch_package(tag, files)?;
    let package = PackageSourceSet::read(&dir.join("package.ipe"))?;
    let (db, log) = logged_db();
    ipe::infer_package_capabilities_in(&db, &package)?;

    let keys = log.keys();
    let distinct: BTreeSet<&String> = keys.iter().collect();
    assert_eq!(
        distinct.len(),
        keys.len(),
        "a query instance executed more than once: {keys:?}"
    );
    let modules = package.module_count();
    assert!(
        log.executions_of("parse") <= modules,
        "parse ran more often than there are modules ({modules})"
    );
    assert!(
        log.executions_of("canonicalize") <= modules,
        "canonicalize ran more often than there are modules ({modules})"
    );
    assert_eq!(
        log.executions_of("lower_program"),
        package.entry_module_paths().count(),
        "every entry is lowered exactly once, on the caller's database"
    );

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn each_module_is_analyzed_once_per_package() -> Result<(), Box<dyn Error>> {
    assert_each_module_analyzed_once("once_unsafe", UNSAFE_AND_SIBLING_NETWORK)?;
    assert_each_module_analyzed_once("once_broken", NETWORK_CLOCK_WITH_BROKEN_SIBLING)
}
