use crate::{help, CliError, run_version, package_manifest, project, cli_args, Path, PathBuf, resolve_vendored_runtime_dir, toolchain, watch, bluegreen_enabled, style, delivery, resolve_analysis_entry, io_bounded, Interner, find_manifest_for_ipe_file, build_plan, emit_pipeline_json, apply_fixes_cmd, runtime_dep_from_env, BuildOptions, build_with_sibling_discovery_with_options, build_project_with_options, run_sandbox, ffi, render_capabilities, Write, RuntimeContext, runtime_embed, fs, io_err, ALL_CODES, title, explain_page, BTreeMap, Diagnostic, attribute_canon_errors, home_to_source_map, attribute_post_link_error, collect_entry_and_siblings, create_source_root, unsafe_ack, web_consent, native_ffi_consent, gate_decoder_pipelines};

/// The misuse reason shown when `build` / `run` / `watch` are invoked with no
/// entry and none can be discovered. Just the reason — the command's own
/// `--help` page (appended by [`CliError::CommandUsage`]) carries the synopsis
/// and options, so this never re-lists them.
pub const NO_ENTRY: &str = "nothing to build here — pass a source file or run inside a project (a \
     package.ipe, or a src/Main.ipe)";

/// A request for help asks for output, not an error: it prints to stdout and
/// exits successfully. Returned by [`intercept_help`] so [`run_cli`] can honour
/// it before any command runs.
pub struct HelpRequest;

/// Recognise a help request in `args` and, when found, print the matching page
/// to stdout. Handles the top-level screen (no args, or a leading `--help` /
/// `-h` / `help`) and every per-command page (`<cmd> --help` or `help <cmd>`).
///
/// Returns `Some(HelpRequest)` when help was printed (the caller returns `Ok`),
/// or `None` when `args` is an ordinary command to dispatch.
pub fn intercept_help(args: &[String]) -> Option<HelpRequest> {
    let is_help_flag = |a: &str| a == "--help" || a == "-h" || a == "help";

    // No arguments, or a leading bare help token: the top-level screen.
    match args.split_first() {
        None => {
            print!("{}", help::top_level(&std::io::stdout()));
            return Some(HelpRequest);
        }
        Some((first, rest)) if is_help_flag(first) => {
            // `help <cmd>` / `--help <cmd>`: that command's page, else the
            // top-level screen.
            let named = rest
                .first()
                .and_then(|c| help::command(c, &std::io::stdout()));
            match named {
                Some(page) => print!("{page}"),
                None => print!("{}", help::top_level(&std::io::stdout())),
            }
            return Some(HelpRequest);
        }
        _ => {}
    }

    // `<cmd> --help`: the command's own page, when the command is known.
    if let Some((cmd, rest)) = args.split_first()
        && help::is_command(cmd)
        && rest.iter().any(|a| is_help_flag(a))
        && let Some(page) = help::command(cmd, &std::io::stdout())
    {
        print!("{page}");
        return Some(HelpRequest);
    }
    None
}

/// Parse `argv` (excluding the program name) and run the requested command.
///
/// # Errors
/// Returns [`CliError`] on misuse, a compile failure, or a filesystem error.
pub fn run_cli(args: &[String]) -> Result<(), CliError> {
    if intercept_help(args).is_some() {
        return Ok(());
    }
    // `--version`/`-V` is an alias of the `version` command, symmetric with the
    // `--help`/`-h` interception above: the near-universal version probe that
    // editors, version managers, and CI use must not fall through to
    // unknown-command. Any trailing flags (e.g. `--json`) pass to the command.
    if let Some((first, rest)) = args.split_first()
        && (first == "--version" || first == "-V")
    {
        return with_help_on_misuse("version", run_version(rest));
    }
    let Some((cmd, rest)) = args.split_first() else {
        // A bare `ipe` (no command) carries an empty token and just shows help.
        return Err(CliError::UnknownCommand {
            attempted: String::new(),
        });
    };
    // `ipe explain` has been folded into `ipe doc`. Print a pointer and
    // forward to `run_explain` so existing scripts keep working with a
    // deprecation notice rather than a hard failure.
    if cmd == "explain" {
        return with_help_on_misuse("doc", run_explain(rest));
    }
    // One registry drives both dispatch and help: a command runs exactly when it
    // is described, so the two cannot drift. The handler carries the canonical
    // static name its misuse `--help` page keys on. `--version`/`-V` is aliased
    // to the `version` command above the dispatch table.
    match help::handler(cmd.as_str()) {
        Some((name, run)) => with_help_on_misuse(name, run(rest)),
        // An unknown command is misuse: show the top-level help and fail. Unlike
        // an explicit `--help`, this is not a request, so it exits non-zero. The
        // typed token is kept so a near-miss can be suggested.
        None => Err(CliError::UnknownCommand {
            attempted: cmd.clone(),
        }),
    }
}

/// Map a known command's raw usage error into a [`CliError::CommandUsage`] so the
/// caller prints that command's full, indented `--help` page — the uniform
/// "misuse shows help" output. Any non-usage error (a compile failure, a
/// filesystem error) passes through untouched, since it is not a help-worthy
/// misuse. `command` is always a known command name.
pub fn with_help_on_misuse(
    command: &'static str,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    match result {
        Err(CliError::Usage(reason)) => Err(CliError::CommandUsage {
            command,
            reason: reason.to_owned(),
        }),
        Err(CliError::UsageOwned(reason)) => Err(CliError::CommandUsage { command, reason }),
        other => other,
    }
}

/// Project-aware default entry when no positional argument is given to
/// `build`, `run`, or `watch`.
///
/// Resolution order:
/// 1. `./package.ipe` exists — entry `"."` (project mode; `discover_manifest`
///    reads the directory's `package.ipe`).
/// 2. `./src/Main.ipe` exists — entry `"src/Main.ipe"` (single-file
///    shorthand without a manifest).
/// 3. A bare `./ipe.toml` with no `package.ipe` — a clear migration error, so
///    the legacy manifest never silently governs a build.
/// 4. Neither — usage error: nothing to build here.
pub fn default_entry() -> Result<String, CliError> {
    if std::path::Path::new(package_manifest::PACKAGE_IPE).exists() {
        return Ok(".".to_owned());
    }
    if std::path::Path::new("src/Main.ipe").exists() {
        return Ok("src/Main.ipe".to_owned());
    }
    if project::migration_pending(std::path::Path::new(".")) {
        return Err(CliError::Usage(project::MIGRATE_CONFIG_HINT));
    }
    Err(CliError::Usage(NO_ENTRY))
}

/// `ipe watch [<path>]` — rebuild and re-run on every source change
/// (`crate::watch`). Never returns
/// `Err` for a build failure (INV-3: a red build is logged, not fatal);
/// only misuse / setup failures propagate.
pub fn run_watch(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_watch(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };

    // Validate the delivery grammar against the shape `main` pins: the `[shape]`
    // cross-check and the `[runtime] [host]` tail. `ipe watch` is a dev
    // build-run-reload loop that serves the live runtime; it takes no `--static`.
    // A grammar refusal (e.g. `web spa ios`, which cannot be watched — see the
    // spec's mobile-watch note) is caught here before the loop starts.
    let _delivery = resolve_delivery(Path::new(&entry), &args.delivery, false, "watch")?;

    let out_dir = args
        .out
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);
    // Watch is always a native dependency-model dev build (it never vendors the
    // runtime tree, nor targets wasm), so — like `ipe build` on its default path
    // — it must NOT require the vendored runtime source subtree. It resolves the
    // dependency crate root itself via `runtime_embed::resolve` once the loop
    // starts (see `watch::run`); the vendored tree is honoured only when passed
    // explicitly with `--runtime`. Requiring `resolve_runtime` up front made
    // `ipe watch` fail to locate the runtime in an installed checkout where the
    // vendored subtree is absent but the dependency crate root resolves fine.
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, false)?;

    // Fail closed before the watch loop starts: `ipe watch` rebuilds with cargo
    // on every change, so a missing toolchain is reported once, up front, with
    // its root cause — not as a per-rebuild opaque spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Watch)?;

    let mut opts = watch::WatchOptions::new(PathBuf::from(entry), out_dir, runtime_dir);
    opts.port = args.port;
    opts.cargo_path = cargo_bin.path().to_path_buf();
    opts.quiet = args.quiet;
    opts.bluegreen = bluegreen_enabled();
    opts.reset_state = args.reset_state;
    opts.debugger = args.debugger;
    // Version header: human mode only (not quiet, not piped).
    if !args.quiet {
        use std::io::IsTerminal as _;
        if std::io::stderr().is_terminal() {
            style::print_command_header();
        }
    }
    watch::run(&opts)
}

/// Classify the shape `main` pins for the entry the user named, reading the
/// entry `.ipe` source and inspecting `main`'s head (spec § 0, § 1).
///
/// The shape is the single source of truth for delivery: it is derived from
/// code, never from config or the CLI. `entry_arg` is the raw positional (a
/// `.ipe` file, a project directory, or a bare `.`); it is routed to its entry
/// source file the same way the analysis surfaces route it. A source that does
/// not parse yields [`delivery::Shape::Script`] — the cross-check then does not
/// fire and the build pipeline reports the real parse error with a blamed span.
///
/// # Errors
/// [`CliError::Io`] when the entry source cannot be read, or the manifest /
/// entry-resolution errors of [`resolve_analysis_entry`].
pub fn classify_entry_shape(entry_arg: &Path) -> Result<delivery::Shape, CliError> {
    let entry_file = resolve_analysis_entry(entry_arg)?;
    let source = io_bounded::read_to_string_capped(&entry_file, io_bounded::SOURCE_READ_CAP)?;
    let mut interner = Interner::new();
    // A parse failure is the compile pipeline's to report (with a blamed span);
    // the shape cross-check simply does not fire, so classify as a script.
    let shape = ipe_parse::parse_module(&source, &mut interner)
        .map_or(ipe_canon::shape_source::MainShape::Script, |module| {
            ipe_canon::shape_source::classify_main_shape(&module, &interner)
        });
    Ok(delivery::Shape::from_main(shape))
}

/// Resolve the delivery for a `build` / `run` / `watch` invocation: classify the
/// shape `main` pins, cross-check the optional `[shape]` positional against it,
/// resolve the `[runtime] [host]` tail, and gate `--static`.
///
/// This is the one place the CLI turns the delivery grammar into a validated
/// [`delivery::Delivery`], from which `is_webview_native()` drives the backend
/// `webview_host` signal and the packager routing. Every refusal is a
/// pedagogical [`delivery::DeliveryError`], surfaced as a usage error.
///
/// # Errors
/// [`CliError::UsageOwned`] carrying the delivery lesson on a shape mismatch, an
/// invalid runtime/host combination, or a `--static` request the delivery
/// cannot honour; the I/O errors of [`classify_entry_shape`].
pub fn resolve_delivery(
    entry_arg: &Path,
    positionals: &cli_args::DeliveryPositionals,
    wants_static: bool,
    command: &str,
) -> Result<delivery::Delivery, CliError> {
    let pinned = classify_entry_shape(entry_arg)?;
    delivery::Delivery::resolve_checked(
        pinned,
        positionals.stated_shape,
        &positionals.tokens,
        wants_static,
    )
    .map_err(|e| CliError::UsageOwned(format!("ipe {command}: {e}")))
}

/// Route an entry argument to its `package.ipe`, when one governs it:
/// a directory must contain one, and a `.ipe` entry walks up the tree looking
/// for one (returning no manifest — single-file mode — when none exists). A
/// directory carrying only a legacy `ipe.toml` is a clear migration error.
pub fn discover_manifest(entry_path: &Path) -> Result<Option<PathBuf>, CliError> {
    if entry_path.is_dir() {
        if let Some(manifest) = project::manifest_in_dir(entry_path) {
            return Ok(Some(manifest));
        }
        if project::migration_pending(entry_path) {
            return Err(CliError::Usage(project::MIGRATE_CONFIG_HINT));
        }
        Err(CliError::Usage(
            "directory supplied but no package.ipe found inside it",
        ))
    } else {
        Ok(find_manifest_for_ipe_file(entry_path))
    }
}

