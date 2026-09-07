//! Inert `update`-arm Cmd WIRING: which compiled effect an arm fires, as data.
//!
//! Where [`super::transition`] makes an arm's MODEL change data, this makes an
//! arm's Cmd WIRING data — WHICH effect fires, not the effect's body. The effect
//! BODY stays compiled (a real `Task`/`Cmd` the compiler emits); the arm records
//! only a stable EFFECT ID selecting which of its compiled effects to fire (or
//! none). A wiring edit — `Increment` now also fires `save` — swaps the selected
//! id, a data patch; a genuinely NEW effect body has no id yet, so it grows the
//! arm's compiled effect table and recompiles.
//!
//! # The mechanism (mirrors the transition table exactly)
//!
//! The compiler bakes an arm's wiring into a [`CmdWiring`] JSON string (an
//! optional effect id) and emits the arm's Cmd production as a call to the ONE
//! compiled [`select_cmd_hot`] over that baked datum and the arm's fixed table of
//! compiled effect thunks. Prod holds only the baked id, so the arm fires exactly
//! the effect a direct compiled arm would — one wiring semantics, dev == prod. In
//! dev a wiring edit ships a new id; the running program's next dispatch of that
//! arm selects the edited effect from its ALREADY-COMPILED table, with no
//! recompile.
//!
//! # Inert + bounded + fail-closed
//!
//! A [`CmdWiring`] carries only an optional effect id (a `u32`). [`select_effect`]
//! returns that id ONLY when it indexes a real slot in the arm's compiled effect
//! table; an id past the table (only reachable when a wiring names an effect this
//! build did not compile — e.g. a stale/crafted dev patch) selects NO effect
//! (fires `Cmd.none`), never an out-of-range index and never an unintended
//! effect. The dev patch channel is untrusted; every unprovable wiring is a total
//! no-effect, so a bad datum can only ever DROP the edit, never fire an effect the
//! arm never wired.

/// An inert description of one `update` arm's Cmd wiring: which compiled effect
/// (by stable id) the arm fires, or `None` for no effect (`Cmd.none`).
///
/// This is the whole datum the compiler bakes and the dev channel patches. It
/// carries no code — an untrusted instance can drive nothing but a bounded
/// index-in-range check that selects one of the arm's OWN compiled effects.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CmdWiring {
    /// The stable id of the compiled effect this arm fires, or `None` for no
    /// effect. An id is an index into the arm's compiled effect table; an id past
    /// the table selects no effect (fail-closed), never an out-of-range access.
    pub effect: Option<u32>,
}

impl CmdWiring {
    /// The no-effect wiring (`Cmd.none`).
    #[must_use]
    pub const fn none() -> Self {
        CmdWiring { effect: None }
    }

    /// The wiring that fires the compiled effect with stable id `id`.
    #[must_use]
    pub const fn effect(id: u32) -> Self {
        CmdWiring { effect: Some(id) }
    }
}

/// Select which effect id an arm fires from an inert [`CmdWiring`], bounded by the
/// arm's compiled effect table length `effect_count`.
///
/// Returns `Some(index)` ONLY when the wiring names an id that indexes a real slot
/// (`id < effect_count`); `None` (fire no effect) when the wiring is `None` OR
/// names an id past the table. An id past the table is only reachable when a
/// wiring names an effect THIS build did not compile (a stale or crafted dev
/// patch); selecting no effect there is the fail-closed choice — it can never
/// index out of range and can never fire an effect the arm did not compile.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn select_effect(wiring: &CmdWiring, effect_count: usize) -> Option<usize> {
    let id = wiring.effect?;
    let idx = id as usize;
    // Fail-closed bound: an id naming a slot this build did not compile selects no
    // effect, never an out-of-range index into the arm's effect table.
    (idx < effect_count).then_some(idx)
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::{CmdWiring, select_effect};

    #[test]
    fn none_wiring_selects_no_effect() {
        assert_eq!(select_effect(&CmdWiring::none(), 3), None);
    }

    #[test]
    fn in_range_id_selects_that_slot() {
        assert_eq!(select_effect(&CmdWiring::effect(0), 3), Some(0));
        assert_eq!(select_effect(&CmdWiring::effect(2), 3), Some(2));
    }

    #[test]
    fn out_of_range_id_selects_no_effect() {
        // An id naming a slot this build did not compile fires no effect — never an
        // out-of-range access, never an unintended effect.
        assert_eq!(select_effect(&CmdWiring::effect(3), 3), None);
        assert_eq!(select_effect(&CmdWiring::effect(99), 3), None);
    }

    #[test]
    fn empty_table_selects_no_effect() {
        assert_eq!(select_effect(&CmdWiring::effect(0), 0), None);
    }
}

