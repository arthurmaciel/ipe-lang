//! #194 / #197 / #202 regression — the ten stdlib modules whose runtime kernels
//! exist but were NOT registered in the compiler's `Ffi.kernel`-resolvable
//! registry, so `import`/member-use failed closed (SKY-N0028 unknown kernel,
//! SKY-N0026 reserved-type collision, or SKY-T0001 annotation mismatch).
//!
//! Each module below is a byte-identical reference Layer-3 port whose members
//! are point-free `Ffi.kernel "Mod_fn"` aliases. Registering each family's
//! kernels (variant + `decl` + type-scheme + arity + naming + pretty) closes the
//! resolution hole; the runtime fns already exist and are re-exported into the
//! emitted crate unconditionally, so skyc-0 implies cargo-0 (THE SEAL).
//!
//! Two invariants, one per module:
//!
//! * **Resolution + emit** (always): `import <Module>` and a member call resolve
//!   and the frontend emits clean Rust (skyc exit 0). A resolution regression
//!   (SKY-N0028 / N0026 / T0001) fails HERE — never a silent skip.
//! * **Seal** (`SKY_E2E`): the emitted crate `cargo build`s AND runs cleanly.
//!   skyc exit 0 AND cargo exit 0 — no exit-0-then-cargo-fail.
//!
//! ```text
//! cargo test -p skyc --test golden_stdlib_module_seal
//! SKY_E2E=1 cargo test -p skyc --test golden_stdlib_module_seal
//! ```

use std::fs;
use std::path::{Path, PathBuf};

mod support;

/// A runtime `false` the optimiser cannot fold.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

fn write_project(dir: &Path, main: &str) -> bool {
    let src = dir.join("src");
    let _ = fs::remove_dir_all(dir);
    if fs::create_dir_all(&src).is_err() {
        return false;
    }
    fs::write(src.join("Main.sky"), main).is_ok()
}

fn e2e_enabled() -> bool {
    std::env::var("SKY_E2E").is_ok()
}

/// Compile `main` (a full `Main.sky` program) through the skyc frontend into an
/// emitted Rust project rooted at a per-`slug` temp dir. Asserts skyc exit 0 —
/// a resolution/seal regression fails loudly. Returns the emitted-project dir.
fn compile_module_probe(slug: &str, main: &str) -> Option<PathBuf> {
    // Unique dir PER CALL: the `_resolves_and_emits` and `_builds_and_runs` tests for
    // one module share a slug and run concurrently under nextest, so a shared temp dir
    // races (write vs remove_dir_all) and flakily fails write_project. A monotonic
    // counter makes every probe dir distinct. Declared at scope top (before any
    // statement) to satisfy clippy::items_after_statements.
    static PROBE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let Ok(runtime) = skyc::resolve_runtime() else {
        return None; // runtime unavailable in this environment — caller skips
    };
    let uid = PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("skyc_stdlib_seal_{slug}_{uid}"));
    assert!(
        write_project(&tmp, main),
        "must write the {slug} fixture project"
    );
    let entry = tmp.join("src").join("Main.sky");
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stdlib_seal_{slug}_{uid}_out"));
    let _ = fs::remove_dir_all(&out);

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    if let Err(e) = built {
        assert!(
            false_marker(),
            "module `{slug}` must resolve + emit through skyc (exit 0), got: {e:?}"
        );
        return None;
    }
    Some(out)
}

/// Full end-to-end seal for one module: skyc emits, then the emitted crate
/// `cargo build`s + runs, and stdout matches `expected`.
fn seal_module(slug: &str, main: &str, expected: &str) {
    if !e2e_enabled() {
        return;
    }
    let Some(dir) = compile_module_probe(slug, main) else {
        return;
    };
    let out = support::build_and_run_emitted(slug, &dir);
    assert_eq!(
        out.exit_code,
        Some(0),
        "emitted `{slug}` crate must build + run cleanly, got exit {:?}",
        out.exit_code
    );
    assert_eq!(
        out.stdout.trim(),
        expected,
        "module `{slug}` stdout mismatch"
    );
}

// ── #194: Sky.Core.Regex ─────────────────────────────────────────────────────

