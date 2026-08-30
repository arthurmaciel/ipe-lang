//! The asserted-call surface (`Rust.Ffi.call`).
//!
//! Validates an author-asserted signature against the installed-crate catalog
//! and emits its two artifacts — the `Rust.Ffi` interface module and the
//! `_bindings.rs` shim region.
//!
//! The discipline: the escape hatch skips ceremony, never soundness. The path is a parsed
//! [`ipe_canon::asserted::AssertedPath`], never text; the signature may only
//! name carriers the boundary already admits; the shim carries the asserted
//! types verbatim under the **exact-carrier rule** — this module never calls
//! into `num_coerce`, so no clamp or widening can hide between the Ipê type
//! and the Rust type. When the target is in the cached inspection, the
//! assertion is checked here at build preparation; when it is not, the shim's
//! `rustc` check is the checker of record and a wrong assertion fails the
//! emitted build inside the shim's commented region, attributed to the
//! assertion — never undefined behavior, never a silent success.
//!
//! Every shim body executes the foreign call under
//! `std::panic::catch_unwind`, folding a foreign panic into the wrapper's
//! typed `Err` — the same panic boundary every inspected wrapper is born
//! inside — which is why an asserted signature must end in `Result Error T`:
//! that result is the error channel the boundary needs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use ipe_canon::asserted::AssertedPath;
use ipe_intern::{Interner, Symbol};
use ipe_syntax::TypeAnnotation;

use crate::carrier::Carrier;
use crate::diag::{AssertedDefect, Diagnostic};
use crate::driver::InstalledCrate;
use crate::pkginfo::Effect;

/// One validated asserted call: the parsed path, the parsed signature, the
/// derived names, and every Rust type the emitters render — all resolved
/// here, so emission is a total function over this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertedSpec {
    /// The validated target path.
    pub path: AssertedPath,
    /// The parsed asserted signature.
    pub sig: AssertedSig,
    /// The Ipê definition name in the generated `Rust.Ffi` module.
    pub def_name: String,
    /// The `_bindings.rs` shim identifier.
    pub wrapper_ident: String,
    /// The target crate's interface-module name (`Rust.Semver`) — where the
    /// signature's opaque nominals are imported from.
    pub crate_module: String,
    /// Rendered Rust parameter types, aligned with the forwarder arity (a
    /// unit-param signature renders `()`).
    pub param_rust: Vec<String>,
    /// The rendered Rust result type (the `Ok` payload).
    pub result_rust: String,
    /// The opaque nominals the signature uses (for the interface import).
    pub opaque_imports: BTreeSet<String>,
}

/// A parsed asserted signature: owned carriers in, `Result Error <carrier>`
/// out. Parsing is the only constructor, so an out-of-set type is
/// unrepresentable downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertedSig {
    /// The parameter carriers. Empty exactly when [`Self::unit_param`].
    pub params: Vec<Carrier>,
    /// `true` for the `() -> Result Error T` shape (a zero-argument target;
    /// the forwarder stays unary over the unit value, like every zero-param
    /// inspected wrapper).
    pub unit_param: bool,
    /// The `Ok` payload carrier of the `Result Error` return.
    pub result: Carrier,
}

impl AssertedSig {
    /// The forwarder arity (a unit-param signature is unary).
    #[must_use]
    pub const fn arity(&self) -> usize {
        if self.unit_param {
            1
        } else {
            self.params.len()
        }
    }

    /// The canonical Ipê signature string for the generated interface def.
    #[must_use]
    pub fn ipe_sig(&self) -> String {
        let mut parts: Vec<String> = if self.unit_param {
            vec!["()".to_owned()]
        } else {
            self.params
                .iter()
                .map(|c| c.ipe_surface().to_owned())
                .collect()
        };
        parts.push(format!("Result Error {}", self.result.ipe_surface()));
        parts.join(" -> ")
    }

