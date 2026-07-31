//! The `Rust.Ffi.call` asserted-call vocabulary: the validated Rust path a
//! call site names, and the deterministic identifiers derived from it.
//!
//! Two independent consumers must agree byte-for-byte on those identifiers —
//! the build driver (which generates the `Rust.Ffi` interface module and the
//! `_bindings.rs` shim) and the resolver (which rewrites a user's
//! `Rust.Ffi.call "path"` into a reference to the generated definition). This
//! module is their single source; neither derives a name any other way.

/// The dotted module every asserted binding lives in. Driver-generated with
/// [`crate::ModuleOrigin::FfiInterface`]; the resolver rewrites user call
/// sites into references to its definitions.
pub const ASSERTED_MODULE: &str = "Rust.Ffi";

/// The reserved `_bindings.rs` identifier prefix of every asserted shim.
///
/// Installed-crate wrapper identifiers derive from `<slug>_<fn>` and a slug
/// cannot begin with `ipe_asserted` under the crate-name gate, but the driver
/// still refuses any installed crate claiming the prefix — the prefix is what
/// classifies a lowered foreign call as asserted, so it must be unforgeable.
pub const ASSERTED_WRAPPER_PREFIX: &str = "ipe_asserted_";

/// A validated `crate::path::function` target of an asserted call.
///
/// Parsing is the only constructor: an ill-formed path — empty, one segment,
/// an illegal identifier character, generics — is unrepresentable, so no
/// free-text path ever reaches a derived identifier or generated Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertedPath {
    raw: String,
    segments: Vec<String>,
}

impl AssertedPath {
    /// Parse a `crate::path::function` string.
    ///
    /// # Errors
    /// A human-readable rule name (the caller wraps it in
    /// [`ipe_diagnostics::NameError::AssertedCallMalformed`]).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("the path is empty".to_owned());
        }
        if trimmed != raw {
            return Err("the path has leading or trailing whitespace".to_owned());
        }
        let segments: Vec<&str> = raw.split("::").collect();
        if segments.len() < 2 {
            return Err(format!(
                "`{raw}` has no `::` — the path must name both the crate and the function \
                 (`<crate>::<function>`)"
            ));
        }
        for seg in &segments {
            if seg.is_empty() {
                return Err(format!("`{raw}` has an empty path segment"));
            }
            let mut chars = seg.chars();
            let head_ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            if !head_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!(
                    "`{seg}` is not a plain Rust path segment (letters, digits, `_`; \
                     no generics or spaces)"
                ));
            }
        }
        Ok(Self {
            raw: raw.to_owned(),
            segments: segments.into_iter().map(str::to_owned).collect(),
        })
    }

    /// The full path as written (`some_crate::frobnicate`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// The crate the call targets — the first segment, as it appears in Rust
    /// `use` position (underscored, never hyphenated).
    #[must_use]
    pub fn crate_ident(&self) -> &str {
        // Parsing guarantees ≥ 2 segments; `first` keeps the accessor total.
        self.segments.first().map_or("", String::as_str)
    }

    /// The final segment — the function name.
    #[must_use]
    pub fn fn_name(&self) -> &str {
        self.segments.last().map_or("", String::as_str)
    }

    /// Whether the path is exactly `crate::function` (no intermediate
    /// modules) — the only shape the inspected-signature cross-check can look
    /// up, since inspection records crate-top-level names.
    #[must_use]
    pub const fn is_crate_top_level(&self) -> bool {
        self.segments.len() == 2
    }

    /// The absolute call path the generated shim invokes (`::a::b::c`).
    #[must_use]
    pub fn rust_call_path(&self) -> String {
        format!("::{}", self.segments.join("::"))
    }

    /// The Ipê definition name in the generated `Rust.Ffi` module.
    ///
    /// Lowercased segments keep the emitted identifiers `snake_case`
    /// regardless of associated-fn path casing; the hash of the ORIGINAL path
    /// disambiguates case-folded collisions (`Version::parse` vs
    /// `version::parse`) and segmentation collisions (`a::b_c` vs `a_b::c`).
    #[must_use]
    pub fn def_name(&self) -> String {
        format!("asserted_{}", self.mangled_tail())
    }

    /// The `_bindings.rs` shim identifier (`ipe_asserted_…`).
    #[must_use]
    pub fn wrapper_ident(&self) -> String {
        format!("{ASSERTED_WRAPPER_PREFIX}{}", self.mangled_tail())
    }

    fn mangled_tail(&self) -> String {
        let joined = self
            .segments
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("_");
        // Deliberate 32-bit fold of the 64-bit hash: eight hex chars keep the
        // identifiers readable, and the driver refuses the (astronomically
        // unlikely) collision of two distinct paths loudly at dedupe.
        let folded = u32::try_from(fnv1a64(&self.raw) & u64::from(u32::MAX)).unwrap_or_default();
        format!("{joined}__{folded:08x}")
    }
}

