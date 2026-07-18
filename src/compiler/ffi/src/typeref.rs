//! Type references inside a foreign-call AST, at two trust levels.
//!
//! The WIRE level ([`WireTypeRef`]) byte-mirrors the inspector JSON via a
//! hand-written single-discriminator visitor (never `#[serde(untagged)]`,
//! which swallows *which* variant failed and can mis-route an adversarial
//! map). The DOMAIN level splits into [`ArgTypeRef`] (a direct wrapper-arg
//! slot, where a closure is legal) and [`InnerTypeRef`] (every other
//! position), so a closure nested in a container, return, type-argument, or
//! turbofish is UNREPRESENTABLE after conversion — the render functions are
//! total with no placeholder fallback.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};

use crate::diag::{CallDefect, WireDefect};
use crate::naming::{RustTypeExpr, mangle_tvar};

/// The Rust closure trait a closure-typed wrapper param must satisfy.
///
/// `Fn` / `FnMut` additionally require `+ ::core::clone::Clone` in their
/// emitted bound: multi-call closures are cloned by the wrapper internals.
/// `FnOnce` is consumed at most once — no clone needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureKind {
    /// `Fn(args) -> ret + Clone`.
    Fn,
    /// `FnMut(args) -> ret + Clone`.
    FnMut,
    /// `FnOnce(args) -> ret` (no Clone — consumed once).
    FnOnce,
}

impl ClosureKind {
    /// The Rust trait name (`Fn` / `FnMut` / `FnOnce`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fn => "Fn",
            Self::FnMut => "FnMut",
            Self::FnOnce => "FnOnce",
        }
    }

    /// Whether the emitted bound needs `+ ::core::clone::Clone`.
    #[must_use]
    pub const fn needs_clone(self) -> bool {
        !matches!(self, Self::FnOnce)
    }
}

impl<'de> Deserialize<'de> for ClosureKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "Fn" => Ok(Self::Fn),
            "FnMut" => Ok(Self::FnMut),
            "FnOnce" => Ok(Self::FnOnce),
            _ => Err(de::Error::custom(
                WireDefect::UnknownClosureKind { got: s }.to_string(),
            )),
        }
    }
}

/// A wire-level type reference, decoded but not yet position-checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireTypeRef {
    /// `{param: i}` — the i-th generic param.
    Param(usize),
    /// `{prim: "i64"}` — a concrete Rust primitive leaf.
    Prim(String),
    /// `{ctor: "Name", args: […]}` — `Name<args…>` (`::`-joined name ok).
    Ctor(String, Vec<Self>),
    /// `{closure: {kind, byRef, argTypes, ret}}` — a closure-typed arg.
    Closure {
        /// The closure trait kind.
        kind: ClosureKind,
        /// `true` ⇒ the foreign param is `Fn(&A)` (owned-clone bridge).
        by_ref: bool,
        /// The closure's own argument types.
        arg_types: Vec<Self>,
        /// The closure's return type.
        ret: Box<Self>,
    },
    /// `{serdeValue: true}` — a serde-bound generic reduced to
    /// `serde_json::Value`; the Ipê-facing surface is `String` (JSON text).
    SerdeValue,
    /// `{serdeValueRef: true}` — a `&T` serde-Serialize INPUT whose `T` was
    /// reduced to `serde_json::Value`; the call site passes `&sv_j`.
    SerdeValueRef,
}

/// The `{closure: …}` inner object.
#[derive(Debug, serde::Deserialize)]
struct WireClosure {
    kind: ClosureKind,
    #[serde(default, rename = "byRef")]
    by_ref: bool,
    #[serde(rename = "argTypes")]
    arg_types: Vec<WireTypeRef>,
    ret: Box<WireTypeRef>,
}