// ─── dev-only Cmd-wiring hot-swap overlay ───────────────────────────────────
//
// The running web app owns each arm's Cmd wiring through the emitted
// `select_cmd_hot` call, which rebuilds a `CmdWiring` from the baked JSON on
// every dispatch. To hot-swap a wiring (which compiled effect an arm fires)
// without a recompile, the dev control path registers a replacement `CmdWiring`
// keyed by the arm's *baked-datum signature*; the next `select_cmd_hot` for that
// signature selects the replacement's id instead of the baked id, so the next
// dispatch fires the edited (already-compiled) effect.
//
// The overlay is inert unless [`dev_wiring_active`] holds (flag on AND
// non-production) and shares the `IPE_WATCH_HOT_APPEARANCE` gate — one dev-overlay
// switch for the whole program-as-data surface. In production the flag is off and
// the control path is never mounted, so `select_cmd_hot` selects exactly the
// baked id — one wiring semantics, dev == prod. A replacement can only ever name
// an id in the arm's OWN compiled effect table (or a stale id, which fails closed
// to no effect), so a wiring patch can never fire an effect the arm never
// compiled.

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::collections::HashMap;
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::sync::{Mutex, OnceLock};

/// The registered dev replacements, keyed by an arm's baked-datum JSON signature.
/// Guarded by a `Mutex`; a poisoned lock is recovered (the map holds only inert
/// [`CmdWiring`] data, so a panic mid-dispatch cannot leave it unsound).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
type DevWiringOverlay = HashMap<String, CmdWiring>;

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_wiring_overlay() -> &'static Mutex<DevWiringOverlay> {
    static OVERLAY: OnceLock<Mutex<DevWiringOverlay>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether the Cmd-wiring hot-swap overlay may affect a dispatch: shares the
/// appearance overlay's [`super::literal_table::dev_overlay_active`] gate (the
/// `IPE_WATCH_HOT_APPEARANCE` flag set to a truthy value AND non-production).
///
/// With the flag off (the default) or in production this is `false`, so
/// [`select_cmd_hot`] selects exactly the baked id and the overlay is never
/// consulted — one wiring semantics, dev == prod.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn dev_wiring_active() -> bool {
    super::literal_table::dev_overlay_active()
}

/// Register (or replace) the dev replacement for the arm whose baked datum JSON
/// is `default_json`. A subsequent [`select_cmd_hot`] for that signature selects
/// `replacement`'s id instead of the baked id. No-op when the overlay is
/// inactive, so a stray call in a production build changes nothing.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub fn register_dev_wiring(default_json: &str, replacement: CmdWiring) {
    if !dev_wiring_active() {
        return;
    }
    let mut map = dev_wiring_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.insert(default_json.to_owned(), replacement);
}

/// The dev replacement registered for an arm's baked-datum signature, if any.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_wiring_replacement_for(default_json: &str) -> Option<CmdWiring> {
    let map = dev_wiring_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.get(default_json).cloned()
}

/// Clear all registered dev wiring replacements. Test-support for asserting the
/// flag-off / inert path without cross-test overlay leakage.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub(crate) fn clear_dev_wiring_for_test() {
    let mut map = dev_wiring_overlay()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.clear();
}

