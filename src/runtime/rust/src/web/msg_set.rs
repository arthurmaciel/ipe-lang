//! The additive-only `Msg` SET codec: the schema-level gate that decides whether
//! a live source edit which adds a `Msg` variant (plus its `update` arm and a
//! view button firing it) may hot-swap, or must fall back to a clean recompile.
//!
//! # What a `Msg` hot-swap actually is
//!
//! A returning session's in-flight `handler_id`s are `ipe-id` tree addresses (see
//! [`crate::dom::dispatch`]); each resolves to an `Event` baked into the `view`,
//! and each `Event::OnMsg` carries a `Msg` VARIANT. Adding a new variant is
//! additive iff every variant the running program already dispatches keeps its
//! name AND its payload signature: an old `handler_id` still resolves to the same
//! variant, and the server's accepted `Msg` set only GAINS a case. The button
//! that fires the new variant is a view-template patch, the arm that handles it
//! is a [`super::transition`] patch, and the variant itself extends this SET — so
//! all three hot-swap together with no recompile.
//!
//! A NON-additive `Msg` change — a variant removed, or a variant's payload
//! retyped — could orphan or hijack a live `handler_id` (an old id resolving to a
//! variant whose meaning moved). Such a change is REFUSED here, so the watch loop
//! recompiles and the session re-inits cleanly. This is the exact discipline of
//! [`super::additive`]'s Model-superset rule, applied to the variant set instead
//! of the field set.
//!
//! # The self-describing, schema-tagged descriptor (the soundness argument)
//!
//! A [`MsgSet`] is the running program's `Msg` variant set described as data: a
//! SCHEMA tag (a version marker for this descriptor format) plus, for each
//! variant, its NAME and a closed [`PayloadShape`] (its arity/type signature, NOT
//! its runtime value). [`is_additive_superset`] proves a candidate set is an
//! additive superset of the live one: same schema tag, and every live variant
//! present in the candidate with a byte-identical signature. A missing variant is
//! a removal; a variant whose signature differs is a retype; either fails the
//! proof, so a non-additive change can never be accepted as a hot-swap.
//!
//! # Inert + bounded + fail-closed
//!
//! A [`MsgSet`] carries only variant NAMES and closed shape tags — no code, no
//! payload value, no handler. The descriptor arrives over the untrusted dev
//! channel; it is strict-decoded (parse, don't validate) and every decision is a
//! total, allocation-bounded comparison that returns a `bool`/`None` on any
//! malformed, oversized, or non-additive input. Nothing here mutates the
//! server-held Model or resolves a handler; it only GATES whether the sibling
//! view/transition patches may apply without a recompile. With the dev flag off
//! or in production the overlay is never consulted, so the program dispatches
//! exactly its compiled `Msg` set — dev == prod.

/// The current descriptor-format version. Bumped only if the [`MsgSet`] wire
/// shape itself changes; a candidate carrying a different tag is not comparable
/// to the live set (→ refuse, recompile), so an old dev client can never drive a
/// mismatched-format hot-swap.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MSG_SET_SCHEMA: u32 = 1;

/// Hard ceiling on a serialized [`MsgSet`] descriptor considered for an additive
/// comparison. The descriptor arrives over the dev channel (attacker-influenceable
/// at that boundary); a body beyond this is refused BEFORE it is parsed, so a
/// crafted length can never drive an allocation spike. Mirrors
/// [`super::additive::MAX_ADDITIVE_BODY_BYTES`] so every value-level Web codec
/// shares one bound.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MAX_MSG_SET_BYTES: usize = 64 * 1024 * 1024;