const REGEX_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Std.Log exposing (println)\n\
    import Sky.Core.Regex as Regex\n\n\
    hit : String\n\
    hit = if Regex.match \"\\\\d+\" \"a1\" then \"MATCH\" else \"NOMATCH\"\n\n\
    miss : String\n\
    miss = if Regex.match \"(\" \"x\" then \"MATCH\" else \"NOMATCH\"\n\n\
    sub : String\n\
    sub = Regex.replace \"\\\\d\" \"#\" \"a1b2\"\n\n\
    firstDigits : String\n\
    firstDigits = case Regex.find \"\\\\d+\" \"abc42\" of\n\
    \x20   Just d -> d\n\
    \x20   Nothing -> \"-\"\n\n\
    allDigits : String\n\
    allDigits = String.join \",\" (Regex.findAll \"\\\\d\" \"a1b2c3\")\n\n\
    parts : String\n\
    parts = String.join \"|\" (Regex.split \",\" \"a,b,c\")\n\n\
    main = println (hit ++ \" \" ++ miss ++ \" \" ++ sub ++ \" \" ++ firstDigits ++ \" \" ++ allDigits ++ \" \" ++ parts)\n";

#[test]
fn regex_resolves_and_emits() {
    let _ = compile_module_probe("regex", REGEX_MAIN);
}

#[test]
fn regex_builds_and_runs() {
    // hit=MATCH, invalid pattern is total → NOMATCH, sub=a#b#, first=42,
    // all=1,2,3, parts=a|b|c.
    seal_module("regex", REGEX_MAIN, "MATCH NOMATCH a#b# 42 1,2,3 a|b|c");
}

// ── #202: Sky.Core.Path ──────────────────────────────────────────────────────

const PATH_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Std.Log exposing (println)\n\
    import Sky.Core.Path as Path\n\n\
    abs : String\n\
    abs = if Path.isAbsolute \"/a/b\" then \"ABS\" else \"REL\"\n\n\
    main = println (Path.base \"/a/b/c.txt\" ++ \" \" ++ Path.dir \"/a/b/c.txt\" ++ \" \" ++ Path.ext \"/a/b/c.txt\" ++ \" \" ++ abs)\n";

#[test]
fn path_resolves_and_emits() {
    let _ = compile_module_probe("path", PATH_MAIN);
}

#[test]
fn path_builds_and_runs() {
    seal_module("path", PATH_MAIN, "c.txt /a/b .txt ABS");
}

// ── #197: Sky.Core.Pure — SKY-T0001 fix (point-free Uuid kernel aliases) ─────
// The whole module failed to type-check because `uuidV4Kernel : Task Error
// String` mis-annotated the `Uuid_v4` kernel (real scheme `() -> Task Error
// String`). We only need it to RESOLVE + EMIT — a runtime UUID is nondeterministic
// so we do not assert a concrete E2E stdout; we assert the program builds+runs
// (exit 0) via a fixed printed marker.

const PURE_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Sky.Core.Task as Task\n\
    import Std.Log exposing (println)\n\
    import Sky.Core.Pure as Pure\n\n\
    genId : Task Error String\n\
    genId = Pure.uuidV4 ()\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = genId\n\
    \x20   in\n\
    \x20   println \"PURE_OK\"\n";

#[test]
fn pure_resolves_and_emits() {
    let _ = compile_module_probe("pure", PURE_MAIN);
}

#[test]
fn pure_builds_and_runs() {
    seal_module("pure", PURE_MAIN, "PURE_OK");
}

// ── #197: Std.Trace ──────────────────────────────────────────────────────────

const TRACE_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Sky.Core.Task as Task\n\
    import Std.Log exposing (println)\n\
    import Std.Trace as Trace\n\n\
    work : Task Error String\n\
    work = Trace.span \"unit\" (Task.succeed \"TRACE_OK\")\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = Trace.event \"start\"\n\
    \x20       _ = Trace.attr \"k\" \"v\"\n\
    \x20       _ = work\n\
    \x20   in\n\
    \x20   println \"TRACE_OK\"\n";

#[test]
fn trace_resolves_and_emits() {
    let _ = compile_module_probe("trace", TRACE_MAIN);
}

#[test]
fn trace_builds_and_runs() {
    seal_module("trace", TRACE_MAIN, "TRACE_OK");
}

// ── #197: Std.Compression ────────────────────────────────────────────────────