/// Resolve the static request with full precedence — CLI flags > env
/// (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`) > `package.ipe` `[rust]` > AUTO —
/// into a typed plan (or a typed refusal — no artifact), run the toolchain
/// preflight, and surface the mimalloc opt-in notice. Shared by `build` and
/// `run`; resolved ONCE before any compilation starts.
///
/// `IPE_TARGET=wasm` is a wasm-target axis signal (resolved by
/// [`resolve_wasm_target`]) and is NOT a static-link triple; it is stripped
/// here so it never reaches the musl-triple gate in [`build_plan::resolve`].
pub fn resolve_static_plan(
    cli_layer: build_plan::StaticRequestLayer,
    manifest: Option<&Path>,
) -> Result<Option<ipe_backend_rust::static_build::StaticPlan>, CliError> {
    let toml_layer = match manifest {
        Some(m) => project::parse_manifest(m)?.static_request,
        None => build_plan::StaticRequestLayer::default(),
    };
    let mut env = build_plan::env_layer()?;
    if env.target.as_deref() == Some("wasm") {
        env.target = None;
    }
    let merged = cli_layer.or(env).or(toml_layer);
    let static_plan = build_plan::resolve(&merged)?;
    if let Some(plan) = &static_plan {
        build_plan::preflight(plan)?;
        if plan.allocator() == ipe_backend_rust::static_build::StaticAllocator::Mimalloc {
            // The design's explicit opt-in notice: the C cost is acknowledged,
            // never silent.
            eprintln!(
                "{}",
                style::gutter(
                    "note: mimalloc adds a C toolchain and unsafe FFI, vendors C source, and \
                     freezes it into the artifact for CVE-rebuild purposes; chosen explicitly."
                )
            );
        }
    }
    Ok(static_plan)
}

/// Resolve the wasm-vs-native target with the three-tier precedence chain:
/// CLI flag (`--target wasm`) > `IPE_TARGET=wasm` env > `[wasm].mode` in
/// `package.ipe` > default native.
///
/// `cli_wasm` carries the parsed `--target wasm` flag from `BuildMode::Emit`.
/// `wasm_config` is `None` when there is no manifest (sibling-discovery build).
///
/// Returns `true` when the resolved target is `WasmClient`.
pub fn resolve_wasm_target(cli_wasm: bool, wasm_config: Option<&project::WasmConfig>) -> bool {
    cli_wasm
        || std::env::var("IPE_TARGET").ok().as_deref() == Some("wasm")
        || wasm_config.is_some_and(project::WasmConfig::implies_wasm_target)
}

/// `ipe build [<path>]` — compile a program to a native or WebAssembly artifact.
// A linear pipeline (parse → discover manifest → acknowledge unsafe → resolve
// target → emit → cargo build); the steps share enough locals that splitting
// reads worse than the whole.
/// The outcome of a successful `ipe build`, carrying the facts needed to render
/// either a human progress line or a JSON success object.
pub struct BuildSuccess {
    /// The entry source file that was compiled.
    entry: String,
    /// The output directory holding the emitted Rust project.
    out_dir: PathBuf,
}

#[allow(clippy::too_many_lines)]
pub fn run_build(rest: &[String]) -> Result<(), CliError> {
    // Parse args once to learn the format before running the body.
    let format = cli_args::parse_build(rest)
        .map(|a| a.format)
        .unwrap_or_default();
    let result = run_build_body(rest);
    match result {
        Err(e) => Err(if format == cli_args::OutputFormat::Json {
            emit_pipeline_json(e)
        } else {
            e
        }),
        Ok(success) => {
            if format == cli_args::OutputFormat::Json {
                // Machine-readable success: one JSON object to stdout.
                let json = serde_json::json!({
                    "status": "ok",
                    "entry": success.entry,
                    "out": success.out_dir.to_string_lossy(),
                });
                println!("{json}");
            }
            // Human progress line already printed inside run_build_body.
            Ok(())
        }
    }
}

/// Inner implementation of `run_build`, format-agnostic on the success path.
/// Returns a [`BuildSuccess`] describing the outcome; the caller renders it.
#[allow(clippy::too_many_lines)]
pub fn run_build_body(rest: &[String]) -> Result<BuildSuccess, CliError> {
    let args = cli_args::parse_build(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);

    let wants_static = matches!(
        &args.mode,
        cli_args::BuildMode::Emit { static_layer, .. }
            if static_layer.static_build == Some(true)
    );

    // Parse guaranteed `--emit-ir` composes with no emit-affecting flag, so the
    // IR-dump path carries no options to drop.
    let (out, wasm_target, cli_layer) = match args.mode {
        cli_args::BuildMode::EmitIr => {
            // `--emit-ir` reads a single entry file, so route a directory / bare
            // `.` project root to its entry `.ipe` — the same convention the
            // analysis surfaces use — rather than handing the directory straight
            // to the source reader (which would fail with a raw "Is a directory").
            let ir_entry = resolve_analysis_entry(&entry_path)?;
            let tree = emit_ir_text(&ir_entry)?;
            print!("{tree}");
            return Ok(BuildSuccess {
                entry,
                out_dir: PathBuf::new(),
            });
        }
        cli_args::BuildMode::Emit {
            out,
            wasm,
            static_layer,
        } => (out, wasm, static_layer),
    };

    let out_dir = out.map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);

    // Route the build:
    //   1. Directory → expect package.ipe inside it.
    //   2. .ipe file → walk up looking for package.ipe (project-mode); fall back
    //      to sibling discovery when no manifest exists, so a multi-file project
    //      built via the file-path shorthand still compiles the whole module
    //      graph rather than the single entry file.
    let manifest = discover_manifest(&entry_path)?;

    // Parse the manifest early to read [wasm].mode for target inference.
    // build_project_with_options re-parses it later to fill in publicEnv /
    // hydrate-mode; the double parse is acceptable (manifests are small).
    let manifest_parsed = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?;
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    // Static-flag contradictions (--cfree + C-requiring allocator,
    // --target without --static, talc-without-arena) are pure over the CLI +
    // env + manifest layers and touch no source. Resolving here — before
    // resolve_delivery reads the entry file — ensures a refused build produces
    // no artifact and touches nothing, even when the entry path does not exist.
    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;

    // Resolve the delivery grammar against the shape `main` pins: the optional
    // `[shape]` cross-check, the `[runtime] [host]` tail, and the `--static`
    // gate. Runs after the static-plan check so a flag contradiction fires
    // before the entry file is read. A webview-native `web desktop` drives
    // `webview_host` below.
    let delivery = resolve_delivery(&entry_path, &args.delivery, wants_static, "build")?;

    // `--fix` carries durable authorization: apply machine-applicable fixes
    // non-interactively before the (re-run) build sees the source.
    if args.fix {
        apply_fixes_cmd(&entry_path, true, &mut std::io::stdout())?;
    }

    // Acknowledge any disclosed `.Unsafe` escape-hatch import BEFORE the (costly)
    // emit + cargo build. The safe path (no `.Unsafe` import) returns silently;
    // an exposed program requires `--accept-risks`, the manifest token, or an
    // interactive yes, and a non-interactive build without consent fails closed
    // rather than blocking on a prompt.
    acknowledge_unsafe_imports(
        manifest_parsed.as_ref(),
        manifest.as_deref(),
        &entry_path,
        args.accept_risks,
    )?;

    // App-boundary web-capability consent: a disclosed `js-port:<axis>` reached by
    // a dependency must be granted by THIS app's `[capabilities] accept`, else the
    // build fails closed naming the disclosing module.
    gate_web_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    // App-boundary native-crossing consent: a disclosed `native-ffi` crossing
    // reached by a dependency must be granted by THIS app's `[capabilities]
    // declared`, else the build fails closed naming the disclosing `Rust.<Crate>`.
    gate_native_ffi_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    // Precedence: CLI --target wasm > IPE_TARGET=wasm > [wasm].mode != "off".
    let wasm_target = resolve_wasm_target(wasm_target, manifest_wasm.as_ref());

    // The dependency model (native OR wasm) needs no vendored tree — the runtime
    // is a path dependency. Only a dep-model-OFF build vendors the source subtree.
    let runtime_dep = runtime_dep_from_env();
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

    // Fail closed before emitting: `ipe build` compiles the emitted project so a
    // reported success means the crate actually built. A missing toolchain is a
    // clear root-cause error now, not an opaque OS spawn error after the
    // (wasted) emit. The wasm branch delegates to `bundle_wasm`, which resolves
    // cargo itself, so only the native branch resolves here — the resolved path
    // is reused for its build.
    let native_cargo = if wasm_target {
        None
    } else {
        Some(toolchain::require_cargo(toolchain::ToolIntent::Build)?)
    };

    let options = BuildOptions {
        static_plan,
        target: if wasm_target {
            ipe_ir::Target::WasmClient
        } else {
            ipe_ir::Target::Native
        },
        wasm_public_env: Vec::new(),
        wasm_hydrate_mode: false,
        // `ipe build` is a development artifact — Debug.* is permitted.
        production: false,
        runtime_dep,
        // `ipe build` never tree-shakes the vendored tree — a dep-model build
        // carries no vendored source, and a vendored (`IPE_RUNTIME_VENDORED`)
        // build keeps the full tree so rustc, not the driver, drops the unreached
        // files. Only `ipe eject` sets this.
        tree_shake_vendored: false,
        // Filled in by build_project_with_options once the manifest is parsed.
        cargo_name: String::new(),
        debugger: args.debugger,
        // `ipe build` never emits appearance hot-swap scaffolding — that is a
        // `ipe watch`-only dev affordance. A release artifact stays clean.
        hot_appearance: false,
        // A webview-native `web desktop` delivery links the system webview and
        // selects the webview executor; every other delivery does not.
        webview_host: delivery.is_webview_native(),
        // Filled from the manifest `delivery.desktop` in
        // build_project_with_options once the manifest is parsed.
        webview_window: None,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // bracket it with a start/done line. Shown only on an interactive terminal so
    // piped / CI output stays clean; status goes to stderr (stdout carries data).
    // Suppressed in quiet mode (only warnings/errors) and in JSON mode (machine
    // output only — one JSON object to stdout at the end).
    let show_progress = !args.quiet && args.format != cli_args::OutputFormat::Json && {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        style::print_command_header();
        eprintln!(
            "{}",
            style::gutter(&format!("{} building {entry}", style::glyph::STEP))
        );
    }

    // No manifest found: compile entry + all sibling .ipe files in the same
    // directory. Byte-identical to `build` when the directory holds only the
    // entry file (regression-covered by the golden suite).
    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    if wasm_target {
        bundle_wasm(&out_dir)?;
    } else {
        compile_and_finalize_native_build(
            &out_dir,
            native_cargo,
            static_plan,
            runtime_dep,
            manifest.as_deref(),
            &entry_path,
            args.quiet,
        )?;
    }

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!(
                "{} built → {}",
                style::glyph::OK,
                out_dir.display()
            ))
        );
    }
    Ok(BuildSuccess { entry, out_dir })
}

