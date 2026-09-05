//! Recognise the data-describable `subscriptions` entries and reduce each to an
//! inert sub-description datum — the TEA-loop counterpart of
//! [`crate::transition_classify`]'s `update`-arm partition.
//!
//! A Web/TEA `subscriptions` function returns `Sub Msg`. An entry is
//! DATA-DESCRIBABLE iff it is exactly a tick source — `Time.every <interval> <msg>`
//! or `Sub.every <interval> <msg>` — whose interval is an integer literal and whose
//! message is a serde-encodable literal `Msg` value (a nullary variant, or a
//! single-`Int`/`Bool`/`String`-payload variant). [`sub_of_entry`] returns `Some`
//! only for such an entry and `None` for everything else, so a non-describable
//! subscription (a `Sub.batch`, a `Sub.map`, a computed message, a WebSocket
//! source, a model-dependent interval) stays compiled — conservative by
//! construction, exactly the appearance-vs-logic split.
//!
//! ## Why a shape match, fail-closed
//!
//! The recognised shape is exactly the one the runtime's `sub_every_hot` executes:
//! an `Every { ms, msg }` built from an interval and the message's serde JSON.
//! EVERY other entry — a different kernel, a non-literal interval, a computed or
//! non-literal message — refuses via a final wildcard-free decision and keeps the
//! entry compiled. A new shape is therefore never encoded by default.
//!
//! ## Inert by construction (dev == prod)
//!
//! A [`CompileSubDescription`] carries only an interval and the message JSON; it
//! has no code and no nesting, mirroring the runtime `web::sub_desc::SubDescription`.
//! Its JSON serialization ([`CompileSubDescription::to_json`]) is byte-identical to
//! the runtime `SubDescription`'s serde form (pinned by a test), and its embedded
//! message JSON is byte-identical to `serde_json::to_string(&msg)` for the compiled
//! `Msg` value (pinned by the conformance test), so the emitted baked datum decodes
//! back into exactly the subscription it described and `sub_every_hot` builds
//! byte-identically what the direct compiled `Time.every`/`Sub.every` would — one
//! subscription semantics, dev == prod.

use crate::emit_template::write_json_string;
use ipe_intern::Symbol;
use ipe_ir::{Callee, Expr, KernelFn};

/// An inert, fully-described single tick `subscriptions` entry reduced to data.
///
/// The `msg_json` is the serde JSON of the concrete tick `Msg` value — a nullary
/// variant serializes to its externally-tagged JSON string (`"Tick"`), a
/// single-payload variant to its tagged object (`{"SetTo":7}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileSubDescription {
    pub interval_ms: i64,
    pub msg_json: String,
}

impl CompileSubDescription {
    /// Serialize to the JSON the runtime `SubDescription` decodes — `serde_json`'s
    /// default representation of `{"interval_ms":…,"msg_json":"…"}`. Deterministic
    /// (fixed field order, deterministic string escaping); byte-identical to
    /// `serde_json::to_string(&SubDescription)` (pinned by a test).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\"interval_ms\":");
        out.push_str(&self.interval_ms.to_string());
        out.push_str(",\"msg_json\":");
        write_json_string(&self.msg_json, &mut out);
        out.push('}');
        out
    }
}

/// Reduce a data-describable `subscriptions` entry to a [`CompileSubDescription`],
/// or `None`.
///
/// Returns `None` when the entry is not provably a single tick source with a
/// literal interval and a serde-encodable literal message (any other kernel, a
/// non-literal interval, a computed message, a nested `batch`/`map`) — the caller
/// then keeps the entry compiled (recompile path).
///
/// `resolve_variant` maps a variant [`Symbol`] to its serde tag (the emitted Rust
/// variant ident, which — no serde rename — is exactly the tag serde uses); a
/// symbol that does not resolve refuses.
///
/// Fail-closed everywhere: this is the conservative half of the appearance/logic
/// split for `subscriptions`. A false `None` is merely a slower rebuild; a false
/// `Some` that diverged from the compiled entry would be a correctness defect — so
/// every unrecognised shape refuses.
pub fn sub_of_entry(
    entry: &Expr,
    resolve_variant: &impl Fn(Symbol) -> Option<String>,
) -> Option<CompileSubDescription> {
    // Only `Sub.every` / `Time.every` — the tick-source kernels — are
    // data-describable. Every other subscription kernel (`Sub.none`, `Sub.batch`,
    // `Sub.map`, `Sub.subscribeTopic`, an HTTP stream) keeps the entry compiled.
    let Expr::Call { callee, args, .. } = entry else {
        return None;
    };
    let Callee::Kernel(kernel) = callee else {
        return None;
    };
    sub_of_call(*kernel, args, resolve_variant)
}

