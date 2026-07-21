//! The closed carrier set for Ipê-defined Rust types (the `provide` surface).
//!
//! When Ipê DEFINES a Rust type — a struct field type, or a closure parameter
//! / result type — the type it names must be one the wrapper can lift an owned,
//! immutable Ipê value into and out of *totally*. That set is closed and small:
//! the scalar carriers plus a nominal opaque handle already vouched by the
//! crate's own inspection. Anything outside it is refused at the decode
//! boundary (over-drop the whole `provide` entry) rather than emitted as Rust
//! the wrapper cannot soundly coerce — the same parse-don't-validate discipline
//! the `PkgInfo` and `Call` boundaries hold.
//!
//! This module is a pure decode LEAF: it renders no Rust and touches no
//! sandbox path. It is the parse boundary the later `provide` emitters render
//! from, so no raw manifest string ever reaches generated source.

use crate::diag::WireDefect;
use crate::naming::RustIdent;

/// A type an Ipê-defined Rust struct field or closure component may carry.
///
/// Every variant maps to exactly one owned Rust type the existing
/// `owned_value_coercion` path can lift an Ipê value into; `Opaque` is a
/// nominal handle the crate's inspection already validated (its `RustIdent`
/// spelling, never a path — the path resolves through the crate's opaque map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carrier {
    /// The Ipê `Int` carrier (`i64`).
    Int,
    /// The Ipê `Float` carrier (`f64`).
    Float,
    /// The Ipê `Bool` carrier (`bool`).
    Bool,
    /// The Ipê `Char` carrier (`char`).
    Char,
    /// The Ipê `String` carrier (owned `String`).
    Str,
    /// The Ipê `Bytes` carrier (`Vec<u8>`).
    Bytes,
    /// A nominal opaque handle named by the crate — its type identifier, whose
    /// absolute path resolves through the crate's opaque-type map at emission.
    Opaque(RustIdent),
}