/// Compile the just-emitted native crate and write its runtime-enforcement
/// artifacts. Split out of [`run_build`] so each stays a readable unit.
///
/// The compile is the SEAL: a reported build success MUST mean the crate
/// actually built, so a non-zero cargo exit surfaces as a typed
/// [`CliError::EmittedBuildFailed`] rather than a silent exit-0 that would mask a
/// miscompile. It also produces the `target/debug/ipe-app` binary that
/// `ipe exec` later runs. CWD = the emitted crate dir so the generated
/// `.cargo/config.toml` is discovered; a static plan additionally selects the
/// target triple explicitly.
///
/// A native-bearing artifact then carries its own runtime enforcement — an
/// `ipe.profile` mirror plus the authoritative capability floor embedded in the
/// binary — so the jail travels with a copied-off-host artifact (ADR 0040). A
/// pure Ipê artifact is structurally bounded and needs neither profile nor floor.
///
/// # Errors
/// - [`CliError::EmittedBuildFailed`] when the emitted crate fails to compile.
/// - The toolchain, manifest-parse, and capability-resolution errors of the
///   steps it composes.
pub fn compile_and_finalize_native_build(
    out_dir: &Path,
    native_cargo: Option<toolchain::CargoBin>,
    static_plan: Option<ipe_backend_rust::static_build::StaticPlan>,
    runtime_dep: bool,
    manifest: Option<&Path>,
    entry_path: &Path,
    quiet: bool,
) -> Result<(), CliError> {
    // `native_cargo` is `Some` on every native path (the caller's wasm branch
    // returns before here); the fallback re-resolves rather than unwrapping so
    // the toolchain error stays typed even if that invariant ever changes.
    let cargo_bin = match native_cargo {
        Some(bin) => bin,
        None => toolchain::require_cargo(toolchain::ToolIntent::Build)?,
    };
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo.arg("build").current_dir(out_dir);
    if quiet {
        cargo.arg("-q");
    } else {
        force_cargo_terminal_ui(&mut cargo);
    }
    if let Some(plan) = &static_plan {
        cargo.args(["--target", plan.triple.as_str()]);
    }
    let runtime_ctx = if runtime_dep {
        runtime_context_for_message()
    } else {
        None
    };
    build_emitted_project(&mut cargo, "the emitted program", runtime_ctx, out_dir)?;

    let manifest_parsed = match manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved = run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest, entry_path)?;
    if run_sandbox::is_native_bearing(&resolved.union()) {
        let profile = run_sandbox::build_profile(&resolved, driver)?;
        run_sandbox::write_build_artifacts(out_dir, &profile)?;
    }
    Ok(())
}

/// `ipe eject [<path>] --out <dir>` — emit a self-contained Rust Cargo project a
/// user can `cargo build` with no `ipe` toolchain installed.
///
/// The escape hatch from the dependency-crate model: where `ipe build` emits a
/// project that names the runtime as a path dependency (resolved by the
/// toolchain), eject VENDORS the runtime source into the output — and tree-shakes
/// it to only the modules the program reaches. The emitted `ipe_runtime/mod.rs`
/// already declares `pub mod X;` for exactly the reached top-level modules, so
/// [`build_emit_manifest`] copies only those source files. The result is a
/// small, auditable, offline-buildable crate: pure, reviewable Rust with no
/// external runtime path and no registry fetch.
///
/// Eject is native-only and FFI-free by contract:
///   - A foreign-crate FFI binding would need external crates pulled from a
///     registry, which the source-only, self-contained contract forbids — so an
///     FFI-bearing program is a hard [`CliError::EjectUnsupported`] refusal
///     rather than a tree that would not resolve offline.
///   - `--target wasm` is a distinct compilation axis with its own bundling
///     step; eject targets a plain-`cargo build` native crate.
///
/// # Errors
/// [`CliError::EjectUnsupported`] for an FFI-bearing program; the same
/// pipeline / filesystem / runtime-resolution errors as [`build_project`].
pub fn run_eject(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_eject(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);
    let out_dir = PathBuf::from(&args.out);

    // Fail closed on an FFI-bearing project BEFORE any emit: eject vendors only
    // the embedded runtime SOURCE, so a program binding a foreign Rust crate
    // cannot be made self-contained (its external crates would be a registry
    // fetch at the ejected project's `cargo build`). Detecting it from the
    // installed FFI catalog is the same trusted signal the build pipeline reads;
    // a non-empty catalog means at least one `Rust.` binding is in scope.
    if !ffi::load_catalog_for(&entry_path)?.is_empty() {
        return Err(CliError::EjectUnsupported {
            reason: "this program binds a foreign Rust crate (FFI). Eject vendors only the \
                     embedded runtime source, so it cannot produce a self-contained project for \
                     a program that pulls external crates — build it with `ipe build` instead"
                .to_owned(),
        });
    }

    let manifest = discover_manifest(&entry_path)?;

    // Eject targets a plain native `cargo build`; the wasm target has its own
    // bundling step and a distinct closed vendoring template. Refuse a wasm
    // request from ANY tier — the `IPE_TARGET=wasm` env OR a project's
    // `[wasm].mode` — rather than silently emit a native tree for a browser app.
    // (`parse_eject` has no `--target` flag, so the CLI tier cannot select wasm
    // here; `false` for the CLI axis is exact.)
    let manifest_wasm: Option<project::WasmConfig> = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?
        .map(|m| m.wasm);
    if resolve_wasm_target(false, manifest_wasm.as_ref()) {
        return Err(CliError::EjectUnsupported {
            reason: "eject produces a native Cargo project; the wasm target has a separate \
                     bundling step — use `ipe build --target wasm`"
                .to_owned(),
        });
    }

    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, true)?;

    // Force the vendored, tree-shaken emit shape: a self-contained project names
    // no runtime path dependency (`runtime_dep = false`) and carries only the
    // reached runtime source (`tree_shake_vendored = true`). Static/wasm options
    // stay at their defaults — eject is the plain native standalone shape.
    let options = BuildOptions {
        runtime_dep: false,
        tree_shake_vendored: true,
        ..BuildOptions::default()
    };

    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} ejecting {entry}", style::glyph::STEP))
        );
    }

    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!(
                "{} ejected → {} (self-contained; `cd {} && cargo build`)",
                style::glyph::OK,
                out_dir.display(),
                out_dir.display()
            ))
        );
    }
    Ok(())
}