    /// Parse an author's annotation into the asserted shape.
    ///
    /// # Errors
    /// The [`AssertedDefect`] naming the first rule broken (the caller wraps
    /// it with the offending path).
    pub fn from_annotation(
        annotation: &TypeAnnotation,
        interner: &Interner,
    ) -> Result<Self, AssertedDefect> {
        // Split the arrow spine: `A -> B -> R` ⇒ params [A, B], result R.
        let mut segments: Vec<&TypeAnnotation> = Vec::new();
        let mut cursor = annotation;
        while let TypeAnnotation::TLambda(param, rest) = cursor {
            segments.push(param);
            cursor = rest;
        }
        if segments.is_empty() {
            return Err(AssertedDefect::NotAFunction);
        }
        let unit_param = matches!(segments.as_slice(), [TypeAnnotation::TUnit]);
        let mut params = Vec::with_capacity(segments.len());
        if !unit_param {
            for seg in &segments {
                if matches!(seg, TypeAnnotation::TUnit) {
                    return Err(AssertedDefect::UnitParamNotSole);
                }
                params.push(parse_carrier(seg, interner)?);
            }
        }
        let result = parse_result(cursor, interner)?;
        Ok(Self {
            params,
            unit_param,
            result,
        })
    }
}

/// Parse one leaf carrier from an annotation node: a scalar spelling or an
/// opaque nominal. Everything else — type variables, tuples, records,
/// nested functions, applied constructors — is outside the closed set.
fn parse_carrier(node: &TypeAnnotation, interner: &Interner) -> Result<Carrier, AssertedDefect> {
    let outside = |ty: String| AssertedDefect::CarrierOutsideClosedSet { ty };
    match node {
        TypeAnnotation::TType(_, name_segments, args) => {
            if !args.is_empty() {
                return Err(outside(render_ty(node, interner)));
            }
            let name = last_segment(name_segments, interner);
            Carrier::parse(&name).map_err(|_| outside(render_ty(node, interner)))
        }
        _ => Err(outside(render_ty(node, interner))),
    }
}

/// Parse the result position: exactly `Result Error <carrier>`.
fn parse_result(node: &TypeAnnotation, interner: &Interner) -> Result<Carrier, AssertedDefect> {
    let shape = |ty: String| AssertedDefect::ResultShape { ty };
    let TypeAnnotation::TType(_, name_segments, args) = node else {
        return Err(shape(render_ty(node, interner)));
    };
    let ([err_arg, ok_arg], "Result") = (
        args.as_slice(),
        last_segment(name_segments, interner).as_str(),
    ) else {
        return Err(shape(render_ty(node, interner)));
    };
    let err_ok = matches!(
        err_arg,
        TypeAnnotation::TType(_, err_segments, err_args)
            if err_args.is_empty() && last_segment(err_segments, interner) == "Error"
    );
    if !err_ok {
        return Err(shape(render_ty(node, interner)));
    }
    parse_carrier(ok_arg, interner)
}

/// The final dotted name segment, resolved (empty string on an interner miss
/// — the caller's `Carrier::parse` then refuses it).
fn last_segment(segments: &[Symbol], interner: &Interner) -> String {
    segments
        .last()
        .and_then(|s| interner.resolve(*s))
        .unwrap_or_default()
        .to_owned()
}

