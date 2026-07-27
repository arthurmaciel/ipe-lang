//! Typed command-line argument parsing — the single validation boundary for
//! every subcommand's optional flags (parse, don't validate).
//!
//! Each subcommand parses its raw `&[String]` tail into a typed value in which
//! an invalid combination CANNOT be constructed: mutually-exclusive options are
//! an enum rather than two independent booleans, an option that requires another
//! is a variant that carries it, and a value option that may appear at most once
//! is rejected loudly on a second occurrence rather than silently last-writing.
//!
//! The parse returns `Ok(TypedArgs)` or a precise [`CliError::Usage`] /
//! [`CliError::UsageOwned`] naming exactly what is wrong — never a panic, never a
//! silently-ignored flag. `run_build` / `run_run` / `run_watch` / `run_fix` /
//! `run_fmt` consume the typed value; the scattered ad-hoc checks they used to
//! carry are folded into these parses.

use crate::CliError;
use crate::build_plan::{AllocatorChoice, StaticRequestLayer};

/// How a data-producing command renders its result.
///
/// The default is the human-friendly form; `--plain` and `--json` are the two
/// machine forms, and they are mutually exclusive — a request for both is a usage
/// error rather than a silent last-wins, so a caller never gets a format it did
/// not ask for.
///
/// Only the commands that emit machine-consumable data (`capabilities`, `diff`,
/// `version`, `explain` with no code) accept these flags; `run` / `build` /
/// `init` / `watch` / `fix` / `fmt` and `--help` do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// The default: human-friendly, guttered, coloured on a terminal.
    #[default]
    Human,
    /// `--plain` — unstyled, flush-left, one record per line (pipe-friendly).
    Plain,
    /// `--json` — a stable documented schema (machine-parseable).
    Json,
}

/// Recognise `--plain` / `--json` in `flag`, folding the choice into `slot`.
/// Returns `Ok(true)` when `flag` was an output-format flag (consumed),
/// `Ok(false)` when it is some other token the caller must handle.
///
/// The two forms are mutually exclusive: a second, different format flag — or a
/// repeat that would re-assert the same one — is rejected here so `--plain
/// --json` can never resolve to a single silent winner.
///
/// # Errors
/// [`CliError::UsageOwned`] when a format flag is given after a different one,
/// naming `command` so the message points at the misused command.
fn consume_format(
    slot: &mut Option<OutputFormat>,
    flag: &str,
    command: &str,
) -> Result<bool, CliError> {
    let requested = match flag {
        "--plain" => OutputFormat::Plain,
        "--json" => OutputFormat::Json,
        _ => return Ok(false),
    };
    match slot {
        None => {
            *slot = Some(requested);
            Ok(true)
        }
        Some(existing) if *existing == requested => Err(CliError::UsageOwned(format!(
            "ipe {command}: {flag} given more than once"
        ))),
        Some(_) => Err(CliError::UsageOwned(format!(
            "ipe {command}: --plain and --json are mutually exclusive"
        ))),
    }
}

/// Parse the shared output-format flags out of a command's argument tail.
///
/// Returns the chosen [`OutputFormat`] (defaulting to [`OutputFormat::Human`])
/// and the positional tokens with the format flags removed.
///
/// # Errors
/// [`CliError::UsageOwned`] on `--plain --json` together, or a repeated format
/// flag.
pub fn split_format<'a>(
    rest: &'a [String],
    command: &str,
) -> Result<(OutputFormat, Vec<&'a str>), CliError> {
    let mut format: Option<OutputFormat> = None;
    let mut positional: Vec<&'a str> = Vec::new();
    for arg in rest {
        if !consume_format(&mut format, arg, command)? {
            positional.push(arg);
        }
    }
    Ok((format.unwrap_or_default(), positional))
}