/// `ipe release [<path>] [--out <dir>] [--target wasm|<triple>] [--embed]` —
/// build the production artifact for every app kind.
///
/// The artifact kind is determined by the app and the `--target` flag:
///
/// - **Native-bearing** (app crosses into `Rust.` / FFI): the jailed bundle —
///   `ipe-wrapper` (jailed launcher), `ipe-app` (statically-linked app binary),
///   and `ipe.profile` (serialised capability profile). The `--embed` flag
///   (the default) fuses all three into a single self-jailing binary.
/// - **Pure native** (no native/FFI content): a plain optimised binary under
///   the release cargo profile. No jail wrapper is needed; the binary is
///   structurally bounded to its inferred capabilities.
/// - **Browser/wasm** (`--target wasm`): the production browser bundle
///   (optimised `.wasm` + generated glue + assets) exactly as `ipe build
///   --target wasm` produces, but with the production flag set so the
///   `Ipe.Debug` gate (IPE-L0140) fires.
///
/// Every path sets `production = true` so the `Ipe.Debug.*` gate fires for
/// all app kinds. `ipe build` and `ipe run` leave `production = false`
/// (development — `Debug.*` is permitted there).
///
/// ## Honest limit (native-bearing)
///
/// The inner `ipe-app` is a native ELF/Mach-O/PE binary — an operator can run
/// it directly without the wrapper, bypassing the jail. The wrapper makes the
/// sanctioned, jailed, profile-verified path the easy toolchain-free one; it
/// does not make unjailed execution impossible for a sufficiently privileged
/// local operator. This limit is documented, not a defect.
///
/// ## Security boundary
///
/// The jail enforcement is the SAME code path as `ipe exec` — both call into
/// `ipe_sandbox::run_jail::{scan_capfloor, satisfies_capfloor, exec_in_run_jail}`.
/// There is no second jail implementation; any future change to the jail
/// mechanism automatically applies to both paths.
///
/// # Errors
///
/// Build, toolchain, manifest-parse, filesystem, and capability-resolution
/// errors.
#[allow(clippy::too_many_lines)]
pub fn run_release(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_release(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);

    // `--capabilities` / `--show-profile`: inspect the inferred capability
    // model without building or writing anything.
    if args.capabilities_only {
        let manifest = discover_manifest(&entry_path)?;
        return run_release_capabilities(&entry_path, manifest.as_deref(), args.format);
    }

    // Discover the manifest (same logic as build/eject).
    let manifest = discover_manifest(&entry_path)?;

    let manifest_parsed = match manifest.as_deref() {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    // Route on the typed target: Wasm → browser bundle; Native → static binary.
    // `resolve_wasm_target` also checks the `IPE_TARGET` env var and manifest.
    let wasm_target = resolve_wasm_target(
        args.target == cli_args::ReleaseTarget::Wasm,
        manifest_wasm.as_ref(),
    );

    if wasm_target {
        // Browser/wasm production path.
        let out_dir = args
            .out
            .as_deref()
            .map_or_else(|| PathBuf::from("release"), PathBuf::from);
        let runtime_dep = runtime_dep_from_env();
        let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

        let show_progress = {
            use std::io::IsTerminal as _;
            std::io::stderr().is_terminal()
        };
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!("{} releasing {entry} (wasm)", style::glyph::STEP))
            );
        }

        // Emit the Rust wasm project with production=true so the Debug gate fires.
        let options = BuildOptions {
            static_plan: None,
            target: ipe_ir::Target::WasmClient,
            wasm_public_env: manifest_parsed
                .as_ref()
                .map(|m| m.wasm.public_env.clone())
                .unwrap_or_default(),
            wasm_hydrate_mode: manifest_wasm
                .as_ref()
                .is_some_and(|w| w.mode.as_deref() == Some("hydrate")),
            production: true,
            runtime_dep,
            tree_shake_vendored: false,
            cargo_name: String::new(),
            // The debugger is never enabled on a release build.
            debugger: false,
            // A release build never carries appearance hot-swap scaffolding.
            hot_appearance: false,
            // Set from the resolved delivery; a webview-native `web desktop` is wired
            // where the classified shape is known (build_project_with_options).
            webview_host: false,
            webview_window: None,
        };
        manifest.as_ref().map_or_else(
            || {
                build_with_sibling_discovery_with_options(
                    &entry_path,
                    &out_dir,
                    &runtime_dir,
                    options.clone(),
                )
            },
            |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
        )?;
        bundle_wasm(&out_dir)?;
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {}/www/",
                    style::glyph::OK,
                    out_dir.display()
                ))
            );
        }
        return Ok(());
    }

    // Native path: extract the triple already validated at parse time.
    let triple = match args.target {
        cli_args::ReleaseTarget::Native(t) => t,
        cli_args::ReleaseTarget::Wasm => {
            // `wasm_target` above is true when `args.target == Wasm`, so this
            // branch is unreachable in practice; the exhaustive match keeps the
            // compiler satisfied without a panic or unreachable!().
            return Ok(());
        }
    };

    // Resolve capabilities up-front to discriminate between native-bearing
    // (needs jail wrapper) and pure-native (plain optimised binary).
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved =
        run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, false)?;

    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };

    if !run_sandbox::is_native_bearing(&resolved.union()) {
        // Pure-native path: emit and build a plain release binary.
        let out_dir = args
            .out
            .as_deref()
            .map_or_else(|| PathBuf::from("release"), PathBuf::from);

        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!("{} releasing {entry}", style::glyph::STEP))
            );
        }

        let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Build)?;

        let app_static_plan = Some(ipe_backend_rust::static_build::StaticPlan {
            triple,
            c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
                allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
            },
        });

        let options = BuildOptions {
            static_plan: app_static_plan,
            target: ipe_ir::Target::Native,
            production: true,
            runtime_dep: runtime_dep_from_env(),
            tree_shake_vendored: false,
            ..BuildOptions::default()
        };
        manifest.as_ref().map_or_else(
            || {
                build_with_sibling_discovery_with_options(
                    &entry_path,
                    &out_dir,
                    &runtime_dir,
                    options.clone(),
                )
            },
            |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
        )?;

        let mut app_cargo = std::process::Command::new(cargo_bin.path());
        app_cargo
            .arg("build")
            .arg("--release")
            .args(["--target", triple.as_str()])
            .current_dir(&out_dir);
        force_cargo_terminal_ui(&mut app_cargo);
        build_emitted_project(&mut app_cargo, "the release binary", None, &out_dir)?;

        let app_target_dir = cargo_target_directory(&out_dir)?;
        let bin_name = emitted_bin_name(&out_dir);
        let bin_path = app_target_dir
            .join(triple.as_str())
            .join("release")
            .join(&bin_name);
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {}",
                    style::glyph::OK,
                    bin_path.display()
                ))
            );
        }
        return Ok(());
    }

    // Native-bearing path: jailed bundle (same substance as the predecessor).
    let out_dir = args
        .out
        .as_deref()
        .map_or_else(|| PathBuf::from("release"), PathBuf::from);

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} releasing {entry}", style::glyph::STEP))
        );
    }

    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Build)?;

    // Step 1: emit + build the app binary (static, musl, production).
    let app_out = out_dir.join("app");
    let app_static_plan = Some(ipe_backend_rust::static_build::StaticPlan {
        triple,
        c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
            allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
        },
    });
    let options = BuildOptions {
        static_plan: app_static_plan,
        target: ipe_ir::Target::Native,
        production: true,
        runtime_dep: runtime_dep_from_env(),
        tree_shake_vendored: false,
        ..BuildOptions::default()
    };
    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &app_out,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &app_out, &runtime_dir, options.clone()),
    )?;

    let mut app_cargo = std::process::Command::new(cargo_bin.path());
    app_cargo
        .arg("build")
        .arg("--release")
        .args(["--target", triple.as_str()])
        .current_dir(&app_out);
    force_cargo_terminal_ui(&mut app_cargo);
    build_emitted_project(&mut app_cargo, "the release app", None, &app_out)?;

    // Write the capability enforcement artifacts (ipe.profile + embedded floor).
    let profile = run_sandbox::build_profile(&resolved, driver)?;
    run_sandbox::write_build_artifacts(&app_out, &profile)?;

    // Locate the compiled app binary. The target dir may be a global
    // `CARGO_TARGET_DIR` (set by the user or the agent lane), so we resolve
    // it via cargo metadata rather than assuming `app_out/target/`.
    let app_target_dir = cargo_target_directory(&app_out)?;
    let release_bin_name = emitted_bin_name(&app_out);
    let app_binary = app_target_dir
        .join(triple.as_str())
        .join("release")
        .join(&release_bin_name);
    if !app_binary.is_file() {
        return Err(CliError::UsageOwned(format!(
            "ipe release: expected app binary at {} — cargo build succeeded but binary is missing",
            app_binary.display()
        )));
    }
    let profile_src = app_out.join("ipe.profile");

    // Step 2: build the wrapper binary (static, musl).
    let wrapper_triple = triple;
    let wrapper_static_plan = ipe_backend_rust::static_build::StaticPlan {
        triple: wrapper_triple,
        c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
            allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
        },
    };

    let mut wrapper_cargo = std::process::Command::new(cargo_bin.path());
    wrapper_cargo
        .arg("build")
        .arg("--release")
        .arg("--package")
        .arg("ipe_wrapper")
        .args(["--target", wrapper_static_plan.triple.as_str()]);

    if matches!(args.mode, cli_args::ReleaseMode::Embed) {
        // Embed mode: pass the app binary + profile as env vars so build.rs
        // copies them into OUT_DIR and enables the embed_mode cfg.
        wrapper_cargo
            .env("IPE_EMBED_APP", &app_binary)
            .env("IPE_EMBED_PROFILE", &profile_src);
    }

    // Run from the workspace root so cargo finds the workspace Cargo.toml.
    let workspace_root = find_workspace_root()?;
    wrapper_cargo.current_dir(&workspace_root);
    force_cargo_terminal_ui(&mut wrapper_cargo);

    build_emitted_project(
        &mut wrapper_cargo,
        "the release wrapper",
        None,
        &workspace_root,
    )?;

    // Step 3: lay out the bundle.
    let bundle_dir = out_dir.join("bundle");
    std::fs::create_dir_all(&bundle_dir).map_err(|e| CliError::Io {
        path: bundle_dir.clone(),
        source: e,
    })?;

    // Locate the wrapper binary. As with the app binary, the target dir may be
    // a global CARGO_TARGET_DIR; resolve via cargo metadata.
    let wrapper_target_dir = cargo_target_directory(&workspace_root)?;
    let wrapper_src = wrapper_target_dir
        .join(wrapper_static_plan.triple.as_str())
        .join("release")
        .join("ipe-wrapper");

    let artifact = match args.mode {
        cli_args::ReleaseMode::Embed => {
            // Single-file embed: copy only the wrapper (app + profile baked in).
            let dest = bundle_dir.join("ipe-wrapper");
            std::fs::copy(&wrapper_src, &dest).map_err(|e| CliError::Io {
                path: dest.clone(),
                source: e,
            })?;
            #[cfg(unix)]
            set_executable(&dest)?;
            dest
        }
        cli_args::ReleaseMode::Bundle => {
            // Bundle: wrapper + app + profile as siblings.
            let wrapper_dest = bundle_dir.join("ipe-wrapper");
            let app_dest = bundle_dir.join("ipe-app");
            let profile_dest = bundle_dir.join("ipe.profile");
            std::fs::copy(&wrapper_src, &wrapper_dest).map_err(|e| CliError::Io {
                path: wrapper_dest.clone(),
                source: e,
            })?;
            std::fs::copy(&app_binary, &app_dest).map_err(|e| CliError::Io {
                path: app_dest.clone(),
                source: e,
            })?;
            std::fs::copy(&profile_src, &profile_dest).map_err(|e| CliError::Io {
                path: profile_dest.clone(),
                source: e,
            })?;
            #[cfg(unix)]
            {
                set_executable(&wrapper_dest)?;
                set_executable(&app_dest)?;
            }
            bundle_dir
        }
    };

    if show_progress {
        // Post-build report: how the binary is linked, where it landed, and the
        // capability model it will enforce.
        let cap_names: Vec<&'static str> = resolved.union().iter().map(|c| c.as_str()).collect();
        eprint!(
            "{}",
            release_bundle_report(&artifact, &cap_names, args.mode)
        );
        match args.mode {
            cli_args::ReleaseMode::Embed => eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {} (single self-jailing binary; \
                     run `--capabilities` to audit)",
                    style::glyph::OK,
                    artifact.display()
                ))
            ),
            cli_args::ReleaseMode::Bundle => eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released (bundle) → {} (run `./ipe-wrapper -- <args>`; \
                     WARNING: ipe-app can be run directly, bypassing the sandbox — \
                     prefer embed mode for production)",
                    style::glyph::OK,
                    artifact.display()
                ))
            ),
        }
    }
    Ok(())
}

/// Inspect the inferred capability model for `entry_path` without building or
/// writing anything — the body of `ipe release --capabilities` / `--show-profile`.
pub fn run_release_capabilities(
    entry_path: &Path,
    manifest: Option<&Path>,
    format: cli_args::OutputFormat,
) -> Result<(), CliError> {
    let manifest_parsed = match manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let resolved = run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest, entry_path)?;
    let names: Vec<&'static str> = resolved.union().iter().map(|c| c.as_str()).collect();
    print!(
        "{}",
        render_capabilities(&names, format, &std::io::stdout())
    );
    Ok(())
}

/// The human-readable post-build report for a native-bearing release bundle:
/// link kind, artifact path, and the enforced capability model.
pub fn release_bundle_report(
    artifact: &Path,
    capabilities: &[&str],
    mode: cli_args::ReleaseMode,
) -> String {
    use std::fmt::Write as _;

    let kind = match mode {
        cli_args::ReleaseMode::Embed => "single self-jailing binary",
        cli_args::ReleaseMode::Bundle => "multi-file bundle (wrapper + app + profile)",
    };
    let mut body = String::new();
    let _ = writeln!(body, "link: static (musl)");
    let _ = writeln!(body, "shape: {kind}");
    let _ = writeln!(body, "artifact: {}", artifact.display());
    if capabilities.is_empty() {
        let _ = writeln!(body, "capabilities: none");
    } else {
        let _ = writeln!(body, "capabilities: {}", capabilities.join(", "));
    }
    style::frame(&style::gutter(&body))
}

/// Walk parent directories from the current directory to find the workspace
/// root (the directory containing the root `Cargo.toml` with `[workspace]`).
///
/// # Errors
///
/// [`CliError::UsageOwned`] if the workspace root cannot be found.
pub fn find_workspace_root() -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Io {
        path: PathBuf::from("."),
        source: e,
    })?;
    let mut candidate = cwd.as_path();
    loop {
        let toml = candidate.join("Cargo.toml");
        if toml.is_file() {
            let text = std::fs::read_to_string(&toml).map_err(|e| CliError::Io {
                path: toml.clone(),
                source: e,
            })?;
            if text.contains("[workspace]") {
                return Ok(candidate.to_path_buf());
            }
        }
        match candidate.parent() {
            Some(p) => candidate = p,
            None => {
                return Err(CliError::UsageOwned(
                    "ipe release: cannot locate workspace root (no Cargo.toml with [workspace] \
                     found in any parent directory)"
                        .to_owned(),
                ));
            }
        }
    }
}

/// Set the executable bit on a file (Unix only; no-op on other platforms).
///
/// # Errors
///
/// [`CliError::Io`] when the permission cannot be set.
#[cfg(unix)]
pub fn set_executable(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::metadata(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut perms = meta.permissions();
    let mode = perms.mode() | 0o111;
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Run a `cargo build` of an emitted project to completion, *streaming* its
/// stderr to this process's stderr line by line as `cargo` emits it — so the
/// user sees the live compile progress (which crate is building, warnings)
/// rather than a silent wait that only reveals itself once `cargo` has already
/// finished. The same lines are accumulated so that on a non-zero exit the
/// captured text is returned inside a typed [`CliError::EmittedBuildFailed`]:
/// the failure renders as a clean `ipe`-level diagnostic — a targeted
/// runtime-feature line when `cargo` reports a missing feature, otherwise the
/// trimmed `cargo` error under a plain header — and never the command's `--help`
/// page. `what` names what was built; `runtime` is the crate the project linked
/// against, when the caller resolved one.
///
/// `cargo`'s stdout is inherited untouched (a `cargo build` writes only status
/// to stderr; nothing on stdout needs capture), so any tool output stays on
/// stdout while progress stays on stderr.
///
/// # Errors
/// - [`CliError::Io`] if `cargo` cannot be spawned or its stderr pipe cannot be
///   opened.
/// - [`CliError::EmittedBuildFailed`] if `cargo` exits non-zero.
pub fn build_emitted_project(
    cargo: &mut std::process::Command,
    what: &'static str,
    runtime: Option<RuntimeContext>,
    io_path: &Path,
) -> Result<(), CliError> {
    use std::io::BufReader;
    use std::process::Stdio;

    let io_err = |e: std::io::Error| CliError::Io {
        path: io_path.to_path_buf(),
        source: e,
    };

    // Pipe stderr so we can both forward it live AND capture it for the typed
    // error; leave stdout inherited (a `cargo build` writes only to stderr).
    let mut child = cargo
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_err)?;

    // The pipe is present because we just set `Stdio::piped()`; the fallback
    // keeps this panic-free rather than unwrapping the `Option`.
    let mut captured = String::new();
    if let Some(stderr) = child.stderr.take() {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        // Read raw bytes per chunk so a carriage-return progress bar (which
        // carries no newline) still surfaces; `read_line` alone would block on
        // cargo's in-place progress line until the next newline.
        loop {
            line.clear();
            let read = read_progress_chunk(&mut reader, &mut line).map_err(io_err)?;
            if read == 0 {
                break;
            }
            // Forward this chunk live so the user sees cargo's progress as it
            // happens; also accumulate it for a failure diagnostic.
            eprint!("{line}");
            let _ = std::io::stderr().flush();
            captured.push_str(&line);
        }
    }

    let status = child.wait().map_err(io_err)?;
    if status.success() {
        return Ok(());
    }
    Err(CliError::EmittedBuildFailed {
        what,
        code: status.code().unwrap_or(1),
        stderr: captured,
        runtime,
    })
}

/// Read the next chunk of `cargo`'s stderr into `out`, stopping at either a
/// newline (a completed message line) or a carriage return (the boundary of
/// cargo's in-place progress bar, which carries no newline). Returns the number
/// of bytes read; `0` marks end of stream. Reading to *either* terminator keeps
/// the live progress bar flowing rather than buffering until the next `\n`.
///
/// Bytes are decoded lossily so a non-UTF-8 byte from a compiler message never
/// aborts the build's progress relay.
///
/// # Errors
/// Propagates the underlying read error from the `cargo` stderr pipe.
pub fn read_progress_chunk<R: std::io::Read>(
    reader: &mut R,
    out: &mut String,
) -> std::io::Result<usize> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut total = 0usize;
    loop {
        let mut byte = [0u8; 1];
        let n = reader.read(&mut byte)?;
        if n == 0 {
            break;
        }
        total += n;
        bytes.push(byte[0]);
        if byte[0] == b'\n' || byte[0] == b'\r' {
            break;
        }
    }
    out.push_str(&String::from_utf8_lossy(&bytes));
    Ok(total)
}