/// Select which of an arm's compiled effects (by index into its `effect_count`
/// table) to fire, consulting the dev overlay first. The compiler emits a
/// classified arm's Cmd production as
/// `select_cmd_hot("<baked datum JSON>", <effect count>)`.
///
/// When the dev overlay is active AND a replacement is registered for this exact
/// baked signature, the replacement's id is selected (bounded by `effect_count`);
/// otherwise the baked datum's id is selected. With the flag off / in production
/// the overlay is never consulted, so this selects exactly the baked id — the arm
/// fires byte-identically the effect a direct compiled arm would (dev == prod).
///
/// Total and fail-closed at every seam: a baked datum that fails to decode (only
/// reachable on a codegen defect) or an id past the arm's effect table selects NO
/// effect (`None` → `Cmd.none`), never an out-of-range access and never an
/// unintended effect. The untrusted dev channel can register nothing but an inert
/// [`CmdWiring`], which drives only the bounded [`select_effect`].
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn select_cmd_hot(default_json: &str, effect_count: usize) -> Option<usize> {
    // The overlay is consulted only under the dev gate; in production the branch
    // is never taken, so the baked datum path below is the sole behaviour.
    if dev_wiring_active()
        && let Some(replacement) = dev_wiring_replacement_for(default_json)
    {
        return select_effect(&replacement, effect_count);
    }
    // Decode the baked datum. A decode failure is unreachable for
    // compiler-emitted JSON; it fails closed (no effect) rather than panicking, so
    // even a corrupt constant can never fire an unintended effect.
    match serde_json::from_str::<CmdWiring>(default_json) {
        Ok(baked) => select_effect(&baked, effect_count),
        Err(_) => None,
    }
}

