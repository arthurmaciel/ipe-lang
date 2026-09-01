//! Additive-superset Model reconstruction for a returning session whose
//! persisted checkpoint predates a purely-additive `Model` change.
//!
//! When an app's `Model` gains a NEW field (and nothing else changes), the
//! opaque schema tag on a persisted checkpoint no longer matches the live
//! binary's, so the reject-before-deserialize gate drops the session and the
//! user loses their live state (scroll / form / counter). This module lets the
//! runtime instead KEEP that state: it decodes the persisted fields and fills
//! each genuinely-new field from the app's own `init` value.
//!
//! # Why a self-describing body
//! A positional codec (bincode) cannot decode an N-field blob into an
//! N+1-field struct — the appended field has no bytes to read. A field-keyed
//! JSON object can: the persisted object carries every old field by NAME, the
//! live `init` object carries every current field by name, and the merge is
//! "start from `init`, overlay every field the persisted object still has".
//!
//! # The additive-superset rule (the whole soundness argument)
//! [`reconstruct`] attempts the splice ONLY when BOTH sides are JSON objects
//! and the persisted object's key set is a SUBSET of the live `init` object's
//! key set — i.e. the change added keys and removed none. It then overlays the
//! persisted value for each shared key onto the `init` object and deserializes
//! the merged object into the strict `Model`. That final strict decode is the
//! backstop: a field kept its name but changed type (a retype) leaves an
//! old-typed value under a live key, and the strict decode REJECTS it, so a
//! retype degrades to a clean re-init rather than a coerced Model. A removed
//! field is a persisted key absent from `init` — the subset check fails, so a
//! removal is NOT a superset and also re-inits. A reorder is invisible to a
//! keyed object and needs no handling. Any parse failure, any non-object on
//! either side, any oversized input → `None` (the caller re-inits).
//!
//! The result is fail-closed: a splice happens only on a PROVEN additive
//! superset, and even then only if the merged object decodes strictly. Every
//! other shape — including an adversarial persisted blob — falls back to the
//! same clean `init` the reject-before-deserialize gate always produced.

/// Hard ceiling on a persisted checkpoint body considered for an additive
/// splice. A persisted blob's length is attacker-influenceable at the storage
/// boundary (a crafted at-rest row); parsing an unbounded JSON body would let a
/// crafted length drive allocation. Mirrors the bincode path's
/// `store::MAX_CHECKPOINT_BYTES` so both decode boundaries share one ceiling.
/// A body beyond this is not spliced (→ `None`, clean re-init), never parsed.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MAX_ADDITIVE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Attempt an additive-superset reconstruction of `Model`.
///
/// `persisted_body` is the raw checkpoint body (a self-describing JSON object,
/// serialized under the OLD schema). `init_model` is the live app's `init`
/// value under the CURRENT schema — the source of every new field's default.
///
/// Returns `Some(model)` only when the persisted schema is a PROVEN additive
/// subset of the current one (every persisted field still present by name, only
/// new fields appended) AND the spliced object decodes strictly into `Model`.
/// Returns `None` on every non-additive change (removed / retyped field), on
/// any parse failure, on a non-object body, on an oversized body, or on a
/// corrupt blob — the caller then re-inits cleanly. Never panics; the persisted
/// body is untrusted input and every failure is a typed `None`.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub fn reconstruct<Model>(persisted_body: &[u8], init_model: &Model) -> Option<Model>
where
    Model: serde::Serialize + serde::de::DeserializeOwned,
{
    // Bounded parse: an oversized body is turned back BEFORE serde_json walks
    // it, so a crafted length can never drive an allocation spike here.
    if persisted_body.len() > MAX_ADDITIVE_BODY_BYTES {
        return None;
    }
    let persisted: serde_json::Value = serde_json::from_slice(persisted_body).ok()?;
    let live: serde_json::Value = serde_json::to_value(init_model).ok()?;
    let merged = merge_additive(&persisted, &live)?;
    // Strict decode into the real Model is the soundness backstop: a retyped
    // field whose old value does not fit the new type fails HERE → None → the
    // caller re-inits, never a coerced or torn Model.
    serde_json::from_value(merged).ok()
}