/// Set a value option that may appear at most once, rejecting a duplicate with a
/// specific message rather than silently overwriting the earlier value.
///
/// # Errors
/// [`CliError::UsageOwned`] when `slot` already holds a value.
fn set_once<T>(slot: &mut Option<T>, value: T, flag: &str, command: &str) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::UsageOwned(format!(
            "ipe {command}: {flag} given more than once"
        )));
    }
    *slot = Some(value);
    Ok(())
}

/// Pull the value that follows a value-taking flag, or fail with a message
/// naming the flag whose argument is missing (rather than the generic synopsis).
///
/// # Errors
/// [`CliError::UsageOwned`] when the iterator is exhausted.
fn take_value(
    it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    flag: &str,
    command: &str,
) -> Result<String, CliError> {
    it.next()
        .cloned()
        .ok_or_else(|| CliError::UsageOwned(format!("ipe {command}: {flag} needs a value")))
}

/// Take the leading positional entry, if any: the first token, but ONLY when it
/// is not a flag. A leading `--flag` (e.g. `ipe build --emit-ir`) leaves the
/// entry unset — so the flag is parsed as a flag rather than silently swallowed
/// as a bogus entry path — and the caller falls back to its project-aware
/// default. Advances `it` past the entry only when one is taken.
fn take_leading_entry(
    it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
) -> Option<String> {
    match it.peek() {
        Some(first) if !first.starts_with('-') => it.next().cloned(),
        _ => None,
    }
}

/// The static-request flags shared by `build` and `run`, parsed into a typed
/// layer. Each value flag is rejected on a second occurrence; the boolean flags
/// are idempotent (a repeat is harmless and stays accepted).
///
/// This carries `--target` as a raw string still (its closed form is
/// [`crate::build_plan::StaticTriple`], resolved during static-plan resolution
/// together with the env / `ipe.toml` layers), but `--allocator` is parsed into
/// the closed [`AllocatorChoice`] enum at this boundary so an out-of-set name
/// can never reach resolution.
#[derive(Default)]
struct StaticFlags {
    static_flag: bool,
    target: Option<String>,
    allocator: Option<AllocatorChoice>,
    allow_slow_allocator: bool,
}

