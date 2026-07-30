//! The ten stdlib modules whose runtime kernels exist and are registered in the
//! compiler's `Ffi.kernel`-resolvable registry. Without registration,
//! `import`/member-use fails closed (IPE-N0028 unknown kernel, IPE-N0026
//! reserved-type collision, or IPE-T0001 annotation mismatch).
//!
//! Each module below is a byte-identical reference Layer-3 port whose members
//! are point-free `Ffi.kernel "Mod_fn"` aliases. Registering each family's
//! kernels (variant + `decl` + type-scheme + arity + naming + pretty) closes the
//! resolution hole; the runtime fns already exist and are re-exported into the
//! emitted crate unconditionally, so ipe-0 implies cargo-0 (THE SEAL).
//!
//! Two invariants, one per module:
//!
//! * **Resolution + emit** (always): `import <Module>` and a member call resolve
//!   and the frontend emits clean Rust (ipe exit 0). A resolution regression
//!   (IPE-N0028 / N0026 / T0001) fails HERE — never a silent skip.
//! * **Seal** (`IPE_E2E`): the emitted crate `cargo build`s AND runs cleanly.
//!   ipe exit 0 AND cargo exit 0 — no exit-0-then-cargo-fail.
//!
//! ```text
//! cargo test -p ipe --test golden_stdlib_module_seal
//! IPE_E2E=1 cargo test -p ipe --test golden_stdlib_module_seal
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
    fs::write(src.join("Main.ipe"), main).is_ok()
}

fn e2e_enabled() -> bool {
    std::env::var("IPE_E2E").is_ok()
}

/// Compile `main` (a full `Main.ipe` program) through the ipe frontend into an
/// emitted Rust project rooted at a per-`slug` temp dir. Asserts ipe exit 0 —
/// a resolution/seal regression fails loudly. Returns the emitted-project dir.
fn compile_module_probe(slug: &str, main: &str) -> Option<PathBuf> {
    // Unique dir PER CALL: the `_resolves_and_emits` and `_builds_and_runs` tests for
    // one module share a slug and run concurrently under nextest, so a shared temp dir
    // races (write vs remove_dir_all) and flakily fails write_project. A monotonic
    // counter makes every probe dir distinct. Declared at scope top (before any
    // statement) to satisfy clippy::items_after_statements.
    static PROBE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return None; // runtime unavailable in this environment — caller skips
    };
    let uid = PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("ipec_stdlib_seal_{slug}_{uid}"));
    assert!(
        write_project(&tmp, main),
        "must write the {slug} fixture project"
    );
    let entry = tmp.join("src").join("Main.ipe");
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("stdlib_seal_{slug}_{uid}_out"));
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    if let Err(e) = built {
        assert!(
            false_marker(),
            "module `{slug}` must resolve + emit through ipec (exit 0), got: {e:?}"
        );
        return None;
    }
    Some(out)
}

/// Full end-to-end seal for one module: ipe emits, then the emitted crate
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

// ── Ipe.Regex ─────────────────────────────────────────────────────

const REGEX_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Io as Io\n\
    import Ipe.Regex as Regex\n\n\
