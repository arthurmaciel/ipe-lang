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

/// A single closure bound.
///
/// The bound set a `provide.closure` signature may carry is exactly
/// `{Send, Sync, 'static}` — a CLOSED enum, never free text, so no bound
/// spelling from the manifest reaches emitted Rust as a raw string.
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

/// The closed bound set a closure signature carries.
///
/// The adapter always captures the Ipê function value by move into a
/// `Send + Sync + 'static` box, so `Send`, `Sync`, and `'static` are the only
/// bounds a signature may name; the set is rendered from these variants, never
/// from raw text.
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

/// One allowlisted `#[derive]` a `provide.struct` / `provide.enum` may request.
///
/// The set is the `MODELLABLE_5` fence (`{Hash, Eq, Ord, Clone, Default}`) — the
/// two-way cross-crate assertion the parametric monomorphiser already relies on
/// — plus `Debug`, which every closed carrier implements totally (no IEEE-754
/// hazard) and which real crate trait bounds routinely require (Iced's
/// `Sandbox::Message: Debug`). Never free text, so no derive spelling from the
/// manifest reaches the emitted `#[derive(...)]` list as a raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Derive {
    /// `#[derive(Hash)]`.
    Hash,
    /// `#[derive(Eq)]` (implies `PartialEq`).
    Eq,
    /// `#[derive(Ord)]` (implies `PartialOrd`).
    Ord,
    /// `#[derive(Clone)]`.
    Clone,
    /// `#[derive(Default)]`.
    Default,
    /// `#[derive(Debug)]`. Total for every closed carrier (and every unit /
    /// scalar-payload variant), so it carries no IEEE-754 hazard — unlike
    /// `Eq`/`Ord`/`Hash`, a `Float` field/payload may derive it freely.
    Debug,
}

impl Derive {
    /// Parse one derive token against the closed allowlist.
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Hash" => Some(Self::Hash),
            "Eq" => Some(Self::Eq),
            "Ord" => Some(Self::Ord),
            "Clone" => Some(Self::Clone),
            "Default" => Some(Self::Default),
            "Debug" => Some(Self::Debug),
            _ => None,
        }
    }

    /// The Rust spelling this derive renders to.
    #[must_use]
    pub const fn rust(self) -> &'static str {
        match self {
            Self::Hash => "Hash",
            Self::Eq => "Eq",
            Self::Ord => "Ord",
            Self::Clone => "Clone",
            Self::Default => "Default",
            Self::Debug => "Debug",
        }
    }

    /// Whether this derive is unsound for a struct/enum carrying an IEEE-754
    /// field. `f64` has no total `Eq`/`Ord`/`Hash`, so a type with a `Float`
    /// field/payload may derive only `Clone`/`Default`/`Debug` — the cell the
    /// `MODELLABLE_5` fence already guards, re-checked here at the define
    /// boundary. `Debug` is total for `f64`, so it is NOT gated.
    const fn requires_total_eq(self) -> bool {
        matches!(self, Self::Hash | Self::Eq | Self::Ord)
    }
}

/// The closed derive set a `provide.struct` requests, rendered from allowlisted
/// variants only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveSet(std::collections::BTreeSet<Derive>);