impl StaticFlags {
    /// Consume `flag` as a static-request flag, pulling its value from `it` where
    /// it takes one. Returns `Ok(false)` when `flag` is not a static-request flag
    /// (so the caller can try its own flags next).
    ///
    /// # Errors
    /// [`CliError::UsageOwned`] on a missing value, a duplicate value flag, or an
    /// allocator name outside the closed set.
    fn consume(
        &mut self,
        flag: &str,
        it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
        command: &str,
    ) -> Result<bool, CliError> {
        match flag {
            "--static" => self.static_flag = true,
            "--target" => set_once(
                &mut self.target,
                take_value(it, "--target", command)?,
                "--target",
                command,
            )?,
            "--allocator" => {
                let raw = take_value(it, "--allocator", command)?;
                let choice = AllocatorChoice::parse(&raw)
                    .map_err(|refusal| CliError::UsageOwned(refusal.to_string()))?;
                set_once(&mut self.allocator, choice, "--allocator", command)?;
            }
            "--allow-slow-allocator" => self.allow_slow_allocator = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The CLI precedence layer these flags express.
    fn layer(self) -> StaticRequestLayer {
        StaticRequestLayer {
            static_build: self.static_flag.then_some(true),
            target: self.target,
            allocator: self.allocator,
            allow_slow_allocator: self.allow_slow_allocator.then_some(true),
        }
    }
}

/// The compilation surface `ipe build` produces — dump the lowered IR, or emit
/// a native/wasm project.
///
/// Making this an enum is what forbids `--emit-ir --out X` / `--emit-ir
/// --static` / `--emit-ir --target wasm`: the IR-dump variant carries no emit
/// fields at all, so an emit flag combined with `--emit-ir` has nowhere to land
/// and is rejected at parse time rather than silently ignored by an early
/// return.
#[derive(Debug, PartialEq, Eq)]
pub enum BuildMode {
    /// `--emit-ir` — pretty-print the lowered IR to stdout and stop before
    /// codegen. Composes with nothing but `--fix` (a pre-pass over the source).
    EmitIr,
    /// The ordinary path — emit a Cargo project. Carries the emit-affecting
    /// options that `--emit-ir` cannot take.
    Emit {
        /// `--out <dir>` — where to write the emitted project.
        out: Option<String>,
        /// `--target wasm` selects the browser target (a distinct compilation
        /// axis), captured here so `--static` / `--allocator` cannot also apply.
        wasm: bool,
        /// The native static-request layer (`--static` / `--target <triple>` /
        /// `--allocator` / `--allow-slow-allocator`). Empty under `--target wasm`.
        static_layer: StaticRequestLayer,
    },
}

/// Fully-parsed `ipe build` arguments.
pub struct BuildArgs {
    /// The positional entry (`None` → project-aware default).
    pub entry: Option<String>,
    /// `--runtime <dir>` — vendor the runtime from here.
    pub runtime: Option<String>,
    /// `--fix` — apply machine-applicable fixes before building.
    pub fix: bool,
    /// `--optimize` — a PRODUCTION build. Rejects any development-only
    /// `Debug.*` escape hatch (IPE-L0140). Mirrors Elm's `--optimize` gate on
    /// its own `Debug` module. Does not compose with `--emit-ir` (which stops
    /// before the emit demand the gate runs on).
    pub production: bool,
    /// The emit surface (IR dump vs project emit).
    pub mode: BuildMode,
}

/// Parse `ipe build`'s argument tail.
///
/// Rejects, at this single boundary: `--emit-ir` combined with any
/// emit-affecting flag (`--out` / `--static` / `--target` / `--allocator` /
/// `--allow-slow-allocator`); `--target wasm` combined with `--static` /
/// `--allocator` / `--allow-slow-allocator` (native-only flags); a duplicate
/// value flag; an allocator name outside the closed set; and an unknown flag.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_build(rest: &[String]) -> Result<BuildArgs, CliError> {
    let mut it = rest.iter().peekable();
    let entry = take_leading_entry(&mut it);

    let mut out: Option<String> = None;
    let mut runtime: Option<String> = None;
    let mut emit_ir = false;
    let mut fix = false;
    let mut production = false;
    let mut static_flags = StaticFlags::default();
    while let Some(flag) = it.next() {
        if static_flags.consume(flag, &mut it, "build")? {
            continue;
        }
        match flag.as_str() {
            "--out" => set_once(
                &mut out,
                take_value(&mut it, "--out", "build")?,
                "--out",
                "build",
            )?,
            "--runtime" => set_once(
                &mut runtime,
                take_value(&mut it, "--runtime", "build")?,
                "--runtime",
                "build",
            )?,
            "--emit-ir" => emit_ir = true,
            "--fix" => fix = true,
            "--optimize" => production = true,
            other => {
                return Err(CliError::UsageOwned(format!(
                    "ipe build: unknown flag `{other}`"
                )));
            }
        }
    }

    // `--target wasm` is a compilation-target axis, not a static-link triple; it
    // never enters static-request resolution and does not compose with the
    // native static flags.
    let wasm = static_flags.target.as_deref() == Some("wasm");
    if wasm && (static_flags.static_flag || static_flags.allocator.is_some()) {
        return Err(CliError::Usage(
            "--static / --allocator are native-target flags; they do not compose with --target wasm",
        ));
    }
    if wasm && static_flags.allow_slow_allocator {
        return Err(CliError::Usage(
            "--allow-slow-allocator is a native-target flag; it does not compose with --target wasm",
        ));
    }

    let mode = if emit_ir {
        // `--emit-ir` stops before codegen, so every emit-affecting flag is
        // meaningless with it. Reject rather than silently ignore (the old early
        // return dropped them without a word).
        if out.is_some() {
            return Err(CliError::Usage("--emit-ir does not compose with --out"));
        }
        if static_flags.static_flag {
            return Err(CliError::Usage("--emit-ir does not compose with --static"));
        }
        if static_flags.target.is_some() {
            return Err(CliError::Usage("--emit-ir does not compose with --target"));
        }
        if static_flags.allocator.is_some() {
            return Err(CliError::Usage(
                "--emit-ir does not compose with --allocator",
            ));
        }
        if static_flags.allow_slow_allocator {
            return Err(CliError::Usage(
                "--emit-ir does not compose with --allow-slow-allocator",
            ));
        }
        if production {
            return Err(CliError::Usage(
                "--emit-ir does not compose with --optimize",
            ));
        }
        BuildMode::EmitIr
    } else if wasm {
        // Clear the pseudo-triple so it never enters static resolution.
        BuildMode::Emit {
            out,
            wasm: true,
            static_layer: StaticRequestLayer::default(),
        }
    } else {
        BuildMode::Emit {
            out,
            wasm: false,
            static_layer: static_flags.layer(),
        }
    };

    Ok(BuildArgs {
        entry,
        runtime,
        fix,
        production,
        mode,
    })
}

/// Fully-parsed `ipe run` arguments.
pub struct RunArgs {
    /// The positional entry (`None` → project-aware default).
    pub entry: Option<String>,
    /// `--out <dir>`.
    pub out: Option<String>,
    /// `--runtime <dir>`.
    pub runtime: Option<String>,
    /// The native static-request layer.
    pub static_layer: StaticRequestLayer,
    /// Arguments after `--`, forwarded verbatim to the compiled binary.
    pub bin_args: Vec<String>,
}

/// Parse `ipe run`'s argument tail.
///
/// Splits on the first `--`: everything before is `ipe`-owned, everything after
/// is forwarded to the emitted binary untouched. `ipe run` builds and executes a
/// NATIVE process, so `--target wasm` (which has no native binary to run) is
/// rejected here rather than flowing into static resolution and surfacing as a
/// confusing "target requires --static" refusal.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_run(rest: &[String]) -> Result<RunArgs, CliError> {
    let dash_dash = rest.iter().position(|a| a == "--");
    // `pos` is a valid index; `pos + 1 <= rest.len()` (a trailing `--` gives an
    // empty tail), so both splits are in bounds without an indexing panic.
    let (ipe_args, bin_args): (&[String], Vec<String>) = dash_dash.map_or_else(
        || (rest, Vec::new()),
        |pos| {
            let (before, after_incl) = rest.split_at(pos);
            (before, after_incl.get(1..).unwrap_or(&[]).to_vec())
        },
    );