/// Apply three environment variables to `cmd` so `cargo` emits ANSI colour and
/// its `Building [===]` progress bar even through a pipe — but only when our own
/// stderr is a real terminal (`NO_COLOR` unset). Without the explicit width,
/// `cargo` draws no bar at all (it reads the bar width from its piped stderr,
/// which reports no size).
#[cfg(unix)]
pub fn force_cargo_terminal_ui(cmd: &mut std::process::Command) {
    let stderr = std::io::stderr();
    if !crate::style::use_color(&stderr) {
        return;
    }
    cmd.env("CARGO_TERM_COLOR", "always");
    cmd.env("CARGO_TERM_PROGRESS_WHEN", "always");
    let cols = terminal_width(&stderr).unwrap_or(80);
    cmd.env("CARGO_TERM_PROGRESS_WIDTH", cols.to_string());
}

/// No-op shim for non-Unix targets where `rustix::termios` is unavailable.
#[cfg(not(unix))]
pub(crate) fn force_cargo_terminal_ui(_cmd: &mut std::process::Command) {}

/// The column width of `stream`'s terminal, or `None` when it is not a terminal
/// or the size cannot be read. Uses `TIOCGWINSZ` via rustix — no libc binding.
#[cfg(unix)]
pub fn terminal_width(stream: &impl std::os::fd::AsFd) -> Option<u16> {
    let ws = rustix::termios::tcgetwinsize(stream).ok()?;
    (ws.ws_col > 0).then_some(ws.ws_col)
}

/// The runtime crate the emit will link against, as a [`RuntimeContext`] for a
/// build-failure message. `None` when no dependency-model runtime is resolved
/// (a wasm or vendored build), in which case a feature-gap message simply omits
/// the crate reference. Resolution failure is swallowed to `None` — this is only
/// for enriching an error message, never a gate.
pub fn runtime_context_for_message() -> Option<RuntimeContext> {
    runtime_embed::resolve().ok().map(|r| RuntimeContext {
        root: r.root().to_path_buf(),
        version: r.version().to_owned(),
    })
}

/// Run the three post-emit bundle steps for `--target wasm`:
/// 1. `cargo build --target wasm32-unknown-unknown --release` (THE SEAL cross-target)
/// 2. `wasm-bindgen` CLI — emits the JS glue + `www/pkg/ipe_app_bg.wasm`
/// 3. `wasm-opt -Oz` — optional; silently skipped when not on PATH
///
/// Writes the final `www/pkg/` tree into `out_dir/www/pkg/`. On success the
/// directory at `out_dir/www/` is a self-contained static SPA ready to serve.
///
/// # Errors
/// [`CliError::EmittedBuildFailed`] when the wasm `cargo build` fails;
/// [`CliError::UsageOwned`] when `wasm-bindgen` fails.
pub fn bundle_wasm(out_dir: &Path) -> Result<(), CliError> {
    // Fail closed before the cross-compile: a missing toolchain becomes a clear
    // root-cause message rather than an opaque OS spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::BundleWasm)?;

    // Step 1: compile to .wasm
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo
        .args(["build", "--target", "wasm32-unknown-unknown", "--release"])
        .current_dir(out_dir);
    force_cargo_terminal_ui(&mut cargo);
    // The wasm build uses the SAME dependency-model runtime crate the native path
    // does (selected via the `wasm-client` floor). Attach the resolved runtime
    // context so a `cargo build` failure that names a missing runtime feature can
    // point at the exact crate; resolution failure degrades to `None` (message
    // enrichment only, never a gate — the missing-path-dependency error cargo
    // itself raises is already fail-closed).
    build_emitted_project(
        &mut cargo,
        "the emitted wasm program",
        runtime_context_for_message(),
        out_dir,
    )?;

    // Step 2: wasm-bindgen — locate the .wasm the cargo build just produced
    // (`CARGO_TARGET_DIR` may relocate it; probe the env var first, then the
    // per-project fallback the emitted manifest's `[workspace]` detachment
    // would use).
    let wasm_path = {
        let via_env = std::env::var_os("CARGO_TARGET_DIR").map(|d| {
            std::path::PathBuf::from(d)
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("ipe_app.wasm")
        });
        let via_crate = out_dir
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("ipe_app.wasm");
        via_env.filter(|p| p.is_file()).unwrap_or(via_crate)
    };

    let pkg_dir = out_dir.join("www").join("pkg");
    fs::create_dir_all(&pkg_dir).map_err(|e| io_err(&pkg_dir, e))?;

    let wb_status = std::process::Command::new("wasm-bindgen")
        .args([
            wasm_path.to_string_lossy().as_ref(),
            "--target",
            "web",
            "--no-typescript",
            "--out-dir",
            pkg_dir.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|e| CliError::Io {
            path: wasm_path.clone(),
            source: e,
        })?;
    if !wb_status.success() {
        let code = wb_status.code().unwrap_or(1);
        return Err(CliError::UsageOwned(format!(
            "wasm-bindgen failed (exit {code}); ensure wasm-bindgen-cli {ver} is installed: \
             cargo install wasm-bindgen-cli --version {ver}",
            ver = "0.2.126"
        )));
    }

    // Step 3: wasm-opt -Oz — optional size pass; silently skip when absent
    // (`Command::new` returns `Err` when the tool is missing).
    let bg_wasm = pkg_dir.join("ipe_app_bg.wasm");
    if bg_wasm.is_file()
        && let Ok(status) = std::process::Command::new("wasm-opt")
            .args([
                bg_wasm.to_string_lossy().as_ref(),
                "-Oz",
                "-o",
                bg_wasm.to_string_lossy().as_ref(),
            ])
            .status()
        && !status.success()
    {
        // wasm-opt found but failed — non-fatal; the unoptimised bundle
        // is still correct. Log and continue.
        eprintln!(
            "{}",
            style::gutter(&format!(
                "note: wasm-opt exited {}; bundle is unoptimised but functional",
                status.code().unwrap_or(1)
            ))
        );
    }

    let bundle_kb = bg_wasm.metadata().map_or(0, |m| m.len() / 1024);
    let www = out_dir.join("www");
    eprintln!(
        "{}",
        style::gutter(&format!(
            "wasm bundle ready at {www}/\n\
             bundle size: {bundle_kb} KB ({bg})\n\
             serve with: python3 -m http.server -d {www} 8080",
            www = www.display(),
            bg = bg_wasm.display(),
        ))
    );
    Ok(())
}

/// `ipe run [<path>]` — compile a program and run the resulting binary.
///
/// One-shot build + run: compiles the entry to `out_dir` (same routing as
/// [`run_build`]), then invokes `cargo build` on the emitted project and
/// execs the resulting `ipe-app` binary, forwarding any arguments supplied
/// after `--` and propagating the binary's exit code.
///
/// Build failures (ipe compile step or cargo build step) surface as
/// [`CliError`] and print to stderr via the normal error path. The binary
/// exec step replaces the current process (Unix) or propagates the child's
/// exit code (all platforms) so the caller sees it as `ipe run`'s own exit.
// A linear pipeline (compile → cargo build → resolve capabilities → jail →
// exec); the steps share enough locals that splitting reads worse than the whole.
#[allow(clippy::too_many_lines)]
pub fn run_run(rest: &[String]) -> Result<(), CliError> {
    let format = cli_args::parse_run(rest)
        .map(|a| a.format)
        .unwrap_or_default();
    run_run_body(rest).map_err(|e| {
        if format == cli_args::OutputFormat::Json {
            emit_pipeline_json(e)
        } else {
            e
        }
    })
}

/// Inner implementation of `run_run`, unaware of JSON formatting.
#[allow(clippy::too_many_lines)]
pub fn run_run_body(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_run(rest)?;
    let bin_args = args.bin_args;
    let cli_layer = args.static_layer;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };

    let entry_path = PathBuf::from(&entry);

    let out_dir = args
        .out
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);

    // --- Step 1: ipe compile → emit the Rust project ---
    let manifest = discover_manifest(&entry_path)?;

    // Parse the manifest early to read [wasm].mode for target inference.
    let manifest_parsed = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?;
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    let wants_static = cli_layer.static_build == Some(true);

    // Static-flag contradictions (--cfree + C-requiring allocator,
    // --target without --static, talc-without-arena) are pure over the CLI +
    // env + manifest layers and touch no source. Resolving here — before
    // resolve_delivery reads the entry file — ensures a refused run produces
    // no artifact and touches nothing, even when the entry path does not exist.
    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;

    // Resolve the delivery grammar (shape cross-check, runtime/host, `--static`
    // gate) against the shape `main` pins — same as `ipe build`. A webview-native
    // `web desktop` drives `webview_host` below. Runs after the static-plan check
    // so a flag contradiction fires before the entry file is read.
    let delivery = resolve_delivery(&entry_path, &args.delivery, wants_static, "run")?;

    // Acknowledge any disclosed `.Unsafe` escape-hatch import BEFORE the (costly)
    // emit + cargo build. Same gate as `ipe build`: the safe path is silent, an
    // exposed program needs consent, and a non-interactive run without consent
    // fails closed rather than blocking on a prompt.
    acknowledge_unsafe_imports(
        manifest_parsed.as_ref(),
        manifest.as_deref(),
        &entry_path,
        args.accept_risks,
    )?;

    // App-boundary web-capability consent: same gate as `ipe build` — a disclosed
    // `js-port:<axis>` must be granted by this app's manifest, else fail closed.
    gate_web_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    // App-boundary native-crossing consent: same gate as `ipe build` — a disclosed
    // `native-ffi` crossing must be granted by this app's `[capabilities] declared`,
    // else fail closed naming the disclosing `Rust.<Crate>`.
    gate_native_ffi_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    // When the project declares [wasm].mode != "off", or IPE_TARGET=wasm is
    // set, treat `ipe run` as a wasm build-and-bundle (no native binary to
    // exec). A plain `ipe run` in a non-wasm project stays native.
    let wasm_target = resolve_wasm_target(false, manifest_wasm.as_ref());

    // The dependency model (native OR wasm) needs no vendored tree — the runtime
    // is a path dependency. Only a dep-model-OFF build vendors the source subtree.
    let runtime_dep = runtime_dep_from_env();
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

    // Fail closed before emitting: `ipe run` shells out to cargo to build the
    // emitted project, so a missing toolchain is a clear root-cause error now,
    // not an opaque OS spawn error after the (wasted) compile. The wasm branch
    // delegates to `bundle_wasm`, which resolves cargo itself, so only the
    // native branch resolves here — the resolved path is reused for its build.
    let native_cargo = if wasm_target {
        None
    } else {
        Some(toolchain::require_cargo(toolchain::ToolIntent::Run)?)
    };

    // `ipe run` is a DEVELOPMENT execution, so `Debug.*` is allowed
    // (production = false).
    let options = BuildOptions {
        static_plan,
        target: if wasm_target {
            ipe_ir::Target::WasmClient
        } else {
            ipe_ir::Target::Native
        },
        wasm_public_env: Vec::new(),
        wasm_hydrate_mode: false,
        production: false,
        runtime_dep,
        // `ipe run` builds and executes; it never tree-shakes the vendored tree
        // (only `ipe eject` does).
        tree_shake_vendored: false,
        // Filled in by build_project_with_options once the manifest is parsed.
        cargo_name: String::new(),
        debugger: args.debugger,
        // `ipe run` never emits appearance hot-swap scaffolding — that is a
        // `ipe watch`-only dev affordance.
        hot_appearance: false,
        // A webview-native `web desktop` delivery links the system webview and
        // selects the webview executor; every other delivery does not.
        webview_host: delivery.is_webview_native(),
        // Filled from the manifest `delivery.desktop` in
        // build_project_with_options once the manifest is parsed.
        webview_window: None,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // announce the running step. On a terminal only (piped / CI output stays
    // clean); to stderr, so stdout carries only the program's own output. The
    // cargo build that follows streams its own progress; the exec that ends
    // `ipe run` leaves no room for a settled "done" line, so the run just starts
    // producing the program's output. Suppressed when `--quiet` is set.
    let show_progress = !args.quiet && {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        style::print_command_header();
        eprintln!(
            "{}",
            style::gutter(&format!("{} building {entry}", style::glyph::STEP))
        );
    }

    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    // A wasm project has no native binary to run; `ipe run` for a wasm
    // project produces the browser bundle (same post-emit step as
    // `ipe build --target wasm`) and returns, skipping the native exec steps.
    if wasm_target {
        return bundle_wasm(&out_dir);
    }

    // --- Step 2: cargo build the emitted project ---
    // CWD = the emitted crate dir, so the generated `.cargo/config.toml`
    // (`+crt-static` under a static plan) is discovered. The static plan
    // additionally selects the target triple explicitly — the config carries
    // only rustflags, never a `[build] target` pin.
    // `native_cargo` is `Some` on every path that reaches here: the wasm branch
    // returned above, and the native branch resolved cargo before emitting. The
    // fallback re-resolves rather than unwrapping so the toolchain error stays
    // typed even if the branch invariant ever changes.
    let cargo_bin = match native_cargo {
        Some(bin) => bin,
        None => toolchain::require_cargo(toolchain::ToolIntent::Run)?,
    };
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo.arg("build").current_dir(&out_dir);
    if args.quiet {
        cargo.arg("-q");
    } else {
        force_cargo_terminal_ui(&mut cargo);
    }
    if let Some(plan) = &static_plan {
        cargo.args(["--target", plan.triple.as_str()]);
    }
    let runtime_ctx = if runtime_dep && !wasm_target {
        runtime_context_for_message()
    } else {
        None
    };
    build_emitted_project(&mut cargo, "the emitted program", runtime_ctx, &out_dir)?;

    // --- Step 3: exec the emitted binary, forwarding args and exit code ---
    // The binary name is read from the emitted crate's `Cargo.toml` — the
    // same file cargo just built from, so there is ONE source of truth and
    // no independent re-derivation can drift. Falls back to `"ipe-app"` when
    // the manifest is absent or unparseable (same guarantee as `run_exec`).
    // The target directory is asked of cargo itself (`cargo metadata`) — a
    // `CARGO_TARGET_DIR` env or a user-level `[build] target-dir` pin
    // relocates the artifact, so a hardcoded `<out>/target` would exec a
    // missing or stale binary.
    let bin_name = emitted_bin_name(&out_dir);
    let mut bin = cargo_target_directory(&out_dir)?;
    if let Some(plan) = &static_plan {
        bin.push(plan.triple.as_str());
    }
    bin.push("debug");
    bin.push(&bin_name);

    // --- Step 3a: resolve the capability set and, for native code, the jail ---
    // The jail confines the emitted app to `inferred ∪ declared`. It is scoped to
    // native-bearing programs (ADR 0040): pure Ipê is structurally bounded to its
    // inferred capabilities and runs directly; only a `Rust.` crossing has
    // effects inference cannot prove, and only that is jailed. For a native
    // program a missing primitive is fail-closed (refuses unless recorded
    // consent).
    let manifest_parsed = match &manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved =
        run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;
    let union = resolved.union();
    let native = run_sandbox::is_native_bearing(&union);
    let profile = run_sandbox::build_profile(&resolved, driver)?;
    let bin_args_os: Vec<std::ffi::OsString> =
        bin_args.iter().map(std::ffi::OsString::from).collect();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        if native {
            // The scoped writable tempdir (the sole writable mount when
            // `filesystem` is absent) and the working tree (bound read-write only
            // when granted) — built only for a jailed run.
            let scoped_tmp = run_sandbox::make_scoped_tmp()?;
            let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
                path: PathBuf::from("."),
                source: e,
            })?;
            // The jail is established and `exec_in_run_jail` replaces this process
            // with the jailed app (does not return on success). On a platform with
            // no jail primitive, the fail-closed policy either refuses or (recorded
            // consent) returns to run unconfined below.
            run_sandbox::jail_and_exec(
                &profile,
                &union,
                scoped_tmp.path(),
                &working_tree,
                &bin,
                &bin_args_os,
            )?;
        }
        // Pure Ipê (structural guarantee, no jail) or a native program that
        // proceeded unconfined after the recorded-consent warning: run directly.
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&bin_args);
        let err = cmd.exec();
        Err(CliError::Io {
            path: bin,
            source: err,
        })
    }
    #[cfg(not(unix))]
    {
        if native {
            // Off Unix there is no jail (the documented refuse-gap): `jail_and_exec`
            // applies the fail-closed policy — refuse the native program, or
            // (recorded consent) return Ok to run unconfined below.
            let scoped_tmp = run_sandbox::make_scoped_tmp()?;
            let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
                path: PathBuf::from("."),
                source: e,
            })?;
            run_sandbox::jail_and_exec(
                &profile,
                &union,
                scoped_tmp.path(),
                &working_tree,
                &bin,
                &bin_args_os,
            )?;
        }
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&bin_args);
        let status = cmd.status().map_err(|e| CliError::Io {
            path: bin,
            source: e,
        })?;
        // Propagate the child's exit code.  `CliError` only models failure, so
        // a non-zero exit is surfaced as a usage-owned message; the caller
        // (main.rs) prints it to stderr and exits 1.
        if !status.success() {
            let code = status.code().unwrap_or(1);
            return Err(CliError::UsageOwned(format!(
                "{bin_name} exited with code {code}"
            )));
        }
        Ok(())
    }
}