impl Carrier {
    /// Parse one carrier spelling as it appears in a `provide` manifest entry.
    ///
    /// The scalar spellings are the Ipê-facing carrier names AND their Rust
    /// spellings (both `i64` and `Int` name the integer carrier), so an author
    /// may write either. Any other capitalised identifier is taken as an opaque
    /// handle name and validated as a `RustIdent`.
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidType`] when the spelling is empty, is a bare
    /// lowercase word outside the scalar set (a would-be Rust primitive Ipê has
    /// no carrier for, e.g. `u128`/`str`), or is not a legal identifier.
    pub fn parse(s: &str) -> Result<Self, WireDefect> {
        let t = s.trim();
        let invalid = || WireDefect::InvalidType { got: s.to_owned() };
        match t {
            "i64" | "Int" => return Ok(Self::Int),
            "f64" | "Float" => return Ok(Self::Float),
            "bool" | "Bool" => return Ok(Self::Bool),
            "char" | "Char" => return Ok(Self::Char),
            "String" | "Str" => return Ok(Self::Str),
            "Bytes" => return Ok(Self::Bytes),
            _ => {}
        }
        // A lowercase-led word that was not a known scalar is a Rust primitive
        // or borrow Ipê cannot carry (`u32`, `usize`, `str`, `&T`) — refuse it
        // rather than misread it as an opaque handle.
        if !t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(invalid());
        }
        RustIdent::parse(t).map(Self::Opaque).map_err(|_| invalid())
    }

    /// The owned Rust type this carrier lowers to, for a scalar carrier. An
    /// [`Carrier::Opaque`] returns its bare handle name; the emitter absolutizes
    /// it through the crate's opaque map (this leaf never renders a path).
    #[must_use]
    pub fn rust_owned(&self) -> &str {
        match self {
            Self::Int => "i64",
            Self::Float => "f64",
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Str => "String",
            Self::Bytes => "Vec<u8>",
            Self::Opaque(id) => id.as_str(),
        }
    }

    /// The Ipê surface type this carrier presents to a consumer signature.
    #[must_use]
    pub fn ipe_surface(&self) -> &str {
        match self {
            Self::Int => "Int",
            Self::Float => "Float",
            Self::Bool => "Bool",
            Self::Char => "Char",
            Self::Str => "String",
            Self::Bytes => "Bytes",
            Self::Opaque(id) => id.as_str(),
        }
    }
}

impl Carrier {
    /// This carrier as a [`ScalarCarrier`], or [`None`] when it is an opaque
    /// handle. A total closure return must be a scalar (there is no default
    /// value for an opaque handle to yield when a call aborts), so the return
    /// parser projects through this.
    #[must_use]
    pub const fn as_scalar(&self) -> Option<ScalarCarrier> {
        match self {
            Self::Int => Some(ScalarCarrier::Int),
            Self::Float => Some(ScalarCarrier::Float),
            Self::Bool => Some(ScalarCarrier::Bool),
            Self::Char => Some(ScalarCarrier::Char),
            Self::Str => Some(ScalarCarrier::Str),
            Self::Bytes => Some(ScalarCarrier::Bytes),
            Self::Opaque(_) => None,
        }
    }
}

/// The scalar subset of [`Carrier`] — every variant EXCEPT the opaque handle.
///
/// A total closure return (`-> B` with no error channel) must be a scalar: an
/// opaque handle has no default value to yield if a call cannot produce one, so
/// `Total(Opaque)` is made unrepresentable by construction (an opaque return is
/// legal only inside `Result`/`Option`, where a failed call folds to
/// `Err`/`None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarCarrier {
    /// The Ipê `Int` carrier (`i64`).
    Int,
    /// The Ipê `Float` carrier (`f64`).
    Float,
    /// The Ipê `Bool` carrier (`bool`).
    Bool,
    /// The Ipê `Char` carrier (`char`).
    Char,
    /// The Ipê `String` carrier (owned `String`).
    Str,
    /// The Ipê `Bytes` carrier (`Vec<u8>`).
    Bytes,
}

impl ScalarCarrier {
    /// This scalar as the general [`Carrier`].
    #[must_use]
    pub const fn as_carrier(self) -> Carrier {
        match self {
            Self::Int => Carrier::Int,
            Self::Float => Carrier::Float,
            Self::Bool => Carrier::Bool,
            Self::Char => Carrier::Char,
            Self::Str => Carrier::Str,
            Self::Bytes => Carrier::Bytes,
        }
    }

    /// The owned Rust type this scalar lowers to.
    #[must_use]
    pub const fn rust_owned(self) -> &'static str {
        match self {
            Self::Int => "i64",
            Self::Float => "f64",
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Str => "String",
            Self::Bytes => "Vec<u8>",
        }
    }
}

/// A single closure bound. The bound set a `provide.closure` signature may
/// carry is exactly `{Send, Sync, 'static}` — a CLOSED enum, never free text,
/// so no bound spelling from the manifest reaches emitted Rust as a raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bound {
    /// The `Send` auto-trait bound.
    Send,
    /// The `Sync` auto-trait bound.
    Sync,
    /// The `'static` lifetime bound.
    Static,
}

impl Bound {
    /// Parse one bound token (`Send` / `Sync` / `'static`).
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Send" => Some(Self::Send),
            "Sync" => Some(Self::Sync),
            "'static" => Some(Self::Static),
            _ => None,
        }
    }

    /// The Rust spelling this bound renders to.
    #[must_use]
    pub const fn rust(self) -> &'static str {
        match self {
            Self::Send => "Send",
            Self::Sync => "Sync",
            Self::Static => "'static",
        }
    }
}

/// The closed bound set a closure signature carries. The adapter always
/// captures the Ipê function value by move into a `Send + Sync + 'static` box,
/// so `Send`, `Sync`, and `'static` are the only bounds a signature may name;
/// the set is rendered from these variants, never from raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSet(std::collections::BTreeSet<Bound>);

