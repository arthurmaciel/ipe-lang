//! Output schema for `ipe-ffi-inspector`.
//!
//! The serialised types the inspector emits as JSON — the wire contract the
//! `ipe_ffi::pkginfo` consumer decodes. Every `#[serde]` attribute here is
//! load-bearing: field renames, `skip_serializing_if`, and `TypeRef`'s manual
//! `Serialize` define the exact byte shape. Types are `pub(crate)` so the
//! inspection logic in `main.rs` can construct and read them.

use serde::Serialize;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Param {
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) ty: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) ipe_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) rust_type: String,
}

/// serde `skip_serializing_if` predicate: omit a `u64` field from the JSON when
/// it is the default 0 (keeps the bindings JSON minimal + matches the other
/// `skip_serializing_if` defaults on `Function`).
// serde `skip_serializing_if` predicates must take `&T` by reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
// Mirrors rustdoc JSON's flat item shape; grouping into sub-structs would
// obscure the 1:1 correspondence with the schema.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) params: Vec<Param>,
    pub(crate) results: Vec<Param>,
    pub(crate) variadic: bool,
    pub(crate) effect: String,
    pub(crate) exported: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) recv_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) recv_rust_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) method_name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_field: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_field_set: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_pkg_var: bool,
    // Owned-threading setter: receiver is `&mut self` and the result is the
    // receiver type (`&mut Self`/`&mut RecvType`/`&Self`) or `()`. FfiGen emits
    // a `fn(arg0: Recv, args..) -> Recv { let mut r = arg0; r.m(args); r }`
    // wrapper instead of dropping the borrowed return. Absent (→ false) for
    // Go and every non-setter.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) self_returning: bool,

    // ── Enum-variant binding (S3) ───────────────────────────────────────
    // A foreign enum stays an OPAQUE handle (`::crate::E`); these three flags
    // (mutually exclusive, at most one true) drive the Rust codegen's three
    // total-by-construction accessor kinds. All absent (→ false) for Go and
    // every non-enum function. The Rust-only emit details live in `enum_*`.
    /// Variant CONSTRUCTOR: `<variant> : F1 -> .. -> E` → `E::Variant(args)`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_enum_ctor: bool,
    /// Tag ACCESSOR: `tag_of_<E> : E -> String` → `match e { … }`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_enum_tag: bool,
    /// Single-field payload EXTRACTOR: `<v>_as_variant : E -> Maybe T`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_enum_extract: bool,
    /// Rust variant identifier (ctor + extract). Empty otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) enum_variant: String,
    /// Variant kind: "unit" | "tuple" | "struct" (ctor + extract).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) enum_kind: String,
    /// Struct-variant field names in declaration order (ctor + extract for a
    /// struct variant). Empty for unit / tuple variants.
    ///
    /// EXTRACT REUSE: for a payload extractor this slot holds a SINGLE entry —
    /// the selected binder the body returns: the field NAME for a struct
    /// variant, the positional INDEX (as a string, e.g. "0"/"1") for a tuple
    /// variant. The Rust emit reads slot 0 to pick the field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) enum_struct_fields: Vec<String>,
    /// Extractor only: the variant's TOTAL field arity. The Rust emit binds
    /// every tuple position (`E::V(f0, f1, ..)`) before returning the selected
    /// one, so a multi-field tuple extractor needs the count. 0 / absent for
    /// ctors, tags, and struct-variant extractors (which use `{ name, .. }`).
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub(crate) enum_field_count: u64,
    /// Tag-match arms (tag only): each entry is "<rust-pattern>\t<tag-string>",
    /// e.g. "A\tA", "B(..)\tB", "C{..}\tC". `FfiGen` renders `E::<pat> => "<tag>"`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) enum_arms: Vec<String>,
    /// Whether the tag / extract match needs a trailing `_ => …` wildcard arm
    /// (R3): enum is `non_exhaustive` OR a variant was skipped. Extract always
    /// sets it (the non-matching variants collapse to Nothing). Tag sets it per
    /// the R3 condition only, to stay clippy unreachable-pattern clean.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) enum_wildcard: bool,

    // ── Wall #3 parametric generic stub ─────────────────────────────────
    // Present ONLY for a bindable generic free fn / struct method emitted as a
    // PARAMETRIC stub (Ipe tyvars + a typed call-AST + the FULL-UNION bounds).
    // Absent (→ None) for every non-generic binding and for Go (whose inspector
    // drops generics at the producer). Decoded generator-side under
    // `_ffn_generic = Just`, so the whole field is dead for the Go path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generic: Option<Generic>,
    /// [#109] Public crate-relative call path for a free fn in a SUBMODULE
    /// (`civil::date` — crate segment STRIPPED; codegen prepends `::<crate>::`).
    /// Empty for methods, crate-root free fns, and Go. Free fns only.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) call_path: String,
}

