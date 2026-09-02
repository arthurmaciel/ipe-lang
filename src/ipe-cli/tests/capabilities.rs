//! `ipe capabilities` — the read-only capability report and the
//! declared-set verification primitive.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

use ipe::verify_capabilities;
use ipe_ir::{Capability, WebCapability};

mod support;

type TestResult = Result<(), Box<dyn Error>>;

/// A minimal Web-shape TEA app whose view mounts one `Ui.widget` over a
/// `customElement` handle — the smallest program that ships browser JS, so its
/// inferred capability set must contain `custom-element`.
const WIDGET_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.String as String

type alias WidgetState = { count : Int }

type WidgetUp = Bumped Int

type Msg = FromWidget WidgetUp

type alias Model = { count : Int }

counter : CustomElement WidgetState WidgetUp
counter = customElement "js/counter.js"

init : a -> ( Model, Cmd.Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update msg model =
    case msg of
        FromWidget (Bumped n) ->
            ( { count = model.count + n }, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.column []
        [ Ui.widget counter { count = model.count } FromWidget
        , Ui.text (String.fromInt model.count)
        ]

subscriptions : Model -> Sub.Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = FromWidget (Bumped 0)
        }
"#;

/// The author widget-hook JS. Its exact bytes are what the served page SRI pins;
/// present so the build path's widget-file gate is satisfied when a test builds.
const COUNTER_JS: &str =
    "export function mount(host, emit) {\n  return { onState(state) {} };\n}\n";

/// Materialise a widget project (`package.ipe` + `src/Main.ipe` + `src/js/…`)
/// under a unique temp dir, returning the dir. The caller removes it.
fn widget_project(tag: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "ipe-ce-cap-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/js"))?;
    std::fs::write(
        dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"widgetpkg\", version = \"0.1.0\" }\n",
    )?;
    std::fs::write(dir.join("src/Main.ipe"), WIDGET_APP)?;
    std::fs::write(dir.join("src/js/counter.js"), COUNTER_JS)?;
    Ok(dir)
}

/// Absolute path to a fixture under this crate's `tests/fixtures/capabilities`.
fn fixture(name: &str) -> PathBuf {
    support::manifest_dir()
        .join("tests/fixtures/capabilities")
        .join(name)
}