impl BoundSet {
    /// The three bounds the sync closure adapter always emits and requires.
    #[must_use]
    pub fn full() -> Self {
        Self(
            [Bound::Send, Bound::Sync, Bound::Static]
                .into_iter()
                .collect(),
        )
    }

    /// Whether the set contains every one of `Send`, `Sync`, `'static`.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self == &Self::full()
    }

    /// The bounds in canonical order, joined for a `+ …` suffix.
    #[must_use]
    pub fn rust_suffix(&self) -> String {
        self.0
            .iter()
            .map(|b| b.rust())
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// A closure's declared return, after the total-carrier-return soundness rule.
///
/// Exactly three shapes are representable. A total return is scalar-only
/// (MF-2): an opaque handle has no default to yield on a panic-abort, so it is
/// legal only inside the fallible shapes, where a failed call folds to
/// `Err`/`None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosureRet {
    /// A total scalar return (`-> i64`). A panic in the Ipê closure aborts the
    /// process — a total signature has no error channel to fold into.
    Total(ScalarCarrier),
    /// A `Result<B, E>` return. A panic folds to `Err` via the runtime error
    /// funnel; `B` may be any carrier (opaque included).
    Result(Carrier),
    /// An `Option<B>` return. A panic folds to `None`; `B` may be any carrier.
    Option(Carrier),
}

/// A fully-parsed `provide.closure` signature, rendered from ONLY closed
/// carriers and bounds — never from a raw manifest string. The emitter reads
/// this, exactly as `render_dep_line` reads `CrateVersion`/`FeatureName`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureSig {
    /// The closure's parameter carriers, in order.
    pub params: Vec<Carrier>,
    /// The closure's return, after the total-carrier rule.
    pub ret: ClosureRet,
    /// The closed bound set (must be `{Send, Sync, 'static}` for the sync
    /// adapter).
    pub bounds: BoundSet,
}