// ── Wall #3 Scheme-A call-AST (mirror of generator ipe_ffi::call) ──
//
// The Rust serde here and the generator `FromJSON` MUST agree on the wire shape;
// the drift test (generator-side FfiCallSpec corpus + the Rust `mod tests` shape
// asserts below) keeps them in lock-step. Field names are the camelCase wire
// keys the generator decoder reads.

/// The parametric `generic` block on a `Function`: type-param names (Ipe-source
/// order), per-param FULL-UNION trait bounds, and the typed call-AST.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Generic {
    pub(crate) params: Vec<String>,
    /// per-param trait NAMES (the modellable-5 only reach generator; an
    /// unmodellable bound here is defense-in-depth — Wall #2 still rejects it).
    pub(crate) bounds: std::collections::BTreeMap<String, Vec<String>>,
    pub(crate) call: Call,
    /// [#58 WALL 2] True when at least one type-param was eliminated from
    /// `params` / `order` by the mono pre-pass (`resolve_param_bounds` resolved
    /// it to a concrete type).  Distinguishes the `AsRef`<str>+Send → String
    /// case (fully-mono via the WALL 2 pre-pass; `params` becomes empty) from
    /// a method that was never generic in the first place (assoc-type-only;
    /// `params` also empty but NO mono pre-pass ran).  Only the first case
    /// should trigger the WALL 2 mono-routing branch.
    #[serde(skip)]
    pub(crate) mono_resolved: bool,
}

