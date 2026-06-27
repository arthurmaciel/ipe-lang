//! Project assembly: stitch the fixed templates and the genuinely-emitted user
//! types + functions into the final `src/main.rs`, and pair it with the project
//! `Cargo.toml`.
//!
//! Layout (matching the golden, line by line):
//! ```text
//! <preamble: 1..=30>           header, imports, basic aliases, USER TYPES banner
//! <user types: 31..=43>        emitted from the IR (emit_enum)
//! <blank: 44>
//! <runtime bindings: 45..=127> fixed kernel-wrapper prelude
//! <blank: 128>
//! <user functions: 129..=137>  emitted from the IR (emit_func)
//! <blank: 138>
//! <epilogue: 139..>            Ffi.kernel polyfill, list helpers, entry point
//! ```

use std::collections::BTreeMap;

use sky_backend::{EmittedProject, RelPath};
use sky_diagnostics::{DResult, Diagnostic};
use sky_ir::{Program, TypeDef};

use crate::EmitCtx;
use crate::emit_expr::emit_func;
use crate::emit_types::{emit_enum, emit_record_struct};
use crate::preamble::{epilogue, preamble};

/// The golden M0 program, embedded at compile time. The fixed runtime-bindings
/// block (kernel wrappers, golden lines 45–127) is an exact substring of it.
const GOLDEN: &str = include_str!("../../../tests/golden/m0/main.rs");

/// The project `Cargo.toml`, embedded verbatim from the golden. M0 emits the
/// same manifest for every program (dependency set is fixed by the runtime).
const CARGO_TOML: &str = include_str!("../../../tests/golden/m0/Cargo.toml");

/// The generated `sky_runtime/mod.rs` — the curated set of runtime modules whose
/// dependencies are satisfied by [`CARGO_TOML`]. The vendored runtime source
/// ships a fuller `mod.rs` (declaring `uuid` / `live` / `db` / … modules that
/// pull crates outside the M0 manifest); the driver overwrites it with this
/// trimmed version. M0 emits a fixed module set; later milestones compute it
/// from the kernels a program actually uses.
const RUNTIME_MOD_RS: &str = include_str!("../../../tests/golden/m0/sky_runtime/mod.rs");

/// The generated `sky_runtime/config.rs` (DB/config bindings — empty for M0).
const RUNTIME_CONFIG_RS: &str = include_str!("../../../tests/golden/m0/sky_runtime/config.rs");

/// The `Diagnostic::CompilerBug` raised when a golden anchor is absent — a
/// drifted-golden invariant violation, surfaced (SKY-I0203) instead of a silent
/// empty slice.
fn anchor_missing(anchor: &str) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "backend.golden_anchor",
        detail: format!("golden anchor {anchor:?} not found in the embedded M0 golden"),
    }
}

/// The fixed kernel-wrapper prelude emitted between the user types and the user
/// functions (golden lines 45–127).
///
/// These bindings (`SkyError`, the `log_*` / `system_*` / `time_*` / … wrappers)
/// are identical for every M0 program, so they are sliced out of the embedded
/// golden rather than hand-retyped — the same drift-free strategy the
/// preamble/epilogue use. The slice is anchored entirely on its *own* content
/// (the first alias and the final `crypto_random_token` wrapper), independent of
/// the surrounding user code.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] (SKY-I0203) if either anchor is absent
/// from the embedded golden — a drifted-golden invariant violation, surfaced
/// instead of a silent empty slice.
fn runtime_bindings() -> DResult<&'static str> {
    const START: &str = "type SkyError = String;";
    const END: &str = "    sky_runtime::crypto::crypto_random_token(n)\n}\n";
    let start = GOLDEN.find(START).ok_or_else(|| anchor_missing(START))?;
    let rest = GOLDEN.get(start..).ok_or_else(|| anchor_missing(START))?;
    let end_in_rest = rest.find(END).ok_or_else(|| anchor_missing(END))?;
    let end = start + end_in_rest + END.len();
    GOLDEN.get(start..end).ok_or_else(|| anchor_missing(END))
}

/// Emit the complete project for `program`.
pub fn emit_program(ctx: &EmitCtx, program: &Program) -> DResult<EmittedProject> {
    let mut out = String::new();
    out.push_str(&preamble()?);

    // User types, emitted from the IR.
    for module in &program.modules {
        for ty in &module.types {
            let TypeDef::Enum(def) = ty;
            out.push_str(&emit_enum(ctx, def)?);
        }
    }
    // Synthesised record structs, one per distinct closed record shape. Item
    // order is irrelevant in Rust, so these can reference one another freely;
    // a program with no records emits nothing here, keeping output unchanged.
    for rec in ctx.record_structs() {
        out.push_str(&emit_record_struct(ctx, rec)?);
    }
    out.push('\n');

    // Fixed kernel-wrapper prelude.
    out.push_str(runtime_bindings()?);
    out.push('\n');

    // User functions, emitted from the IR.
    for module in &program.modules {
        for func in &module.funcs {
            out.push_str(&emit_func(ctx, func)?);
        }
    }
    out.push('\n');

    out.push_str(&epilogue()?);

    let mut files = BTreeMap::new();
    files.insert(RelPath::new("src/main.rs")?, out);
    files.insert(
        RelPath::new("src/sky_runtime/mod.rs")?,
        RUNTIME_MOD_RS.to_owned(),
    );
    files.insert(
        RelPath::new("src/sky_runtime/config.rs")?,
        RUNTIME_CONFIG_RS.to_owned(),
    );
    Ok(EmittedProject {
        files,
        cargo_toml: CARGO_TOML.to_owned(),
    })
}