/// `ipe exec <artifact-dir> [-- args…]` — run a built artifact, jailing it when
/// it is native-bearing.
///
/// The deployable launcher. A **native-bearing** artifact (ADR 0040) carries an
/// `ipe.profile` mirror plus a capability floor embedded in the binary, so an
/// artifact copied off the build host still runs confined: the profile is
/// *strictly parsed* (parse-fail ⇒ refuse) and refused if weaker than the
/// embedded floor — a tampered profile cannot under-isolate. A **pure** Ipê
/// artifact carries no floor (structurally bounded to its inferred capabilities)
/// and runs directly. A bare `./ipe-app` invocation is the documented, deliberate
/// deployer escape (the raw binary opts out of the jail); this path does not.
///
/// # Errors
/// [`CliError::UsageOwned`] on a missing binary, a native artifact whose profile
/// is missing/tampered, a refused floor check, or a fail-closed jail refusal.
pub fn run_exec(rest: &[String]) -> Result<(), CliError> {
    // Split `<dir> [-- args…]`.
    let (dir_arg, app_args) = rest
        .iter()
        .position(|a| a == "--")
        .map_or((rest, &[][..]), |i| {
            (
                rest.get(..i).unwrap_or(&[]),
                rest.get(i + 1..).unwrap_or(&[]),
            )
        });
    let dir = dir_arg
        .first()
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);
    if !dir.is_dir() {
        return Err(CliError::UsageOwned(format!(
            "ipe exec: no artifact directory at {}",
            dir.display()
        )));
    }

    // Locate the emitted binary (cargo metadata honours a relocated target dir).
    // The binary name matches the emitted crate's `[package] name`, read from
    // the artifact dir's `Cargo.toml`. Falls back to `"ipe-app"` when the
    // manifest is absent or the name cannot be parsed.
    let exec_bin_name = emitted_bin_name(&dir);
    let mut bin = cargo_target_directory(&dir)?;
    bin.push("debug");
    bin.push(&exec_bin_name);
    if !bin.is_file() {
        return Err(CliError::UsageOwned(format!(
            "ipe exec: no built binary at {} — run `ipe build` first",
            bin.display()
        )));
    }

    let app_args_os: Vec<std::ffi::OsString> =
        app_args.iter().map(std::ffi::OsString::from).collect();

    // A native-bearing artifact carries an embedded capability floor and is
    // jailed; a pure Ipê artifact carries none and runs directly (ADR 0040).
    if run_sandbox::artifact_is_native(&bin)? {
        let profile_path = dir.join("ipe.profile");
        if !profile_path.is_file() {
            return Err(CliError::UsageOwned(format!(
                "ipe exec: {} embeds a capability floor but carries no ipe.profile — the artifact \
                 is incomplete or tampered; refusing to run native code without its jail profile",
                bin.display()
            )));
        }
        // Strictly parse the profile and verify it against the embedded floor.
        let profile = run_sandbox::load_and_verify_artifact(&profile_path, &bin)?;

        // The union for the consent/refusal policy is reconstructed from the
        // profile's granted axes (the deployed artifact has no source to
        // re-infer); the floor's presence already established it is native-bearing.
        let mut union = run_sandbox::profile_axes(&profile);
        union.insert(ipe_ir::Capability::NativeFfi);
        let scoped_tmp = run_sandbox::make_scoped_tmp()?;
        let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
            path: PathBuf::from("."),
            source: e,
        })?;

        run_sandbox::jail_and_exec(
            &profile,
            &union,
            scoped_tmp.path(),
            &working_tree,
            &bin,
            &app_args_os,
        )?;
        // Returns only if recorded consent permitted an unconfined run; fall
        // through to the direct exec below.
    }

    // Pure Ipê artifact, or native that proceeded after the recorded-consent
    // warning: run directly.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(app_args);
        let err = cmd.exec();
        Err(CliError::Io {
            path: bin,
            source: err,
        })
    }
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&bin)
            .args(app_args)
            .status()
            .map_err(|e| CliError::Io {
                path: bin.clone(),
                source: e,
            })?;
        if !status.success() {
            return Err(CliError::UsageOwned(format!(
                "{exec_bin_name} exited with code {}",
                status.code().unwrap_or(1)
            )));
        }
        Ok(())
    }
}

/// Read the `[package] name` from an emitted project's `Cargo.toml` so
/// `ipe run` / `ipe exec` / `ipe test` locate the correct binary. Falls back
/// to `"ipe-app"` when the manifest is absent or unparseable — never panics.
pub fn emitted_bin_name(crate_dir: &Path) -> String {
    let manifest = crate_dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return "ipe-app".to_owned();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return value.to_owned();
                }
            }
        }
    }
    "ipe-app".to_owned()
}