/// One Rust call expression as a structure (parse-don't-validate). `kind` is
/// "method" | "function"; the generator decoder validates receiver-iff-method and
/// every {param}/arg ref at decode time.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Call {
    pub(crate) kind: String,
    pub(crate) path: Vec<String>,
    #[serde(rename = "typeArgs", skip_serializing_if = "Vec::is_empty")]
    pub(crate) type_args: Vec<TypeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) receiver: Option<Receiver>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) args: Vec<usize>,
    #[serde(rename = "argTypes", skip_serializing_if = "Vec::is_empty")]
    pub(crate) arg_types: Vec<TypeRef>,
    pub(crate) ret: TypeRef,
    /// Value-arg indices whose iterator param is bound `Iterator<Item=T>` (NOT
    /// `IntoIterator`): the call site passes `argJ.into_iter()` instead of the
    /// bare `argJ` (a `Vec<T>` is `IntoIterator` but not itself an `Iterator`).
    /// Empty ⇒ omitted from the wire ⇒ byte-identical to a pre-iterators stub,
    /// and every arg renders as the bare identifier ([C6] default = direct).
    #[serde(rename = "iterAdapters", skip_serializing_if = "Vec::is_empty")]
    pub(crate) iter_adapters: Vec<usize>,
    /// [Wall-3b] Value-arg indices whose `&str`/`&Path`/`&OsStr` param was
    /// lowered to Ipe `String` (all three implement `AsRef<str>`, `AsRef<Path>`,
    /// `AsRef<OsStr>`): the call site passes `argJ.as_ref()` so the owned
    /// `String` coerces to the correct borrowed type expected by the foreign fn.
    /// Empty ⇒ omitted from the wire (pre-Wall-3b stubs are byte-identical).
    #[serde(rename = "borrowAsRefArgs", skip_serializing_if = "Vec::is_empty")]
    pub(crate) borrow_as_ref_args: Vec<usize>,
    /// #21 UFCS trait qualifier `(selfPath, traitPath)`. When `Some`, the codegen
    /// renders the callee as `<selfPath as traitPath>::method(recv, args)` (a
    /// trait method on a concrete type — disambiguated, no `use Trait;` needed).
    /// `None` (the inherent / free / non-trait case) ⇒ OMITTED from the wire ⇒
    /// byte-identical to a pre-#21 stub, and the generator renderer falls back to
    /// the historical `path::method` callee (constraint 9).
    #[serde(rename = "traitQualifier", skip_serializing_if = "Option::is_none")]
    pub(crate) trait_qualifier: Option<(String, String)>,
    /// [WALL 3a / #59] Whether the host method is `async fn`. The generic-wrapper
    /// emitter (`synthesiseGenericWrapper`) emits ONLY a sync `ok_res(<body>)`
    /// body; an async serde trait method (firestore `get_obj`/`create_obj`)
    /// needs the async `Box::pin(async move { tokio::task::spawn(... .await) })`
    /// shape (compose with #44 async→Task + #54 self-receiver Send). `false`
    /// (every sync stub) ⇒ OMITTED from the wire ⇒ byte-identical to a pre-WALL-3a
    /// stub.
    #[serde(rename = "isAsync", skip_serializing_if = "std::ops::Not::not")]
    pub(crate) is_async: bool,
    /// [#72] The METHOD's OWN generics' resolved concretes, in DECLARATION order
    /// — the method-level turbofish list `::<C1, C2, …>` for a UFCS/inherent
    /// `Method::<…>` call. A method with TWO generics each reduced by a DIFFERENT
    /// mechanism (the firestore `get_obj<T: DeserializeOwned, S: AsRef<str>>`
    /// shape: T serde→Value, S AsRef→String) leaves the Ipe-visible generic
    /// `order` EMPTY, so the path `type_args` carries nothing — but the Rust
    /// method STILL declares `<T, S>`, so the call MUST name a concrete per
    /// generic (`::<serde_json::Value, String>`). Pre-#72 the codegen hardcoded a
    /// single `::<serde_json::Value>` whenever the call touched serde → `E0107
    /// method takes N generic arguments but 1 was supplied`. Each entry is the
    /// generic's resolved concrete: a serde-reduced one → `SerdeValue`
    /// (`serde_json::Value`), an AsRef/etc-mono'd one → its `Prim`/`Ctor`
    /// concrete, a genuinely-remaining one → `Param(idx)`. EMPTY ⇒ omitted from
    /// the wire ⇒ the generator renderer falls back to the historical single-serde
    /// `::<serde_json::Value>` turbofish (byte-identical for a method with one
    /// own generic, which is every pre-#72 serde stub). FAIL-CLOSED: a method
    /// whose generic can't be resolved to a concrete DROPS before reaching here.
    #[serde(rename = "methodTurbofish", skip_serializing_if = "Vec::is_empty")]
    pub(crate) method_turbofish: Vec<TypeRef>,
}

/// The receiver of a method call: which wrapper value-arg supplies it + borrow.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Receiver {
    pub(crate) arg: usize,
    /// "ref" | "refmut" | "value".
    pub(crate) by: String,
}

/// The Rust closure trait-kind a closure-typed argument satisfies. Mirrors the
/// generator `ipe_ffi::call.ClosureKind` (the metadata-contract consumer).
//
// Phase 5 emits the closure-metadata CLASSIFIER + its types; wiring the
// classifier into `try_parametric_stub`'s arg loop is Phase 6.2 (a closure-bound
// fn still drops for the independent unmodellable-`Fn`-bound reason until then).
// So these items are exercised by the `#[cfg(test)]` suite but not yet by the
// bin's main path — `#[allow(dead_code)]` matches the `split_top_level`
// precedent above. REMOVE the allows when Phase 6.2 wires them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosureKind {
    Fn,
    FnMut,
    FnOnce,
}

impl ClosureKind {
    /// The wire string the generator `FromJSON ClosureKind` decodes (`"Fn"` /
    /// `"FnMut"` / `"FnOnce"`).
    fn as_str(self) -> &'static str {
        match self {
            ClosureKind::Fn => "Fn",
            ClosureKind::FnMut => "FnMut",
            ClosureKind::FnOnce => "FnOnce",
        }
    }
    /// Parse a Rust closure trait NAME (last `::` segment) into a `ClosureKind`.
    pub(crate) fn from_trait_name(name: &str) -> Option<ClosureKind> {
        match name {
            "Fn" => Some(ClosureKind::Fn),
            "FnMut" => Some(ClosureKind::FnMut),
            "FnOnce" => Some(ClosureKind::FnOnce),
            _ => None,
        }
    }
}