impl ClosureSig {
    /// Parse a `provide.closure` signature of the shape
    /// `Fn(P0, P1, …) -> R + Send + Sync + 'static`.
    ///
    /// Every fragment routes through [`Carrier::parse`] or the closed bound
    /// match; any unconsumed tail is a hard refusal (consume-and-assert-empty),
    /// so no manifest text ever reaches the emitted adapter as a raw string.
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidClosureSig`] naming the broken rule: a missing
    /// `Fn(...)` head, an unbalanced parameter list, a parameter or return
    /// component outside the carrier set, a bound outside `{Send, Sync,
    /// 'static}`, a total (non-`Result`/`Option`) return that is not a scalar,
    /// or unconsumed trailing text.
    pub fn parse(s: &str) -> Result<Self, WireDefect> {
        let raw = s;
        let refuse = |reason: &str| WireDefect::InvalidClosureSig {
            got: raw.to_owned(),
            reason: reason.to_owned(),
        };
        let t = s.trim();
        // A leading `dyn ` / `Box<dyn ` is tolerated so an author may paste the
        // exact crate spelling; only `Fn` is accepted (sync, immutable).
        let t = t.strip_prefix("Box<dyn ").map_or(t, |r| r);
        let t = t.strip_prefix("dyn ").unwrap_or(t).trim();
        let after_fn = t
            .strip_prefix("Fn")
            .ok_or_else(|| refuse("signature must begin with `Fn`"))?
            .trim_start();
        let after_open = after_fn
            .strip_prefix('(')
            .ok_or_else(|| refuse("`Fn` must be followed by a `(` parameter list"))?;
        let close = after_open
            .find(')')
            .ok_or_else(|| refuse("unterminated `(` parameter list"))?;
        let params_src = after_open.get(..close).unwrap_or("");
        let mut params = Vec::new();
        for p in params_src.split(',') {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            params.push(
                Carrier::parse(p)
                    .map_err(|_| refuse(&format!("parameter `{p}` is outside the carrier set")))?,
            );
        }
        let mut rest = after_open.get(close + 1..).unwrap_or("").trim();
        // Drop an optional trailing `>` left by a `Box<dyn …>` wrapper.
        if let Some(stripped) = rest.strip_suffix('>') {
            rest = stripped.trim_end();
        }
        // The return arrow is mandatory: a `-> R` names the value the crate
        // consumes; a bare `Fn(...)` (unit return) is deferred (no P2 fixture).
        let after_arrow = rest
            .strip_prefix("->")
            .ok_or_else(|| refuse("closure must declare a `-> return` type"))?
            .trim_start();
        // Split the return type from the trailing `+ Bound` list at the first
        // top-level `+` (respecting `<…>` nesting so `Result<i64, E>` stays
        // whole). Everything before is the return; everything after is bounds.
        let (ret_src, bounds_src) = split_ret_and_bounds(after_arrow);
        let ret = parse_ret(ret_src.trim()).map_err(|reason| refuse(&reason))?;
        let mut bounds = std::collections::BTreeSet::new();
        for b in bounds_src.split('+') {
            let b = b.trim();
            if b.is_empty() {
                continue;
            }
            let bound = Bound::parse(b).ok_or_else(|| {
                refuse(&format!("bound `{b}` is outside {{Send, Sync, 'static}}"))
            })?;
            bounds.insert(bound);
        }
        Ok(Self {
            params,
            ret,
            bounds: BoundSet(bounds),
        })
    }

    /// The Rust `dyn Fn(...) -> R + …` type this signature renders to (without
    /// the `Box<>` wrapper), from closed carriers/bounds only.
    #[must_use]
    pub fn rust_dyn_fn(&self) -> String {
        let params = self
            .params
            .iter()
            .map(Carrier::rust_owned)
            .collect::<Vec<_>>()
            .join(", ");
        let ret = match &self.ret {
            ClosureRet::Total(sc) => sc.rust_owned().to_owned(),
            ClosureRet::Result(c) => format!("Result<{}, IpeError>", c.rust_owned()),
            ClosureRet::Option(c) => format!("Option<{}>", c.rust_owned()),
        };
        let bounds = self.bounds.rust_suffix();
        if bounds.is_empty() {
            format!("dyn Fn({params}) -> {ret}")
        } else {
            format!("dyn Fn({params}) -> {ret} + {bounds}")
        }
    }
}

/// Split a `R + Bound + Bound` tail into `(return-type, bounds)` at the first
/// `+` that sits at angle-bracket depth zero, so `Result<i64, E> + Send` splits
/// after the `>`.
fn split_ret_and_bounds(s: &str) -> (&str, &str) {
    let mut depth = 0_i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            '+' if depth == 0 => {
                return (s.get(..i).unwrap_or(s), s.get(i + 1..).unwrap_or(""));
            }
            _ => {}
        }
    }
    (s, "")
}

