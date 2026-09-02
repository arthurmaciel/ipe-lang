//! The per-symbol probe: generate a minimal program that references a symbol,
//! and lower / build / run it.
//!
//! A symbol that type-checks but does not lower, or lowers but does not build or
//! run, is the seam the dynamic columns close. The generator emits the smallest
//! program that forces the symbol through the requested stage: a top-level
//! binding to the symbol as a first-class value (which the resolver must resolve
//! and the lowerer must lower), and — for a higher-order symbol — a nested-in-a-
//! closure form that forces the lowerer to descend into the combinator in a
//! nested position, the shape a composed combinator that type-checked but never
//! lowered failed on.
//!
//! The generator references the symbol as a value rather than fully applying it,
//! so it need not fabricate well-typed arguments from an arbitrary scheme: a
//! point-free reference is enough to force name-resolution and lowering, and it
//! cannot manufacture a spurious type error the way a guessed application could.

use std::fmt::Write as _;
use std::path::Path;

use crate::coverage::contract::StdlibSymbol;

/// Why a probe could not be formed for a symbol — distinct from a stage failure,
/// so a symbol the generator cannot express is reported as inapplicable rather
/// than as a false hole.
#[derive(Clone, Debug)]
pub enum ProbeUnavailable {
    /// The symbol is not a value (a type or constructor) — the value-reference
    /// probe does not apply.
    NotAValue,
    /// The symbol is not reachable by a qualified reference (no compiled-source
    /// module to import it from).
    Unaddressable,
}

/// The outcome of driving a probe through one stage.
#[derive(Clone, Debug)]
pub enum StageOutcome {
    /// The stage succeeded.
    Ok,
    /// The stage failed with a diagnostic or process message — a real gap.
    Failed(String),
}

/// The short module import header for a symbol: `import Ipe.List as List`.
///
/// A symbol is referenced qualified (`List.map`) under this import, so the probe
/// resolves the exact surface member without an `exposing (..)` widening that
/// could mask a resolution gap behind a re-export.
fn import_header(sym: &StdlibSymbol) -> Option<(String, String)> {
    let dotted = sym.module.join(".");
    if dotted.is_empty() {
        return None;
    }
    let short = dotted.split('.').next_back().unwrap_or(&dotted).to_owned();
    Some((dotted, short))
}

/// Generate a minimal module that binds the symbol as a first-class value,
/// forcing the resolver and lowerer to reach it.
///
/// `probe = <Short>.<name>` — a point-free reference. The `main` entry is a
/// minimal valid task so the module is a complete, buildable program when the
/// build column asks for it.
///
/// # Errors
/// [`ProbeUnavailable`] when the symbol is not a value or cannot be addressed by
/// a qualified import.
pub fn reference_program(sym: &StdlibSymbol) -> Result<String, ProbeUnavailable> {
    use crate::coverage::contract::SymbolKind;
    if sym.kind != SymbolKind::Value {
        return Err(ProbeUnavailable::NotAValue);
    }
    let Some((dotted, short)) = import_header(sym) else {
        return Err(ProbeUnavailable::Unaddressable);
    };
    let mut out = String::from("module Main exposing (main)\n\n");
    let _ = writeln!(out, "import {dotted} as {short}");
    out.push_str("import Ipe.Io as Io\n\n");
    let _ = writeln!(out, "probe = {short}.{name}", name = sym.name);
    out.push_str("\nmain : Task Error ()\n");
    out.push_str("main = Io.println \"\"\n");
    Ok(out)
}

/// Generate a module that references the symbol in a NESTED closure position,
/// forcing the lowerer to descend into the combinator inside another combinator
/// — the composition shape a symbol that type-checked but did not lower failed
/// on.
///
/// `probe = List.map (\_ -> <Short>.<name>) []` nests the reference inside a
/// lambda passed to `List.map`; the lowerer must walk into the lambda body and
/// lower the combinator there. This is a value reference nested two constructs
/// deep, so it stays well-typed for any value symbol without fabricating typed
/// arguments, while still exercising the descend-into-nested-position path.
///
/// # Errors
/// [`ProbeUnavailable`] as for [`reference_program`].
pub fn nested_program(sym: &StdlibSymbol) -> Result<String, ProbeUnavailable> {
    use crate::coverage::contract::SymbolKind;
    if sym.kind != SymbolKind::Value {
        return Err(ProbeUnavailable::NotAValue);
    }
    let Some((dotted, short)) = import_header(sym) else {
        return Err(ProbeUnavailable::Unaddressable);
    };
    let mut out = String::from("module Main exposing (main)\n\n");
    let _ = writeln!(out, "import {dotted} as {short}");
    out.push_str("import Ipe.List as List\n");
    out.push_str("import Ipe.Io as Io\n\n");
    let _ = writeln!(
        out,
        "probe = List.map (\\_ -> {short}.{name}) []",
        name = sym.name
    );
    out.push_str("\nmain : Task Error ()\n");
    out.push_str("main = Io.println \"\"\n");
    Ok(out)
}

/// Lower a probe program, returning whether it lowered.
///
/// Writes the source to `snippet` and drives it through the same source-graph
/// lowering pipeline `ipe build --emit-ir` uses (name-resolution + type-check +
/// lower), so a symbol that resolves and type-checks but does not lower is
/// reported as a stage failure.
pub fn lower(source: &str, snippet: &Path) -> StageOutcome {
    if let Err(e) = std::fs::write(snippet, source) {
        return StageOutcome::Failed(format!("could not write probe source: {e}"));
    }
    match crate::lower_entry_via_graph(snippet) {
        Ok(_) => StageOutcome::Ok,
        Err(err) => StageOutcome::Failed(err.to_string()),
    }
}

/// Type-check a probe program, returning whether it type-checks.
///
/// The precondition a generated nested probe relies on: if the value-reference
/// form does not even type-check, the generator (not the symbol) is at fault, so
/// the nested-lowering column reports the symbol inapplicable rather than a false
/// hole.
pub fn typechecks(source: &str, snippet: &Path) -> StageOutcome {
    if let Err(e) = std::fs::write(snippet, source) {
        return StageOutcome::Failed(format!("could not write probe source: {e}"));
    }
    match crate::typecheck_entry_via_graph(snippet) {
        Ok(()) => StageOutcome::Ok,
        Err(err) => StageOutcome::Failed(err.to_string()),
    }
}

/// Build and RUN a probe program, returning whether the emitted crate builds and
/// the produced binary runs to a zero exit.
///
/// Re-invokes this binary as `ipe run <snippet>` — the same emit → cargo build →
/// execute path a user's `ipe run` takes — so a symbol whose program emits and
/// type-checks but whose emitted crate does not build or whose binary does not
/// run is a real gap. Heavy: the caller gates this behind the E2E path.
pub fn build_and_run(source: &str, snippet: &Path) -> StageOutcome {
    use std::process::Command;
    if let Err(e) = std::fs::write(snippet, source) {
        return StageOutcome::Failed(format!("could not write probe source: {e}"));
    }
    let ipe_bin = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return StageOutcome::Failed(format!("could not locate the ipe binary: {e}")),
    };
    let output = match Command::new(&ipe_bin).arg("run").arg(snippet).output() {
        Ok(o) => o,
        Err(e) => return StageOutcome::Failed(format!("ipe run failed to spawn: {e}")),
    };
    if output.status.success() {
        StageOutcome::Ok
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        StageOutcome::Failed(format!("ipe run exited non-zero: {stderr}"))
    }
}