/// The closed payload signature of one `Msg` variant — its arity/type shape, not
/// its runtime value. Exhaustive and wildcard-free: a new payload kind forces a
/// compile-time decision here and in the compiler descriptor, never a silent
/// mis-encode that could equate two differently-typed variants.
///
/// This is a SHAPE, deliberately coarse: it distinguishes the arities a
/// view-driven variant can carry (nullary button, a string/bool/int payload from
/// an input, an opaque compound payload) so a RETYPE across these shapes is
/// detected as non-additive. Two variants with the same name must carry the same
/// shape to count as the same variant.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PayloadShape {
    /// A nullary variant (`Increment`, `Reset`) — a plain button's `OnMsg`.
    Unit,
    /// A single `String` payload (`SetName String`) — an input's `OnString`.
    Str,
    /// A single `Bool` payload (`Toggle Bool`) — a checkbox's `OnBool`.
    Bool,
    /// A single `Int` payload.
    Int,
    /// Any other payload shape (a record, a nested ADT, a multi-arg variant). Two
    /// `Compound` variants with the same name are treated as the same signature
    /// ONLY when their opaque descriptor strings are byte-identical, so a change
    /// inside a compound payload is a retype (refused).
    Compound(String),
}

/// One variant's schema entry: its NAME and its closed [`PayloadShape`]. Carries
/// no value and no code.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MsgVariant {
    /// The variant's name (its emitted Rust ident / serde tag).
    pub name: String,
    /// The variant's closed payload signature.
    pub shape: PayloadShape,
}

/// The running program's `Msg` variant set, described as schema-tagged data.
///
/// This is the whole datum the compiler bakes and the dev channel patches. It
/// carries only a schema tag and a list of `(name, shape)` pairs — no payload
/// value, no handler, no code. An untrusted instance can drive nothing but the
/// bounded [`is_additive_superset`] comparison.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MsgSet {
    /// The descriptor-format version. Must equal [`MSG_SET_SCHEMA`] to be
    /// comparable; a foreign tag is not additively comparable (→ refuse).
    pub schema: u32,
    /// The variant entries. Order is irrelevant to the additive proof (the
    /// comparison is name-keyed), so a pure reorder is not a change.
    pub variants: Vec<MsgVariant>,
}

impl MsgSet {
    /// Build a descriptor at the current schema version from `variants`.
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
    #[must_use]
    pub fn new(variants: Vec<MsgVariant>) -> Self {
        Self {
            schema: MSG_SET_SCHEMA,
            variants,
        }
    }

    /// The signature of the variant named `name`, if present. `None` when the set
    /// has no such variant. A duplicate name (a malformed descriptor) resolves to
    /// the FIRST entry — the additive proof below then still requires that entry's
    /// signature to match, so a duplicate cannot smuggle in a mismatched variant.
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
    fn shape_of(&self, name: &str) -> Option<&PayloadShape> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .map(|v| &v.shape)
    }
}

/// Prove `candidate` is an additive superset of `live`: same schema tag, and
/// every variant in `live` present in `candidate` with a byte-identical
/// [`PayloadShape`].
///
/// Returns `true` ONLY on a proven additive superset — the change added variants
/// and removed/retyped none. Returns `false` on:
/// * a schema-tag mismatch (incomparable descriptor formats);
/// * a `live` variant absent from `candidate` (a REMOVAL — an old `handler_id`
///   would orphan);
/// * a `live` variant whose signature differs in `candidate` (a RETYPE — an old
///   `handler_id` would resolve to a variant whose meaning moved).
///
/// It never inspects a payload VALUE and never resolves a handler; it only
/// answers whether the sibling view/transition patches may hot-swap. A `false`
/// answer is merely a recompile (conservative); a spurious `true` would let a
/// non-additive change hijack a live `handler_id`, so every unprovable case is
/// `false`.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn is_additive_superset(live: &MsgSet, candidate: &MsgSet) -> bool {
    // Incomparable descriptor formats: refuse. An old dev client speaking a
    // different schema version cannot drive a hot-swap against this program.
    if live.schema != candidate.schema {
        return false;
    }
    // Every live variant must survive UNCHANGED in the candidate: present by name
    // AND carrying the identical payload signature. A missing one is a removal; a
    // differing signature is a retype. Either refuses the whole superset proof.
    live.variants.iter().all(|v| {
        candidate
            .shape_of(&v.name)
            .is_some_and(|cand_shape| *cand_shape == v.shape)
    })
}

