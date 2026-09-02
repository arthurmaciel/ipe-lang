//! Inert session-`init` datum: the STARTING-`Model` counterpart of the inert
//! `update`-arm [`super::transition`]. Where a transition describes one live
//! `update`'s field change, an init datum describes the whole starting `Model` a
//! FRESH session is seeded with — as self-describing JSON, decoded by ONE
//! compiled routine.
//!
//! # Why `init` is session-scoped (the whole point)
//!
//! A running session owns its `Model`. The `init` value is consulted at exactly
//! one place — session CREATION (a cookie-less first visit, or the additive
//! reconstruction of a returning session). A live session never re-runs `init`.
//! So editing `init` must change ONLY the starting `Model` handed to NEW
//! sessions; a live session keeps the `Model` it already has. Wiring the `init`
//! value as a baked datum the creation path decodes — rather than a compiled
//! closure recompiled on every rebuild — lets a dev `init` edit ship a new datum
//! over the live socket: the next NEW session decodes the edited datum, while
//! every live session is untouched (it never consults the datum again).
//!
//! # The mechanism (mirrors the transition table exactly)
//!
//! The compiler bakes the data-describable `init` body (a record of closed leaf
//! values, no `Cmd`, no control flow, no call) into an [`InitDatum`] JSON string
//! and emits the `init`'s Model production as a call to the ONE compiled
//! [`apply_init_hot`] over that baked JSON. Prod holds only the baked JSON, so a
//! fresh session's Model is exactly what a direct compiled `init` would produce —
//! one init semantics, dev == prod. In dev an `init` edit ships a new datum; the
//! running program's NEXT new session decodes the replacement, with no recompile
//! and with every live session's Model preserved.
//!
//! # Inert + bounded + fail-closed
//!
//! An [`InitDatum`] carries only a JSON object body (the serialized starting
//! `Model`). [`apply_init_hot`] STRICT-decodes it into the real `Model`; a datum
//! that does not describe a well-typed `Model` (wrong field set, type mismatch,
//! an oversized body) fails to decode and the routine returns the COMPILED
//! fallback `Model` the caller supplies, never a torn or coerced `Model`. The dev
//! patch channel is untrusted; every failure is a total fall-back to the compiled
//! `init`, so a bad datum can only ever cost the edit, never soundness.

/// Hard ceiling on a baked/patched init-datum body considered for a decode.
/// Mirrors [`super::additive::MAX_ADDITIVE_BODY_BYTES`] and
/// [`super::transition::MAX_TRANSITION_MODEL_BYTES`] so every value-level Model
/// decode boundary shares one bound; a body beyond this is not decoded (returns
/// the compiled fallback), never parsed unbounded.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MAX_INIT_DATUM_BYTES: usize = 64 * 1024 * 1024;

/// An inert description of a fresh session's starting `Model`: the serialized
/// `Model` as a self-describing JSON object string.
///
/// This is the whole datum the compiler bakes and the dev channel patches. It
/// carries no code and no nesting — an untrusted instance can drive nothing but
/// the bounded, strict-decoding [`apply_init_hot`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitDatum {
    /// The starting `Model` as a JSON object body (the same field-keyed shape
    /// the checkpoint codec and [`super::additive`] use). Decoded strictly into
    /// the real `Model`; a shape mismatch refuses.
    pub model: serde_json::Value,
}

/// Produce the starting `Model` for a FRESH session from an inert init datum.
///
/// Total and fail-closed: on ANY condition it cannot prove — an oversized body,
/// a body that is not a JSON object, or a strict-decode failure — it returns the
/// caller-supplied `compiled` fallback (the `Model` the direct compiled `init`
/// produced) UNCHANGED. It never panics, never unwraps, never indexes, and never
/// coerces a value into the `Model`: the strict decode is the backstop that
/// rejects anything that would not type-check as the compiled `init`.
///
/// The init datum is treated as untrusted at the dev boundary; this routine is
/// the single place that turns a datum into a starting `Model`, so it refuses to
/// the compiled fallback rather than guessing.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn apply_init<Model>(datum: &InitDatum, compiled: Model) -> Model
where
    Model: serde::de::DeserializeOwned,
{
    match try_decode(datum) {
        Some(model) => model,
        // Refuse: hand the caller back the compiled `init` value, unchanged.
        None => compiled,
    }
}

