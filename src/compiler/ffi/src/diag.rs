//! Typed diagnostics for the FFI generator — the `IPE-F####` block.
//!
//! Every fallible public function in this crate returns
//! `Result<T, Diagnostic>`; there is no `Result<_, String>` on any public
//! surface. Each variant maps to exactly one taxonomy [`Code`] and carries a
//! closed defect enum, so a caller can match on the failure class without
//! string inspection.

use std::fmt;

use ipe_diagnostics::{Code, IPE_F4400, IPE_F4401, IPE_F4402};

/// One FFI-generator diagnostic: the failure class plus enough context to
/// name the offending binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// `IPE-F4400` — a foreign-call AST failed one of the structural checks
    /// that make `render_call` total; it is refused before any emission.
    CallUnrenderable {
        /// The wrapper-reference name of the function whose call was refused.
        function: String,
        /// Which structural check failed.
        defect: CallDefect,
    },
    /// `IPE-F4401` — inspector wire data carried a value outside its closed
    /// legal set (unknown enum string, illegal identifier, malformed JSON).
    WireMalformed {
        /// Where in the wire document the defect was met (crate or function).
        context: String,
        /// Which wire rule was broken.
        defect: WireDefect,
    },
    /// `IPE-F4402` — a function's shape flags are contradictory (two of the
    /// mutually-exclusive accessor flags set at once). The one binding is
    /// dropped; the rest of the package survives.
    ShapeContradiction {
        /// The function whose flags contradict.
        function: String,
        /// The flag names that were simultaneously set.
        flags: Vec<&'static str>,
    },
    /// `IPE-F4400` — a reached generic FFI call site (or the generic binding
    /// itself) cannot be soundly bound: the instantiation falls outside the
    /// closed bindable set, a trait bound is unsatisfied or unmodellable, or
    /// a multi-call closure captures a non-Clone value. Raised at the call
    /// site by the instance gate — never a deferred cargo failure.
    GenericNotBindable {
        /// The qualified Ipê callee (`Rust.Box1.make`).
        callee: String,
        /// Which bindability rule was broken.
        defect: GenericBindDefect,
    },
}

impl Diagnostic {
    /// The stable taxonomy code for this diagnostic.
    #[must_use]
    pub const fn code(&self) -> Code {
        match self {
            Self::CallUnrenderable { .. } | Self::GenericNotBindable { .. } => IPE_F4400,
            Self::WireMalformed { .. } => IPE_F4401,
            Self::ShapeContradiction { .. } => IPE_F4402,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CallUnrenderable { function, defect } => {
                write!(
                    f,
                    "{}: foreign call for `{function}` cannot be rendered: {defect}",
                    self.code().as_str()
                )
            }
            Self::WireMalformed { context, defect } => {
                write!(
                    f,
                    "{}: malformed inspection data in {context}: {defect}",
                    self.code().as_str()
                )
            }
            Self::ShapeContradiction { function, flags } => {
                write!(
                    f,
                    "{}: `{function}` declares contradictory shape flags: {}",
                    self.code().as_str(),
                    flags.join(" + ")
                )
            }
            Self::GenericNotBindable { callee, defect } => {
                write!(f, "{}: `{callee}`: {defect}", self.code().as_str())
            }
        }
    }
}

impl std::error::Error for Diagnostic {}

/// Why a concrete instantiation type falls outside the closed bindable set.
///
/// A residual type variable means monomorphisation did not specialise the
/// call — the sound answer is this rejection, never a boxed/`any` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosedSetViolation {
    /// A bare type variable survived monomorphisation.
    UnresolvedTypeVariable(String),
    /// A named constructor outside the closed set (opaque foreign types are
    /// conservatively rejected pending derive-scan metadata).
    NonClosedConstructor(String),
    /// A record type.
    RecordType,
    /// A tuple type.
    TupleType,
    /// A function type.
    FunctionType,
    /// A type alias (unexpanded).
    TypeAlias(String),
}

impl fmt::Display for ClosedSetViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnresolvedTypeVariable(n) => write!(f, "unresolved type variable `{n}`"),
            Self::NonClosedConstructor(n) => write!(f, "non-closed type constructor `{n}`"),
            Self::RecordType => write!(f, "record type"),
            Self::TupleType => write!(f, "tuple type"),
            Self::FunctionType => write!(f, "function type"),
            Self::TypeAlias(n) => write!(f, "type alias `{n}`"),
        }
    }
}

