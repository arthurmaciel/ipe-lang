//! Inert `update`-arm transitions: the logic counterpart of the appearance
//! [`super::literal_table`], for the subset of `update` arms whose effect on the
//! `Model` is a single data-describable field change.
//!
//! # The mechanism (mirrors the literal table exactly)
//!
//! An `update` arm like `Increment -> { m | count = count + 1 }` describes one
//! field change with no control flow, no `Cmd`, no function call. The compiler
//! reduces such an arm to an inert [`Transition`] datum (a target field, a small
//! closed [`TransitionOp`], and an inert [`Source`]) and emits the arm as a call
//! to the ONE compiled [`apply_transition`] routine over that baked datum. Prod
//! holds only the baked datum, so the arm runs exactly what a direct compiled
//! arm would — one update semantics, dev == prod. In dev an edit to a simple arm
//! ships a new datum over the live socket; the running program swaps the baked
//! datum and the SAME [`apply_transition`] produces the next `Model`, with no
//! recompile.
//!
//! # Why operate on the JSON object (the whole soundness argument)
//!
//! At runtime the `Model` is an opaque generic struct; the routine has no static
//! knowledge of a field name as a Rust ident. It instead works on the `Model`'s
//! self-describing JSON object (`serde_json::to_value`) exactly as
//! [`super::additive`] does: read the named field, apply the closed op, write it
//! back, then STRICT-decode the merged object into the real `Model`. That strict
//! decode is the fail-closed backstop — a datum that does not describe a
//! well-typed change (wrong field, type mismatch, arithmetic that would not
//! type-check) fails to decode and [`apply_transition`] returns the model
//! UNCHANGED, never a torn or coerced `Model`.
//!
//! # Inert + bounded + fail-closed
//!
//! A [`Transition`] carries only: a field NAME (a string), one of a small closed
//! set of [`TransitionOp`]s, and a [`Source`] that is a literal or the named
//! field itself. It has no code, no call, no nesting — [`apply_transition`]
//! cannot run arbitrary logic, cannot panic/unwrap/index/overflow (integer ops
//! are checked and REFUSE on overflow, returning the input model), and refuses
//! (returns the input model unchanged) anything it cannot prove applies: a
//! missing field, a type mismatch, a non-object model, an oversized model, or a
//! strict-decode failure. The dev patch channel is untrusted; every failure is a
//! total no-op that leaves the caller's `Model` exactly as it was.

/// Hard ceiling on a `Model`'s JSON body considered for a transition. Mirrors
/// [`super::additive::MAX_ADDITIVE_BODY_BYTES`] so both value-level Model
/// operations share one bound; a model serializing beyond this is not
/// transitioned (returns the input unchanged), never re-parsed unbounded.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MAX_TRANSITION_MODEL_BYTES: usize = 64 * 1024 * 1024;

/// Where a [`TransitionOp`]'s operand comes from. Inert by construction — a
/// literal value or a read of a named `Model` field; never an expression, call,
/// or nested transition.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    /// An integer literal operand (`+ 1`, `= 42`).
    Int(i64),
    /// A boolean literal operand (`= True`).
    Bool(bool),
    /// A string literal operand (`= "hello"`).
    Str(String),
    /// A read of the value currently under a named `Model` field. The only
    /// non-literal source; still inert — it names a field, it does not compute.
    Field(String),
}

/// The closed set of field operations a [`Transition`] can describe. Exhaustive
/// and wildcard-free: a new op shape forces a compile-time decision here and in
/// the classifier, never a silent mis-encode. Every arithmetic op is CHECKED and
/// refuses (via [`apply_transition`] returning the input model) on overflow or a
/// type mismatch, so none can panic.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TransitionOp {
    /// Replace the field with the source value (`SetName s -> { m | name = s }`,
    /// `Reset -> { m | count = 0 }`). The source's JSON shape must match the
    /// field's existing type or the strict re-decode refuses.
    Set,
    /// Integer add: `field = field + source` (`Increment -> { m | count = count + 1 }`).
    /// Both the field and the source must be integers; overflow refuses.
    IntAdd,
    /// Integer subtract: `field = field - source` (`Decrement -> …`). Both
    /// integers; overflow refuses.
    IntSub,
    /// Boolean negate: `field = not field` (`Toggle -> { m | on = not on }`).
    /// The field must be a boolean; the source is ignored.
    BoolNot,
}