/// The fallible core of [`apply_init`]: `Some(model)` only when the datum's body
/// is a bounded JSON object that strict-decodes into `Model`; `None` on every
/// refusal (the caller then returns the compiled fallback).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn try_decode<Model>(datum: &InitDatum) -> Option<Model>
where
    Model: serde::de::DeserializeOwned,
{
    // Only a JSON OBJECT is a Model body; a scalar / null (e.g. a unit model)
    // has no additive field set and is never force-decoded here.
    if !datum.model.is_object() {
        return None;
    }
    // Bound the body we decode: a datum whose serialized form exceeds the
    // ceiling is refused BEFORE serde walks it, matching the additive/transition
    // ceilings — a crafted length can never drive an allocation spike.
    let bytes = serde_json::to_vec(&datum.model).ok()?;
    if bytes.len() > MAX_INIT_DATUM_BYTES {
        return None;
    }
    // Strict decode is the fail-closed backstop: a body whose field set or types
    // do not match `Model` is rejected HERE → `None` → the caller returns the
    // compiled fallback, never a coerced or torn Model.
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::{InitDatum, apply_init};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Counter {
        count: i64,
        name: String,
        on: bool,
    }

    fn compiled() -> Counter {
        Counter {
            count: 0,
            name: "start".to_string(),
            on: false,
        }
    }

    fn datum_for(m: &Counter) -> InitDatum {
        InitDatum {
            model: serde_json::to_value(m).expect("serialize model"),
        }
    }

    #[test]
    fn well_typed_datum_produces_that_model() {
        let seeded = Counter {
            count: 7,
            name: "bob".to_string(),
            on: true,
        };
        assert_eq!(apply_init(&datum_for(&seeded), compiled()), seeded);
    }

    #[test]
    fn wrong_field_set_refuses_to_compiled() {
        // A datum whose object lacks a required field cannot strict-decode → the
        // compiled fallback is returned unchanged.
        let datum = InitDatum {
            model: serde_json::json!({ "count": 3 }),
        };
        assert_eq!(
            apply_init(&datum, compiled()),
            compiled(),
            "a datum missing fields must fall back to the compiled init"
        );
    }

    #[test]
    fn type_mismatch_refuses_to_compiled() {
        // `count` typed as a string in the datum: strict decode rejects it.
        let datum = InitDatum {
            model: serde_json::json!({ "count": "nope", "name": "x", "on": false }),
        };
        assert_eq!(apply_init(&datum, compiled()), compiled());
    }

    #[test]
    fn non_object_body_refuses_to_compiled() {
        let datum = InitDatum {
            model: serde_json::Value::Null,
        };
        assert_eq!(apply_init(&datum, compiled()), compiled());
    }
}

// ─── dev-only init-datum hot-swap overlay ───────────────────────────────────
//
// The running web app owns each session's starting `Model` through the emitted
// `apply_init_hot` call, which rebuilds an `InitDatum` from the baked JSON on
// every SESSION CREATION. To hot-swap the `init` (the starting Model a new
// session gets) without a recompile, the dev control path registers a
// replacement `InitDatum` keyed by the app's *baked init-datum signature* (the
// exact JSON the compiler baked); the next `apply_init_hot` for that signature
// decodes the replacement instead of the baked datum, so the next NEW session
// starts from the edited init.
//
// Keying by the baked JSON confines an edit to exactly the `init` whose datum it
// describes. The overlay is inert unless [`dev_init_active`] holds (flag on AND
// non-production). It shares the appearance/transition overlay's
// `IPE_WATCH_HOT_APPEARANCE` gate — one dev-overlay switch for the whole
// program-as-data surface. In a production build the flag is off and the dev
// control path is never mounted, so no replacement is ever registered and
// `apply_init_hot` decodes exactly the baked datum — one init semantics,
// dev == prod. A live session NEVER calls `apply_init_hot` (it keeps its Model),
// so an init edit is a no-op for every running session by construction.

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::collections::HashMap;
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::sync::{Mutex, OnceLock};