/// Strict-decode a dev-supplied [`MsgSet`] descriptor from its JSON body, bounded
/// and fail-closed.
///
/// Returns `Some(set)` only when `body` is within [`MAX_MSG_SET_BYTES`] and
/// strict-decodes into a well-formed [`MsgSet`]. Returns `None` on an oversized
/// body (refused BEFORE parsing), a malformed body, or any decode failure — the
/// caller then treats the edit as non-additive and recompiles. The body is
/// untrusted dev-channel input; every failure is a typed `None`, never a panic.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn decode_msg_set(body: &[u8]) -> Option<MsgSet> {
    if body.len() > MAX_MSG_SET_BYTES {
        return None;
    }
    let set: MsgSet = serde_json::from_slice(body).ok()?;
    Some(set)
}

// ─── dev-only accepted-Msg-set overlay ──────────────────────────────────────
//
// The running web app's compiled `Msg` set is fixed at build time. To let a
// live edit that ADDS a variant (plus its arm and button) hot-swap, the dev
// control path registers the edited program's `MsgSet` descriptor — but ONLY
// after proving it is an additive superset of the live baked set. The overlay
// then records the accepted superset, so a returning session's in-flight
// `handler_id`s are known to still resolve (every live variant is still present,
// unchanged) and the sibling view/transition patches may apply.
//
// Keying is a single accepted set (not per-arm): the `Msg` set is a whole-program
// fact, so one descriptor describes the current accepted variant surface. A later
// additive edit replaces it (the newest superset fully describes the surface).
//
// The overlay is inert unless [`dev_msg_set_active`] holds (flag on AND
// non-production). It shares the appearance/transition overlay's
// `IPE_WATCH_HOT_APPEARANCE` gate — one dev switch for the whole program-as-data
// surface. In production the flag is off and the control path is never mounted,
// so no descriptor is ever registered and the program dispatches exactly its
// compiled `Msg` set — dev == prod.

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::sync::{Mutex, OnceLock};

/// The accepted additive-superset `Msg` set, if a dev edit has registered one.
/// `None` until the first acceptance. Guarded by a `Mutex`; a poisoned lock is
/// recovered (the datum is inert schema data, so a panic mid-update cannot leave
/// it unsound).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_msg_set_overlay() -> &'static Mutex<Option<MsgSet>> {
    static OVERLAY: OnceLock<Mutex<Option<MsgSet>>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(None))
}

/// Whether the `Msg`-set hot-swap overlay may be consulted: shares the appearance
/// overlay's [`super::literal_table::dev_overlay_active`] gate (the
/// `IPE_WATCH_HOT_APPEARANCE` flag truthy AND non-production).
///
/// With the flag off (the default) or in production this is `false`, so no
/// descriptor is ever accepted and the overlay is never read — dev == prod.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn dev_msg_set_active() -> bool {
    super::literal_table::dev_overlay_active()
}

/// Register a dev-edited `Msg`-set descriptor IF it is a proven additive superset
/// of `live` (the running program's compiled set). Returns whether the descriptor
/// was accepted.
///
/// No-op returning `false` when the overlay is inactive (a stray call in a
/// production build accepts nothing) or when `candidate` is not an additive
/// superset of `live` (a removal/retype is refused, so the caller recompiles).
/// On acceptance the candidate becomes the recorded accepted set, so a returning
/// session's live `handler_id`s are known to still resolve.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub fn register_dev_msg_set(live: &MsgSet, candidate: MsgSet) -> bool {
    if !dev_msg_set_active() {
        return false;
    }
    // Parse, don't validate: only a PROVEN additive superset is recorded. A
    // non-additive candidate is refused here, so the overlay can only ever hold a
    // set in which every live variant survives unchanged.
    if !is_additive_superset(live, &candidate) {
        return false;
    }
    let mut slot = dev_msg_set_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *slot = Some(candidate);
    true
}