    let mut it = ipe_args.iter().peekable();
    let entry = take_leading_entry(&mut it);

    let mut out: Option<String> = None;
    let mut runtime: Option<String> = None;
    let mut static_flags = StaticFlags::default();
    while let Some(flag) = it.next() {
        if static_flags.consume(flag, &mut it, "run")? {
            continue;
        }
        match flag.as_str() {
            "--out" => set_once(
                &mut out,
                take_value(&mut it, "--out", "run")?,
                "--out",
                "run",
            )?,
            "--runtime" => set_once(
                &mut runtime,
                take_value(&mut it, "--runtime", "run")?,
                "--runtime",
                "run",
            )?,
            other => {
                return Err(CliError::UsageOwned(format!(
                    "ipe run: unknown flag `{other}`"
                )));
            }
        }
    }

    if static_flags.target.as_deref() == Some("wasm") {
        return Err(CliError::Usage(
            "ipe run builds and executes a native binary; --target wasm has no native artifact to \
             run — use `ipe build --target wasm` to produce a browser bundle",
        ));
    }

    Ok(RunArgs {
        entry,
        out,
        runtime,
        static_layer: static_flags.layer(),
        bin_args,
    })
}

/// Fully-parsed `ipe watch` arguments.
pub struct WatchArgs {
    /// The positional entry (`None` → project-aware default).
    pub entry: Option<String>,
    /// `--out <dir>`.
    pub out: Option<String>,
    /// `--runtime <dir>`.
    pub runtime: Option<String>,
    /// `--port <n>` — the parsed, in-range port (default 8000).
    pub port: u16,
}