/// One well-formed asserted-call site found by [`scan_module`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertedUse {
    /// The validated target path.
    pub path: AssertedPath,
    /// The author's annotation — the asserted signature, still source-shaped.
    pub annotation: ipe_syntax::TypeAnnotation,
    /// The call site's span, for blame.
    pub span: ipe_diagnostics::Span,
}

/// Scan one parsed module for asserted-call sites, enforcing the ONE accepted
/// shape: a top-level annotated zero-parameter definition whose whole body is
/// `Rust.Ffi.call "<path>"`.
///
/// The build driver runs this over every user module before compilation; the
/// resolver's rewrite ([`crate::canonicalise`]) recognises the same callee
/// spelling, so the two can never disagree about what an asserted site is.
/// Any OTHER occurrence of `Rust.Ffi.call` — nested in an expression, missing
/// its annotation, taking parameters — is refused here, fail-closed, with the
/// span of the offending use.
///
/// # Errors
/// [`ipe_diagnostics::NameError::AssertedCallMalformed`] (IPE-N0038) at the
/// first malformed site.
pub fn scan_module(
    module: &ipe_syntax::Module,
    interner: &ipe_intern::Interner,
) -> Result<Vec<AssertedUse>, ipe_diagnostics::Diagnostic> {
    let mut uses = Vec::new();
    for value in &module.values {
        let v = &value.value;
        if let ipe_syntax::Expr_::Call(callee, args) = &v.body.value
            && is_asserted_callee(callee, interner)
        {
            let malformed =
                |span: ipe_diagnostics::Span, detail: String| ipe_diagnostics::Diagnostic::Name {
                    span,
                    msg: ipe_diagnostics::NameError::AssertedCallMalformed {
                        detail: detail.into_boxed_str(),
                    },
                };
            if !v.patterns.is_empty() {
                return Err(malformed(
                    value.span,
                    "the definition takes parameters on the left — the annotation \
                     carries the whole arrow type and callers apply the value"
                        .to_owned(),
                ));
            }
            let Some(annotation) = &v.type_annotation else {
                return Err(malformed(
                    value.span,
                    "the definition has no type annotation — the annotation IS the \
                     asserted foreign signature"
                        .to_owned(),
                ));
            };
            let [path_expr] = args.as_slice() else {
                return Err(malformed(
                    callee.span,
                    "it must be applied to exactly one string-literal path".to_owned(),
                ));
            };
            let ipe_syntax::Expr_::Str(raw) = &path_expr.value else {
                return Err(malformed(
                    path_expr.span,
                    "the path must be a string literal, never a computed value".to_owned(),
                ));
            };
            let path =
                AssertedPath::parse(raw).map_err(|detail| malformed(path_expr.span, detail))?;
            uses.push(AssertedUse {
                path,
                annotation: annotation.value.clone(),
                span: callee.span,
            });
            continue;
        }
        // Not the accepted whole-body shape: any asserted-call spelling
        // anywhere inside this body is misplaced.
        if let Some(span) = find_asserted_use(&v.body, interner) {
            return Err(ipe_diagnostics::Diagnostic::Name {
                span,
                msg: ipe_diagnostics::NameError::AssertedCallMalformed {
                    detail: "an asserted call must be the ENTIRE body of a top-level \
                             annotated definition, not part of a larger expression"
                        .into(),
                },
            });
        }
    }
    Ok(uses)
}

/// Whether `callee` is the `Rust.Ffi.call` qualified reference.
fn is_asserted_callee(callee: &ipe_syntax::Expr, interner: &ipe_intern::Interner) -> bool {
    let ipe_syntax::Expr_::VarQual(qualifier, member) = &callee.value else {
        return false;
    };
    interner.resolve(*qualifier) == Some(ASSERTED_MODULE)
        && interner.resolve(*member) == Some("call")
}