/// The currently accepted additive-superset `Msg` set, if any. `None` when no dev
/// edit has been accepted (the program dispatches its compiled set).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn accepted_dev_msg_set() -> Option<MsgSet> {
    dev_msg_set_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Clear the accepted `Msg`-set overlay. Test-support for asserting the flag-off /
/// inert path without cross-test overlay leakage.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub(crate) fn clear_dev_msg_set_for_test() {
    let mut slot = dev_msg_set_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *slot = None;
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::{
        MSG_SET_SCHEMA, MsgSet, MsgVariant, PayloadShape, decode_msg_set, is_additive_superset,
    };

    fn variant(name: &str, shape: PayloadShape) -> MsgVariant {
        MsgVariant {
            name: name.to_owned(),
            shape,
        }
    }

    /// The counter app's live set: `Increment`, `Decrement` (both nullary).
    fn live_counter() -> MsgSet {
        MsgSet::new(vec![
            variant("Increment", PayloadShape::Unit),
            variant("Decrement", PayloadShape::Unit),
        ])
    }

    // ── additive: adding a variant is a superset ──────────────────────────

    #[test]
    fn adding_a_nullary_variant_is_additive() {
        // The edit adds `Reset` (a new button firing a new nullary variant).
        let candidate = MsgSet::new(vec![
            variant("Increment", PayloadShape::Unit),
            variant("Decrement", PayloadShape::Unit),
            variant("Reset", PayloadShape::Unit),
        ]);
        assert!(
            is_additive_superset(&live_counter(), &candidate),
            "adding a variant while keeping all live ones is additive"
        );
    }

    #[test]
    fn adding_a_payload_variant_is_additive() {
        // A new `SetName String` variant (an input firing OnString).
        let candidate = MsgSet::new(vec![
            variant("Increment", PayloadShape::Unit),
            variant("Decrement", PayloadShape::Unit),
            variant("SetName", PayloadShape::Str),
        ]);
        assert!(is_additive_superset(&live_counter(), &candidate));
    }

    #[test]
    fn identical_set_is_a_superset_of_itself() {
        assert!(
            is_additive_superset(&live_counter(), &live_counter()),
            "an unchanged set trivially preserves every live variant"
        );
    }

    #[test]
    fn reordered_variants_are_still_additive() {
        // The comparison is name-keyed, so a pure reorder preserves every live
        // variant unchanged.
        let candidate = MsgSet::new(vec![
            variant("Decrement", PayloadShape::Unit),
            variant("Increment", PayloadShape::Unit),
        ]);
        assert!(is_additive_superset(&live_counter(), &candidate));
    }

    // ── non-additive: removal / retype refuses ────────────────────────────

    #[test]
    fn removing_a_variant_refuses() {
        // `Decrement` dropped: a live `handler_id` bound to it would orphan.
        let candidate = MsgSet::new(vec![variant("Increment", PayloadShape::Unit)]);
        assert!(
            !is_additive_superset(&live_counter(), &candidate),
            "a removed variant is not an additive superset — must recompile"
        );
    }

    #[test]
    fn retyping_a_variant_payload_refuses() {
        // `Increment` kept its NAME but gained a `String` payload: a live
        // `handler_id` firing the old nullary variant would now resolve to a
        // differently-shaped variant — a hijack. Refuse.
        let candidate = MsgSet::new(vec![
            variant("Increment", PayloadShape::Str),
            variant("Decrement", PayloadShape::Unit),
        ]);
        assert!(
            !is_additive_superset(&live_counter(), &candidate),
            "a retyped variant payload is not additive — must recompile"
        );
    }

    #[test]
    fn compound_payload_change_refuses() {
        let live = MsgSet::new(vec![variant(
            "Save",
            PayloadShape::Compound("{email:String}".to_owned()),
        )]);
        // The compound payload's inner shape changed — a retype.
        let candidate = MsgSet::new(vec![variant(
            "Save",
            PayloadShape::Compound("{email:String,name:String}".to_owned()),
        )]);
        assert!(
            !is_additive_superset(&live, &candidate),
            "a changed compound payload is a retype — refuse"
        );
    }

    #[test]
    fn schema_tag_mismatch_refuses() {
        let mut candidate = live_counter();
        candidate.schema = MSG_SET_SCHEMA + 1;
        assert!(
            !is_additive_superset(&live_counter(), &candidate),
            "an incomparable descriptor format must refuse, never guess"
        );
    }

    // ── decode: bounded + fail-closed ─────────────────────────────────────

    #[test]
    fn decode_roundtrips_a_wellformed_descriptor() {
        let set = live_counter();
        let body = serde_json::to_vec(&set).expect("serialize");
        assert_eq!(decode_msg_set(&body), Some(set));
    }

    #[test]
    fn decode_refuses_malformed_body() {
        assert_eq!(decode_msg_set(b"{not json"), None);
    }

    #[test]
    fn decode_refuses_oversized_body_before_parsing() {
        let oversized = vec![b'0'; super::MAX_MSG_SET_BYTES + 1];
        assert_eq!(
            decode_msg_set(&oversized),
            None,
            "a body past the ceiling is refused before parsing"
        );
    }
}