impl<'de> Deserialize<'de> for WireTypeRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct TypeRefVisitor;

        impl<'de> Visitor<'de> for TypeRefVisitor {
            type Value = WireTypeRef;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a TypeRef object with exactly one discriminator key")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut param: Option<usize> = None;
                let mut prim: Option<String> = None;
                let mut ctor: Option<String> = None;
                let mut args: Option<Vec<WireTypeRef>> = None;
                let mut closure: Option<WireClosure> = None;
                let mut serde_value = false;
                let mut serde_value_ref = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "param" => param = Some(map.next_value()?),
                        "prim" => prim = Some(map.next_value()?),
                        "ctor" => ctor = Some(map.next_value()?),
                        "args" => args = Some(map.next_value()?),
                        "closure" => closure = Some(map.next_value()?),
                        "serdeValue" => serde_value = map.next_value()?,
                        "serdeValueRef" => serde_value_ref = map.next_value()?,
                        _ => {
                            let IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let mut present: Vec<&'static str> = Vec::new();
                if param.is_some() {
                    present.push("param");
                }
                if prim.is_some() {
                    present.push("prim");
                }
                if ctor.is_some() {
                    present.push("ctor");
                }
                if closure.is_some() {
                    present.push("closure");
                }
                if serde_value {
                    present.push("serdeValue");
                }
                if serde_value_ref {
                    present.push("serdeValueRef");
                }
                if present.len() != 1 {
                    return Err(de::Error::custom(
                        WireDefect::TypeRefDiscriminator { present }.to_string(),
                    ));
                }
                if let Some(i) = param {
                    return Ok(WireTypeRef::Param(i));
                }
                if let Some(p) = prim {
                    return Ok(WireTypeRef::Prim(p));
                }
                if let Some(nm) = ctor {
                    return Ok(WireTypeRef::Ctor(nm, args.unwrap_or_default()));
                }
                if let Some(c) = closure {
                    return Ok(WireTypeRef::Closure {
                        kind: c.kind,
                        by_ref: c.by_ref,
                        arg_types: c.arg_types,
                        ret: c.ret,
                    });
                }
                if serde_value {
                    return Ok(WireTypeRef::SerdeValue);
                }
                Ok(WireTypeRef::SerdeValueRef)
            }
        }

        d.deserialize_map(TypeRefVisitor)
    }
}

/// A validated type reference in a NON-argument position (return, path
/// type-argument, method turbofish, or nested constructor argument).
///
/// No closure variant exists — a closure in one of these positions is
/// rejected during domain conversion, so rendering is total by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InnerTypeRef {
    /// The i-th generic param (mangled `a` → `A` at render).
    Param(usize),
    /// A concrete Rust primitive leaf, validated at decode so it renders
    /// verbatim without opening a statement.
    Prim(RustTypeExpr),
    /// `Name<args…>` — the ctor NAME is a validated path/type expression.
    Ctor(RustTypeExpr, Vec<Self>),
    /// A serde-reduced node rendering as `serde_json::Value`.
    SerdeValue,
    /// A `&T` serde-Serialize input rendering as `&serde_json::Value`.
    SerdeValueRef,
}

/// A validated type reference in a DIRECT wrapper-argument slot — the one
/// position where a closure is legal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgTypeRef {
    /// A non-closure argument type.
    Inner(InnerTypeRef),
    /// A closure-typed wrapper arg; its own args/ret are closure-free.
    Closure {
        /// The closure trait kind.
        kind: ClosureKind,
        /// `true` ⇒ the foreign param is `Fn(&A)` (owned-clone bridge).
        by_ref: bool,
        /// The closure's own argument types.
        arg_types: Vec<InnerTypeRef>,
        /// The closure's return type.
        ret: InnerTypeRef,
    },
}

impl TryFrom<WireTypeRef> for InnerTypeRef {
    type Error = CallDefect;