/// The target directory cargo will use for a build with CWD = `crate_dir`,
/// resolved by cargo itself (`cargo metadata`) so every relocation source —
/// `CARGO_TARGET_DIR`, a user-level `[build] target-dir` pin, a config in an
/// ancestor dir — is honoured instead of guessed at.
pub fn cargo_target_directory(crate_dir: &Path) -> Result<PathBuf, CliError> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(crate_dir)
        .output()
        .map_err(|e| CliError::Io {
            path: crate_dir.to_path_buf(),
            source: e,
        })?;
    if !output.status.success() {
        return Err(CliError::UsageOwned(format!(
            "cargo metadata failed in {}: {}",
            crate_dir.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        CliError::UsageOwned(format!("cargo metadata emitted unparseable JSON: {e}"))
    })?;
    meta.get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::UsageOwned("cargo metadata reported no target_directory".to_owned())
        })
}

/// `ipe explain` has been folded into `ipe doc`.
///
/// Invoking `ipe explain` emits a pointer to `ipe doc` and returns a usage
/// error so the dispatcher shows the `ipe doc` help page. The command is no
/// longer advertised; the COMMANDS registry entry was removed.
pub fn run_explain(_rest: &[String]) -> Result<(), CliError> {
    Err(CliError::UsageOwned(
        "`ipe explain` has moved: use `ipe doc <key>` instead\n\
         \n\
         Examples:\n\
           ipe doc IPE-L0107   look up a diagnostic code\n\
           ipe doc case        look up a language construct\n\
           ipe doc List.map    look up a stdlib symbol\n\
           ipe doc version     look up a command"
            .to_owned(),
    ))
}

/// `ipe fix <path>` — apply machine-applicable fixes to the source file.
/// Default is interactive per-edit confirmation;
/// `--yes` is durable authorization to apply every machine-applicable edit.
pub fn run_fix(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_fix(rest)?;
    apply_fixes_cmd(
        &PathBuf::from(&args.entry),
        args.auto,
        &mut std::io::stdout(),
    )?;
    Ok(())
}

// ===========================================================================
// `explain` — code index, lookup, and did-you-mean
// ===========================================================================

/// The one-line-per-code index: `<CODE>  <title>\n`, in taxonomy order.
#[must_use]
pub fn code_index() -> String {
    let mut s = String::new();
    for &c in ALL_CODES {
        s.push_str(c.as_str());
        s.push_str("  ");
        s.push_str(title(c));
        s.push('\n');
    }
    s
}

/// Resolve a (case-insensitive) code string to its embedded explain page.
///
/// The input is trimmed and upper-cased before matching, so `ipe-t0001` and
/// `IPE-T0001` both resolve.
///
/// # Errors
/// Returns [`CliError::UnknownCode`] (carrying a deterministic did-you-mean
/// list) when the string is not a taxonomy code.
pub fn explain_lookup(input: &str) -> Result<&'static str, CliError> {
    let canonical = input.trim().to_ascii_uppercase();
    for &c in ALL_CODES {
        if c.as_str() == canonical {
            // `explain_page` is `Some` for every `ALL_CODES` member; the `None`
            // arm is surfaced as a typed error rather than a panic.
            return explain_page(c).map_or_else(
                || {
                    Err(CliError::UnknownCode {
                        input: input.trim().to_owned(),
                        suggestions: Vec::new(),
                    })
                },
                Ok,
            );
        }
    }
    Err(CliError::UnknownCode {
        input: input.trim().to_owned(),
        suggestions: did_you_mean_codes(&canonical),
    })
}

/// The known command closest to `attempted` by Levenshtein distance, within a
/// small edit threshold — the "maybe ...?" hint for a mistyped command. `None`
/// when nothing is close enough, so a wildly different token gets only the help
/// screen, not a misleading guess.
pub fn nearest_command(attempted: &str) -> Option<&'static str> {
    help::command_names()
        .into_iter()
        .map(|name| (levenshtein(attempted, name), name))
        .filter(|&(dist, _)| dist <= 3)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, name)| name)
}

/// The closest known codes to `canonical` (already upper-cased), ranked by
/// `(Levenshtein, code)` and filtered to a small edit distance. Deterministic.
pub fn did_you_mean_codes(canonical: &str) -> Vec<&'static str> {
    let mut scored: Vec<(usize, &'static str)> = ALL_CODES
        .iter()
        .map(|&c| (levenshtein(canonical, c.as_str()), c.as_str()))
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .filter(|&(dist, _)| dist <= 3)
        .take(3)
        .map(|(_, name)| name)
        .collect()
}

/// Classic two-row Levenshtein edit distance. Uses no slice indexing (only
/// `get`/`push`/`last`), so it cannot panic.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur: Vec<usize> = Vec::with_capacity(b.len().saturating_add(1));
        cur.push(i.saturating_add(1));
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let del = prev.get(j.saturating_add(1)).copied().unwrap_or(usize::MAX);
            let ins = cur.get(j).copied().unwrap_or(usize::MAX);
            let sub = prev.get(j).copied().unwrap_or(usize::MAX);
            cur.push(
                del.saturating_add(1)
                    .min(ins.saturating_add(1))
                    .min(sub.saturating_add(cost)),
            );
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

// ===========================================================================
// `--emit-ir` — pretty-print the lowered IR
// ===========================================================================

/// Run parse → canon → types → lower and return the pretty-printed IR tree,
/// stopping before codegen.
///
/// # Errors
/// Returns [`CliError::Pipeline`] when the compiler rejects the program, or
/// [`CliError::Io`] when the entry file cannot be read.
pub fn emit_ir_text(entry: &Path) -> Result<String, CliError> {
    let (db, program) = lower_entry_via_graph(entry)?;
    let interner = ipe_db::Db::interner(&db).lock();
    Ok(ipe_ir::pretty(&program, &interner))
}

// ===========================================================================
// `capabilities` — report / verify a program's inferred capability set
// ===========================================================================

/// The whole set of security capabilities a program discloses: the kernel-derived
/// set [`ipe_lower::program_capabilities`] infers from the lowered program, PLUS
/// [`ipe_ir::Capability::CustomElement`] whenever the program constructs any
/// `customElement` handle.
///
/// The custom-element axis is derived from the SAME walk emission serves from —
/// [`ipe_canon::custom_element_gate::collect_widget_files`] over the pre-DCE
/// `linked` module — so the served-asset set and the disclosed-capability set are
/// one set by construction. A handle that is constructed but never mounted (and
/// so lowers to a capability-free leaf that DCE may drop) still ships its browser
/// JS through the emitted `widget_assets::register`, and this derivation discloses
/// it regardless of the lowered program's reachability. `collect_widget_files`
/// walks the whole linked program, so a handle constructed in an imported module
/// is disclosed transitively.
///
/// This is the single inference point every capability consumer routes through —
/// the report, the declared-set verify, package inference, and index admission —
/// so none of them can disclose a different set than the emitter serves.
pub fn capabilities_including_served_widgets(
    db: &dyn ipe_db::Db,
    root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
    program: &ipe_ir::Program,
) -> std::collections::BTreeSet<ipe_ir::Capability> {
    let mut caps = ipe_lower::program_capabilities(program);
    if program_constructs_a_widget(db, root, entry_file) {
        caps.insert(ipe_ir::Capability::CustomElement);
    }
    caps
}

/// True when the linked program constructs at least one `customElement` handle —
/// i.e. the emitter serves at least one widget asset for it. Reuses the exact
/// [`ipe_canon::custom_element_gate::collect_widget_files`] walk emission uses, so
/// the serve decision and this disclose decision are the same decision.
///
/// A program whose linking fails has no served widget (nothing is emitted), so a
/// link failure conservatively contributes no widget disclosure here; the failing
/// pipeline surfaces its own diagnostic through the caller's own lowering.
pub fn program_constructs_a_widget(
    db: &dyn ipe_db::Db,
    root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
) -> bool {
    ipe_db::linked_program(db, root, entry_file).is_ok_and(|linked| {
        !ipe_canon::custom_element_gate::collect_widget_files(&linked.module).is_empty()
    })
}

/// Lower a single `.ipe` entry through the SAME injection-aware source-graph
/// pipeline the build path uses, returning the owning database (its interner
/// backs any downstream `ipe_ir::pretty`) and the lowered program.
///
/// This routes through sibling discovery + compiled-source stdlib injection +
/// the salsa `lower_program` query rather than a bare single-module
/// parse→canon→infer→lower. Without injection an entry importing a
/// compiled-source stdlib module (e.g. `Ipe.Test`) fails name resolution with
/// IPE-N0004 even though a real `ipe build` of the same program succeeds — the
/// analysis surfaces (`ipe capabilities`, `ipe build --emit-ir`) must resolve
/// such a module identically to the build.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
pub fn lower_entry_via_graph(
    entry: &Path,
) -> Result<(ipe_db::IpeDatabase, std::sync::Arc<ipe_ir::Program>), CliError> {
    let graph = build_source_graph(entry)?;
    let program = graph.run_attributed(entry, |db, root, file| {
        ipe_db::lower_program(db, root, file)
    })?;
    Ok((graph.db, program))
}

/// The salsa inputs one analysis needs: the owning database, the whole-program
/// source root, and the entry module's [`ipe_db::SourceFile`] handle — the
/// product of sibling discovery + compiled-source stdlib injection shared by
/// every single-entry analysis path.
pub struct SourceGraph {
    pub(crate) db: ipe_db::IpeDatabase,
    pub(crate) source_root: ipe_db::SourceRoot,
    pub(crate) entry_file: ipe_db::SourceFile,
    /// The whole module set (path → (file, src)) — every module a diagnostic
    /// span may index into, so a rejecting query can be framed against the
    /// source that OWNS the span rather than the entry file (the caret bug).
    sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    /// The entry module's dotted path — its `(file, src)` is the fallback frame
    /// for a homeless / dummy-span diagnostic.
    entry_module_path: Vec<String>,
}

impl SourceGraph {
    /// Run the per-module canonicalisation blame loop, then map a rejecting
    /// query's `(diag, home)` to the source file that OWNS it — the SAME
    /// attribution the build path uses (`attribute_canon_errors` +
    /// `attribute_post_link_error`), so `ipe type-check` and every other analysis
    /// surface frame a given diagnostic against the identical source as
    /// `ipe build`.
    ///
    /// A canon error (e.g. IPE-N0020) surfaces from the blame loop already
    /// framed against its own module; only a post-link error reaches the
    /// `run_query` closure, where its `home` (or the byte-offset heuristic over
    /// the linked program) selects the owning source.
    ///
    /// # Errors
    /// [`CliError::Pipeline`] carrying the first compiler diagnostic; the query
    /// closure's own error otherwise.
    pub(crate) fn run_attributed<T>(
        &self,
        blame_path: &Path,
        run_query: impl FnOnce(
            &ipe_db::IpeDatabase,
            ipe_db::SourceRoot,
            ipe_db::SourceFile,
        ) -> Result<T, (Diagnostic, Vec<ipe_intern::Symbol>)>,
    ) -> Result<T, CliError> {
        attribute_canon_errors(
            &self.db,
            self.source_root,
            &self.sources,
            self.entry_file,
            blame_path,
        )?;
        run_query(&self.db, self.source_root, self.entry_file).map_err(|(diag, home)| {
            // Canon succeeded, so the linked program exists; use it for the
            // byte-offset fallback when `home` is empty. A link failure here
            // (empty home, no linked program) frames against the entry file.
            let entry = self
                .sources
                .get(&self.entry_module_path)
                .cloned()
                .unwrap_or_else(|| (blame_path.to_path_buf(), String::new()));
            let interner = ipe_db::Db::interner(&self.db).clone();
            let home_to_source = home_to_source_map(&interner, &self.sources);
            match ipe_db::linked_program(&self.db, self.source_root, self.entry_file) {
                Ok(linked) => {
                    attribute_post_link_error(&linked.module, &home_to_source, &entry, diag, &home)
                }
                Err(link_diag) => {
                    // A link error has no linked program to scan; frame the
                    // ORIGINAL query diagnostic (not the link error) against the
                    // home module if known, else the entry file.
                    let (file, src) = if home.is_empty() {
                        entry
                    } else {
                        home_to_source.get(&home).cloned().unwrap_or(entry)
                    };
                    // `link_diag` is discarded: the query's own diagnostic is the
                    // one the user asked about; a link error would already have
                    // surfaced from the canon blame loop or a build.
                    let _ = link_diag;
                    CliError::Pipeline {
                        file,
                        src,
                        diag: Box::new(diag),
                    }
                }
            }
        })
    }
}