impl DeriveSet {
    /// Parse a derive list, refusing any token outside the `MODELLABLE_5`
    /// allowlist and any `Eq`/`Ord`/`Hash` request on a struct that carries a
    /// `Float` field (no total equality on IEEE-754).
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidType`] naming the first offending derive.
    pub fn parse(tokens: &[String], has_float_field: bool) -> Result<Self, WireDefect> {
        let mut set = std::collections::BTreeSet::new();
        for tok in tokens {
            let d = Derive::parse(tok).ok_or_else(|| WireDefect::InvalidType {
                got: format!("derive `{tok}` is outside {{Hash, Eq, Ord, Clone, Default, Debug}}"),
            })?;
            if has_float_field && d.requires_total_eq() {
                return Err(WireDefect::InvalidType {
                    got: format!(
                        "derive `{}` on a struct with a Float field — IEEE-754 has no total \
                         Eq/Ord/Hash",
                        d.rust()
                    ),
                });
            }
            set.insert(d);
        }
        Ok(Self(set))
    }

    /// Whether the set is empty (a derive-free struct — no `#[derive]` line).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The derives in canonical order, joined for a `#[derive(...)]` attribute.
    #[must_use]
    pub fn rust_list(&self) -> String {
        self.0
            .iter()
            .map(|d| d.rust())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A fully-parsed `provide.struct` definition: the Rust type to define, its
/// owned fields, and the derive set — rendered from closed carriers/derives
/// only, never from raw manifest text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDef {
    /// The nominal Rust type name to define.
    pub name: RustIdent,
    /// The struct's fields in declaration order (each an owned carrier).
    pub fields: Vec<(RustIdent, Carrier)>,
    /// The closed derive set.
    pub derives: DeriveSet,
}

impl StructDef {
    /// Assemble a validated struct definition from decoded parts.
    ///
    /// `raw_fields` is `(field-name, carrier-spelling)` pairs; `raw_derives` is
    /// the requested derive tokens. Every field name gates through
    /// [`RustIdent`], every field type through [`Carrier::parse`], and the
    /// derive set through [`DeriveSet::parse`] (which also enforces the
    /// IEEE-754 fence against a `Float` field).
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidType`] naming the first offending name, field type,
    /// or derive; [`WireDefect::InvalidClosureSig`] is never produced here.
    pub fn parse(
        name: &str,
        raw_fields: &[(String, String)],
        raw_derives: &[String],
    ) -> Result<Self, WireDefect> {
        let name = RustIdent::parse(name).map_err(|_| WireDefect::InvalidType {
            got: format!("struct name `{name}`"),
        })?;
        let mut fields = Vec::with_capacity(raw_fields.len());
        let mut has_float = false;
        for (fname, ftype) in raw_fields {
            let field_name = RustIdent::parse(fname).map_err(|_| WireDefect::InvalidType {
                got: format!("field name `{fname}`"),
            })?;
            let carrier = Carrier::parse(ftype)?;
            if carrier == Carrier::Float {
                has_float = true;
            }
            fields.push((field_name, carrier));
        }
        let derives = DeriveSet::parse(raw_derives, has_float)?;
        Ok(Self {
            name,
            fields,
            derives,
        })
    }

    /// The first opaque field's handle name, or [`None`] when every field is a
    /// scalar carrier.
    ///
    /// A sound emit for an opaque field must resolve the handle through the
    /// crate's opaque-type map (a bare `Version` is not in scope in the emitted
    /// `_bindings.rs`), which the first `provide.struct` increment does not yet
    /// thread — so the decode boundary refuses an opaque field for now, keeping
    /// the SEAL (no emitted-and-cargo-failing struct), and this projects the
    /// offending field name for the refusal.
    #[must_use]
    pub fn first_opaque_field(&self) -> Option<&str> {
        self.fields.iter().find_map(|(_, c)| match c {
            Carrier::Opaque(id) => Some(id.as_str()),
            _ => None,
        })
    }

    /// The struct definition + `#[derive]` lines this renders to, from closed
    /// carriers/derives only. The opaque-field absolutization is the emitter's
    /// job (this leaf never renders a crate path); a scalar field renders its
    /// owned Rust type directly.
    #[must_use]
    pub fn definition_lines(&self, opaque_rust_ty: &dyn Fn(&RustIdent) -> String) -> Vec<String> {
        let mut out = Vec::new();
        if !self.derives.is_empty() {
            out.push(format!("#[derive({})]", self.derives.rust_list()));
        }
        out.push(format!("pub struct {} {{", self.name.as_str()));
        for (fname, carrier) in &self.fields {
            let ty = match carrier {
                Carrier::Opaque(id) => opaque_rust_ty(id),
                _ => carrier.rust_owned().to_owned(),
            };
            out.push(format!("    pub {}: {ty},", fname.as_str()));
        }
        out.push("}".to_owned());
        out
    }
}

/// One variant of a `provide.enum`: a name and its tuple-payload carriers.
///
/// An empty payload is a unit variant. Named-field (struct) variants are not
/// represented — a `Message` enum's variants are unit or positional, and a tuple
/// variant already covers every payload the closed carrier set can carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariant {
    /// The Rust variant identifier.
    pub name: RustIdent,
    /// The variant's positional payload carriers, in order (empty ⇒ unit).
    pub payload: Vec<Carrier>,
}