/// Run the built `ipe` binary and return its captured `(status_success, stdout)`.
fn run_ipe(args: &[&str]) -> Result<(bool, String), Box<dyn Error>> {
    let out = Command::new(support::ipe_bin()).args(args).output()?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

#[test]
fn reports_network_for_an_http_program() -> TestResult {
    let (ok, stdout) = run_ipe(&[
        "capabilities",
        "--plain",
        &fixture("uses_http.ipe").to_string_lossy(),
    ])?;
    assert!(ok, "capabilities must exit 0");
    assert_eq!(stdout.trim(), "network");
    Ok(())
}

#[test]
fn reports_none_for_a_pure_program() -> TestResult {
    let (ok, stdout) = run_ipe(&[
        "capabilities",
        "--plain",
        &fixture("pure_string.ipe").to_string_lossy(),
    ])?;
    assert!(ok, "capabilities must exit 0");
    // Under `--plain`, a pure program emits zero records (empty) — the "no
    // capabilities" wording lives only in the human default and `--json`'s `[]`.
    assert!(
        stdout.trim().is_empty(),
        "a pure program has no --plain capability lines, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn reports_unsafe_for_an_html_unsafe_script_program() -> TestResult {
    // Importing `Ipe.Html.Unsafe` (the inline-`<script>` / raw-HTML escape-hatch
    // home) discloses the `unsafe` capability — the import itself is the signal,
    // so a raw HTML/script sink cannot hide from `ipe capabilities`.
    let (ok, stdout) = run_ipe(&[
        "capabilities",
        "--plain",
        &fixture("uses_html_unsafe_script.ipe").to_string_lossy(),
    ])?;
    assert!(ok, "capabilities must exit 0");
    assert_eq!(stdout.trim(), "unsafe");
    Ok(())
}

#[test]
fn capabilities_help_page_lists_the_command() -> TestResult {
    let (ok, stdout) = run_ipe(&["capabilities", "--help"])?;
    assert!(ok, "--help exits 0");
    assert!(
        stdout.contains("capabilities"),
        "help page names the command, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn verify_accepts_the_exact_declared_set() {
    let declared = BTreeSet::from([Capability::Network]);
    let r = verify_capabilities(&fixture("uses_http.ipe"), &declared);
    assert!(r.is_ok(), "an exact declaration verifies: {r:?}");
}

#[test]
fn verify_rejects_underdeclared() {
    // The program uses `network` but declares nothing.
    let declared = BTreeSet::new();
    let r = verify_capabilities(&fixture("uses_http.ipe"), &declared);
    assert!(r.is_err(), "an empty declaration must be rejected");
}

#[test]
fn verify_rejects_overdeclared() {
    // The pure program uses nothing but declares `filesystem`.
    let declared = BTreeSet::from([Capability::Filesystem]);
    let r = verify_capabilities(&fixture("pure_string.ipe"), &declared);
    assert!(r.is_err(), "an over-declaration must be rejected");
}

/// Acceptance test: a program using both `Http.get` (network) and `Time.now`
/// (clock) must report exactly `{network, clock}`. Any drift is a mis-classified
/// tag, caught against a real program rather than a minimal fixture.
#[test]
fn acceptance_http_and_clock_example_infers_network_and_clock() -> TestResult {
    let example = fixture("uses_http_and_clock.ipe");
    let (ok, stdout) = run_ipe(&["capabilities", "--plain", &example.to_string_lossy()])?;
    assert!(ok, "capabilities must exit 0 on the example");
    let reported: BTreeSet<&str> = stdout.split_whitespace().collect();
    assert_eq!(
        reported,
        BTreeSet::from(["network", "clock"]),
        "unexpected capability set for uses_http_and_clock, got:\n{stdout}"
    );

    // The library verifier agrees with the reported set exactly.
    let declared = BTreeSet::from([Capability::Network, Capability::Clock]);
    let r = verify_capabilities(&example, &declared);
    assert!(r.is_ok(), "the exact inferred set must verify: {r:?}");
    Ok(())
}

// ── the `custom-element` disclosure axis ────────────────────────────────────

/// A program that mounts a `Ui.widget` ships browser JS, so its inferred
/// capability set must contain `custom-element`. Proven through the same
/// `verify_capabilities` inference `ipe capabilities` reports, over a real
/// Web-shape widget app.
#[test]
fn a_widget_program_discloses_custom_element() -> TestResult {
    let dir = widget_project("infer")?;
    let entry = dir.join("src/Main.ipe");
    // Declaring exactly `{custom-element}` must verify: it is the whole inferred
    // set of a widget app that reaches no other effect.
    let declared = BTreeSet::from([Capability::CustomElement]);
    let r = verify_capabilities(&entry, &declared);
    assert!(
        r.is_ok(),
        "a widget program's inferred set is exactly {{custom-element}}: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Fail-closed: a program that ships a widget but declares NOTHING on the
/// `custom-element` axis is rejected — a widget-bearing module can never hide the
/// disclosure. This is the load-bearing invariant.
#[test]
fn a_widget_program_that_hides_custom_element_is_rejected() -> TestResult {
    let dir = widget_project("hide")?;
    let entry = dir.join("src/Main.ipe");
    // Declare the empty set even though the program ships a widget.
    let declared = BTreeSet::new();
    let r = verify_capabilities(&entry, &declared);
    assert!(
        matches!(
            &r,
            Err(ipe::CliError::CapabilityMismatch { missing, .. })
                if missing.contains(&"custom-element")
        ),
        "a widget program that omits `custom-element` must be rejected as under-declared, got: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A Web-shape app that CONSTRUCTS a `customElement` handle at top level but never
/// mounts it in `view`. The emitter still serves the author JS (the handle is a
/// served asset the moment it is constructed), so disclosure must follow serving:
/// the inferred set contains `custom-element` even though no `Ui.widget` is
/// reachable and the handle DCEs out of the lowered program. The prior kernel-only
/// inference reported nothing here while the emitter served the JS — a
/// served-but-undisclosed browser-JS hole. The `js/counter.js` marker in the
/// served bytes is the same file the mounted-case project ships.
const UNMOUNTED_WIDGET_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.String as String

type alias WidgetState = { count : Int }

type WidgetUp = Bumped Int

type Msg = Noop

type alias Model = { count : Int }

counter : CustomElement WidgetState WidgetUp
counter = customElement "js/counter.js"

init : a -> ( Model, Cmd.Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update msg model =
    case msg of
        Noop ->
            ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.column []
        [ Ui.text (String.fromInt model.count)
        ]

subscriptions : Model -> Sub.Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Noop
        }
"#;

/// Materialise an unmounted-handle widget project (a `customElement` handle
/// constructed but never mounted), returning its dir. The caller removes it.
fn unmounted_widget_project(tag: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "ipe-ce-unmounted-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/js"))?;
    std::fs::write(
        dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"unmountedpkg\", version = \"0.1.0\" }\n",
    )?;
    std::fs::write(dir.join("src/Main.ipe"), UNMOUNTED_WIDGET_APP)?;
    std::fs::write(dir.join("src/js/counter.js"), COUNTER_JS)?;
    Ok(dir)
}

/// An unmounted `customElement` handle still ships browser JS, so its inferred
/// capability set contains `custom-element`: declaring exactly `{custom-element}`
/// verifies. Disclosure derives from the served-asset walk, not from a reachable
/// `Ui.widget` kernel.
#[test]
fn an_unmounted_handle_still_discloses_custom_element() -> TestResult {
    let dir = unmounted_widget_project("infer")?;
    let entry = dir.join("src/Main.ipe");
    let declared = BTreeSet::from([Capability::CustomElement]);
    let r = verify_capabilities(&entry, &declared);
    assert!(
        r.is_ok(),
        "an unmounted-handle app's inferred set is exactly {{custom-element}}: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Fail-closed on the unmounted-handle case: constructing a `customElement` handle
/// ships its JS, so declaring NOTHING is rejected with a `custom-element`-naming
/// mismatch — the exact hole a mounted-only test never exercised.
#[test]
fn an_unmounted_handle_that_hides_custom_element_is_rejected() -> TestResult {
    let dir = unmounted_widget_project("hide")?;
    let entry = dir.join("src/Main.ipe");
    let declared = BTreeSet::new();
    let r = verify_capabilities(&entry, &declared);
    assert!(
        matches!(
            &r,
            Err(ipe::CliError::CapabilityMismatch { missing, .. })
                if missing.contains(&"custom-element")
        ),
        "an unmounted-handle app that omits `custom-element` must be rejected as under-declared, got: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Transitivity: a `customElement` handle constructed in an IMPORTED module and
/// never mounted anywhere still ships its JS, so the entry's inferred set
/// discloses `custom-element`. The served-asset walk covers the whole linked
/// program, so a handle a dependency constructs is disclosed by the consumer.
#[test]
fn a_handle_constructed_in_an_imported_module_discloses_custom_element() -> TestResult {
    let dir = std::env::temp_dir().join(format!(
        "ipe-ce-transitive-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/js"))?;
    std::fs::write(
        dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"transpkg\", version = \"0.1.0\" }\n",
    )?;
    // Module B constructs the handle; nothing mounts it.
    std::fs::write(
        dir.join("src/Widgets.ipe"),
        "module Widgets exposing (counter)\n\ntype alias WidgetState = { count : Int }\n\ntype WidgetUp = Bumped Int\n\ncounter : CustomElement WidgetState WidgetUp\ncounter = customElement \"js/counter.js\"\n",
    )?;
    // Main imports Widgets but never mounts `counter`.
    std::fs::write(
        dir.join("src/Main.ipe"),
        "module Main exposing (main)\n\nimport Ipe.Tea.Web as Web\nimport Ipe.Ui as Ui\nimport Ipe.Tea.Web.Cmd as Cmd\nimport Ipe.Tea.Web.Sub as Sub\nimport Ipe.String as String\nimport Widgets\n\ntype Msg = Noop\n\ntype alias Model = { count : Int }\n\ninit : a -> ( Model, Cmd.Cmd Msg )\ninit _req =\n    ( { count = 0 }, Cmd.none )\n\nupdate : Msg -> Model -> ( Model, Cmd.Cmd Msg )\nupdate msg model =\n    case msg of\n        Noop ->\n            ( model, Cmd.none )\n\nview : Model -> Element Msg\nview model =\n    Ui.column [] [ Ui.text (String.fromInt model.count) ]\n\nsubscriptions : Model -> Sub.Sub Msg\nsubscriptions _model =\n    Sub.none\n\nmain =\n    Web.app\n        { init = init, update = update, view = view, subscriptions = subscriptions\n        , routes = [], notFound = Noop\n        }\n",
    )?;
    std::fs::write(dir.join("src/js/counter.js"), COUNTER_JS)?;

    let entry = dir.join("src/Main.ipe");
    let declared = BTreeSet::from([Capability::CustomElement]);
    let r = verify_capabilities(&entry, &declared);
    assert!(
        r.is_ok(),
        "a handle constructed in an imported module must disclose `custom-element` transitively: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The manifest-less single-file audit path (the `entry` alone, no `package.ipe`
/// up-tree) must disclose `custom-element` for a constructed-but-unmounted
/// `customElement` handle. This is the seam `ipe release … --capabilities` and the
/// run-jail resolver consume via [`ipe::run_sandbox::resolve_for_run`]; routing it
/// through the served-widget-aware inference keeps it consistent with `ipe
/// capabilities` / `package audit`. The `customElement "js/counter.js"` literal
/// resolves against the lone entry's own directory, so the JS sits beside it.
#[test]
fn a_manifest_less_single_file_handle_discloses_custom_element() -> TestResult {
    let dir = std::env::temp_dir().join(format!(
        "ipe-ce-singlefile-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("js"))?;
    std::fs::write(dir.join("Main.ipe"), UNMOUNTED_WIDGET_APP)?;
    std::fs::write(dir.join("js/counter.js"), COUNTER_JS)?;

    let entry = dir.join("Main.ipe");
    let resolved = ipe::run_sandbox::resolve_for_run(None, None, &entry)?;
    assert!(
        resolved.inferred.contains(&Capability::CustomElement),
        "a manifest-less single file that constructs a served widget handle must \
         disclose `custom-element` from the single-file audit path, got: {:?}",
        resolved.inferred
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

// ── the `js-port` disclosure axis ───────────────────────────────────────────

/// A Web-shape app that reaches a `Js.send` / `Js.subscribe` port exchanges raw
/// typed values with page JavaScript, so its inferred capability set must contain
/// `js-port`. A port program discloses the same way a widget program discloses
/// `custom-element`: through the `verify_capabilities` inference `ipe
/// capabilities` reports.
const JS_PORT_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Js as Js
import Ipe.Json.Decode as Decode

type alias Model = { n : Int }

type Msg = Tick | Got Int

init : a -> ( Model, Cmd.Cmd Msg )
init _r =
    ( { n = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( model, Js.send model.n )

        Got k ->
            ( { n = k }, Cmd.none )

view : Model -> Element Msg
view _model =
    Ui.text "ok"

subscriptions : Model -> Sub.Sub Msg
subscriptions _model =
    Js.subscribe Decode.int Got

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
"#;

/// Write [`JS_PORT_APP`] as a manifest-less single file and return its dir.
fn js_port_project(tag: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "ipe-jsport-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("Main.ipe"), JS_PORT_APP)?;
    Ok(dir)
}

/// A hand-rolled port program's inferred set is exactly `{js-port:raw}`: the raw
/// `Js.send`/`Js.subscribe` kernels disclose the uncharacterised `:raw` floor
/// (no `Ipe.Browser.<Api>` import characterises the axis). Declaring that set
/// must verify.
#[test]
fn a_port_program_discloses_js_port() -> TestResult {
    let dir = js_port_project("infer")?;
    let entry = dir.join("Main.ipe");
    let declared = BTreeSet::from([Capability::JsPort(WebCapability::Raw)]);
    let r = verify_capabilities(&entry, &declared);
    assert!(
        r.is_ok(),
        "a hand-rolled port program's inferred set is exactly {{js-port:raw}}: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Fail-closed: a program that reaches a port but declares NOTHING on the
/// `js-port` axis is rejected as under-declared — a port-bearing module can never
/// hide the disclosure.
#[test]
fn a_port_program_that_hides_js_port_is_rejected() -> TestResult {
    let dir = js_port_project("hide")?;
    let entry = dir.join("Main.ipe");
    let declared = BTreeSet::new();
    let r = verify_capabilities(&entry, &declared);
    assert!(
        matches!(
            &r,
            Err(ipe::CliError::CapabilityMismatch { missing, .. })
                if missing.contains(&"js-port:raw")
        ),
        "a port program that omits `js-port:raw` must be rejected as under-declared, got: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