/// The closed set of generic-FFI bindability defects (`IPE-F4400`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericBindDefect {
    /// The instantiation type is outside the closed Ipê↔Rust bindable set.
    OutsideClosedSet {
        /// The type-param name.
        param: String,
        /// The rendered instantiation type.
        ty: String,
        /// Why the type is outside the set.
        violation: ClosedSetViolation,
    },
    /// The bound is modellable but the concrete Rust type lacks the trait.
    BoundUnsatisfied {
        /// The type-param name.
        param: String,
        /// The rendered Ipê instantiation type.
        ty: String,
        /// The closed Rust type it maps to.
        rust_ty: String,
        /// The unsatisfied trait bound.
        bound: String,
    },
    /// The declared bound is outside the modellable `MODELLABLE_5` table —
    /// the backend cannot prove it holds for an arbitrary bindable type, so
    /// it refuses to emit an unsound wrapper (names the BOUND, not the type).
    UnmodellableBound {
        /// The type-param name.
        param: String,
        /// The unmodellable trait bound.
        bound: String,
    },
    /// An Ipê lambda captures a non-Clone value into a multi-call
    /// (`Fn`/`FnMut`) closure slot, whose owned-clone bridge re-clones every
    /// capture per call.
    CaptureNotClone {
        /// The captured variable name.
        capture: String,
        /// The rendered Ipê type of the capture.
        ty: String,
    },
}

impl fmt::Display for GenericBindDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideClosedSet {
                param,
                ty,
                violation,
            } => write!(
                f,
                "type parameter `{param}` is instantiated at `{ty}` ({violation}), outside the \
                 Ipê↔Rust bindable set; use a primitive (Int / Float / Bool / Char / String), a \
                 `List` or `Maybe` of one, or bind a non-generic FFI wrapper for `{ty}`"
            ),
            Self::BoundUnsatisfied {
                param,
                ty,
                rust_ty,
                bound,
            } => write!(
                f,
                "type parameter `{param}` is instantiated at `{ty}` (Rust `{rust_ty}`), but the \
                 binding requires the Rust trait bound `{bound}` and `{rust_ty}` does not \
                 implement `{bound}`"
            ),
            Self::UnmodellableBound { param, bound } => write!(
                f,
                "the binding declares the Rust trait bound `{bound}` on type parameter `{param}`, \
                 but the backend can only model the bounds {{Hash, Eq, Ord, Clone, Default}}; it \
                 will not emit an unsound generic wrapper — drop the `{bound}` bound or bind a \
                 non-generic wrapper at the concrete type(s) you need"
            ),
            Self::CaptureNotClone { capture, ty } => write!(
                f,
                "capture `{capture}` of type `{ty}` is passed into a multi-call closure that Rust \
                 requires to be `Fn + Clone`, but `{ty}` is not provably Clone; use a Clone-able \
                 captured value, capture nothing, or pass it to a single-call FnOnce slot"
            ),
        }
    }
}

/// The closed set of structural defects a foreign-call AST can carry.
///
/// A negative argument index has no variant: wire indices decode as `usize`,
/// so a negative value is rejected at the serde layer as [`WireDefect::Json`]
/// before a call AST exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallDefect {
    /// A `{param: i}` type reference is outside the declared param count.
    ParamRefOutOfRange {
        /// The out-of-range index.
        index: usize,
        /// How many type params the enclosing generic block declares.
        n_params: usize,
    },
    /// Call kind `method` but no `receiver` present.
    ReceiverMissingForMethod,
    /// Call kind `function` but a `receiver` is present.
    ReceiverForbiddenForFunction,
    /// A value-arg index feeds two slots (a use-after-move in rendered Rust).
    ArgIndexDuplicated {
        /// The index referenced more than once.
        index: usize,
    },
    /// Value-arg indices are not contiguous from 0.
    ArgIndexGap {
        /// The smallest never-referenced index below the arity.
        missing: usize,
    },
    /// `argTypes` length disagrees with the call's value-arg count.
    ArgTypeArityMismatch {
        /// The number of `argTypes` entries present.
        arg_types_len: usize,
        /// The value-arg count the call references.
        arity: usize,
    },
    /// A closure type appears somewhere other than a direct argument slot
    /// (nested in a container, the return, a type-argument, or a method
    /// turbofish) — unrenderable as valid Rust.
    ClosureNestedOrNonDirect,
    /// An `iterAdapters` index does not reference a real value-arg slot.
    IterAdapterOutOfRange {
        /// The out-of-range adapter index.
        index: usize,
        /// The call's value-arg count.
        arity: usize,
    },
    /// An `iterAdapters` index targets a non-`Vec` argument type
    /// (`.into_iter()` is sound only on a `Vec` arg).
    IterAdapterTargetNotVec {
        /// The adapter index whose slot is not a `Vec`.
        index: usize,
    },
}