/// Parse `ipe watch`'s argument tail. `--port` is parsed into a `u16` at this
/// boundary, and each value flag is rejected on a second occurrence.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_watch(rest: &[String]) -> Result<WatchArgs, CliError> {
    let mut it = rest.iter().peekable();
    let entry = take_leading_entry(&mut it);

    let mut out: Option<String> = None;
    let mut runtime: Option<String> = None;
    let mut port: Option<u16> = None;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => set_once(
                &mut out,
                take_value(&mut it, "--out", "watch")?,
                "--out",
                "watch",
            )?,
            "--runtime" => set_once(
                &mut runtime,
                take_value(&mut it, "--runtime", "watch")?,
                "--runtime",
                "watch",
            )?,
            "--port" => {
                let raw = take_value(&mut it, "--port", "watch")?;
                let parsed = raw.parse::<u16>().map_err(|_| {
                    CliError::UsageOwned(format!("ipe watch: invalid --port value: {raw}"))
                })?;
                set_once(&mut port, parsed, "--port", "watch")?;
            }
            other => {
                return Err(CliError::UsageOwned(format!(
                    "ipe watch: unknown flag `{other}`"
                )));
            }
        }
    }

    Ok(WatchArgs {
        entry,
        out,
        runtime,
        port: port.unwrap_or(8000),
    })
}

/// Fully-parsed `ipe fix` arguments.
pub struct FixArgs {
    /// The positional source file (required).
    pub entry: String,
    /// `--yes` — durable authorization to apply every fix without prompting.
    pub auto: bool,
}

/// Parse `ipe fix`'s argument tail. The `<path>` positional is required; a
/// second positional, or a flag other than `--yes`, is rejected.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_fix(rest: &[String]) -> Result<FixArgs, CliError> {
    let mut entry: Option<String> = None;
    let mut auto = false;
    for arg in rest {
        match arg.as_str() {
            "--yes" => auto = true,
            flag if flag.starts_with("--") => {
                return Err(CliError::UsageOwned(format!(
                    "ipe fix: unknown flag `{flag}`"
                )));
            }
            positional => set_once(&mut entry, positional.to_owned(), "<path>", "fix")?,
        }
    }
    let entry = entry.ok_or(CliError::Usage("usage: ipe fix <path> [--yes]"))?;
    Ok(FixArgs { entry, auto })
}

/// Fully-parsed `ipe fmt` mode — three dispatch paths, no ambiguous states.
///
/// Constructed exclusively by [`parse_fmt`], which rejects invalid combinations
/// at the boundary (parse, don't validate).
pub enum FmtMode {
    /// Format (or check) every `.ipe` file under `path` in place.
    /// `None` means the current directory `.`.
    InPlace { path: Option<String>, check: bool },
    /// Read from stdin, write formatted result to stdout.
    Stdin,
    /// Read from stdin, print diff to stdout without writing.
    StdinCheck,
}

/// Parse `ipe fmt`'s argument tail into a [`FmtMode`].
///
/// * No flags, no path → `InPlace { path: None, check: false }`
/// * One path → `InPlace { path: Some(…), check: false }`
/// * `--check` → `InPlace { …, check: true }`
/// * `--stdin` → `Stdin`
/// * `--stdin --check` → `StdinCheck`
/// * `--stdin` + positional path → error (mutually exclusive)
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_fmt(rest: &[String]) -> Result<FmtMode, CliError> {
    let mut path: Option<String> = None;
    let mut check = false;
    let mut stdin = false;
    for arg in rest {
        match arg.as_str() {
            "--check" => check = true,
            "--stdin" => stdin = true,
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe fmt: unknown flag `{flag}`"
                )));
            }
            positional => {
                if path.is_some() {
                    return Err(CliError::Usage("fmt: expected a single <path> argument"));
                }
                path = Some(positional.to_owned());
            }
        }
    }
    if stdin && path.is_some() {
        return Err(CliError::Usage(
            "fmt: --stdin and a <path> argument are mutually exclusive",
        ));
    }
    if stdin {
        if check {
            Ok(FmtMode::StdinCheck)
        } else {
            Ok(FmtMode::Stdin)
        }
    } else {
        Ok(FmtMode::InPlace { path, check })
    }
}