// ─── dev-only accepted-Msg-set overlay ──────────────────────────────────────
//
// The overlay + its gate are process-global, so these tests serialise on the
// appearance overlay's guard (the shared gate) and restore the override + clear
// the registry on the way out. They prove the dev == prod crux at the acceptance
// seam: with the overlay OFF, no descriptor is ever accepted; with it ON, only a
// proven additive superset is recorded — a removal/retype is refused.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod overlay_tests {
    use super::super::literal_table::{overlay_test_lock, set_dev_overlay_active_for_test};
    use super::{
        MsgSet, MsgVariant, PayloadShape, accepted_dev_msg_set, clear_dev_msg_set_for_test,
        register_dev_msg_set,
    };

    fn variant(name: &str, shape: PayloadShape) -> MsgVariant {
        MsgVariant {
            name: name.to_owned(),
            shape,
        }
    }

    fn live() -> MsgSet {
        MsgSet::new(vec![
            variant("Increment", PayloadShape::Unit),
            variant("Decrement", PayloadShape::Unit),
        ])
    }

    fn with_reset() -> MsgSet {
        MsgSet::new(vec![
            variant("Increment", PayloadShape::Unit),
            variant("Decrement", PayloadShape::Unit),
            variant("Reset", PayloadShape::Unit),
        ])
    }

    #[test]
    fn overlay_off_accepts_nothing() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_msg_set_for_test();

        assert!(
            !register_dev_msg_set(&live(), with_reset()),
            "an inactive overlay accepts no descriptor (dev == prod)"
        );
        assert_eq!(accepted_dev_msg_set(), None);

        clear_dev_msg_set_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_on_accepts_only_additive_superset() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_msg_set_for_test();

        // The additive-Msg SEAL: adding `Reset` is accepted; the accepted set is
        // recorded (every live variant survives, so live handler_ids resolve).
        assert!(
            register_dev_msg_set(&live(), with_reset()),
            "an additive superset is accepted"
        );
        assert_eq!(accepted_dev_msg_set(), Some(with_reset()));

        // A non-additive candidate (drop `Decrement`) is REFUSED and does not
        // disturb the accepted set.
        let removal = MsgSet::new(vec![variant("Increment", PayloadShape::Unit)]);
        assert!(
            !register_dev_msg_set(&live(), removal),
            "a removal is refused (would orphan a live handler_id)"
        );
        assert_eq!(
            accepted_dev_msg_set(),
            Some(with_reset()),
            "a refused candidate must not overwrite the accepted set"
        );

        clear_dev_msg_set_for_test();
        set_dev_overlay_active_for_test(None);
    }
}