impl fmt::Display for CallDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParamRefOutOfRange { index, n_params } => write!(
                f,
                "type-param ref {{param:{index}}} is out of range (declared {n_params} param(s))"
            ),
            Self::ReceiverMissingForMethod => {
                write!(
                    f,
                    "call kind \"method\" requires a `receiver`, but none is present"
                )
            }
            Self::ReceiverForbiddenForFunction => {
                write!(f, "call kind \"function\" must not carry a `receiver`")
            }
            Self::ArgIndexDuplicated { index } => {
                write!(f, "value-arg {{arg:{index}}} is referenced more than once")
            }
            Self::ArgIndexGap { missing } => write!(
                f,
                "value-arg index {missing} is never referenced (arg indices must be contiguous from 0)"
            ),
            Self::ArgTypeArityMismatch {
                arg_types_len,
                arity,
            } => write!(
                f,
                "argTypes has {arg_types_len} entry(ies) but the call references {arity} value-arg(s)"
            ),
            Self::ClosureNestedOrNonDirect => write!(
                f,
                "a closure type may only appear as a direct wrapper argument, not nested inside a container, return, type-argument, or method turbofish"
            ),
            Self::IterAdapterOutOfRange { index, arity } => write!(
                f,
                "iterAdapters index {index} is out of range (the call references {arity} value-arg(s))"
            ),
            Self::IterAdapterTargetNotVec { index } => write!(
                f,
                "iterAdapters index {index} targets a non-Vec argType (`.into_iter()` is sound only on a Vec arg)"
            ),
        }
    }
}

/// The closed set of wire-level defects the validating decoders reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireDefect {
    /// A call `kind` string outside `method` / `function`.
    UnknownCallKind {
        /// The value met on the wire.
        got: String,
    },
    /// A receiver `by` string outside `ref` / `refmut` / `value`.
    UnknownByKind {
        /// The value met on the wire.
        got: String,
    },
    /// A closure `kind` string outside `Fn` / `FnMut` / `FnOnce`.
    UnknownClosureKind {
        /// The value met on the wire.
        got: String,
    },
    /// An `effect` string outside `pure` / `fallible` / `effectful`.
    UnknownEffect {
        /// The value met on the wire.
        got: String,
    },
    /// A `TypeRef` object with zero or more than one discriminator key.
    TypeRefDiscriminator {
        /// The discriminator keys that were present.
        present: Vec<&'static str>,
    },
    /// A name that must be a legal Rust identifier is not.
    InvalidIdent {
        /// The offending name.
        got: String,
    },
    /// A module path segment that is not a legal Rust identifier path.
    InvalidModulePath {
        /// The offending path.
        got: String,
    },
    /// The document is not the JSON shape the wire contract declares
    /// (carries the rendered serde error as detail).
    Json {
        /// The serde decode error, rendered.
        detail: String,
    },
}

impl fmt::Display for WireDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCallKind { got } => write!(
                f,
                "unknown call kind {got:?} (expected \"method\" or \"function\")"
            ),
            Self::UnknownByKind { got } => write!(
                f,
                "unknown receiver `by` kind {got:?} (expected \"ref\", \"refmut\", or \"value\")"
            ),
            Self::UnknownClosureKind { got } => write!(
                f,
                "unknown closure kind {got:?} (expected \"Fn\", \"FnMut\", or \"FnOnce\")"
            ),
            Self::UnknownEffect { got } => write!(
                f,
                "unknown effect {got:?} (expected \"pure\", \"fallible\", or \"effectful\")"
            ),
            Self::TypeRefDiscriminator { present } => {
                if present.is_empty() {
                    write!(
                        f,
                        "TypeRef must have exactly one of `param`, `prim`, `ctor`, `closure`, `serdeValue`, or `serdeValueRef`"
                    )
                } else {
                    write!(
                        f,
                        "TypeRef carries more than one discriminator: {}",
                        present.join(", ")
                    )
                }
            }
            Self::InvalidIdent { got } => {
                write!(f, "{got:?} is not a legal Rust identifier")
            }
            Self::InvalidModulePath { got } => {
                write!(f, "{got:?} is not a legal Rust identifier path")
            }
            Self::Json { detail } => write!(f, "{detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_maps_to_its_taxonomy_code() {
        let call = Diagnostic::CallUnrenderable {
            function: "parse".into(),
            defect: CallDefect::ReceiverMissingForMethod,
        };
        assert_eq!(call.code().as_str(), "IPE-F4400");

        let wire = Diagnostic::WireMalformed {
            context: "crate `semver`".into(),
            defect: WireDefect::UnknownEffect {
                got: "spooky".into(),
            },
        };
        assert_eq!(wire.code().as_str(), "IPE-F4401");

        let shape = Diagnostic::ShapeContradiction {
            function: "major_from_version".into(),
            flags: vec!["isField", "isEnumCtor"],
        };
        assert_eq!(shape.code().as_str(), "IPE-F4402");
    }

    #[test]
    fn display_carries_code_context_and_defect() {
        let d = Diagnostic::CallUnrenderable {
            function: "left".into(),
            defect: CallDefect::ParamRefOutOfRange {
                index: 3,
                n_params: 2,
            },
        };
        let s = d.to_string();
        assert!(s.contains("IPE-F4400"), "{s}");
        assert!(s.contains("`left`"), "{s}");
        assert!(s.contains("{param:3}"), "{s}");
        assert!(s.contains("2 param(s)"), "{s}");
    }
}