/// Prove `persisted` is an additive subset of `live` and produce the merged
/// object: `live` with every key `persisted` still carries overlaid from
/// `persisted`. `None` unless BOTH are objects and every persisted key is
/// present in `live` (subset — no removed field).
///
/// A nested object is NOT recursed into: only the TOP-LEVEL Model field set is
/// treated additively. A change inside a nested field's own shape is a type
/// change of that field, handled by the strict decode backstop in
/// [`reconstruct`] (it either still fits — kept as-is — or fails → re-init),
/// never a silent partial merge of a nested value.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn merge_additive(
    persisted: &serde_json::Value,
    live: &serde_json::Value,
) -> Option<serde_json::Value> {
    let persisted_obj = persisted.as_object()?;
    let live_obj = live.as_object()?;
    // Subset check: every persisted field must still exist in the live schema.
    // A persisted key missing from `live` is a REMOVED field — not additive —
    // so the whole splice is refused (clean re-init).
    for key in persisted_obj.keys() {
        if !live_obj.contains_key(key) {
            return None;
        }
    }
    // Start from the live `init` object (carries every current field, so new
    // fields already hold their init value) and overlay each persisted field.
    let mut merged = live_obj.clone();
    for (key, value) in persisted_obj {
        // `key` was proven present above; this only overwrites, never inserts a
        // key absent from the live schema.
        merged.insert(key.clone(), value.clone());
    }
    Some(serde_json::Value::Object(merged))
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::reconstruct;
    use serde::{Deserialize, Serialize};

    // The "old" Model: two fields. A persisted checkpoint is JSON of THIS.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Old {
        count: i64,
        name: String,
    }

    // The "new" Model: the old two fields PLUS an appended `scroll` field —
    // a purely additive change.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct NewAdditive {
        count: i64,
        name: String,
        scroll: i64,
    }

    // A retyped Model: `count` changed Int -> String (same name, new type).
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Retyped {
        count: String,
        name: String,
    }

    // A field-removed Model: `name` dropped.
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Removed {
        count: i64,
    }

    fn old_body() -> Vec<u8> {
        serde_json::to_vec(&Old {
            count: 7,
            name: "alice".to_string(),
        })
        .expect("serializing the old model cannot fail")
    }

    /// (a) Additive: an old checkpoint restores into the new Model with every
    /// old field preserved and the NEW field taking `init`'s value.
    #[test]
    fn additive_field_preserves_old_state_and_fills_new_from_init() {
        // The live app's init for the CURRENT (three-field) Model.
        let init = NewAdditive {
            count: 0,
            name: String::new(),
            scroll: 99, // init's value for the new field
        };
        let got: Option<NewAdditive> = reconstruct(&old_body(), &init);
        assert_eq!(
            got,
            Some(NewAdditive {
                count: 7,             // preserved from the checkpoint
                name: "alice".into(), // preserved from the checkpoint
                scroll: 99,           // filled from init (the new field)
            }),
            "an additive Model change must keep old fields and fill the new \
             field from init"
        );
    }

    /// (b1) Retype: a field kept its name but changed type → clean re-init
    /// (`None`), never a coerced value.
    #[test]
    fn retyped_field_falls_back_to_reinit() {
        let init = Retyped {
            count: String::new(),
            name: String::new(),
        };
        // The persisted `count` is the integer 7; the new `count` is a String.
        // The strict decode of the merged object must reject it.
        let got: Option<Retyped> = reconstruct(&old_body(), &init);
        assert_eq!(
            got, None,
            "a retyped field must fall back to a clean re-init, never coerce"
        );
    }

    /// (b2) Remove: a field was dropped → not an additive superset → clean
    /// re-init (`None`).
    #[test]
    fn removed_field_falls_back_to_reinit() {
        let init = Removed { count: 0 };
        // The persisted object carries `name`, which the current schema lacks:
        // not a superset, so the splice is refused.
        let got: Option<Removed> = reconstruct(&old_body(), &init);
        assert_eq!(
            got, None,
            "a removed field means the persisted set is not a subset of the \
             current one — must re-init, never partially splice"
        );
    }

    /// (b3) Reorder: the same field set in a different declaration order still
    /// restores (a keyed object is order-independent) with state preserved.
    #[test]
    fn reordered_same_fields_still_restores() {
        // Same two fields as `Old`, declared in the opposite order.
        #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
        struct Reordered {
            name: String,
            count: i64,
        }
        let init = Reordered {
            name: String::new(),
            count: 0,
        };
        let got: Option<Reordered> = reconstruct(&old_body(), &init);
        assert_eq!(
            got,
            Some(Reordered {
                name: "alice".into(),
                count: 7,
            }),
            "a pure reorder preserves state — a keyed object is order-independent"
        );
    }

    /// (c1) Corrupt body: non-JSON bytes → clean re-init (`None`), no panic.
    #[test]
    fn corrupt_body_falls_back_to_reinit() {
        let init = NewAdditive {
            count: 0,
            name: String::new(),
            scroll: 0,
        };
        let got: Option<NewAdditive> = reconstruct(b"{not valid json", &init);
        assert_eq!(got, None, "a corrupt body must re-init, never panic");
    }

    /// (c2) Non-object body: a JSON scalar (a bare number) is not an additive
    /// Model object → clean re-init (`None`).
    #[test]
    fn scalar_body_falls_back_to_reinit() {
        let init = NewAdditive {
            count: 0,
            name: String::new(),
            scroll: 0,
        };
        let got: Option<NewAdditive> = reconstruct(b"42", &init);
        assert_eq!(
            got, None,
            "a non-object persisted body is not an additive Model — re-init"
        );
    }

    /// (c3) Oversized body: a body beyond the ceiling is refused BEFORE it is
    /// parsed → clean re-init (`None`), never an allocation spike.
    #[test]
    fn oversized_body_is_refused_before_parsing() {
        let init = NewAdditive {
            count: 0,
            name: String::new(),
            scroll: 0,
        };
        let oversized = vec![b'0'; super::MAX_ADDITIVE_BODY_BYTES + 1];
        let got: Option<NewAdditive> = reconstruct(&oversized, &init);
        assert_eq!(
            got, None,
            "a body past the ceiling must be refused before parsing"
        );
    }

    /// The same-schema case still round-trips: an unchanged Model restores
    /// byte-identically (the splice is a no-op overlay of every field).
    #[test]
    fn unchanged_schema_round_trips() {
        let init = Old {
            count: 0,
            name: String::new(),
        };
        let got: Option<Old> = reconstruct(&old_body(), &init);
        assert_eq!(
            got,
            Some(Old {
                count: 7,
                name: "alice".into(),
            }),
            "an unchanged schema restores every field from the checkpoint"
        );
    }

    /// An empty-Model app (unit `()` serializes to JSON `null`, not an object)
    /// is not spliceable → `None`. Guards the `as_object()` gate: a non-object
    /// live/persisted side is never force-merged.
    #[test]
    fn unit_model_is_not_spliceable() {
        let got: Option<()> = reconstruct(b"null", &());
        assert_eq!(
            got, None,
            "a non-object model has no additive field set — re-init"
        );
    }
}