/// The Rust iterator trait-kind a generic param is bound by (epic #30). An
/// `IntoIterator<Item=T>` param takes a `Vec<T>` DIRECTLY (`Vec<T>:
/// IntoIterator<Item=T>`); an `Iterator<Item=T>` param needs `.into_iter()`
/// at the call site (a `Vec` is not itself an `Iterator`). Both lower the param
/// to the same `Vec<ItemT>` argType — only the CALL FORM differs (`arg0` vs
/// `arg0.into_iter()`), recorded per-arg in `Call::iter_adapters`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IterKind {
    /// `IntoIterator<Item=T>` — pass the `Vec<T>` directly.
    IntoIter,
    /// `Iterator<Item=T>` — pass `arg.into_iter()`.
    Iter,
}

impl IterKind {
    /// Parse a Rust iterator trait NAME (last `::` segment) into an `IterKind`.
    pub(crate) fn from_trait_name(name: &str) -> Option<IterKind> {
        match name {
            "IntoIterator" => Some(IterKind::IntoIter),
            "Iterator" => Some(IterKind::Iter),
            _ => None,
        }
    }
}

/// A type reference inside typeArgs / argTypes / ret. Exactly one of the four
/// variants is serialised (serde-untagged via a manual Serialize-shape: each
/// arm emits its own discriminator key). `Closure` mirrors the generator
/// `TRClosure` — it is valid ONLY as a direct `argTypes` element (validated on
/// the generator side, C-B), never nested in a ctor / ret.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TypeRef {
    Param(usize),
    Prim(String),
    Ctor(String, Vec<TypeRef>),
    /// `{closure:{kind, byRef, argTypes, ret}}`. `by_ref` ⇒ the foreign param is
    /// `Fn(&A)` (a borrowed input — drives the owned-clone bridge downstream).
    Closure {
        kind: ClosureKind,
        by_ref: bool,
        arg_types: Vec<TypeRef>,
        ret: Box<TypeRef>,
    },
    /// [WALL 3a / #59] `{serdeValue:true}` — a serde-bound generic param/return
    /// reduced to `serde_json::Value`.  Ipe-facing type is `String` (the JSON
    /// text); the generator `TRSerdeValue` renders it as `serde_json::Value` and
    /// the generic-wrapper body emitter injects the `from_str` prelude (param) /
    /// `to_string` return-wrap + the method-level `::<serde_json::Value>`
    /// turbofish.  Replaces the position-blind `{"__ipe_serde_value":true}`
    /// sig-substitution sentinel with a typed call-AST node on the UFCS path.
    SerdeValue,
    /// [WALL 3a-&I / #65] `{serdeValueRef:true}` — a `&T` SERIALIZE (input) param
    /// whose `T` was serde-reduced to `serde_json::Value`. Ipe-facing type is
    /// `String` (the JSON text), IDENTICAL to `SerdeValue` on the wrapper param
    /// and the `from_str` prelude. The divergence is at the CALL SITE: the host
    /// wants `&Value`, so the generator `TRSerdeValueRef` renders the arg as
    /// `&sv_j` (a reference to the owned deserialised local, which lives for the
    /// call — sound). Admitted ONLY for a NON-MUT `&T` in an INPUT position on a
    /// SERIALIZE-only param (census gate); a `&mut T` or a Deserialize param
    /// STAYS dropped.
    SerdeValueRef,
}