/// Parse a closure return type into the closed [`ClosureRet`], enforcing the
/// total-carrier-return rule: a bare (non-`Result`/`Option`) return must be a
/// scalar carrier.
fn parse_ret(s: &str) -> Result<ClosureRet, String> {
    let inner_of = |head: &str| -> Option<&str> {
        s.strip_prefix(head)
            .and_then(|r| r.strip_suffix('>'))
            .map(str::trim)
    };
    if let Some(inner) = inner_of("Result<") {
        // `Result<B, E>` or `Result<B>`: the error half is funnelled through
        // the runtime error type and never named on the Ipê surface, so only
        // the Ok carrier is parsed; a present error half is accepted and
        // discarded (it renders as the runtime `IpeError`).
        let ok = inner.split(',').next().unwrap_or(inner).trim();
        let c = Carrier::parse(ok)
            .map_err(|_| format!("`Result` Ok type `{ok}` is outside the carrier set"))?;
        return Ok(ClosureRet::Result(c));
    }
    if let Some(inner) = inner_of("Option<") {
        let c = Carrier::parse(inner)
            .map_err(|_| format!("`Option` type `{inner}` is outside the carrier set"))?;
        return Ok(ClosureRet::Option(c));
    }
    // A total return: MUST be a scalar carrier. An opaque handle has no default
    // to yield on a panic-abort, so it is refused here (representable only
    // inside Result/Option).
    let c =
        Carrier::parse(s).map_err(|_| format!("return type `{s}` is outside the carrier set"))?;
    c.as_scalar().map(ClosureRet::Total).ok_or_else(|| {
        format!(
            "a total return `{s}` must be a scalar carrier — an opaque handle return \
             is representable only inside `Result`/`Option`"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_spellings_parse_by_either_name() {
        for (rust, ipe, carrier) in [
            ("i64", "Int", Carrier::Int),
            ("f64", "Float", Carrier::Float),
            ("bool", "Bool", Carrier::Bool),
            ("char", "Char", Carrier::Char),
            ("String", "Str", Carrier::Str),
        ] {
            assert_eq!(Carrier::parse(rust), Ok(carrier.clone()), "{rust}");
            assert_eq!(Carrier::parse(ipe), Ok(carrier.clone()), "{ipe}");
        }
        assert_eq!(Carrier::parse("Bytes"), Ok(Carrier::Bytes));
        // Whitespace is trimmed.
        assert_eq!(Carrier::parse("  Int  "), Ok(Carrier::Int));
    }

    #[test]
    fn a_capitalised_word_is_an_opaque_handle() {
        let c = Carrier::parse("Counter").expect("opaque");
        assert_eq!(c, Carrier::Opaque(RustIdent::parse("Counter").unwrap()));
        assert_eq!(c.rust_owned(), "Counter");
        assert_eq!(c.ipe_surface(), "Counter");
    }

    #[test]
    fn rust_primitives_without_an_ipe_carrier_are_refused() {
        // Widths Ipê collapses to Int/Float on the READ side have no carrier on
        // the DEFINE side (Ipê only offers i64/f64), so a struct field cannot
        // name them — refuse rather than silently widen and mis-coerce.
        for bad in ["u8", "u32", "u64", "usize", "i32", "f32", "str", "isize"] {
            assert!(
                matches!(Carrier::parse(bad), Err(WireDefect::InvalidType { .. })),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn injection_and_borrow_shapes_die_at_the_boundary() {
        for bad in [
            "",
            "   ",
            "&Counter",
            "Vec<u8>",
            "Box<dyn Fn()>",
            "String; std::process::exit(1)",
            "A B",
            "9lives",
        ] {
            assert!(
                matches!(Carrier::parse(bad), Err(WireDefect::InvalidType { .. })),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn owned_rust_and_ipe_surface_agree_with_the_existing_coercion_table() {
        // These are exactly the owned types `ipe_type_to_rust` /
        // `owned_value_coercion` already lift, so a struct built from them uses
        // the existing inbound path unchanged.
        assert_eq!(Carrier::Int.rust_owned(), "i64");
        assert_eq!(Carrier::Float.rust_owned(), "f64");
        assert_eq!(Carrier::Str.rust_owned(), "String");
        assert_eq!(Carrier::Bytes.rust_owned(), "Vec<u8>");
        assert_eq!(Carrier::Bool.ipe_surface(), "Bool");
    }

    // ── ClosureSig ──────────────────────────────────────────────────────────

    #[test]
    fn a_total_scalar_closure_parses_and_renders() {
        let sig = ClosureSig::parse("Fn(Counter, Message) -> Counter + Send + Sync + 'static");
        // `Counter` return is opaque → refused as a TOTAL return.
        assert!(sig.is_err(), "opaque total return must be refused");

        let sig = ClosureSig::parse("Fn(Int, Bool) -> Int + Send + Sync + 'static")
            .expect("scalar total closure parses");
        assert_eq!(sig.params, vec![Carrier::Int, Carrier::Bool]);
        assert_eq!(sig.ret, ClosureRet::Total(ScalarCarrier::Int));
        assert!(sig.bounds.is_full());
        assert_eq!(
            sig.rust_dyn_fn(),
            "dyn Fn(i64, bool) -> i64 + Send + Sync + 'static"
        );
    }

    #[test]
    fn an_opaque_return_is_legal_inside_result_and_option() {
        let r = ClosureSig::parse("Fn(Counter) -> Result<Counter, Error> + Send + Sync + 'static")
            .expect("Result<opaque> parses");
        assert_eq!(
            r.ret,
            ClosureRet::Result(Carrier::Opaque(RustIdent::parse("Counter").unwrap()))
        );
        assert_eq!(
            r.rust_dyn_fn(),
            "dyn Fn(Counter) -> Result<Counter, IpeError> + Send + Sync + 'static"
        );
        let o = ClosureSig::parse("Fn(Int) -> Option<Counter> + Send + Sync + 'static")
            .expect("Option<opaque> parses");
        assert_eq!(
            o.ret,
            ClosureRet::Option(Carrier::Opaque(RustIdent::parse("Counter").unwrap()))
        );
    }

    #[test]
    fn a_box_dyn_wrapper_spelling_is_tolerated() {
        let sig = ClosureSig::parse("Box<dyn Fn(Int) -> Bool + Send + Sync + 'static>")
            .expect("Box<dyn …> spelling parses");
        assert_eq!(sig.ret, ClosureRet::Total(ScalarCarrier::Bool));
        assert!(sig.bounds.is_full());
    }

    #[test]
    fn a_bound_outside_the_closed_set_is_refused() {
        for bad in [
            "Fn(Int) -> Int + Send + Clone",
            "Fn(Int) -> Int + 'a",
            "Fn(Int) -> Int + Debug",
        ] {
            assert!(
                matches!(
                    ClosureSig::parse(bad),
                    Err(WireDefect::InvalidClosureSig { .. })
                ),
                "{bad:?} must be refused (bound outside {{Send, Sync, 'static}})"
            );
        }
    }

    #[test]
    fn a_total_opaque_return_is_unrepresentable() {
        // The single new soundness rule: a total (non-Result/Option) return
        // must be a scalar. An opaque handle has no default to yield on a
        // panic-abort, so it is refused at parse — never a runtime surprise.
        assert!(matches!(
            ClosureSig::parse("Fn(Int) -> Widget + Send + Sync + 'static"),
            Err(WireDefect::InvalidClosureSig { .. })
        ));
    }

    #[test]
    fn injection_and_malformed_signatures_die_at_the_boundary() {
        for bad in [
            "",
            "   ",
            // no Fn head
            "(Int) -> Int",
            // unterminated param list
            "Fn(Int -> Int",
            // param outside the carrier set
            "Fn(u128) -> Int + Send + Sync + 'static",
            // no return arrow
            "Fn(Int) + Send",
            // return outside carrier set
            "Fn(Int) -> Vec<u8> + Send + Sync + 'static",
            // statement-injection payload in the return position
            "Fn(Int) -> Int; std::process::exit(1) + Send",
            // injection payload in a bound position
            "Fn(Int) -> Int + Send } fn evil() {}",
            // garbage trailing tail after the bounds
            "Fn(Int) -> Int + Send Sync",
        ] {
            assert!(
                matches!(
                    ClosureSig::parse(bad),
                    Err(WireDefect::InvalidClosureSig { .. })
                ),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_result_signature_ignores_the_error_half_and_keeps_the_ok_carrier() {
        let sig = ClosureSig::parse("Fn(String) -> Result<Int, Error> + Send + Sync + 'static")
            .expect("parses");
        assert_eq!(sig.ret, ClosureRet::Result(Carrier::Int));
        assert_eq!(
            sig.rust_dyn_fn(),
            "dyn Fn(String) -> Result<i64, IpeError> + Send + Sync + 'static"
        );
    }
}