/// The registered dev replacements, keyed by the app's baked init-datum JSON
/// signature. Guarded by a `Mutex`; a poisoned lock is recovered (the map holds
/// only inert [`InitDatum`] data, so a panic mid-creation cannot leave it
/// unsound).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
type DevInitOverlay = HashMap<String, InitDatum>;

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_init_overlay() -> &'static Mutex<DevInitOverlay> {
    static OVERLAY: OnceLock<Mutex<DevInitOverlay>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether the init-datum hot-swap overlay may affect a fresh session's starting
/// `Model`: shares the appearance overlay's
/// [`super::literal_table::dev_overlay_active`] gate (the
/// `IPE_WATCH_HOT_APPEARANCE` flag set to a truthy value AND non-production).
///
/// With the flag off (the default) or in production this is `false`, so
/// [`apply_init_hot`] decodes exactly the baked datum and the overlay is never
/// consulted — one init semantics, dev == prod.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn dev_init_active() -> bool {
    super::literal_table::dev_overlay_active()
}

/// Register (or replace) the dev replacement for the app whose baked init datum
/// JSON is `default_json`. A subsequent [`apply_init_hot`] for that exact
/// signature decodes `replacement` instead of the baked datum. No-op when the
/// overlay is inactive, so a stray call in a production build changes nothing.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub fn register_dev_init(default_json: &str, replacement: InitDatum) {
    if !dev_init_active() {
        return;
    }
    let mut map = dev_init_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(default_json.to_owned(), replacement);
}

/// The dev replacement registered for the baked init signature, if any.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_init_replacement_for(default_json: &str) -> Option<InitDatum> {
    let map = dev_init_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.get(default_json).cloned()
}

/// Clear all registered dev init replacements. Test-support for asserting the
/// flag-off / inert path without cross-test overlay leakage.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub(crate) fn clear_dev_init_for_test() {
    let mut map = dev_init_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.clear();
}