/// Reduce a tick-subscription kernel call to a [`CompileSubDescription`], or `None`.
///
/// The kernel-and-args entry point the emit hook calls directly (it has the callee
/// and args in hand, not the whole [`Expr::Call`]); [`sub_of_entry`] delegates here
/// after peeling the call.
pub fn sub_of_call(
    kernel: KernelFn,
    args: &[Expr],
    resolve_variant: &impl Fn(Symbol) -> Option<String>,
) -> Option<CompileSubDescription> {
    if !matches!(kernel, KernelFn::SubEvery | KernelFn::TimeEvery) {
        return None;
    }
    let [interval_expr, msg_expr] = args else {
        return None;
    };
    // The interval must be an integer literal — a model-dependent or computed
    // interval keeps the entry compiled (the runtime datum carries a bare `i64`).
    let Expr::Int(interval_ms) = interval_expr else {
        return None;
    };
    let msg_json = msg_json_of(msg_expr, resolve_variant)?;
    Some(CompileSubDescription {
        interval_ms: *interval_ms,
        msg_json,
    })
}

/// Serialize a literal tick-message `Expr` to the serde JSON of the `Msg` value it
/// constructs, or `None` for anything not a serde-encodable literal.
///
/// The recognised shapes, exhaustively, match `serde_json`'s externally-tagged
/// enum form for a generated `Msg`:
/// * a nullary variant `V` → the JSON string `"V"`;
/// * a single-`Int`/`Bool`/`String`-payload variant `V x` → the tagged object
///   `{"V":<x>}`.
///
/// Everything else — a multi-field variant, a non-literal payload (a field read, a
/// call, an expression), a bare literal that is not a `Msg` constructor — refuses.
/// The tick message a `subscriptions` entry fires is a concrete `Msg` value, so
/// only a literal `Msg` constructor is faithful to the compiled `Every { msg }`.
fn msg_json_of(msg: &Expr, resolve_variant: &impl Fn(Symbol) -> Option<String>) -> Option<String> {
    let Expr::Ctor { variant, args, .. } = msg else {
        return None;
    };
    let tag = resolve_variant(*variant)?;
    match args.as_slice() {
        // Nullary variant → the externally-tagged JSON string `"V"`.
        [] => {
            let mut out = String::new();
            write_json_string(&tag, &mut out);
            Some(out)
        }
        // Single serde-scalar-literal payload → the tagged object `{"V":<x>}`.
        [payload] => {
            let value = scalar_json(payload)?;
            let mut out = String::new();
            out.push('{');
            write_json_string(&tag, &mut out);
            out.push(':');
            out.push_str(&value);
            out.push('}');
            Some(out)
        }
        // A multi-field variant is not a single serde scalar payload — refuse.
        _ => None,
    }
}