/// A fully-parsed `provide.enum` definition: the Rust enum, its variants, and
/// the derive set.
///
/// Rendered from closed carriers/derives only, never from raw manifest text —
/// the `StructDef` discipline generalised to a sum: every variant name gates
/// through [`RustIdent`], every payload carrier through [`Carrier::parse`], and
/// the derive set through [`DeriveSet::parse`] (the IEEE-754 fence fires if ANY
/// variant carries a `Float` payload).
///
/// This is the P4 form of the `provide` roadmap: an Ipê union → a Rust enum, the
/// shape an Iced/TEA `Message` needs. Like `provide.struct`, it solves "define a
/// Rust type" with ZERO new trust surface — the emitted definition and every
/// variant constructor are total functions of decode-validated data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDef {
    /// The nominal Rust enum name to define.
    pub name: RustIdent,
    /// The enum's variants in declaration order.
    pub variants: Vec<EnumVariant>,
    /// The closed derive set.
    pub derives: DeriveSet,
}

impl EnumDef {
    /// Assemble a validated enum definition from decoded parts.
    ///
    /// `raw_variants` is `(variant-name, payload-carrier-spellings)` pairs in
    /// declaration order; `raw_derives` is the requested derive tokens. Every
    /// variant name gates through [`RustIdent`], every payload spelling through
    /// [`Carrier::parse`], and the derive set through [`DeriveSet::parse`] (which
    /// enforces the IEEE-754 fence against any `Float` payload). An enum with no
    /// variants is refused (an uninhabited type no constructor can build).
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidType`] naming the first offending name, payload type,
    /// or derive, or reporting a variantless enum.
    pub fn parse(
        name: &str,
        raw_variants: &[(String, Vec<String>)],
        raw_derives: &[String],
    ) -> Result<Self, WireDefect> {
        let name = RustIdent::parse(name).map_err(|_| WireDefect::InvalidType {
            got: format!("enum name `{name}`"),
        })?;
        if raw_variants.is_empty() {
            return Err(WireDefect::InvalidType {
                got: format!("enum `{}` has no variants", name.as_str()),
            });
        }
        let mut variants = Vec::with_capacity(raw_variants.len());
        let mut has_float = false;
        for (vname, payload_types) in raw_variants {
            let variant_name = RustIdent::parse(vname).map_err(|_| WireDefect::InvalidType {
                got: format!("variant name `{vname}`"),
            })?;
            let mut payload = Vec::with_capacity(payload_types.len());
            for pty in payload_types {
                let carrier = Carrier::parse(pty)?;
                if carrier == Carrier::Float {
                    has_float = true;
                }
                payload.push(carrier);
            }
            variants.push(EnumVariant {
                name: variant_name,
                payload,
            });
        }
        let derives = DeriveSet::parse(raw_derives, has_float)?;
        Ok(Self {
            name,
            variants,
            derives,
        })
    }

    /// The first opaque payload carrier's handle name, or [`None`] when every
    /// variant carries scalar carriers only.
    ///
    /// A sound emit for an opaque payload must resolve the handle through the
    /// crate's opaque-type map, which the first `provide.enum` increment does not
    /// yet thread — so the decode boundary refuses an opaque payload for now,
    /// keeping the SEAL (no emitted-and-cargo-failing enum). Scalar payloads and
    /// unit variants are fully supported.
    #[must_use]
    pub fn first_opaque_payload(&self) -> Option<&str> {
        self.variants.iter().find_map(|v| {
            v.payload.iter().find_map(|c| match c {
                Carrier::Opaque(id) => Some(id.as_str()),
                _ => None,
            })
        })
    }

    /// The enum definition + `#[derive]` lines this renders to, from closed
    /// carriers/derives only. A unit variant renders bare (`Increment,`); a
    /// payload-bearing variant renders a tuple (`SetValue(i64),`). Opaque-payload
    /// absolutization is the emitter's job (this leaf never renders a crate
    /// path); a scalar payload renders its owned Rust type directly.
    #[must_use]
    pub fn definition_lines(&self, opaque_rust_ty: &dyn Fn(&RustIdent) -> String) -> Vec<String> {
        let mut out = Vec::new();
        if !self.derives.is_empty() {
            out.push(format!("#[derive({})]", self.derives.rust_list()));
        }
        out.push(format!("pub enum {} {{", self.name.as_str()));
        for v in &self.variants {
            if v.payload.is_empty() {
                out.push(format!("    {},", v.name.as_str()));
            } else {
                let tys: Vec<String> = v
                    .payload
                    .iter()
                    .map(|c| match c {
                        Carrier::Opaque(id) => opaque_rust_ty(id),
                        _ => c.rust_owned().to_owned(),
                    })
                    .collect();
                out.push(format!("    {}({}),", v.name.as_str(), tys.join(", ")));
            }
        }
        out.push("}".to_owned());
        out
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

/// A fully-parsed `provide.closure` signature.
///
/// Rendered from ONLY closed carriers and bounds — never from a raw manifest
/// string. The emitter reads this, exactly as `render_dep_line` reads
/// `CrateVersion`/`FeatureName`.
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
        let mut tail = after_open.get(close + 1..).unwrap_or("").trim();
        // Drop an optional trailing `>` left by a `Box<dyn …>` wrapper.
        if let Some(stripped) = tail.strip_suffix('>') {
            tail = stripped.trim_end();
        }
        // The return arrow is mandatory: a `-> R` names the value the crate
        // consumes; a bare `Fn(...)` unit-return closure is not yet supported.
        let after_arrow = tail
            .strip_prefix("->")
            .ok_or_else(|| refuse("closure must declare a `-> return` type"))?
            .trim_start();
        // Split the return type from the trailing `+ Bound` list at the first
        // top-level `+` (respecting `<…>` nesting so `Result<i64, E>` stays
        // whole). Everything before is the return; everything after is bounds.
        let (ret_text, bound_list) = split_ret_and_bounds(after_arrow);
        let ret = parse_ret(ret_text.trim()).map_err(|reason| refuse(&reason))?;
        let mut bounds = std::collections::BTreeSet::new();
        for b in bound_list.split('+') {
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

    // ── StructDef / DeriveSet ────────────────────────────────────────────────

    #[test]
    fn a_scalar_struct_parses_and_renders_a_definition() {
        let s = StructDef::parse(
            "Counter",
            &[("value".to_owned(), "i64".to_owned())],
            &["Default".to_owned(), "Clone".to_owned()],
        )
        .expect("scalar struct parses");
        assert_eq!(s.name.as_str(), "Counter");
        assert_eq!(s.derives.rust_list(), "Clone, Default");
        let lines = s.definition_lines(&|id| format!("demo::{}", id.as_str()));
        assert_eq!(
            lines,
            vec![
                "#[derive(Clone, Default)]".to_owned(),
                "pub struct Counter {".to_owned(),
                "    pub value: i64,".to_owned(),
                "}".to_owned(),
            ]
        );
    }

    #[test]
    fn an_opaque_field_absolutizes_through_the_emitter_hook() {
        let s = StructDef::parse("Wrap", &[("inner".to_owned(), "Widget".to_owned())], &[])
            .expect("opaque-field struct parses");
        let lines = s.definition_lines(&|id| format!("demo::{}", id.as_str()));
        // No derives ⇒ no `#[derive]` line; the opaque field absolutizes.
        assert_eq!(
            lines,
            vec![
                "pub struct Wrap {".to_owned(),
                "    pub inner: demo::Widget,".to_owned(),
                "}".to_owned(),
            ]
        );
    }

    #[test]
    fn a_derive_outside_the_allowlist_is_refused() {
        for bad in ["Serialize", "Copy", "PartialOrd", "Send"] {
            assert!(
                matches!(
                    DeriveSet::parse(&[bad.to_owned()], false),
                    Err(WireDefect::InvalidType { .. })
                ),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn debug_is_allowlisted_and_total_on_a_float() {
        // Iced's `Sandbox::Message: Debug` bound needs this. `Debug` is total
        // for every carrier, so it is accepted even with a Float field/payload —
        // unlike Eq/Ord/Hash.
        let d = DeriveSet::parse(&["Debug".to_owned(), "Clone".to_owned()], true)
            .expect("Debug is allowlisted and Float-safe");
        assert_eq!(d.rust_list(), "Clone, Debug");
    }

    #[test]
    fn total_eq_derives_on_a_float_field_are_refused() {
        // The IEEE-754 fence: a struct with a Float field may derive Clone /
        // Default but never Eq / Ord / Hash.
        for bad in ["Eq", "Ord", "Hash"] {
            assert!(
                matches!(
                    StructDef::parse(
                        "P",
                        &[("x".to_owned(), "f64".to_owned())],
                        &[bad.to_owned()],
                    ),
                    Err(WireDefect::InvalidType { .. })
                ),
                "{bad} on a Float field must be refused"
            );
        }
        // Clone / Default on a Float field are fine.
        assert!(
            StructDef::parse(
                "P",
                &[("x".to_owned(), "f64".to_owned())],
                &["Clone".to_owned(), "Default".to_owned()],
            )
            .is_ok()
        );
    }

    #[test]
    fn a_struct_field_type_outside_the_carrier_set_is_refused() {
        assert!(matches!(
            StructDef::parse("P", &[("x".to_owned(), "u32".to_owned())], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
        // A bad struct NAME is refused too.
        assert!(matches!(
            StructDef::parse("9bad", &[], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
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

    // ── EnumDef ──────────────────────────────────────────────────────────────

    #[test]
    fn a_unit_variant_enum_parses_and_renders_a_definition() {
        // The Iced/TEA `Message` shape: unit variants only.
        let e = EnumDef::parse(
            "Message",
            &[
                ("Increment".to_owned(), vec![]),
                ("Decrement".to_owned(), vec![]),
            ],
            &["Clone".to_owned()],
        )
        .expect("unit-variant enum parses");
        assert_eq!(e.name.as_str(), "Message");
        assert_eq!(e.derives.rust_list(), "Clone");
        let lines = e.definition_lines(&|id| format!("demo::{}", id.as_str()));
        assert_eq!(
            lines,
            vec![
                "#[derive(Clone)]".to_owned(),
                "pub enum Message {".to_owned(),
                "    Increment,".to_owned(),
                "    Decrement,".to_owned(),
                "}".to_owned(),
            ]
        );
    }

    #[test]
    fn a_tuple_payload_variant_renders_its_carriers() {
        let e = EnumDef::parse(
            "Event",
            &[
                ("Tick".to_owned(), vec![]),
                ("SetValue".to_owned(), vec!["i64".to_owned()]),
                ("Move".to_owned(), vec!["i64".to_owned(), "i64".to_owned()]),
            ],
            &[],
        )
        .expect("payload-variant enum parses");
        let lines = e.definition_lines(&|id| format!("demo::{}", id.as_str()));
        // No derives ⇒ no `#[derive]` line; unit + tuple variants render.
        assert_eq!(
            lines,
            vec![
                "pub enum Event {".to_owned(),
                "    Tick,".to_owned(),
                "    SetValue(i64),".to_owned(),
                "    Move(i64, i64),".to_owned(),
                "}".to_owned(),
            ]
        );
    }

    #[test]
    fn an_opaque_payload_variant_absolutizes_through_the_hook() {
        let e = EnumDef::parse(
            "Wrap",
            &[("Hold".to_owned(), vec!["Widget".to_owned()])],
            &[],
        )
        .expect("opaque-payload enum parses");
        assert_eq!(e.first_opaque_payload(), Some("Widget"));
        let lines = e.definition_lines(&|id| format!("demo::{}", id.as_str()));
        assert_eq!(
            lines,
            vec![
                "pub enum Wrap {".to_owned(),
                "    Hold(demo::Widget),".to_owned(),
                "}".to_owned(),
            ]
        );
    }

    #[test]
    fn a_variantless_enum_is_refused() {
        assert!(matches!(
            EnumDef::parse("Void", &[], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
    }

    #[test]
    fn a_bad_enum_or_variant_name_is_refused() {
        assert!(matches!(
            EnumDef::parse("9Bad", &[("A".to_owned(), vec![])], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
        assert!(matches!(
            EnumDef::parse("E", &[("9bad".to_owned(), vec![])], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
    }

    #[test]
    fn an_enum_payload_outside_the_carrier_set_is_refused() {
        assert!(matches!(
            EnumDef::parse("E", &[("A".to_owned(), vec!["u32".to_owned()])], &[]),
            Err(WireDefect::InvalidType { .. })
        ));
    }

    #[test]
    fn total_eq_derives_on_a_float_payload_are_refused() {
        // The IEEE-754 fence generalises to a sum: any variant carrying a Float
        // payload forbids Eq/Ord/Hash on the whole enum.
        for bad in ["Eq", "Ord", "Hash"] {
            assert!(
                matches!(
                    EnumDef::parse(
                        "E",
                        &[("A".to_owned(), vec!["f64".to_owned()])],
                        &[bad.to_owned()],
                    ),
                    Err(WireDefect::InvalidType { .. })
                ),
                "{bad} on a Float-payload variant must be refused"
            );
        }
        // Clone/Default are fine on a Float payload.
        assert!(
            EnumDef::parse(
                "E",
                &[("A".to_owned(), vec!["f64".to_owned()])],
                &["Clone".to_owned()],
            )
            .is_ok()
        );
    }
}