    fn try_from(w: WireTypeRef) -> Result<Self, CallDefect> {
        let to_type = |s: String| -> Result<RustTypeExpr, CallDefect> {
            RustTypeExpr::parse(&s).map_err(|_| CallDefect::TypeUnrenderable { got: s })
        };
        match w {
            WireTypeRef::Param(i) => Ok(Self::Param(i)),
            WireTypeRef::Prim(p) => Ok(Self::Prim(to_type(p)?)),
            WireTypeRef::Ctor(nm, args) => {
                let name = to_type(nm)?;
                let inner: Result<Vec<Self>, CallDefect> =
                    args.into_iter().map(Self::try_from).collect();
                Ok(Self::Ctor(name, inner?))
            }
            WireTypeRef::Closure { .. } => Err(CallDefect::ClosureNestedOrNonDirect),
            WireTypeRef::SerdeValue => Ok(Self::SerdeValue),
            WireTypeRef::SerdeValueRef => Ok(Self::SerdeValueRef),
        }
    }
}

impl TryFrom<WireTypeRef> for ArgTypeRef {
    type Error = CallDefect;

    fn try_from(w: WireTypeRef) -> Result<Self, CallDefect> {
        match w {
            WireTypeRef::Closure {
                kind,
                by_ref,
                arg_types,
                ret,
            } => {
                let inner_args: Result<Vec<InnerTypeRef>, CallDefect> =
                    arg_types.into_iter().map(InnerTypeRef::try_from).collect();
                Ok(Self::Closure {
                    kind,
                    by_ref,
                    arg_types: inner_args?,
                    ret: InnerTypeRef::try_from(*ret)?,
                })
            }
            other => Ok(Self::Inner(InnerTypeRef::try_from(other)?)),
        }
    }
}

impl InnerTypeRef {
    /// Render to a Rust type string. `Param(i)` renders the mangled name of
    /// `params[i]`; validation already proved `i` in range, so the `()` arm is
    /// an unreachable-in-practice total fallback (never a panic).
    #[must_use]
    pub fn render(&self, params: &[String]) -> String {
        match self {
            Self::Param(i) => params
                .get(*i)
                .map_or_else(|| "()".to_owned(), |p| mangle_tvar(p)),
            Self::Prim(p) => p.as_str().to_owned(),
            Self::Ctor(nm, args) => {
                if args.is_empty() {
                    nm.as_str().to_owned()
                } else {
                    let rendered: Vec<String> = args.iter().map(|a| a.render(params)).collect();
                    format!("{}<{}>", nm.as_str(), rendered.join(", "))
                }
            }
            Self::SerdeValue => "serde_json::Value".to_owned(),
            Self::SerdeValueRef => "&serde_json::Value".to_owned(),
        }
    }

    /// Every `Param` index reachable from this node.
    pub(crate) fn param_indices(&self, out: &mut Vec<usize>) {
        match self {
            Self::Param(i) => out.push(*i),
            Self::Ctor(_, args) => {
                for a in args {
                    a.param_indices(out);
                }
            }
            Self::Prim(_) | Self::SerdeValue | Self::SerdeValueRef => {}
        }
    }

    /// Whether a serde-reduced node appears at or anywhere inside this node.
    #[must_use]
    pub fn any_serde(&self) -> bool {
        match self {
            Self::SerdeValue | Self::SerdeValueRef => true,
            Self::Ctor(_, args) => args.iter().any(Self::any_serde),
            Self::Param(_) | Self::Prim(_) => false,
        }
    }

    /// Whether this is the `Vec<_>` ctor an `Iterator`-bound param lowers to.
    /// The inspector emits `::Vec`; the bare `Vec` is accepted too.
    #[must_use]
    pub fn is_vec_ctor(&self) -> bool {
        matches!(self, Self::Ctor(nm, _) if nm.as_str() == "::Vec" || nm.as_str() == "Vec")
    }

    /// Re-serialize to the wire JSON shape. The cached `kernel.json` carries
    /// this re-serialization of the ALREADY-VALIDATED domain value, so a warm
    /// build re-runs the identical decode gate on read.
    #[must_use]
    pub fn to_wire_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Param(i) => json!({"param": i}),
            Self::Prim(p) => json!({"prim": p.as_str()}),
            Self::Ctor(nm, args) => {
                if args.is_empty() {
                    json!({"ctor": nm.as_str()})
                } else {
                    let rendered: Vec<serde_json::Value> =
                        args.iter().map(Self::to_wire_json).collect();
                    json!({"ctor": nm.as_str(), "args": rendered})
                }
            }
            Self::SerdeValue => json!({"serdeValue": true}),
            Self::SerdeValueRef => json!({"serdeValueRef": true}),
        }
    }
}