import Ipe.String
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
    main = Io.println (hit ++ \" \" ++ miss ++ \" \" ++ sub ++ \" \" ++ firstDigits ++ \" \" ++ allDigits ++ \" \" ++ parts)\n";

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

// ── Ipe.Path ──────────────────────────────────────────────────────

const PATH_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Io as Io\n\
    import Ipe.Path as Path\n\n\
    render : Path.Path -> Path.Path -> String\n\
    render dirP fileP =\n\
    \x20   let\n\
    \x20       abs = if Path.isAbsolute dirP then \"ABS\" else \"REL\"\n\
    \x20   in\n\
    \x20   Path.base fileP ++ \" \" ++ Path.dir fileP ++ \" \" ++ Path.ext fileP ++ \" \" ++ abs\n\n\
    main =\n\
    \x20   case (Path.fromString \"/a/b\", Path.fromString \"/a/b/c.txt\") of\n\
    \x20       (Ok dirP, Ok fileP) -> Io.println (render dirP fileP)\n\
    \x20       _ -> Io.println \"PATH_ERR\"\n";

#[test]
fn path_resolves_and_emits() {
    let _ = compile_module_probe("path", PATH_MAIN);
}

#[test]
fn path_builds_and_runs() {
    seal_module("path", PATH_MAIN, "c.txt /a/b .txt ABS");
}

// ── Ipe.Process — subprocess execution (no shell) ──────────────────
// `Process.run` runs `printf %s SEALED` with a DIRECT argv (never `sh -c`),
// captures its stdout, and prints it. A resolution/scheme/emit regression
// fails at `_resolves_and_emits`; the seal runs the child and asserts stdout.

const PROCESS_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Io as Io\n\
    import Ipe.Process as Process\n\n\
    run : Task Error String\n\
    run = Process.run \"printf\" [ \"%s\", \"SEALED\" ]\n\n\
    main =\n\
    \x20   Task.andThen (\\out -> Io.println out) run\n";

#[test]
fn process_resolves_and_emits() {
    let _ = compile_module_probe("process", PROCESS_MAIN);
}

#[test]
fn process_builds_and_runs() {
    seal_module("process", PROCESS_MAIN, "SEALED");
}

// ── Ipe.Pure — point-free Uuid kernel aliases ─────
// The whole module failed to type-check because `uuidV4Kernel : Task Error
// String` mis-annotated the `Uuid_v4` kernel (real scheme `() -> Task Error
// String`). We only need it to RESOLVE + EMIT — a runtime UUID is nondeterministic
// so we do not assert a concrete E2E stdout; we assert the program builds+runs
// (exit 0) via a fixed printed marker.

const PURE_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Io as Io\n\
    import Ipe.Pure as Pure\n\n\
    genId : Task Error String\n\
    genId = Pure.uuidV4 ()\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = genId\n\
    \x20   in\n\
    \x20   Io.println \"PURE_OK\"\n";

#[test]
fn pure_resolves_and_emits() {
    let _ = compile_module_probe("pure", PURE_MAIN);
}

#[test]
fn pure_builds_and_runs() {
    seal_module("pure", PURE_MAIN, "PURE_OK");
}

// ── Ipe.Trace ──────────────────────────────────────────────────────────

const TRACE_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Io as Io\n\
    import Ipe.Trace as Trace\n\n\
    work : Task Error String\n\
    work = Trace.span \"unit\" (Task.succeed \"TRACE_OK\")\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = Trace.event \"start\"\n\
    \x20       _ = Trace.attr \"k\" \"v\"\n\
    \x20       _ = work\n\
    \x20   in\n\
    \x20   Io.println \"TRACE_OK\"\n";

#[test]
fn trace_resolves_and_emits() {
    let _ = compile_module_probe("trace", TRACE_MAIN);
}

#[test]
fn trace_builds_and_runs() {
    seal_module("trace", TRACE_MAIN, "TRACE_OK");
}

// ── Ipe.Compression ────────────────────────────────────────────────────

const COMPRESSION_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Bytes as Bytes\n\
    import Ipe.Io as Io\n\
    import Ipe.Compression as Compression\n\n\
import Ipe.Maybe
    roundTrip : Task Error Bytes\n\
    roundTrip =\n\
    \x20   Compression.gzip (Bytes.fromString \"hello\") |> Task.andThen Compression.gunzip\n\n\
    main =\n\
    \x20   Task.map (\\b -> \"GZ:\" ++ Maybe.withDefault \"?\" (Bytes.toString b)) roundTrip\n\
    \x20       |> Task.andThen (\\msg -> Io.println msg)\n";

#[test]
fn compression_resolves_and_emits() {
    let _ = compile_module_probe("compression", COMPRESSION_MAIN);
}

#[test]
fn compression_builds_and_runs() {
    seal_module("compression", COMPRESSION_MAIN, "GZ:hello");
}

// ── Ipe.Csv ────────────────────────────────────────────────────────────

const CSV_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Io as Io\n\
    import Ipe.Csv as Csv\n\n\
import Ipe.String
    headerLine : String\n\
    headerLine =\n\
    \x20   case Csv.parse \"a,b\\n1,2\" of\n\
    \x20       Ok doc -> String.join \"|\" doc.header\n\
    \x20       Err _ -> \"ERR\"\n\n\
    main = Io.println headerLine\n";

#[test]
fn csv_resolves_and_emits() {
    let _ = compile_module_probe("csv", CSV_MAIN);
}

#[test]
fn csv_builds_and_runs() {
    seal_module("csv", CSV_MAIN, "a|b");
}

// ── Ipe.Cache ──────────────────────────────────────────────────────────
// Exercises the full surface example 36-composite-server uses: `defaultCfg` +
// `withMaxEntries`/`withTTL` builders → `new` (a `CacheCfg` record literal
// consumed by `Cache_newRaw`), `put`, then `get` (a `Cache String String`
// value pattern-matched through the `Cache k v` ADT). Proves the three emit
// fixes together: the phantom `k`/`v` enum params (E0392), the `CacheCfg`
// record → runtime-struct fold (E0308), and the `PartialEq` generic bound the
// runtime `cache_put`/`cache_get` require.

const CACHE_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Io as Io\n\
    import Ipe.Cache as Cache\n\n\
import Ipe.Maybe
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
    \x20   program |> Task.andThen (\\v -> Io.println (\"CACHE:\" ++ v))\n";

#[test]
fn cache_resolves_and_emits() {
    let _ = compile_module_probe("cache", CACHE_MAIN);
}

#[test]
fn cache_builds_and_runs() {
    // put "k"="hit" then get "k" → Just "hit"; withDefault → "hit".
    seal_module("cache", CACHE_MAIN, "CACHE:hit");
}

// ── Ipe.PubSub ─────────────────────────────────────────────────────────
// PubSub.publish : String -> any -> Task Error Int.  No Web.app runs in this
// probe so publish resolves to Err(Unavailable) — Task.onError swallows it and
// the program prints the marker.  The test asserts ipe-0 ⇒ cargo-0 ⇒ exit-0.

const PUBSUB_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Task as Task\n\
    import Ipe.Json.Encode as JsonEnc\n\
    import Ipe.PubSub as PubSub\n\
    import Ipe.Io as Io\n\n\
    main =\n\
    \x20   let\n\
    \x20       _ = PubSub.publish \"t\" (JsonEnc.string \"hi\")\n\
    \x20               |> Task.onError (\\_ -> Task.succeed 0)\n\
    \x20   in\n\
    \x20   Io.println \"PUBSUB_OK\"\n";

#[test]
fn pubsub_resolves_and_emits() {
    let _ = compile_module_probe("pubsub", PUBSUB_MAIN);
}

#[test]
fn pubsub_builds_and_runs() {
    seal_module("pubsub", PUBSUB_MAIN, "PUBSUB_OK");
}

// ── Ipe.Config ─────────────────────────────────────────────────────────
// Exercises the 16 `Config_*` kernels over the SHARED `Decoder` carrier: the
// four primitives (string/int/float/bool), `field`/`at`/`list`/`nullable`,
// `map`/`andThen`/`succeed`/`fail`, and all three format front-ends
// (`decodeToml`/`decodeYaml`/`decodeJson`). Proves two properties together:
// the `type Decoder a` re-declaration resolves (IPE-N0026 carrier exemption) with
// no competing enum emitted, and every kernel emits the shared JSON `decode_*` /
// `config_decode_*` runtime fns (ipe-0 ⇒ cargo-0). Uses SINGLE-decoder
// composition — the multi-parameter applicative `succeed (\a b -> …)` builder is
// the same documented distinct surface `Ipe.Json.Decode` has (divergences
// §A8), not a Config-specific gap.

const CONFIG_MAIN: &str = "module Main exposing (main)\n\
    import Ipe.Prelude exposing (..)\n\
    import Ipe.Config as Config\n\
    import Ipe.Io as Io\n\n\
import Ipe.List
import Ipe.Maybe
import Ipe.Result
import Ipe.String
    hostD : Config.Decoder String\n\
    hostD = Config.field \"host\" Config.string\n\n\
    portD : Config.Decoder Int\n\
    portD = Config.at [\"db\", \"port\"] Config.int\n\n\
    tagsD : Config.Decoder (List String)\n\
    tagsD = Config.field \"tags\" (Config.list Config.string)\n\n\
    noteD : Config.Decoder (Maybe String)\n\
    noteD = Config.nullable (Config.field \"note\" Config.string)\n\n\
    ratioD : Config.Decoder Float\n\
    ratioD = Config.map (\\r -> r) (Config.field \"ratio\" Config.float)\n\n\
    checkedPortD : Config.Decoder Int\n\
    checkedPortD =\n\
    \x20   Config.andThen\n\
    \x20       (\\p -> if p > 0 then Config.succeed p else Config.fail \"bad\")\n\
    \x20       portD\n\n\
    main =\n\
    \x20   let\n\
    \x20       toml = \"host = \\\"h\\\"\\ntags = [\\\"a\\\"]\\nratio = 1.0\\n\\n[db]\\nport = 5\\n\"\n\
    \x20       yaml = \"note: hi\\n\"\n\
    \x20       json = \"{\\\"host\\\": \\\"j\\\"}\"\n\
    \x20       h = Result.withDefault \"?\" (Config.decodeToml toml hostD)\n\
    \x20       p = Result.withDefault 0 (Config.decodeToml toml checkedPortD)\n\
    \x20       t = String.fromInt (List.length (Result.withDefault [] (Config.decodeToml toml tagsD)))\n\
    \x20       n = Maybe.withDefault \"none\" (Result.withDefault Nothing (Config.decodeYaml yaml noteD))\n\
    \x20       j = Result.withDefault \"?\" (Config.decodeJson json hostD)\n\
    \x20   in\n\
    \x20   Io.println (\"CONFIG:\" ++ h ++ \":\" ++ String.fromInt p ++ \":\" ++ t ++ \":\" ++ n ++ \":\" ++ j)\n";

#[test]
fn config_resolves_and_emits() {
    let _ = compile_module_probe("config", CONFIG_MAIN);
}

#[test]
fn config_builds_and_runs() {
    seal_module("config", CONFIG_MAIN, "CONFIG:h:5:1:hi:j");
}