/// The span of the first `Rust.Ffi.call` reference anywhere in `expr`, if any.
fn find_asserted_use(
    expr: &ipe_syntax::Expr,
    interner: &ipe_intern::Interner,
) -> Option<ipe_diagnostics::Span> {
    use ipe_syntax::Expr_;
    if is_asserted_callee(expr, interner) {
        return Some(expr.span);
    }
    match &expr.value {
        Expr_::Call(callee, args) => find_asserted_use(callee, interner)
            .or_else(|| args.iter().find_map(|a| find_asserted_use(a, interner))),
        Expr_::Case(scrutinee, arms) => find_asserted_use(scrutinee, interner).or_else(|| {
            arms.iter()
                .find_map(|(_, b)| find_asserted_use(b, interner))
        }),
        Expr_::Lambda(_, body) | Expr_::Access(body, _) => find_asserted_use(body, interner),
        Expr_::Binops(pairs, last) => pairs
            .iter()
            .find_map(|(e, _)| find_asserted_use(e, interner))
            .or_else(|| find_asserted_use(last, interner)),
        Expr_::Let(bindings, body) => bindings
            .iter()
            .find_map(|b| find_asserted_use(&b.body, interner))
            .or_else(|| find_asserted_use(body, interner)),
        Expr_::If(branches, else_expr) => branches
            .iter()
            .find_map(|(c, b)| {
                find_asserted_use(c, interner).or_else(|| find_asserted_use(b, interner))
            })
            .or_else(|| find_asserted_use(else_expr, interner)),
        Expr_::Tuple(items) | Expr_::List(items) => {
            items.iter().find_map(|e| find_asserted_use(e, interner))
        }
        Expr_::Record(fields) => fields
            .iter()
            .find_map(|(_, e)| find_asserted_use(e, interner)),
        Expr_::Update(_, fields) => fields
            .iter()
            .find_map(|(_, e)| find_asserted_use(e, interner)),
        Expr_::VarLocal(_)
        | Expr_::VarQual(..)
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::MultilineStr { .. }
        | Expr_::Char(_)
        | Expr_::PathLit(_)
        | Expr_::Unit => None,
    }
}

/// FNV-1a over the path bytes — deterministic across processes and platforms
/// (no `DefaultHasher` seed), which the golden byte-identity gate requires.
fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::AssertedPath;

    #[test]
    fn a_plain_two_segment_path_parses() {
        let p = AssertedPath::parse("some_crate::frobnicate").expect("parses");
        assert_eq!(p.crate_ident(), "some_crate");
        assert_eq!(p.fn_name(), "frobnicate");
        assert!(p.is_crate_top_level());
        assert_eq!(p.rust_call_path(), "::some_crate::frobnicate");
    }

    #[test]
    fn an_associated_fn_path_parses_and_lowercases_its_names() {
        let p = AssertedPath::parse("semver::Version::parse").expect("parses");
        assert!(!p.is_crate_top_level());
        assert!(p.def_name().starts_with("asserted_semver_version_parse__"));
        assert!(
            p.wrapper_ident()
                .starts_with("ipe_asserted_semver_version_parse__")
        );
    }

    #[test]
    fn the_hash_separates_case_folded_and_resegmented_collisions() {
        let a = AssertedPath::parse("semver::Version::parse").expect("parses");
        let b = AssertedPath::parse("semver::version::parse").expect("parses");
        let c = AssertedPath::parse("semver_version::parse").expect("parses");
        assert_ne!(a.def_name(), b.def_name());
        assert_ne!(b.def_name(), c.def_name());
    }

    #[test]
    fn names_are_deterministic() {
        let a = AssertedPath::parse("some_crate::frobnicate").expect("parses");
        let b = AssertedPath::parse("some_crate::frobnicate").expect("parses");
        assert_eq!(a.def_name(), b.def_name());
        assert_eq!(a.wrapper_ident(), b.wrapper_ident());
    }

    #[test]
    fn malformed_paths_are_refused() {
        for bad in [
            "",
            "frobnicate",
            "some_crate::",
            "::frobnicate",
            "a::::b",
            "a::b<c>",
            "a::b c",
            "a::b; use std",
            " a::b",
            "a-b::c",
        ] {
            assert!(AssertedPath::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