impl ArgTypeRef {
    /// Every `Param` index reachable from this slot (recurses into a
    /// closure's own arg/ret positions).
    pub(crate) fn param_indices(&self, out: &mut Vec<usize>) {
        match self {
            Self::Inner(t) => t.param_indices(out),
            Self::Closure { arg_types, ret, .. } => {
                for a in arg_types {
                    a.param_indices(out);
                }
                ret.param_indices(out);
            }
        }
    }

    /// Whether a serde-reduced node appears anywhere in this slot.
    #[must_use]
    pub fn any_serde(&self) -> bool {
        match self {
            Self::Inner(t) => t.any_serde(),
            Self::Closure { arg_types, ret, .. } => {
                arg_types.iter().any(InnerTypeRef::any_serde) || ret.any_serde()
            }
        }
    }

    /// Re-serialize to the wire JSON shape (see
    /// [`InnerTypeRef::to_wire_json`]).
    #[must_use]
    pub fn to_wire_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Inner(t) => t.to_wire_json(),
            Self::Closure {
                kind,
                by_ref,
                arg_types,
                ret,
            } => {
                let args: Vec<serde_json::Value> =
                    arg_types.iter().map(InnerTypeRef::to_wire_json).collect();
                json!({"closure": {
                    "kind": kind.as_str(),
                    "byRef": by_ref,
                    "argTypes": args,
                    "ret": ret.to_wire_json(),
                }})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(json: &str) -> Result<WireTypeRef, String> {
        serde_json::from_str::<WireTypeRef>(json).map_err(|e| e.to_string())
    }

    /// Parse a (test-controlled, always grammar-valid) type expression,
    /// falling back to the infallible test constructor so the test file stays
    /// free of the `unwrap`/`expect` deny-set.
    fn ty(s: &str) -> RustTypeExpr {
        RustTypeExpr::parse(s).unwrap_or_else(|_| RustTypeExpr::for_test(s))
    }

    #[test]
    fn decodes_each_single_discriminator_form() {
        assert_eq!(decode(r#"{"param": 0}"#), Ok(WireTypeRef::Param(0)));
        assert_eq!(
            decode(r#"{"prim": "i64"}"#),
            Ok(WireTypeRef::Prim("i64".into()))
        );
        assert_eq!(
            decode(r#"{"ctor": "Vec", "args": [{"param": 1}]}"#),
            Ok(WireTypeRef::Ctor("Vec".into(), vec![WireTypeRef::Param(1)]))
        );
        assert_eq!(
            decode(r#"{"ctor": "::std::string::String"}"#),
            Ok(WireTypeRef::Ctor("::std::string::String".into(), vec![]))
        );
        assert_eq!(
            decode(r#"{"serdeValue": true}"#),
            Ok(WireTypeRef::SerdeValue)
        );
        assert_eq!(
            decode(r#"{"serdeValueRef": true}"#),
            Ok(WireTypeRef::SerdeValueRef)
        );
        assert_eq!(
            decode(
                r#"{"closure": {"kind": "Fn", "byRef": true, "argTypes": [{"param": 0}], "ret": {"prim": "bool"}}}"#
            ),
            Ok(WireTypeRef::Closure {
                kind: ClosureKind::Fn,
                by_ref: true,
                arg_types: vec![WireTypeRef::Param(0)],
                ret: Box::new(WireTypeRef::Prim("bool".into())),
            })
        );
    }

    #[test]
    fn rejects_two_discriminators_and_zero_discriminators() {
        let two = decode(r#"{"param": 0, "prim": "i64"}"#).unwrap_err();
        assert!(two.contains("more than one discriminator"), "{two}");
        let zero = decode(r"{}").unwrap_err();
        assert!(zero.contains("exactly one of"), "{zero}");
        let false_only = decode(r#"{"serdeValue": false}"#).unwrap_err();
        assert!(false_only.contains("exactly one of"), "{false_only}");
    }

    #[test]
    fn rejects_an_unknown_closure_kind() {
        let e =
            decode(r#"{"closure": {"kind": "FnWeird", "argTypes": [], "ret": {"prim": "()"}}}"#)
                .unwrap_err();
        assert!(e.contains("unknown closure kind"), "{e}");
    }

    #[test]
    fn unknown_keys_are_ignored_for_wire_back_compat() {
        assert_eq!(
            decode(r#"{"param": 0, "futureKey": {"deep": [1, 2]}}"#),
            Ok(WireTypeRef::Param(0))
        );
    }

    #[test]
    fn domain_conversion_rejects_a_nested_closure_everywhere_but_arg_position() {
        let clo = WireTypeRef::Closure {
            kind: ClosureKind::Fn,
            by_ref: false,
            arg_types: vec![],
            ret: Box::new(WireTypeRef::Prim("bool".into())),
        };
        let vec_of_clo = WireTypeRef::Ctor("Vec".into(), vec![clo.clone()]);
        assert_eq!(
            InnerTypeRef::try_from(vec_of_clo.clone()),
            Err(CallDefect::ClosureNestedOrNonDirect)
        );
        // The same Vec<closure> is ALSO illegal as an arg slot (closure only
        // DIRECTLY, never inside a container).
        assert_eq!(
            ArgTypeRef::try_from(vec_of_clo),
            Err(CallDefect::ClosureNestedOrNonDirect)
        );
        // A closure whose own arg is a closure is illegal too.
        let higher_order = WireTypeRef::Closure {
            kind: ClosureKind::Fn,
            by_ref: false,
            arg_types: vec![clo.clone()],
            ret: Box::new(WireTypeRef::Prim("bool".into())),
        };
        assert_eq!(
            ArgTypeRef::try_from(higher_order),
            Err(CallDefect::ClosureNestedOrNonDirect)
        );
        // A direct closure arg is fine.
        assert!(ArgTypeRef::try_from(clo).is_ok());
    }

    #[test]
    fn render_is_total_and_mangles_params() {
        let params = vec!["a".to_owned(), "b".to_owned()];
        let t = InnerTypeRef::Ctor(
            ty("::mycrate::Pair"),
            vec![InnerTypeRef::Param(0), InnerTypeRef::Param(1)],
        );
        assert_eq!(t.render(&params), "::mycrate::Pair<A, B>");
        assert_eq!(InnerTypeRef::Prim(ty("i64")).render(&params), "i64");
        assert_eq!(
            InnerTypeRef::SerdeValue.render(&params),
            "serde_json::Value"
        );
        assert_eq!(
            InnerTypeRef::SerdeValueRef.render(&params),
            "&serde_json::Value"
        );
        // Out-of-range param: total fallback, never a panic.
        assert_eq!(InnerTypeRef::Param(9).render(&params), "()");
    }

    #[test]
    fn serde_detection_recurses_into_containers_and_closures() {
        let ret = InnerTypeRef::Ctor(
            ty("::core::result::Result"),
            vec![
                InnerTypeRef::SerdeValue,
                InnerTypeRef::Ctor(ty("::std::string::String"), vec![]),
            ],
        );
        assert!(ret.any_serde());
        let clo = ArgTypeRef::Closure {
            kind: ClosureKind::Fn,
            by_ref: false,
            arg_types: vec![InnerTypeRef::SerdeValueRef],
            ret: InnerTypeRef::Prim(ty("bool")),
        };
        assert!(clo.any_serde());
        assert!(!ArgTypeRef::Inner(InnerTypeRef::Param(0)).any_serde());
    }

    #[test]
    fn vec_ctor_detection_accepts_both_spellings() {
        assert!(InnerTypeRef::Ctor(ty("::Vec"), vec![]).is_vec_ctor());
        assert!(InnerTypeRef::Ctor(ty("Vec"), vec![]).is_vec_ctor());
        assert!(!InnerTypeRef::Ctor(ty("VecDeque"), vec![]).is_vec_ctor());
    }
}