/// Fire the wiring-selected effect from an arm's ordered table of compiled effect
/// thunks, or [`crate::tea::IpeCmd::None`] when the wiring names no effect.
///
/// The compiler emits a classified `update` arm's Cmd position as
/// `fire_cmd_wiring("<baked wiring JSON>", vec![<effect-0 thunk>, ...])`, where
/// each thunk builds ONE of the arm's OWN compiled effects. This selects which id
/// to fire through the bounded [`select_cmd_hot`] (dev overlay consulted under the
/// dev gate; the baked id otherwise, dev == prod), then runs ONLY that thunk — the
/// others are dropped unrun, so effect construction stays lazy.
///
/// Bounded and fail-closed at every seam. `select_cmd_hot` already guarantees the
/// returned id indexes a real thunk (`id < len`); this consumes the thunk table
/// with `into_iter().nth(id)`, which returns `None` (never an out-of-bounds index,
/// never a panic) for any id past the table, so even a defence-in-depth breach of
/// that guarantee fires no effect rather than aborting. An id naming no effect
/// (`Cmd.none`, a stale/crafted dev id past the table, or a datum that fails to
/// decode) fires [`IpeCmd::None`] — a wiring patch can never fire an effect the arm
/// never compiled.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn fire_cmd_wiring<M>(
    default_json: &str,
    thunks: Vec<Box<dyn FnOnce() -> crate::tea::IpeCmd<M>>>,
) -> crate::tea::IpeCmd<M> {
    match select_cmd_hot(default_json, thunks.len()) {
        // `nth` consumes the table and yields the selected thunk by OWNERSHIP (an
        // `FnOnce` cannot be called through a shared `.get` reference), bounded: a
        // stale id past the table yields `None` and fires no effect — never `[i]`,
        // never a panic. Only the selected thunk runs; the rest drop unrun.
        Some(id) => thunks
            .into_iter()
            .nth(id)
            .map_or(crate::tea::IpeCmd::None, |thunk| thunk()),
        None => crate::tea::IpeCmd::None,
    }
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod hot_tests {
    use super::super::literal_table::{overlay_test_lock, set_dev_overlay_active_for_test};
    use super::{CmdWiring, clear_dev_wiring_for_test, register_dev_wiring, select_cmd_hot};

    /// The baked wiring JSON for an arm that fires no effect (`Cmd.none`).
    fn baked_none() -> String {
        serde_json::to_string(&CmdWiring::none()).expect("serialize wiring")
    }

    #[test]
    fn overlay_off_selects_baked_id_only() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // A registered replacement is IGNORED while inactive — the baked `none`
        // fires no effect, byte-identical to a direct compiled arm.
        register_dev_wiring(&baked_none(), CmdWiring::effect(0));
        assert_eq!(
            select_cmd_hot(&baked_none(), 2),
            None,
            "inactive overlay must select the baked id (dev == prod)"
        );

        clear_dev_wiring_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_on_applies_registered_wiring() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_wiring_for_test();

        // The wiring SEAL: an arm that fired no effect now fires the already-
        // compiled effect id 0 — no recompile.
        register_dev_wiring(&baked_none(), CmdWiring::effect(0));
        assert_eq!(
            select_cmd_hot(&baked_none(), 2),
            Some(0),
            "active overlay selects the registered wiring's effect id"
        );

        clear_dev_wiring_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_on_out_of_range_wiring_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_wiring_for_test();

        // A replacement naming an id past the arm's compiled effect table (a
        // genuinely-new effect body this build never compiled) fires NO effect —
        // a wiring patch can never fire an effect the arm never compiled.
        register_dev_wiring(&baked_none(), CmdWiring::effect(5));
        assert_eq!(
            select_cmd_hot(&baked_none(), 2),
            None,
            "an out-of-range wiring must fail closed to no effect"
        );

        clear_dev_wiring_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn corrupt_baked_json_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // A datum that does not decode selects no effect — never a panic, never an
        // unintended effect.
        assert_eq!(select_cmd_hot("not json", 3), None);

        set_dev_overlay_active_for_test(None);
    }
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod fire_tests {
    use super::super::literal_table::{overlay_test_lock, set_dev_overlay_active_for_test};
    use super::{CmdWiring, clear_dev_wiring_for_test, fire_cmd_wiring, register_dev_wiring};
    use crate::tea::IpeCmd;

    /// A thunk building a marker `Batch` effect — a stand-in for an arm's compiled
    /// effect, distinguishable from `None` by `matches!`.
    fn marker_effect() -> Box<dyn FnOnce() -> IpeCmd<i64>> {
        Box::new(|| IpeCmd::Batch(vec![]))
    }

    fn baked_none() -> String {
        serde_json::to_string(&CmdWiring::none()).expect("serialize wiring")
    }
    fn baked_effect(id: u32) -> String {
        serde_json::to_string(&CmdWiring::effect(id)).expect("serialize wiring")
    }

    #[test]
    fn none_wiring_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // A `Cmd.none` wiring fires `IpeCmd::None` even with effects available.
        let cmd = fire_cmd_wiring::<i64>(&baked_none(), vec![marker_effect()]);
        assert!(matches!(cmd, IpeCmd::None));

        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn in_range_wiring_fires_that_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // The baked id 0 fires the arm's compiled effect 0 (dev == prod).
        let cmd = fire_cmd_wiring::<i64>(&baked_effect(0), vec![marker_effect()]);
        assert!(matches!(cmd, IpeCmd::Batch(_)));

        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn out_of_range_baked_id_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // A baked id past the arm's compiled effect table fires no effect — never an
        // out-of-bounds index, never a panic.
        let cmd = fire_cmd_wiring::<i64>(&baked_effect(5), vec![marker_effect()]);
        assert!(matches!(cmd, IpeCmd::None));

        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn empty_table_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // An id against an empty effect table (no compiled effects) fires no effect.
        let cmd = fire_cmd_wiring::<i64>(&baked_effect(0), Vec::new());
        assert!(matches!(cmd, IpeCmd::None));

        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn corrupt_datum_fires_no_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_wiring_for_test();

        // A datum that does not decode fires no effect — never a panic.
        let cmd = fire_cmd_wiring::<i64>("not json", vec![marker_effect()]);
        assert!(matches!(cmd, IpeCmd::None));

        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_hot_swaps_the_fired_effect() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_wiring_for_test();

        // The wiring SEAL through the fire path: an arm baked to fire no effect now
        // fires the already-compiled effect 0 after a dev wiring patch — no recompile.
        register_dev_wiring(&baked_none(), CmdWiring::effect(0));
        let cmd = fire_cmd_wiring::<i64>(&baked_none(), vec![marker_effect()]);
        assert!(matches!(cmd, IpeCmd::Batch(_)));

        clear_dev_wiring_for_test();
        set_dev_overlay_active_for_test(None);
    }
}