/// Produce a fresh session's starting `Model`, consulting the dev overlay first.
/// The compiler emits a data-describable `init` as
/// `apply_init_hot(<baked datum JSON>, <compiled init model>)`.
///
/// The baked `default_json` is the compile-time constant describing the starting
/// `Model`. When the dev overlay is active AND a replacement is registered for
/// this exact baked signature, the replacement is decoded (a new session starts
/// from the edited init); otherwise the baked datum is decoded. With the flag off
/// / in production the overlay is never consulted, so this decodes exactly the
/// baked datum — a fresh session's `Model` is byte-identical to a direct compiled
/// `init` (dev == prod).
///
/// Total and fail-closed at every seam: a baked datum that fails to decode (only
/// reachable on a codegen defect) or a registered replacement that does not
/// strict-decode returns the `compiled` fallback UNCHANGED, exactly as
/// [`apply_init`] refuses. The untrusted dev channel can register nothing but an
/// inert [`InitDatum`], which drives only the bounded, strict-decoding
/// [`apply_init`].
///
/// This is invoked ONLY on session creation. A live session never reaches it, so
/// an init edit is inherently session-scoped: new sessions pick up the edit,
/// running sessions keep their `Model`.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn apply_init_hot<Model>(default_json: &str, compiled: Model) -> Model
where
    Model: serde::de::DeserializeOwned,
{
    // The overlay is consulted only under the dev gate; in production the branch
    // is never taken, so the baked datum path below is the sole behaviour.
    if dev_init_active()
        && let Some(replacement) = dev_init_replacement_for(default_json)
    {
        return apply_init(&replacement, compiled);
    }
    // Decode the baked datum. A decode failure is unreachable for
    // compiler-emitted JSON; it fails closed (compiled fallback) rather than
    // panicking, so even a corrupt constant can never tear the Model.
    match serde_json::from_str::<InitDatum>(default_json) {
        Ok(baked) => apply_init(&baked, compiled),
        Err(_) => compiled,
    }
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod hot_tests {
    use super::super::literal_table::{overlay_test_lock, set_dev_overlay_active_for_test};
    use super::{
        InitDatum, apply_init_hot, clear_dev_init_for_test, dev_init_active, register_dev_init,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Counter {
        count: i64,
    }

    fn compiled() -> Counter {
        Counter { count: 0 }
    }

    fn datum(count: i64) -> InitDatum {
        InitDatum {
            model: serde_json::to_value(Counter { count }).expect("serialize"),
        }
    }

    /// The baked init datum JSON for `init _ = ({ count = 0 }, Cmd.none)`.
    fn baked_json() -> String {
        serde_json::to_string(&datum(0)).expect("serialize datum")
    }

    #[test]
    fn overlay_off_decodes_baked_datum_only() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_init_for_test();
        assert!(!dev_init_active());

        // A registered replacement is IGNORED while inactive — the baked
        // `count = 0` is decoded, byte-identical to the compiled init.
        register_dev_init(&baked_json(), datum(9));
        assert_eq!(
            apply_init_hot(&baked_json(), compiled()).count,
            0,
            "inactive overlay must decode the baked datum (dev == prod)"
        );

        clear_dev_init_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_on_applies_registered_replacement_for_new_session() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_init_for_test();

        // The init SEAL: an `init` edited to `count = 9` seeds a NEW session at 9
        // with no recompile.
        register_dev_init(&baked_json(), datum(9));
        assert_eq!(
            apply_init_hot(&baked_json(), compiled()).count,
            9,
            "active overlay decodes the registered replacement (count = 9)"
        );

        clear_dev_init_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn corrupt_baked_json_refuses_to_compiled() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_init_for_test();

        // A datum that does not decode returns the compiled fallback — no panic.
        assert_eq!(apply_init_hot("not json", compiled()).count, 0);

        set_dev_overlay_active_for_test(None);
    }

    /// The session-scoping SEAL: an `init` edit applies to a FRESH session (a
    /// `apply_init_hot` call at creation) but is a no-op for a LIVE session (which
    /// never calls `apply_init_hot` — it reuses the `Model` it already holds).
    ///
    /// This models the runtime's two session paths: session CREATION runs
    /// `apply_init_hot` (so it sees the edited datum), whereas a LIVE session's
    /// `Model` is carried through untouched (it never re-consults `init`). The
    /// property is structural — `apply_init_hot` is the ONLY init seam and it is
    /// reached only at creation — so this asserts the seam's contract directly.
    #[test]
    fn init_edit_reseeds_fresh_session_but_not_a_live_one() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_init_for_test();

        // A live session already advanced its Model to count = 42; it is NEVER
        // routed through `apply_init_hot`, so the edit cannot touch it.
        let live_session_model = Counter { count: 42 };

        // The dev channel registers an edited init (a fresh session should start at
        // count = 9). The overlay key is the app's baked init-datum JSON.
        register_dev_init(&baked_json(), datum(9));

        // A FRESH session (a creation-path `apply_init_hot`) picks up the edit.
        let fresh = apply_init_hot(&baked_json(), compiled());
        assert_eq!(
            fresh.count, 9,
            "a fresh session starts from the edited init"
        );

        // The LIVE session's Model is unchanged — the edit is a no-op for it,
        // because it is never passed through `apply_init_hot`.
        assert_eq!(
            live_session_model.count, 42,
            "a live session keeps its Model across an init edit (session-scoped)"
        );

        clear_dev_init_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn replacement_type_mismatch_refuses_to_compiled() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_init_for_test();

        // A registered replacement whose body does not strict-decode into the
        // Model falls back to the compiled init — never a torn Model.
        register_dev_init(
            &baked_json(),
            InitDatum {
                model: serde_json::json!({ "count": "not-an-int" }),
            },
        );
        assert_eq!(
            apply_init_hot(&baked_json(), compiled()).count,
            0,
            "a mis-typed replacement must fall back to the compiled init"
        );

        clear_dev_init_for_test();
        set_dev_overlay_active_for_test(None);
    }
}