impl Serialize for TypeRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            TypeRef::Param(i) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("param", i)?;
                m.end()
            }
            TypeRef::Prim(p) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("prim", p)?;
                m.end()
            }
            TypeRef::Ctor(nm, args) => {
                let n = if args.is_empty() { 1 } else { 2 };
                let mut m = s.serialize_map(Some(n))?;
                m.serialize_entry("ctor", nm)?;
                if !args.is_empty() {
                    m.serialize_entry("args", args)?;
                }
                m.end()
            }
            TypeRef::Closure {
                kind,
                by_ref,
                arg_types,
                ret,
            } => {
                // One key: `closure`, carrying an inner map of {kind, byRef,
                // argTypes, ret}. Matches the hand-stub `clo.kernel.json` shape
                // the generator `FromJSON TypeRef` closure branch reads.
                let mut inner = serde_json::Map::new();
                inner.insert("kind".into(), serde_json::Value::from(kind.as_str()));
                inner.insert("byRef".into(), serde_json::Value::from(*by_ref));
                inner.insert(
                    "argTypes".into(),
                    serde_json::to_value(arg_types).map_err(serde::ser::Error::custom)?,
                );
                inner.insert(
                    "ret".into(),
                    serde_json::to_value(ret.as_ref()).map_err(serde::ser::Error::custom)?,
                );
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("closure", &serde_json::Value::Object(inner))?;
                m.end()
            }
            TypeRef::SerdeValue => {
                // `{serdeValue:true}` — the typed serde-Value node (WALL 3a).
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("serdeValue", &true)?;
                m.end()
            }
            TypeRef::SerdeValueRef => {
                // `{serdeValueRef:true}` — the &T serde-Value input node (WALL 3a-&I).
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("serdeValueRef", &true)?;
                m.end()
            }
        }
    }
}

/// The five trait bounds the Ipe→Rust backend can statically model on a generic
/// FFI type-param. SINGLE SOURCE on the Rust side; the generator side has the same
/// literal in `ipe_ffi::instance.modellableTrait`. The drift fence
/// (4a) is `test_modellable_5_matches` here + the generator
/// "modellable-5 set is EXACTLY {…}" spec — if either side adds/removes a trait
/// without the other, one of the two fails.
///
/// DISTINCT from `MARKER_TRAITS` (a 13-elem auto/marker SUPERSET used by Alt-1's
/// bound-IGNORING resolution): `MODELLABLE_5` is the set the parametric-stub
/// path is allowed to EMIT as a `<T: …>` bound. Do not conflate them.
pub(crate) const MODELLABLE_5: &[&str] = &["Hash", "Eq", "Ord", "Clone", "Default"];

/// Whether a trait NAME (last `::` segment) is in the modellable-5.
pub(crate) fn is_modellable_5(name: &str) -> bool {
    MODELLABLE_5.contains(&name)
}

/// WALL-B (#75): one resolved crate node from the introspection project's
/// `cargo metadata`. Maps the Rust LIB IDENTIFIER (`ident`, the underscored
/// name a `::<crate>::…` path segment uses) to the CANONICAL crates.io PACKAGE
/// name (`name`, almost always hyphenated for multi-word crates) and the EXACT
/// locked `version`. The generator codegen's transitive-dep scanner finds the
/// `ident` in a generated wrapper; this record tells it the real Cargo
/// `[dependencies]` KEY (`name`) and a pinned version requirement (`version`)
/// to emit — so it never has to GUESS `_`→`-` or fall back to `"*"`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransitiveDep {
    /// Rust lib-target identifier (underscored): the `::<ident>::…` path segment.
    pub(crate) ident: String,
    /// Canonical crates.io package name (the Cargo `[dependencies]` KEY).
    pub(crate) name: String,
    /// Exact resolved version from the introspection project's Cargo.lock.
    pub(crate) version: String,
}

/// One reported member of a foreign type: a named struct field or one variant
/// payload slot (a positional slot is named by its decimal index).
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TypeMember {
    pub(crate) name: String,
    /// The Ipê-mapped spelling of the member's type.
    #[serde(rename = "type")]
    pub(crate) ty: String,
    /// The rendered Rust spelling of the member's type.
    pub(crate) rust_type: String,
}

/// One reported variant of a foreign enum.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TypeVariantDecl {
    pub(crate) name: String,
    /// "unit" | "tuple" | "struct".
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) members: Vec<TypeMember>,
}