/// Serialize a scalar literal payload `Expr` to its serde JSON, or `None` for
/// anything that is not an `Int` / `Bool` / `String` literal.
fn scalar_json(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Int(n) => Some(n.to_string()),
        Expr::Bool(b) => Some(if *b {
            "true".to_owned()
        } else {
            "false".to_owned()
        }),
        Expr::Str(s) => {
            let mut out = String::new();
            write_json_string(s, &mut out);
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileSubDescription, sub_of_entry, write_json_string};
    use ipe_intern::Symbol;
    use ipe_ir::{CallPin, Callee, Expr, KernelFn, ModPath, OnFormKind};

    fn tick_sym() -> Symbol {
        Symbol::from_raw(10)
    }
    fn set_to_sym() -> Symbol {
        Symbol::from_raw(11)
    }
    fn msg_ty() -> Symbol {
        Symbol::from_raw(1)
    }

    fn resolver(sym: Symbol) -> Option<String> {
        match sym.as_raw() {
            10 => Some("Tick".to_owned()),
            11 => Some("SetTo".to_owned()),
            _ => None,
        }
    }

    fn ctor(variant: Symbol, args: Vec<Expr>) -> Expr {
        Expr::Ctor {
            home: ModPath(vec![msg_ty()]),
            ty: msg_ty(),
            variant,
            args,
        }
    }

    /// `Time.every <interval> <msg>` — the tick-subscription entry shape.
    fn every(kernel: KernelFn, interval: i64, msg: Expr) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(kernel),
            args: vec![Expr::Int(interval), msg],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    fn classify(entry: &Expr) -> Option<CompileSubDescription> {
        sub_of_entry(entry, &resolver)
    }

    // ── acceptance: the data-describable shapes ───────────────────────────

    #[test]
    fn time_every_nullary_msg_classifies() {
        // Time.every 1000 Tick
        let entry = every(KernelFn::TimeEvery, 1000, ctor(tick_sym(), vec![]));
        assert_eq!(
            classify(&entry),
            Some(CompileSubDescription {
                interval_ms: 1000,
                msg_json: "\"Tick\"".to_owned(),
            })
        );
    }

    #[test]
    fn sub_every_nullary_msg_classifies() {
        // Sub.every 500 Tick — the `Sub.every` spelling, same datum.
        let entry = every(KernelFn::SubEvery, 500, ctor(tick_sym(), vec![]));
        assert_eq!(
            classify(&entry),
            Some(CompileSubDescription {
                interval_ms: 500,
                msg_json: "\"Tick\"".to_owned(),
            })
        );
    }

    #[test]
    fn int_payload_msg_classifies() {
        // Time.every 250 (SetTo 7)
        let entry = every(
            KernelFn::TimeEvery,
            250,
            ctor(set_to_sym(), vec![Expr::Int(7)]),
        );
        assert_eq!(
            classify(&entry),
            Some(CompileSubDescription {
                interval_ms: 250,
                msg_json: "{\"SetTo\":7}".to_owned(),
            })
        );
    }

    // ── refusal: everything not provably a single literal tick ────────────

    #[test]
    fn non_literal_interval_refuses() {
        // Time.every model.rate Tick — a non-literal interval keeps it compiled.
        let entry = Expr::Call {
            callee: Callee::Kernel(KernelFn::TimeEvery),
            args: vec![Expr::Var(Symbol::from_raw(99)), ctor(tick_sym(), vec![])],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        assert_eq!(classify(&entry), None);
    }

    #[test]
    fn non_ctor_message_refuses() {
        // Time.every 1000 someVar — a non-constructor message refuses.
        let entry = every(KernelFn::TimeEvery, 1000, Expr::Var(Symbol::from_raw(99)));
        assert_eq!(classify(&entry), None);
    }

    #[test]
    fn non_literal_payload_refuses() {
        // Time.every 1000 (SetTo model.count) — a computed payload refuses.
        let entry = every(
            KernelFn::TimeEvery,
            1000,
            ctor(set_to_sym(), vec![Expr::Var(Symbol::from_raw(99))]),
        );
        assert_eq!(classify(&entry), None);
    }

    #[test]
    fn unresolved_variant_refuses() {
        // A variant symbol the resolver does not know refuses (never a guessed tag).
        let entry = every(
            KernelFn::TimeEvery,
            1000,
            ctor(Symbol::from_raw(77), vec![]),
        );
        assert_eq!(classify(&entry), None);
    }

    #[test]
    fn non_tick_kernel_refuses() {
        // Sub.none is not a tick source.
        let entry = Expr::Call {
            callee: Callee::Kernel(KernelFn::SubNone),
            args: vec![],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        assert_eq!(classify(&entry), None);
    }

    // ── JSON shape ────────────────────────────────────────────────────────

    #[test]
    fn json_datum_shape() {
        let d = CompileSubDescription {
            interval_ms: 1000,
            msg_json: "\"Tick\"".to_owned(),
        };
        assert_eq!(d.to_json(), r#"{"interval_ms":1000,"msg_json":"\"Tick\""}"#);
    }

    #[test]
    fn json_string_escapes_control() {
        let mut out = String::new();
        write_json_string("\u{01}", &mut out);
        assert_eq!(out, "\"\\u0001\"");
    }
}

/// The dev == prod CRUX, proven at the compiler/runtime seam.
///
/// A data-describable subscription entry is emitted as `sub_every_hot(<baked
/// datum>)`; the running program (dev and prod alike) executes that ONE compiled
/// routine over the baked datum. This module proves the two halves agree: the
/// [`CompileSubDescription`] the classifier produces, serialized by
/// [`CompileSubDescription::to_json`], decodes into the runtime
/// `web::sub_desc::SubDescription` and, run through the compiled `build_sub`,
/// yields EXACTLY the `IpeSub` the direct compiled `Time.every`/`Sub.every` would
/// have — so a hot-swap of the datum can never diverge from a full recompile.
///
/// If these ever disagree, dev lies about what prod does — the one unacceptable
/// failure. The test therefore compares `build_sub(interpret(datum))` against a
/// hand-written reproduction of the compiled entry's `IpeSub::Every` for the
/// message the classifier bakes.
#[cfg(test)]
mod conformance {
    use super::CompileSubDescription;
    use ipe_runtime_rust::tea::IpeSub;
    use ipe_runtime_rust::web::sub_desc::{SubDescription, build_sub};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    enum Msg {
        Tick,
        SetTo(i64),
    }

    /// Decode a compile-time datum into the runtime `SubDescription` through its
    /// JSON — the exact path a baked default takes at runtime. A decode failure is
    /// the test's failure (the serializer and codec disagree), surfaced by the
    /// `expect`, which only ever fires on a genuine drift.
    fn interpret(cs: &CompileSubDescription) -> SubDescription {
        serde_json::from_str(&cs.to_json())
            .expect("compile datum JSON must decode into runtime SubDescription")
    }

    /// The `(ms, msg)` of an `IpeSub::Every`, or `None` for any other variant.
    /// Keeps the conformance tests panic-free: they assert on this `Option`, so an
    /// unexpected variant is a clean `assert_eq!` mismatch, never a `panic!`.
    fn every_fields(sub: IpeSub<Msg>) -> Option<(i64, Msg)> {
        match sub {
            IpeSub::Every { ms, msg } => Some((ms, msg)),
            _ => None,
        }
    }

    #[test]
    fn nullary_tick_matches_compiled_entry() {
        // The classifier bakes `Time.every 1000 Tick` — the message JSON is
        // `serde_json::to_string(&Msg::Tick)`.
        let msg_json = serde_json::to_string(&Msg::Tick).expect("serialize");
        let cs = CompileSubDescription {
            interval_ms: 1000,
            msg_json,
        };
        // The compiled entry `Time.every 1000 Tick` builds `Every { ms: 1000, msg: Tick }`.
        assert_eq!(
            every_fields(build_sub::<Msg>(&interpret(&cs))),
            Some((1000, Msg::Tick))
        );
    }

    #[test]
    fn payload_tick_matches_compiled_entry() {
        let msg_json = serde_json::to_string(&Msg::SetTo(7)).expect("serialize");
        let cs = CompileSubDescription {
            interval_ms: 250,
            msg_json,
        };
        assert_eq!(
            every_fields(build_sub::<Msg>(&interpret(&cs))),
            Some((250, Msg::SetTo(7)))
        );
    }

    /// The compile serializer's bytes ARE the runtime codec's bytes: a runtime
    /// `SubDescription` round-trips through `CompileSubDescription::to_json`'s exact
    /// shape. Pins that the two serde forms never drift.
    #[test]
    fn compile_json_is_runtime_sub_description_serde_shape() {
        let cs = CompileSubDescription {
            interval_ms: 1000,
            msg_json: "\"Tick\"".to_owned(),
        };
        let runtime = SubDescription {
            interval_ms: 1000,
            msg_json: "\"Tick\"".to_owned(),
        };
        assert_eq!(
            cs.to_json(),
            serde_json::to_string(&runtime).expect("serialize")
        );
    }

    /// The classifier's baked message JSON for a nullary variant IS
    /// `serde_json::to_string(&Msg::V)` — pins that the compile-side JSON writer
    /// agrees with serde's externally-tagged nullary form, so `build_sub` decodes
    /// exactly the compiled `Msg`.
    #[test]
    fn nullary_msg_json_is_serde_external_tag() {
        // The classifier writes `"Tick"` for a nullary `Tick`; serde agrees.
        assert_eq!(
            "\"Tick\"",
            serde_json::to_string(&Msg::Tick).expect("serialize")
        );
    }

    /// The classifier's baked message JSON for a single-int-payload variant IS
    /// `serde_json::to_string(&Msg::V(x))` — pins the tagged-object form.
    #[test]
    fn payload_msg_json_is_serde_tagged_object() {
        assert_eq!(
            "{\"SetTo\":7}",
            serde_json::to_string(&Msg::SetTo(7)).expect("serialize")
        );
    }
}