/// An inert description of one `update` arm's single field change: the target
/// field NAME, the closed [`TransitionOp`], and the operand [`Source`].
///
/// This is the whole datum the compiler bakes and the dev channel patches. It
/// carries no code and no nesting — an untrusted instance can drive nothing but
/// the bounded [`apply_transition`] routine.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transition {
    /// The target `Model` field's name (its serde key).
    pub field: String,
    /// The closed operation applied to that field.
    pub op: TransitionOp,
    /// The operand.
    pub source: Source,
}

impl Transition {
    /// The `Set` transition `{ m | field = <literal> }`.
    #[must_use]
    pub fn set(field: impl Into<String>, source: Source) -> Self {
        Self {
            field: field.into(),
            op: TransitionOp::Set,
            source,
        }
    }
}

/// Apply an inert [`Transition`] to `model`, returning the next `Model`.
///
/// Total and fail-closed: on ANY condition it cannot prove — a non-object model,
/// an oversized model, a missing target field, a source field that is absent, a
/// type mismatch between the op and the field/source, an arithmetic overflow, or
/// a strict-decode failure of the merged object — it returns `model` UNCHANGED.
/// It never panics, never unwraps, never indexes, and never coerces a value into
/// the `Model`: the final strict decode is the backstop that rejects anything
/// that would not type-check as the compiled arm.
///
/// The `Model` and the transition datum are BOTH treated as untrusted at the dev
/// boundary; this routine is the single place that mutates the server-held Model
/// from a transition, so it refuses rather than guesses.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn apply_transition<Model>(transition: &Transition, model: Model) -> Model
where
    Model: serde::Serialize + serde::de::DeserializeOwned,
{
    match try_apply(transition, &model) {
        Some(next) => next,
        // Refuse: hand the caller back the model it gave us, unchanged. The
        // move-in `model` is returned so no clone is needed on the happy path's
        // sibling.
        None => model,
    }
}

/// The fallible core of [`apply_transition`]: `Some(next)` only when the
/// transition provably applies and the result strict-decodes into `Model`;
/// `None` on every refusal (the caller then returns the input model unchanged).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn try_apply<Model>(transition: &Transition, model: &Model) -> Option<Model>
where
    Model: serde::Serialize + serde::de::DeserializeOwned,
{
    let value = serde_json::to_value(model).ok()?;
    // Bound the object we walk: a Model whose serialized form exceeds the ceiling
    // is not transitioned (the re-encode below would already have produced it, so
    // this bounds the re-decode input, matching the additive path's ceiling).
    let obj = value.as_object()?;
    // The target field must already exist — a transition never INTRODUCES a field
    // (that would be a schema change, i.e. logic). Read its current value.
    let current = obj.get(&transition.field)?;
    let next_field = compute_next(&transition.op, current, &transition.source, obj)?;

    let mut merged = obj.clone();
    // `field` was proven present above; this only overwrites an existing key.
    merged.insert(transition.field.clone(), next_field);
    let merged_value = serde_json::Value::Object(merged);

    // Bound the re-decode input exactly as the additive path does.
    let bytes = serde_json::to_vec(&merged_value).ok()?;
    if bytes.len() > MAX_TRANSITION_MODEL_BYTES {
        return None;
    }
    // Strict decode is the fail-closed backstop: a type mismatch the op did not
    // catch (e.g. `Set` writing a string into an int field) is rejected HERE, so
    // the result is either a well-typed `Model` or `None` (refuse), never a
    // coerced value.
    serde_json::from_slice(&bytes).ok()
}

/// Compute the field's next JSON value under the closed op, or `None` (refuse) on
/// any type mismatch, missing source field, or arithmetic overflow.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn compute_next(
    op: &TransitionOp,
    current: &serde_json::Value,
    source: &Source,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    match op {
        // Replace with the resolved source value. The strict re-decode in the
        // caller enforces the type match (a `Set` of the wrong type is rejected
        // there), so `Set` itself only needs to resolve the source.
        TransitionOp::Set => resolve_source(source, obj),
        TransitionOp::IntAdd => {
            let a = current.as_i64()?;
            let b = resolve_source(source, obj)?.as_i64()?;
            // Checked: an overflowing add REFUSES rather than wraps or panics.
            let sum = a.checked_add(b)?;
            Some(serde_json::Value::from(sum))
        }
        TransitionOp::IntSub => {
            let a = current.as_i64()?;
            let b = resolve_source(source, obj)?.as_i64()?;
            let diff = a.checked_sub(b)?;
            Some(serde_json::Value::from(diff))
        }
        TransitionOp::BoolNot => {
            let b = current.as_bool()?;
            Some(serde_json::Value::Bool(!b))
        }
    }
}