/// The structural facts of one crate-local public struct/enum — the evidence
/// the generator's decode boundary classifies a type from on the
/// transparent-or-opaque representation axis.
///
/// The inspector reports FACTS, fail-closed: any detection failure surfaces as
/// `hiddenMembers`/`nonExhaustive` = true, and the generator upgrades a type
/// to transparent only from an entry whose facts affirmatively qualify. A type
/// with no entry, hidden members, or a non-exhaustive contract stays an opaque
/// nominal handle.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForeignTypeDecl {
    /// The Ipê-visible nominal (same rendering as an accessor's `recvType`).
    pub(crate) name: String,
    /// The rendered Rust path (same rendering as an accessor's `recvRustType`).
    pub(crate) rust_path: String,
    /// "struct" | "enum".
    pub(crate) kind: String,
    /// `#[non_exhaustive]` on the type: its member set is NOT a stable
    /// contract, so it must never surface as a record / closed union.
    /// Detection failure reads as true.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) non_exhaustive: bool,
    /// Some member is invisible or unreportable: a private / `#[doc(hidden)]`
    /// / stripped field, a hidden or `#[non_exhaustive]` variant, or a member
    /// whose type could not be rendered. The listed members are then a subset
    /// of the real type and it must stay opaque.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) hidden_members: bool,
    /// Struct fields, in declaration order (struct kind only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) fields: Vec<TypeMember>,
    /// Enum variants, in declaration order (enum kind only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) variants: Vec<TypeVariantDecl>,
}

/// One reported public constant: the `Rust.const` surface reads its crate-
/// relative path and Rust type to confirm an author's asserted scalar. Only a
/// bare-scalar-typed constant is ever bound; the consumer's decode gate keeps a
/// well-shaped path and a matching type.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Constant {
    /// The constant's crate-relative path (`f64::consts::PI`).
    pub(crate) path: String,
    /// The constant's Rust type, rendered verbatim (`f64`, `&str`).
    #[serde(rename = "type")]
    pub(crate) ty: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PkgInfo {
    pub(crate) pkg: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) version: String,
    pub(crate) functions: Vec<Function>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) constants: Vec<Constant>,
    // Full `::`-paths of every PUBLIC module in the crate (e.g.
    // "chrono::format").  FfiGen glob-imports each (`use <mod>::*;`) so that
    // types re-exported only in submodules resolve by their bare name in the
    // generated wrappers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) modules: Vec<String>,
    pub(crate) errors: Vec<String>,
    // Diagnostic notes for the user (e.g. facade crate guidance).
    // Not consumed by FfiGen — printed by the `ipe add` CLI.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
    // WALL-B (#75): every crate resolved in the introspection project's
    // `cargo metadata` (direct + TRANSITIVE), keyed by its Rust lib identifier.
    // FfiGen re-emits this into the kernel.json; the Rust codegen consults it to
    // resolve a wrapper's crate-absolute `::<ident>::…` reference to the canonical
    // package name + exact version for the generated `[dependencies]`. Empty for
    // the Go inspector and for any crate whose `cargo metadata` could not run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) transitive_deps: Vec<TransitiveDep>,
    // [#100 Part B] The EFFECTIVE feature set the rustdoc introspection actually
    // SUCCEEDED with — i.e. the set the bound wrappers were generated against.
    // For a no-`full`-feature crate the #89 visibility logic injects ALL features,
    // so the wrappers reference feature-gated APIs (e.g. firestore `caching`); the
    // generated `[dependencies]` must enable the SAME set or those types vanish
    // (E0412/E0433/E0405/E0599 — the firestore #73/#100 dominant class). Empty when
    // rustdoc ran on DEFAULT features (no injection, or the injected set was dropped
    // on the mutually-exclusive-feature fallback) — propagating nothing is then
    // correct. The Rust codegen merges this into the primary FFI crate's dep line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) features: Vec<String>,
    // [foreign-type-one-home] Rendered foreign-nominal path (`::`-prefixed, as
    // it appears in this crate's emitted type strings) → the type's DEFINING
    // path (`doc["paths"]` identity). The generator's catalog unification keys
    // cross-crate nominal identity on the value: one defining path = one Ipê
    // type, whether reached through its definer or a re-exporter.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) foreign_type_ids: std::collections::BTreeMap<String, String>,
    // Structural facts of every crate-local public struct/enum candidate for
    // the transparent representation. The generator's decode classifies from
    // these facts alone; a type without an entry is opaque by default, so the
    // list only ever ENABLES transparency, never forces it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) types: Vec<ForeignTypeDecl>,
    // The absolute path to the author-supplied wrapper crate this package was
    // inspected from (a `--path <dir>` source), or empty for an ordinary
    // crates.io / git inspection. The generator emits a `path` dependency for a
    // wrapper crate rather than a registry pin.
    #[serde(
        rename = "wrapperPath",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub(crate) wrapper_path: String,
}