/// Build the injection-aware whole-program source graph for a single `.ipe`
/// entry: discover its siblings, inject the compiled-source stdlib closure, and
/// create the salsa source root. Shared by [`lower_entry_via_graph`] and
/// [`typecheck_entry_via_graph`] so the build, capabilities, `--emit-ir`, and
/// `check` surfaces all resolve the same module set — a compiled-source stdlib
/// import (e.g. `Ipe.Test`) resolves identically across every one.
///
/// # Errors
/// [`CliError::Pipeline`] when the entry does not parse; [`CliError::Io`] on any
/// filesystem failure; [`CliError::Usage`] if the entry is not in the built map.
pub fn build_source_graph(entry: &Path) -> Result<SourceGraph, CliError> {
    let mut collected = collect_entry_and_siblings(entry)?;
    let injected =
        project::inject_compiled_std_closure(&mut collected.sources, &mut collected.discovered);
    // The SAME FFI seam the build runs: without it, a project with installed
    // crates (or asserted `Rust.Ffi.call` definitions) has no `Rust.*`
    // interface modules here, so `ipe type-check` / `ipe capabilities` /
    // `--emit-ir` would refuse a program the build accepts.
    let ffi_injected = ffi::prepare_ffi(&mut collected.sources, entry)?.injected;

    let db = ipe_db::IpeDatabase::new();
    let source_root = create_source_root(&db, &collected.sources, &injected, &ffi_injected);
    let Some(entry_file) = source_root
        .files(&db)
        .get(&collected.entry_module_path)
        .copied()
    else {
        return Err(CliError::Usage("internal: entry module not in source map"));
    };

    Ok(SourceGraph {
        db,
        source_root,
        entry_file,
        sources: collected.sources,
        entry_module_path: collected.entry_module_path,
    })
}

/// Collect the USER `.ipe` source texts a build sees, for the `.Unsafe`-import
/// scan. A manifest project reads every discovered module under its source root
/// (the same whole-tree posture package-capability inference takes); a
/// single-file entry reads the entry plus its imported siblings.
///
/// Fail-closed: any unreadable module causes an immediate `Err` so the
/// acknowledgment gate never operates on a partial source set.
///
/// # Errors
/// [`CliError::Io`] when any discovered module cannot be read.
pub fn user_sources_for_unsafe_scan(
    manifest: Option<&Path>,
    entry: &Path,
) -> Result<Vec<String>, CliError> {
    if let Some(mpath) = manifest
        && let Ok(m) = project::parse_manifest(mpath)
        && let Ok(discovered) = project::discover_modules(&m.src_root)
    {
        return discovered
            .iter()
            .map(|d| {
                crate::io_bounded::read_to_string_capped(
                    &d.path,
                    crate::io_bounded::SOURCE_READ_CAP,
                )
            })
            .collect::<Result<Vec<_>, _>>();
    }
    // Single file (or a manifest that failed to parse — the build will surface
    // that error itself): the entry and its siblings.
    match collect_entry_and_siblings(entry) {
        Ok(collected) => Ok(collected
            .sources
            .into_values()
            .map(|(_, src)| src)
            .collect()),
        Err(_) => {
            crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)
                .map(|src| vec![src])
        }
    }
}

/// The build-time acknowledgment gate for `Ipe.<M>.Unsafe` escape-hatch imports,
/// shared by `ipe build` and `ipe run`.
///
/// Resolves the program's inferred capabilities the same way the sandbox does,
/// and — only when the disclosed `unsafe` capability is present — surfaces the
/// risk and requires consent (the `--accept-risks` flag, a `[capabilities]
/// accept = ["unsafe"]` manifest token, or an interactive `y`). A non-interactive
/// build without pre-acceptance fails closed (`IPE-S0001`); it never blocks on a
/// prompt. A program with no `.Unsafe` import is untouched.
///
/// # Errors
/// [`CliError::UsageOwned`] (`IPE-S0001`) when consent is required but absent;
/// the capability-resolution errors it composes.
pub fn acknowledge_unsafe_imports(
    manifest_parsed: Option<&project::ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
    accept_risks_flag: bool,
) -> Result<(), CliError> {
    let resolved = run_sandbox::resolve_for_run(manifest_parsed, manifest_path, entry)?;
    // Short-circuit before any source read when the disclosed capability is
    // absent — the safe path does no work at all.
    if !resolved.inferred.contains(&ipe_ir::Capability::Unsafe) {
        return Ok(());
    }
    let sources = user_sources_for_unsafe_scan(manifest_path, entry)?;
    let via = unsafe_ack::unsafe_modules_in_sources(sources.iter().map(String::as_str));
    let manifest_accept = manifest_parsed
        .map(|m| m.capabilities_accept.clone())
        .unwrap_or_default();
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr().lock();
    unsafe_ack::gate(
        &resolved.inferred,
        accept_risks_flag,
        &manifest_accept,
        &via,
        unsafe_ack::is_interactive(),
        &mut stdin,
        &mut stderr,
    )
}

/// The app-boundary web-capability consent gate, shared by `ipe build` and
/// `ipe run` and invoked right after the `.Unsafe` acknowledgment.
///
/// Resolves the program's inferred capabilities the same way the sandbox does; if
/// any disclosed `js-port:<axis>` web capability is present, it demands that the
/// top-level app's `[capabilities] accept` set grant it. An ungranted (or
/// un-attributable) web axis is a fail-closed, typed refusal naming the disclosing
/// module — it never prompts and never composes a dependency's own grant. A
/// program that reaches no web capability is untouched.
///
/// # Errors
/// [`CliError::UsageOwned`] (`IPE-S0002`) when a disclosed web axis is ungranted;
/// the capability-resolution errors it composes.
pub fn gate_web_consent(
    manifest_parsed: Option<&project::ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<(), CliError> {
    let resolved = run_sandbox::resolve_for_run(manifest_parsed, manifest_path, entry)?;
    // Short-circuit before any source read when no web axis is disclosed.
    if !resolved
        .inferred
        .iter()
        .any(|c| matches!(c, ipe_ir::Capability::JsPort(_)))
    {
        return Ok(());
    }
    // Provenance over the whole module set (app + siblings + any dep modules the
    // infer path reads), keyed on the module path so the refusal names the
    // disclosing module. Total by construction: an inferred axis that no source
    // attributes is refused as un-attributable, never dropped.
    let named_sources = named_sources_for_web_scan(manifest_path, entry)?;
    let provenance = web_consent::WebAxisProvenance::from_sources(
        named_sources
            .iter()
            .map(|(name, src)| (name.as_str(), src.as_str())),
    );
    let granted = manifest_parsed
        .map(|m| m.capabilities_accept.clone())
        .unwrap_or_default();
    web_consent::gate(&resolved.inferred, &granted, &provenance)
}

/// The app-boundary native-crossing consent gate, shared by `ipe build` and
/// `ipe run` and invoked right after the web-capability consent.
///
/// Resolves the program's inferred capabilities the same way the sandbox does; if
/// the disclosed `native-ffi` capability is present (any `Rust.` crossing), it
/// demands that the top-level app's `[capabilities] declared` set grant it. An
/// ungranted (or un-attributable) crossing is a fail-closed, typed refusal naming
/// the disclosing `Rust.<Crate>` module — it never prompts and never composes a
/// dependency's own grant. A program that crosses into no native code is
/// untouched.
///
/// The grant surface is `[capabilities] declared` (a package's *own* effects, the
/// same set `verify_capabilities` reconciles), not `accept` (a pre-acceptance of
/// a hazard the build would prompt about): a native crossing is a package's own
/// declared effect, so it belongs on the `declared` axis. The runtime jail
/// CONTAINS the crossing's opaque effects regardless; this gate is the consent
/// half — the crossing must be granted before the (costly) emit + cargo build.
///
/// # Errors
/// [`CliError::UsageOwned`] (`IPE-S0003`) when the disclosed native crossing is
/// ungranted; the capability-resolution errors it composes.
pub fn gate_native_ffi_consent(
    manifest_parsed: Option<&project::ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<(), CliError> {
    let resolved = run_sandbox::resolve_for_run(manifest_parsed, manifest_path, entry)?;
    // Short-circuit before any source read when no native crossing is disclosed.
    if !resolved.inferred.contains(&ipe_ir::Capability::NativeFfi) {
        return Ok(());
    }
    // Provenance over the whole module set (app + siblings + any dep modules the
    // infer path reads), keyed on the crate so the refusal names the disclosing
    // `Rust.<Crate>` import. Total by construction: an inferred crossing that no
    // source attributes is refused as un-attributable, never dropped.
    let named_sources = named_sources_for_web_scan(manifest_path, entry)?;
    let provenance = native_ffi_consent::NativeCrossingProvenance::from_sources(
        named_sources
            .iter()
            .map(|(name, src)| (name.as_str(), src.as_str())),
    );
    let granted = manifest_parsed
        .map(|m| m.capabilities.clone())
        .unwrap_or_default();
    native_ffi_consent::gate(&resolved.inferred, &granted, &provenance)
}

/// Collect `(dotted-module-name, source)` pairs spanning the app entry and its
/// siblings (and, when a manifest is present, every discovered package module),
/// for the web-axis provenance scan. Falls back to the bare entry when sibling
/// discovery fails, exactly as the `.Unsafe` scan does.
pub fn named_sources_for_web_scan(
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<Vec<(String, String)>, CliError> {
    if let Some(mpath) = manifest_path
        && let Ok(manifest) = project::parse_manifest(mpath)
    {
        let discovered = project::discover_modules(&manifest.src_root)?;
        let mut out = Vec::with_capacity(discovered.len());
        for m in &discovered {
            let src = crate::io_bounded::read_to_string_capped(
                &m.path,
                crate::io_bounded::SOURCE_READ_CAP,
            )?;
            out.push((m.module_path.join("."), src));
        }
        return Ok(out);
    }
    match collect_entry_and_siblings(entry) {
        Ok(collected) => Ok(collected
            .sources
            .into_iter()
            .map(|(path, (_, src))| (path.join("."), src))
            .collect()),
        Err(_) => {
            crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)
                .map(|src| vec![(entry.display().to_string(), src)])
        }
    }
}

/// Type-check a single `.ipe` entry through the SAME injection-aware
/// source-graph pipeline the build path uses, stopping at type-checking: it
/// demands the `typecheck` query (parse → canon → link → HM infer) and never
/// lowers to IR or emits Rust. This is what `ipe type-check` runs.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
pub fn typecheck_entry_via_graph(entry: &Path) -> Result<(), CliError> {
    let graph = build_source_graph(entry)?;
    graph.run_attributed(entry, |db, root, file| {
        // Type-check first so an ordinary type error surfaces ahead of the
        // decoder-direction gate; then run the SAME IPE-N0040 gate the build
        // path runs (`gate_decoder_pipelines`) over the linked module, so
        // `ipe type-check` rejects the hand-nested decoder footgun for the
        // earliest possible feedback rather than deferring it to `ipe build`.
        // `linked_program` re-demands the memos `typecheck` just populated.
        ipe_db::typecheck(db, root, file)?;
        let linked = ipe_db::linked_program(db, root, file).map_err(|d| (d, Vec::new()))?;
        gate_decoder_pipelines(&linked.module)
    })
}