/// Render an annotation node for a diagnostic (best-effort, source-shaped).
fn render_ty(node: &TypeAnnotation, interner: &Interner) -> String {
    match node {
        TypeAnnotation::TLambda(a, b) => {
            format!("{} -> {}", render_ty(a, interner), render_ty(b, interner))
        }
        TypeAnnotation::TVar(v) => interner.resolve(*v).unwrap_or("?").to_owned(),
        TypeAnnotation::TType(_, segments, args) => {
            let mut out = segments
                .iter()
                .filter_map(|s| interner.resolve(*s))
                .collect::<Vec<_>>()
                .join(".");
            for a in args {
                let _ = write!(out, " {}", render_ty(a, interner));
            }
            out
        }
        TypeAnnotation::TUnit => "()".to_owned(),
        TypeAnnotation::TTuple(items) => format!(
            "({})",
            items
                .iter()
                .map(|t| render_ty(t, interner))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeAnnotation::TRecord(_) | TypeAnnotation::TRecordOpen(..) => "{…}".to_owned(),
    }
}

/// Validate one asserted call against the installed-crate catalog, resolving
/// every name and Rust type the emitters need.
///
/// # Errors
/// [`Diagnostic::AssertedRefused`] (IPE-F4414) carrying the closed
/// [`AssertedDefect`].
pub fn validate(
    path: AssertedPath,
    annotation: &TypeAnnotation,
    interner: &Interner,
    catalog: &[InstalledCrate],
) -> Result<AssertedSpec, Diagnostic> {
    let refused = |defect: AssertedDefect| Diagnostic::AssertedRefused {
        path: path.as_str().to_owned(),
        defect,
    };
    let Some(target) = catalog.iter().find(|c| c.slug == path.crate_ident()) else {
        return Err(refused(AssertedDefect::TargetCrateNotInstalled {
            crate_ident: path.crate_ident().to_owned(),
        }));
    };
    let sig = AssertedSig::from_annotation(annotation, interner).map_err(&refused)?;

    // Resolve every carrier to its exact Rust rendering through the target
    // crate's own maps — an undeclared nominal dies here.
    let mut opaque_imports = BTreeSet::new();
    let mut resolve = |c: &Carrier| -> Result<String, Diagnostic> {
        match c {
            Carrier::Opaque(id) => {
                let name = id.as_str();
                let rust = resolve_opaque(target, name).ok_or_else(|| {
                    refused(AssertedDefect::OpaqueNotDeclared {
                        name: name.to_owned(),
                    })
                })?;
                // A define-defined nominal lives in the emitted crate itself;
                // only an inspected opaque is imported from the interface.
                if target.opaque_types.contains_key(name) {
                    opaque_imports.insert(name.to_owned());
                }
                Ok(rust)
            }
            scalar => Ok(scalar.rust_owned().to_owned()),
        }
    };
    let mut param_rust = Vec::with_capacity(sig.arity());
    if sig.unit_param {
        param_rust.push("()".to_owned());
    } else {
        for c in &sig.params {
            param_rust.push(resolve(c)?);
        }
    }
    let result_rust = resolve(&sig.result)?;

    // The compile-time checker (design §5.2 rule 1): when the target is in
    // the cached inspection, the assertion must match it exactly — identity
    // carriers only, never a clamp the shim would then have to hide.
    if path.is_crate_top_level()
        && let Some(fact) = target.inspected_free_fns.get(path.fn_name())
    {
        check_against_inspection(&path, &sig, fact, target).map_err(&refused)?;
    }

    Ok(AssertedSpec {
        def_name: path.def_name(),
        wrapper_ident: path.wrapper_ident(),
        crate_module: target.module_name.clone(),
        path,
        sig,
        param_rust,
        result_rust,
        opaque_imports,
    })
}

/// Resolve an opaque nominal to its Rust path through the target crate: an
/// inspected opaque absolutizes its recorded path; a define-defined nominal
/// resolves crate-locally where `_bindings.rs` defines it.
fn resolve_opaque(target: &InstalledCrate, name: &str) -> Option<String> {
    if let Some(p) = target.opaque_types.get(name) {
        let trimmed = p.trim_start_matches(':');
        return Some(format!("::{trimmed}"));
    }
    if target.define_types.contains(name) {
        return Some(format!("crate::ffi::{}::{name}", target.slug));
    }
    None
}

/// The exact-carrier cross-check against an inspected free function.
fn check_against_inspection(
    path: &AssertedPath,
    sig: &AssertedSig,
    fact: &crate::driver::InspectedFnFact,
    target: &InstalledCrate,
) -> Result<(), AssertedDefect> {
    let unsupported = |reason: &str| AssertedDefect::InspectedShapeUnsupported {
        reason: reason.to_owned(),
    };
    if fact.effect != Effect::Pure {
        return Err(unsupported(
            "its effect is not a plain synchronous return — the fallible/async \
             folding lives on the inspected path",
        ));
    }
    let Some(result_ty) = &fact.result else {
        return Err(unsupported("it returns no value"));
    };
    let expected = || AssertedDefect::InspectedMismatch {
        expected: format!(
            "fn {}({}) -> {}",
            path.fn_name(),
            fact.params.join(", "),
            result_ty
        ),
    };
    let declared_params: &[Carrier] = if sig.unit_param { &[] } else { &sig.params };
    if declared_params.len() != fact.params.len() {
        return Err(expected());
    }
    for (carrier, foreign) in declared_params.iter().zip(&fact.params) {
        if !exact_carrier_match(carrier, foreign, target) {
            return Err(expected());
        }
    }
    if !exact_carrier_match(&sig.result, result_ty, target) {
        return Err(expected());
    }
    Ok(())
}

/// Whether an asserted carrier is EXACTLY the inspected Rust type: a scalar
/// matches its one owned spelling byte-for-byte (whitespace-normalized); an
/// opaque matches when the inspected type's final path segment is the
/// nominal the crate maps. No clamp, no widening — identity only.
fn exact_carrier_match(carrier: &Carrier, foreign_ty: &str, target: &InstalledCrate) -> bool {
    let normalized: String = foreign_ty.chars().filter(|c| !c.is_whitespace()).collect();
    match carrier {
        Carrier::Opaque(id) => {
            let name = id.as_str();
            resolve_opaque(target, name).is_some() && normalized.rsplit("::").next() == Some(name)
        }
        scalar => normalized == scalar.rust_owned().replace(' ', ""),
    }
}

/// Fold a batch of validated specs, deduplicating identical assertions and
/// refusing conflicting ones (two different signatures for one path).
///
/// # Errors
/// [`Diagnostic::AssertedRefused`] with
/// [`AssertedDefect::ConflictingAssertions`].
pub fn dedupe(specs: Vec<AssertedSpec>) -> Result<Vec<AssertedSpec>, Diagnostic> {
    let mut by_def: BTreeMap<String, AssertedSpec> = BTreeMap::new();
    for spec in specs {
        match by_def.get(&spec.def_name) {
            None => {
                by_def.insert(spec.def_name.clone(), spec);
            }
            // Identical path AND signature: one definition serves them all. A
            // path mismatch under one derived name is the (astronomically
            // unlikely) hash collision — refused loudly, never merged.
            Some(existing) if existing.sig == spec.sig && existing.path == spec.path => {}
            Some(existing) => {
                return Err(Diagnostic::AssertedRefused {
                    path: spec.path.as_str().to_owned(),
                    defect: AssertedDefect::ConflictingAssertions {
                        first: format!("{} : {}", existing.path.as_str(), existing.sig.ipe_sig()),
                        second: format!("{} : {}", spec.path.as_str(), spec.sig.ipe_sig()),
                    },
                });
            }
        }
    }
    Ok(by_def.into_values().collect())
}

/// Render the driver-generated `Rust.Ffi` interface module.
///
/// One annotated forwarder per asserted call, each body an `Ffi.asserted`
/// binding — mintable only under `ModuleOrigin::FfiInterface`, so the escape
/// hatch compiles down to the same gated interface entries as every other
/// surface.
#[must_use]
pub fn render_asserted_interface(specs: &[AssertedSpec]) -> String {
    let mut exports: Vec<&str> = specs.iter().map(|s| s.def_name.as_str()).collect();
    exports.sort_unstable();
    let mut out = format!(
        "module {} exposing ({})\n",
        ipe_canon::asserted::ASSERTED_MODULE,
        exports.join(", ")
    );
    // One import line per crate module whose opaque nominals appear in a
    // signature.
    let mut imports: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for s in specs {
        for name in &s.opaque_imports {
            imports
                .entry(s.crate_module.as_str())
                .or_default()
                .insert(name);
        }
    }
    for (module, names) in imports {
        let joined = names.into_iter().collect::<Vec<_>>().join(", ");
        let _ = write!(out, "\nimport {module} exposing ({joined})\n");
    }
    let mut ordered: Vec<&AssertedSpec> = specs.iter().collect();
    ordered.sort_unstable_by(|a, b| a.def_name.cmp(&b.def_name));
    for s in ordered {
        let args: Vec<String> = (0..s.sig.arity()).map(crate::naming::arg_name).collect();
        let args_joined = args.join(" ");
        let _ = write!(
            out,
            "\n{} : {}\n{} {} =\n    Ffi.asserted \"{}\" {}\n",
            s.def_name,
            s.sig.ipe_sig(),
            s.def_name,
            args_joined,
            s.wrapper_ident,
            args_joined
        );
    }
    out
}

/// Render the asserted shim region appended to the assembled `src/ffi.rs`.
///
/// Exact-carrier by construction: parameters and the return are the resolved
/// carrier types verbatim, the call forwards the owned arguments untouched,
/// and the body's only transformation is the panic boundary. When the target
/// was not inspected, `rustc` checks this region against the real crate; the
/// comment above each shim attributes such a failure to the assertion.
#[must_use]
pub fn emit_asserted_shims(specs: &[AssertedSpec]) -> String {
    let mut ordered: Vec<&AssertedSpec> = specs.iter().collect();
    ordered.sort_unstable_by(|a, b| a.wrapper_ident.cmp(&b.wrapper_ident));
    let mut lines: Vec<String> = vec![
        "pub mod ipe_asserted {".to_owned(),
        "    //! Author-asserted foreign-call shims (`Rust.Ffi.call`): the exact".to_owned(),
        "    //! declared carriers, no numeric coercion, every call inside the".to_owned(),
        "    //! panic boundary.".to_owned(),
        "    #![allow(unused_imports)]".to_owned(),
        String::new(),
        "    use crate::*;".to_owned(),
    ];
    for s in ordered {
        let params: Vec<String> = s
            .param_rust
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                if s.sig.unit_param {
                    format!("_: {ty}")
                } else {
                    format!("{}: {ty}", crate::naming::arg_name(i))
                }
            })
            .collect();
        let forwarded = if s.sig.unit_param {
            String::new()
        } else {
            (0..s.param_rust.len())
                .map(crate::naming::arg_name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let call = format!("{}({forwarded})", s.path.rust_call_path());
        lines.push(String::new());
        lines.push(format!(
            "    // [asserted] {} : {} = Rust.Ffi.call \"{}\"",
            s.def_name,
            s.sig.ipe_sig(),
            s.path.as_str()
        ));
        lines.push(
            "    // A type error in this shim means the author-asserted signature does".to_owned(),
        );
        lines
            .push("    // not match the real Rust signature — fix the assertion at the".to_owned());
        lines.push(
            "    // definition above, or use the inspected import (`import Rust.<Crate>`)."
                .to_owned(),
        );
        lines.push(format!(
            "    pub fn {}({}) -> IpeResult<IpeError, {}> {{",
            s.wrapper_ident,
            params.join(", "),
            s.result_rust
        ));
        lines.push(format!(
            "        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call})) \
             {{ Ok(v) => IpeResult::Ok(v), Err(__p) => IpeResult::Err(ipe_error_from_panic(\
             \"asserted foreign call panicked\", __p)) }}"
        ));
        lines.push("    }".to_owned());
    }
    lines.push("}".to_owned());
    lines.push("pub use ipe_asserted::*;".to_owned());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkginfo::PkgInfo;

    fn intern(interner: &mut Interner, s: &str) -> Symbol {
        interner.intern(s).expect("interner has capacity")
    }

    /// Build a `TypeAnnotation` arrow chain from leaf names, e.g.
    /// `["Int", "Result Error Int"]` — leaves are built structurally.
    fn ann_leaf(interner: &mut Interner, name: &str) -> TypeAnnotation {
        if name == "()" {
            return TypeAnnotation::TUnit;
        }
        let empty = intern(interner, "");
        if let Some(rest) = name.strip_prefix("Result Error ") {
            let result = intern(interner, "Result");
            let error = intern(interner, "Error");
            let inner = ann_leaf(interner, rest);
            let err_node = TypeAnnotation::TType(empty, vec![error], vec![]);
            return TypeAnnotation::TType(empty, vec![result], vec![err_node, inner]);
        }
        let sym = intern(interner, name);
        TypeAnnotation::TType(empty, vec![sym], vec![])
    }

    fn arrow(interner: &mut Interner, names: &[&str]) -> TypeAnnotation {
        let mut nodes: Vec<TypeAnnotation> = names.iter().map(|n| ann_leaf(interner, n)).collect();
        let mut out = nodes.pop().expect("non-empty arrow");
        while let Some(param) = nodes.pop() {
            out = TypeAnnotation::TLambda(Box::new(param), Box::new(out));
        }
        out
    }

    fn semver_crate() -> InstalledCrate {
        let doc = serde_json::json!({
            "pkg": "semver", "name": "semver", "version": "1.0.26",
            "functions": [
                {
                    "name": "frobnicate",
                    "params": [{"name": "n", "type": "i64"}],
                    "results": [{"name": "", "type": "i64"}],
                    "effect": "pure"
                },
                {
                    "name": "clamped",
                    "params": [{"name": "n", "type": "u32"}],
                    "results": [{"name": "", "type": "u32"}],
                    "effect": "pure"
                },
                {
                    "name": "parse",
                    "params": [{"name": "text", "type": "&str", "ipeType": "String"}],
                    "results": [{"name": "", "type": "Result<Version, Error>",
                                 "rustType": "Result<semver::Version, semver::Error>"}],
                    "effect": "fallible"
                }
            ],
            "errors": [],
            "transitiveDeps": [{"ident": "semver", "name": "semver", "version": "1.0.26"}],
            "foreignTypeIds": {"::semver::Version": "semver::version::Version"}
        })
        .to_string();
        let pkg = PkgInfo::decode_json(&doc).expect("decodes");
        crate::driver::installed_crate_from_pkg("semver".to_owned(), &pkg).expect("installs")
    }

    fn validated(sig_names: &[&str], path: &str) -> Result<AssertedSpec, Diagnostic> {
        let mut interner = Interner::new();
        let ann = arrow(&mut interner, sig_names);
        let path = AssertedPath::parse(path).expect("path parses");
        validate(path, &ann, &interner, &[semver_crate()])
    }

    #[test]
    fn a_matching_inspected_assertion_validates() {
        let spec =
            validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("validates");
        assert_eq!(spec.sig.ipe_sig(), "Int -> Result Error Int");
        assert_eq!(spec.param_rust, vec!["i64".to_owned()]);
        assert_eq!(spec.result_rust, "i64");
        assert!(spec.def_name.starts_with("asserted_semver_frobnicate__"));
        assert!(
            spec.wrapper_ident
                .starts_with("ipe_asserted_semver_frobnicate__")
        );
    }

    #[test]
    fn an_uninstalled_crate_is_refused() {
        let err = validated(&["Int", "Result Error Int"], "nope::f").expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::TargetCrateNotInstalled { crate_ident }, ..
                } if crate_ident == "nope"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_clamp_requiring_target_is_refused_with_the_exact_carrier_named() {
        // `clamped` takes/returns u32 — Int (i64) would need a clamp, which the
        // exact-carrier rule forbids in an asserted shim.
        let err = validated(&["Int", "Result Error Int"], "semver::clamped").expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::InspectedMismatch { expected }, ..
                } if expected.contains("u32")
            ),
            "{err}"
        );
    }

    #[test]
    fn a_fallible_inspected_target_is_refused() {
        let err =
            validated(&["String", "Result Error Version"], "semver::parse").expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::InspectedShapeUnsupported { .. },
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn an_uninspected_symbol_passes_to_the_rustc_checker() {
        // Not in the inspection: no compile-time cross-check, the emitted shim
        // carries the asserted types and rustc is the checker of record.
        let spec = validated(&["Int", "Result Error Int"], "semver::not_inspected")
            .expect("passes to rustc");
        let shims = emit_asserted_shims(std::slice::from_ref(&spec));
        assert!(shims.contains("::semver::not_inspected(arg0)"), "{shims}");
    }

    #[test]
    fn a_total_result_is_refused() {
        let err = validated(&["Int", "Int"], "semver::frobnicate").expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::ResultShape { .. },
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn an_undeclared_opaque_is_refused() {
        let err =
            validated(&["Widget", "Result Error Int"], "semver::frobnicate").expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::OpaqueNotDeclared { name }, ..
                } if name == "Widget"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_declared_opaque_resolves_and_imports() {
        let spec = validated(
            &["Version", "Result Error Version"],
            "semver::not_inspected",
        )
        .expect("validates");
        assert_eq!(spec.param_rust, vec!["::semver::Version".to_owned()]);
        assert!(spec.opaque_imports.contains("Version"));
        let iface = render_asserted_interface(std::slice::from_ref(&spec));
        assert!(
            iface.contains("import Rust.Semver exposing (Version)"),
            "{iface}"
        );
    }

    #[test]
    fn conflicting_assertions_for_one_path_are_refused() {
        let a = validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("validates");
        let b =
            validated(&["Bool", "Result Error Int"], "semver::not_inspected").expect("validates");
        // Same path, different sig: rebuild `b` on `a`'s path.
        let conflicting = AssertedSpec {
            def_name: a.def_name.clone(),
            ..b
        };
        let err = dedupe(vec![a, conflicting]).expect_err("refused");
        assert!(
            matches!(
                &err,
                Diagnostic::AssertedRefused {
                    defect: AssertedDefect::ConflictingAssertions { .. },
                    ..
                }
            ),
            "{err}"
        );
        // Identical assertions deduplicate to one spec.
        let a2 = validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("validates");
        let a3 = validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("validates");
        assert_eq!(dedupe(vec![a2, a3]).expect("dedupes").len(), 1);
    }

    #[test]
    fn the_shim_is_exact_carrier_and_panic_bounded() {
        let spec = validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("ok");
        let shims = emit_asserted_shims(std::slice::from_ref(&spec));
        assert!(shims.contains("catch_unwind"), "{shims}");
        assert!(shims.contains("ipe_error_from_panic"), "{shims}");
        assert!(
            shims.contains(&format!(
                "pub fn {}(arg0: i64) -> IpeResult<IpeError, i64>",
                spec.wrapper_ident
            )),
            "{shims}"
        );
        // The exact-carrier rule, textually: no coercion helper reaches an
        // asserted shim.
        assert!(!shims.contains("num_coerce"), "{shims}");
        assert!(!shims.contains("clamp"), "{shims}");
        assert!(!shims.contains(" as "), "{shims}");
    }

    #[test]
    fn the_interface_renders_the_gated_asserted_body() {
        let spec = validated(&["Int", "Result Error Int"], "semver::frobnicate").expect("ok");
        let iface = render_asserted_interface(std::slice::from_ref(&spec));
        assert!(iface.starts_with("module Rust.Ffi exposing ("), "{iface}");
        assert!(
            iface.contains(&format!(
                "{} : Int -> Result Error Int\n{} arg0 =\n    Ffi.asserted \"{}\" arg0\n",
                spec.def_name, spec.def_name, spec.wrapper_ident
            )),
            "{iface}"
        );
    }

    #[test]
    fn a_unit_param_signature_renders_the_zero_arg_shapes() {
        let spec =
            validated(&["()", "Result Error Int"], "semver::uninspected_zero").expect("validates");
        assert_eq!(spec.sig.ipe_sig(), "() -> Result Error Int");
        assert_eq!(spec.sig.arity(), 1);
        let shims = emit_asserted_shims(std::slice::from_ref(&spec));
        assert!(
            shims.contains("(_: ()) -> IpeResult<IpeError, i64>"),
            "{shims}"
        );
        assert!(shims.contains("::semver::uninspected_zero()"), "{shims}");
    }
}