const COMPRESSION_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Sky.Core.Task as Task\n\
    import Sky.Core.Bytes as Bytes\n\
    import Std.Log exposing (println)\n\
    import Std.Compression as Compression\n\n\
    roundTrip : Task Error Bytes\n\
    roundTrip =\n\
    \x20   Compression.gzip (Bytes.fromString \"hello\") |> Task.andThen Compression.gunzip\n\n\
    main =\n\
    \x20   Task.map (\\b -> \"GZ:\" ++ Maybe.withDefault \"?\" (Bytes.toString b)) roundTrip\n\
    \x20       |> Task.andThen (\\msg -> println msg)\n";

#[test]
fn compression_resolves_and_emits() {
    let _ = compile_module_probe("compression", COMPRESSION_MAIN);
}

#[test]
fn compression_builds_and_runs() {
    seal_module("compression", COMPRESSION_MAIN, "GZ:hello");
}

// ── #197: Std.Csv ────────────────────────────────────────────────────────────

const CSV_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Std.Log exposing (println)\n\
    import Std.Csv as Csv\n\n\
    headerLine : String\n\
    headerLine =\n\
    \x20   case Csv.parse \"a,b\\n1,2\" of\n\
    \x20       Ok doc -> String.join \"|\" doc.header\n\
    \x20       Err _ -> \"ERR\"\n\n\
    main = println headerLine\n";

#[test]
fn csv_resolves_and_emits() {
    let _ = compile_module_probe("csv", CSV_MAIN);
}

#[test]
fn csv_builds_and_runs() {
    seal_module("csv", CSV_MAIN, "a|b");
}

// ── #210: Std.Cache ──────────────────────────────────────────────────────────
// Exercises the full surface example 36-composite-server uses: `defaultCfg` +
// `withMaxEntries`/`withTTL` builders → `new` (a `CacheCfg` record literal
// consumed by `Cache_newRaw`), `put`, then `get` (a `Cache String String`
// value pattern-matched through the `Cache k v` ADT). Proves the three emit
// fixes together: the phantom `k`/`v` enum params (E0392), the `CacheCfg`
// record → runtime-struct fold (E0308), and the `PartialEq` generic bound the
// runtime `cache_put`/`cache_get` require.

const CACHE_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Sky.Core.Task as Task\n\
    import Std.Log exposing (println)\n\
    import Std.Cache as Cache\n\n\
    program : Task Error String\n\
    program =\n\
    \x20   let\n\
    \x20       cfg = Cache.defaultCfg |> Cache.withMaxEntries 64 |> Cache.withTTL 30000\n\
    \x20   in\n\
    \x20   Cache.new cfg\n\
    \x20       |> Task.andThen\n\
    \x20           (\\cache ->\n\
    \x20               Cache.put cache \"k\" \"hit\"\n\
    \x20                   |> Task.andThen (\\_ -> Cache.get cache \"k\")\n\
    \x20                   |> Task.map (\\found -> Maybe.withDefault \"miss\" found)\n\
    \x20           )\n\n\
    main =\n\
    \x20   program |> Task.andThen (\\v -> println (\"CACHE:\" ++ v))\n";

#[test]
fn cache_resolves_and_emits() {
    let _ = compile_module_probe("cache", CACHE_MAIN);
}

#[test]
fn cache_builds_and_runs() {
    // put "k"="hit" then get "k" → Just "hit"; withDefault → "hit".
    seal_module("cache", CACHE_MAIN, "CACHE:hit");
}

// ── #215: Std.PubSub ─────────────────────────────────────────────────────────
// PubSub.publish : String -> any -> Task Error Int.  No Live.app runs in this
// probe so publish resolves to Err(Unavailable) — Task.onError swallows it and
// the program prints the marker.  The test asserts skyc-0 ⇒ cargo-0 ⇒ exit-0.

const PUBSUB_MAIN: &str = "module Main exposing (main)\n\
    import Sky.Core.Prelude exposing (..)\n\
    import Sky.Core.Task as Task\n\
    import Sky.Core.Json.Encode as JsonEnc\n\
    import Std.PubSub as PubSub\n\
    import Std.Log exposing (println)\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = PubSub.publish \"t\" (JsonEnc.string \"hi\")\n\
    \x20               |> Task.onError (\\_ -> Task.succeed 0)\n\
    \x20   in\n\
    \x20   println \"PUBSUB_OK\"\n";

#[test]
fn pubsub_resolves_and_emits() {
    let _ = compile_module_probe("pubsub", PUBSUB_MAIN);
}

#[test]
fn pubsub_builds_and_runs() {
    seal_module("pubsub", PUBSUB_MAIN, "PUBSUB_OK");
}