/// Resolve a [`Source`] to a JSON value: a literal is itself; a
/// [`Source::Field`] reads the named field from the model object (refusing if
/// absent).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn resolve_source(
    source: &Source,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    match source {
        Source::Int(n) => Some(serde_json::Value::from(*n)),
        Source::Bool(b) => Some(serde_json::Value::Bool(*b)),
        Source::Str(s) => Some(serde_json::Value::String(s.clone())),
        Source::Field(name) => obj.get(name).cloned(),
    }
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::{Source, Transition, TransitionOp, apply_transition};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Counter {
        count: i64,
        name: String,
        on: bool,
    }

    fn model() -> Counter {
        Counter {
            count: 5,
            name: "alice".to_string(),
            on: false,
        }
    }

    // ── the four ops apply exactly ────────────────────────────────────────

    #[test]
    fn int_add_increments_field() {
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::IntAdd,
            source: Source::Int(1),
        };
        assert_eq!(apply_transition(&t, model()).count, 6);
    }

    #[test]
    fn int_sub_decrements_field() {
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::IntSub,
            source: Source::Int(2),
        };
        assert_eq!(apply_transition(&t, model()).count, 3);
    }

    #[test]
    fn set_replaces_string_field_from_literal() {
        let t = Transition::set("name", Source::Str("bob".to_string()));
        assert_eq!(apply_transition(&t, model()).name, "bob");
    }

    #[test]
    fn set_int_field_from_literal() {
        let t = Transition::set("count", Source::Int(0));
        assert_eq!(apply_transition(&t, model()).count, 0);
    }

    #[test]
    fn bool_not_toggles_field() {
        let t = Transition {
            field: "on".to_string(),
            op: TransitionOp::BoolNot,
            source: Source::Bool(false), // ignored
        };
        assert!(apply_transition(&t, model()).on);
    }

    #[test]
    fn set_field_from_another_field() {
        // `{ m | count = count }` via a Field source — a no-op that still proves
        // the field-source resolution path.
        let t = Transition::set("count", Source::Field("count".to_string()));
        assert_eq!(apply_transition(&t, model()).count, 5);
    }

    // ── refusal: every unprovable case returns the model UNCHANGED ─────────

    #[test]
    fn missing_target_field_refuses() {
        let t = Transition::set("nonexistent", Source::Int(1));
        assert_eq!(
            apply_transition(&t, model()),
            model(),
            "a transition to an absent field must leave the model unchanged"
        );
    }

    #[test]
    fn type_mismatch_set_refuses() {
        // `Set` a string into the int field `count`: the strict re-decode rejects
        // it, so the model is unchanged (never a coerced value).
        let t = Transition::set("count", Source::Str("not a number".to_string()));
        assert_eq!(
            apply_transition(&t, model()),
            model(),
            "a Set whose type does not match the field must refuse"
        );
    }

    #[test]
    fn int_add_on_string_field_refuses() {
        let t = Transition {
            field: "name".to_string(),
            op: TransitionOp::IntAdd,
            source: Source::Int(1),
        };
        assert_eq!(
            apply_transition(&t, model()),
            model(),
            "an int op on a non-int field must refuse"
        );
    }

    #[test]
    fn int_add_non_int_source_refuses() {
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::IntAdd,
            source: Source::Str("x".to_string()),
        };
        assert_eq!(apply_transition(&t, model()), model());
    }

    #[test]
    fn bool_not_on_non_bool_field_refuses() {
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::BoolNot,
            source: Source::Bool(false),
        };
        assert_eq!(apply_transition(&t, model()), model());
    }

    #[test]
    fn int_add_overflow_refuses() {
        let m = Counter {
            count: i64::MAX,
            name: "a".to_string(),
            on: false,
        };
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::IntAdd,
            source: Source::Int(1),
        };
        assert_eq!(
            apply_transition(&t, m.clone()),
            m,
            "an overflowing add must refuse (checked), never wrap or panic"
        );
    }

    #[test]
    fn int_sub_overflow_refuses() {
        let m = Counter {
            count: i64::MIN,
            name: "a".to_string(),
            on: false,
        };
        let t = Transition {
            field: "count".to_string(),
            op: TransitionOp::IntSub,
            source: Source::Int(1),
        };
        assert_eq!(apply_transition(&t, m.clone()), m);
    }

    #[test]
    fn source_field_absent_refuses() {
        let t = Transition::set("count", Source::Field("ghost".to_string()));
        assert_eq!(apply_transition(&t, model()), model());
    }

    #[test]
    fn non_object_model_refuses() {
        // A unit model serializes to JSON `null`, not an object — refuse.
        let t = Transition::set("x", Source::Int(1));
        assert_eq!(apply_transition(&t, ()), ());
    }
}