#[cfg(test)]
#[allow(clippy::panic)] // a wrong enum variant in a unit test IS the failure
mod tests {
    use super::*;

    // ---- build --------------------------------------------------------------

    #[test]
    fn build_empty_is_emit_defaults() {
        let a = parse_build(&[]).expect("empty build");
        assert!(a.entry.is_none());
        assert!(!a.fix);
        match a.mode {
            BuildMode::Emit {
                out,
                wasm,
                static_layer,
            } => {
                assert!(out.is_none());
                assert!(!wasm);
                assert_eq!(static_layer, StaticRequestLayer::default());
            }
            BuildMode::EmitIr => panic!("default build must emit a project"),
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn build_entry_and_flags() {
        let a =
            parse_build(&s(&["Main.ipe", "--out", "o", "--runtime", "r", "--fix"])).expect("valid");
        assert_eq!(a.entry.as_deref(), Some("Main.ipe"));
        assert_eq!(a.runtime.as_deref(), Some("r"));
        assert!(a.fix);
        match a.mode {
            BuildMode::Emit { out, .. } => assert_eq!(out.as_deref(), Some("o")),
            BuildMode::EmitIr => panic!(),
        }
    }

    #[test]
    fn build_emit_ir_rejects_out() {
        assert!(matches!(
            parse_build(&s(&["--emit-ir", "--out", "o"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn build_emit_ir_rejects_static_and_target_and_allocator() {
        assert!(parse_build(&s(&["--emit-ir", "--static"])).is_err());
        assert!(parse_build(&s(&["--emit-ir", "--target", "wasm"])).is_err());
        assert!(parse_build(&s(&["--emit-ir", "--allocator", "dlmalloc"])).is_err());
        assert!(parse_build(&s(&["--emit-ir", "--allow-slow-allocator"])).is_err());
    }

    #[test]
    fn build_emit_ir_alone_and_with_fix_ok() {
        assert!(matches!(
            parse_build(&s(&["--emit-ir"])).expect("emit-ir").mode,
            BuildMode::EmitIr
        ));
        let a = parse_build(&s(&["Main.ipe", "--emit-ir", "--fix"])).expect("emit-ir + fix");
        assert!(a.fix);
        assert!(matches!(a.mode, BuildMode::EmitIr));
    }

    #[test]
    fn build_wasm_rejects_native_static_flags() {
        assert!(parse_build(&s(&["--target", "wasm", "--static"])).is_err());
        assert!(parse_build(&s(&["--target", "wasm", "--allocator", "dlmalloc"])).is_err());
        assert!(parse_build(&s(&["--target", "wasm", "--allow-slow-allocator"])).is_err());
    }

    #[test]
    fn build_wasm_alone_ok() {
        match parse_build(&s(&["--target", "wasm"])).expect("wasm").mode {
            BuildMode::Emit {
                wasm, static_layer, ..
            } => {
                assert!(wasm);
                // The pseudo-triple must be cleared so it never reaches resolution.
                assert_eq!(static_layer, StaticRequestLayer::default());
            }
            BuildMode::EmitIr => panic!(),
        }
    }

    #[test]
    fn build_static_native_ok() {
        match parse_build(&s(&["--static", "--target", "x86_64-unknown-linux-musl"]))
            .expect("static")
            .mode
        {
            BuildMode::Emit {
                wasm, static_layer, ..
            } => {
                assert!(!wasm);
                assert_eq!(static_layer.static_build, Some(true));
                assert_eq!(
                    static_layer.target.as_deref(),
                    Some("x86_64-unknown-linux-musl")
                );
            }
            BuildMode::EmitIr => panic!(),
        }
    }

    #[test]
    fn build_duplicate_out_rejected() {
        assert!(matches!(
            parse_build(&s(&["--out", "a", "--out", "b"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    #[test]
    fn build_duplicate_target_rejected() {
        assert!(parse_build(&s(&["--target", "a", "--target", "b"])).is_err());
    }

    #[test]
    fn build_unknown_allocator_rejected() {
        assert!(matches!(
            parse_build(&s(&["--static", "--allocator", "jemalloc"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    #[test]
    fn build_missing_value_rejected() {
        assert!(parse_build(&s(&["--out"])).is_err());
        assert!(parse_build(&s(&["Main.ipe", "--target"])).is_err());
    }

    #[test]
    fn build_unknown_flag_rejected() {
        assert!(matches!(
            parse_build(&s(&["--bogus"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    // ---- run ----------------------------------------------------------------

    #[test]
    fn run_empty_defaults() {
        let a = parse_run(&[]).expect("empty run");
        assert!(a.entry.is_none());
        assert!(a.bin_args.is_empty());
        assert_eq!(a.static_layer, StaticRequestLayer::default());
    }

    #[test]
    fn run_dash_dash_forwards_verbatim() {
        let a = parse_run(&s(&["Main.ipe", "--out", "o", "--", "--flag", "x"])).expect("dash");
        assert_eq!(a.entry.as_deref(), Some("Main.ipe"));
        assert_eq!(a.out.as_deref(), Some("o"));
        assert_eq!(a.bin_args, s(&["--flag", "x"]));
    }

    #[test]
    fn run_trailing_dash_dash_is_empty_forward() {
        let a = parse_run(&s(&["Main.ipe", "--"])).expect("trailing");
        assert!(a.bin_args.is_empty());
    }

    #[test]
    fn run_wasm_target_rejected() {
        assert!(matches!(
            parse_run(&s(&["--target", "wasm"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn run_forwarded_args_are_not_parsed() {
        // A `--out` AFTER `--` is the binary's, never ipe's.
        let a = parse_run(&s(&["Main.ipe", "--", "--out", "x", "--target", "wasm"])).expect("fwd");
        assert!(a.out.is_none());
        assert_eq!(a.bin_args, s(&["--out", "x", "--target", "wasm"]));
    }

    #[test]
    fn run_duplicate_out_rejected() {
        assert!(parse_run(&s(&["--out", "a", "--out", "b"])).is_err());
    }

    #[test]
    fn run_unknown_flag_rejected() {
        assert!(parse_run(&s(&["Main.ipe", "--bogus"])).is_err());
    }

    // ---- watch --------------------------------------------------------------

    #[test]
    fn watch_defaults_port_8000() {
        let a = parse_watch(&[]).expect("empty watch");
        assert_eq!(a.port, 8000);
    }

    #[test]
    fn watch_parses_port() {
        let a = parse_watch(&s(&["Main.ipe", "--port", "9090"])).expect("port");
        assert_eq!(a.port, 9090);
        assert_eq!(a.entry.as_deref(), Some("Main.ipe"));
    }

    #[test]
    fn watch_invalid_port_rejected() {
        assert!(parse_watch(&s(&["--port", "notanumber"])).is_err());
        assert!(parse_watch(&s(&["--port", "99999"])).is_err());
    }

    #[test]
    fn watch_duplicate_port_rejected() {
        assert!(parse_watch(&s(&["--port", "1", "--port", "2"])).is_err());
    }

    #[test]
    fn watch_rejects_build_flags() {
        // `--static` belongs to build/run, never watch.
        assert!(parse_watch(&s(&["--static"])).is_err());
    }

    // ---- fix ----------------------------------------------------------------

    #[test]
    fn fix_requires_path() {
        assert!(matches!(parse_fix(&[]), Err(CliError::Usage(_))));
    }

    #[test]
    fn fix_path_and_yes() {
        let a = parse_fix(&s(&["Main.ipe", "--yes"])).expect("fix");
        assert_eq!(a.entry, "Main.ipe");
        assert!(a.auto);
    }

    #[test]
    fn fix_second_positional_rejected() {
        assert!(parse_fix(&s(&["a.ipe", "b.ipe"])).is_err());
    }

    #[test]
    fn fix_unknown_flag_rejected() {
        assert!(parse_fix(&s(&["Main.ipe", "--bogus"])).is_err());
    }

    // ---- fmt ----------------------------------------------------------------

    #[test]
    fn fmt_empty_is_in_place_default() {
        let m = parse_fmt(&[]).expect("empty fmt");
        assert!(matches!(
            m,
            FmtMode::InPlace {
                path: None,
                check: false
            }
        ));
    }

    #[test]
    fn fmt_path_sets_in_place() {
        let m = parse_fmt(&s(&["src"])).expect("fmt src");
        assert!(matches!(m, FmtMode::InPlace { path: Some(p), check: false } if p == "src"));
    }

    #[test]
    fn fmt_check_and_path() {
        let m = parse_fmt(&s(&["src", "--check"])).expect("fmt");
        assert!(matches!(m, FmtMode::InPlace { path: Some(p), check: true } if p == "src"));
    }

    #[test]
    fn fmt_stdin_only() {
        let m = parse_fmt(&s(&["--stdin"])).expect("stdin");
        assert!(matches!(m, FmtMode::Stdin));
    }

    #[test]
    fn fmt_stdin_check() {
        let m = parse_fmt(&s(&["--stdin", "--check"])).expect("stdin check");
        assert!(matches!(m, FmtMode::StdinCheck));
    }

    #[test]
    fn fmt_check_only() {
        let m = parse_fmt(&s(&["--check"])).expect("check only");
        assert!(matches!(
            m,
            FmtMode::InPlace {
                path: None,
                check: true
            }
        ));
    }

    #[test]
    fn fmt_stdin_with_path_rejected() {
        assert!(parse_fmt(&s(&["--stdin", "src"])).is_err());
    }

    #[test]
    fn fmt_two_paths_rejected() {
        assert!(parse_fmt(&s(&["a", "b"])).is_err());
    }

    #[test]
    fn fmt_unknown_flag_rejected() {
        assert!(parse_fmt(&s(&["--bogus"])).is_err());
    }

    // ---- output format ------------------------------------------------------

    #[test]
    fn format_defaults_to_human_and_keeps_positionals() {
        let args = s(&["a", "b"]);
        let (fmt, pos) = split_format(&args, "diff").expect("no flags");
        assert_eq!(fmt, OutputFormat::Human);
        assert_eq!(pos, vec!["a", "b"]);
    }

    #[test]
    fn format_plain_and_json_are_recognised() {
        let plain = s(&["x", "--plain"]);
        let (fmt, pos) = split_format(&plain, "capabilities").expect("plain");
        assert_eq!(fmt, OutputFormat::Plain);
        assert_eq!(pos, vec!["x"]);
        let json = s(&["--json", "x"]);
        let (fmt, _) = split_format(&json, "capabilities").expect("json");
        assert_eq!(fmt, OutputFormat::Json);
    }

    #[test]
    fn format_rejects_both_flags_together() {
        assert!(matches!(
            split_format(&s(&["--plain", "--json"]), "version"),
            Err(CliError::UsageOwned(_))
        ));
        assert!(split_format(&s(&["--json", "--plain"]), "version").is_err());
    }

    #[test]
    fn format_rejects_a_repeated_flag() {
        assert!(split_format(&s(&["--plain", "--plain"]), "diff").is_err());
    }
}
