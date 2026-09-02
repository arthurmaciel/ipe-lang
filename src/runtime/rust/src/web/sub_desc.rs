//! Inert `subscriptions`-entry descriptions: the TEA-loop counterpart of the
//! `update`-arm [`super::transition`] table, for the subset of `subscriptions`
//! entries whose whole effect is a data-describable tick source.
//!
//! # The mechanism (mirrors the transition table exactly)
//!
//! A `subscriptions` entry like `Time.every 1000 Tick` describes one tick source
//! with no control flow, no function call, no computed message. The compiler
//! reduces such an entry to an inert [`SubDescription`] datum (an interval in
//! milliseconds and the tick message as its serde JSON) and emits the entry as a
//! call to the ONE compiled [`sub_every_hot`] routine over that baked datum. Prod
//! holds only the baked datum, so the entry runs exactly what a direct compiled
//! `Sub.every`/`Time.every` would — one subscription semantics, dev == prod. In
//! dev an edit to the interval or the message ships a new datum over the live
//! socket; the running program swaps the baked datum and the SAME [`sub_every_hot`]
//! rebuilds the `IpeSub`, with no recompile.
//!
//! # Why carry the message as its serde JSON (the soundness argument)
//!
//! At runtime the tick message is an opaque generic `Msg` value; the routine has
//! no static knowledge of a variant as a Rust path. The datum instead carries the
//! message's self-describing serde JSON (`serde_json::to_string(&msg)`) exactly as
//! [`super::transition`] operates on the Model's JSON object: decode the baked JSON
//! back into the real `Msg` via [`serde::de::DeserializeOwned`]. That strict decode
//! is the fail-closed backstop — a datum whose message JSON does not describe a
//! well-typed `Msg` (a renamed variant, a type mismatch) fails to decode and
//! [`sub_every_hot`] returns [`IpeSub::None`] (no subscription), never a torn or
//! coerced message.
//!
//! # Inert + bounded + fail-closed
//!
//! A [`SubDescription`] carries only an interval (an `i64`) and a message JSON (a
//! string). It has no code, no call, no nesting — [`sub_every_hot`] cannot run
//! arbitrary logic, cannot panic/unwrap/index/overflow, and refuses (returns
//! [`IpeSub::None`]) anything it cannot prove: a non-positive interval, an oversized
//! datum, or a message JSON that does not decode into `Msg`. The dev patch channel
//! is untrusted; every failure is a total no-op that installs no subscription.

use crate::tea::IpeSub;

/// Hard ceiling on a baked sub-description's message JSON considered for a
/// hot-swap decode. A message serializing beyond this is not installed (returns
/// [`IpeSub::None`]); the bound keeps the untrusted dev channel from driving an
/// unbounded decode.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub const MAX_SUB_MSG_JSON_BYTES: usize = 64 * 1024;

/// An inert description of one data-describable `subscriptions` entry: the tick
/// interval in milliseconds and the tick message as its serde JSON.
///
/// This is the whole datum the compiler bakes and the dev channel patches. It
/// carries no code and no nesting — an untrusted instance can drive nothing but
/// the bounded [`sub_every_hot`] routine, which either installs a single `Every`
/// tick source or nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubDescription {
    /// The tick interval in milliseconds. A non-positive interval installs no
    /// subscription (matching the runtime `SubManager`, which drops `ms <= 0`).
    pub interval_ms: i64,
    /// The tick message, as the serde JSON of the concrete `Msg` value. Decoded
    /// into `Msg` at install time; a decode failure installs no subscription.
    pub msg_json: String,
}

/// Build an [`IpeSub::Every`] from an inert [`SubDescription`], returning
/// [`IpeSub::None`] on any condition it cannot prove.
///
/// Total and fail-closed: on a non-positive interval, an oversized message JSON,
/// or a message JSON that does not strict-decode into `Msg`, it returns
/// [`IpeSub::None`] (no subscription). It never panics, never unwraps, and never
/// coerces a value into `Msg`: the strict decode is the backstop that rejects
/// anything that would not type-check as the compiled `Sub.every`/`Time.every`.
///
/// The datum is treated as untrusted at the dev boundary; this routine is the
/// single place that installs a subscription from a description, so it refuses
/// rather than guesses.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn build_sub<Msg>(desc: &SubDescription) -> IpeSub<Msg>
where
    Msg: serde::de::DeserializeOwned,
{
    if desc.interval_ms <= 0 {
        return IpeSub::None;
    }
    if desc.msg_json.len() > MAX_SUB_MSG_JSON_BYTES {
        return IpeSub::None;
    }
    // Strict decode is the fail-closed backstop: a message JSON that does not
    // describe a well-typed `Msg` (a renamed variant, a type mismatch) is rejected
    // HERE, so the result is either a well-typed `Every` source or `None` (refuse),
    // never a coerced value.
    match serde_json::from_str::<Msg>(&desc.msg_json) {
        Ok(msg) => IpeSub::Every {
            ms: desc.interval_ms,
            msg,
        },
        Err(_) => IpeSub::None,
    }
}

// ─── dev-only sub-description hot-swap overlay ───────────────────────────────
//
// The running web app owns each data-describable `subscriptions` entry through
// the emitted `sub_every_hot` call, which rebuilds an `IpeSub` from the baked
// datum's JSON on every re-subscribe. To hot-swap a simple entry (a `1000` →
// `500` interval, a different tick `Msg`) without a recompile, the dev control
// path registers a replacement [`SubDescription`] keyed by the entry's *baked-
// datum signature* (the exact JSON the compiler baked, in emit form); the next
// `sub_every_hot` for that signature builds from the replacement instead of the
// baked datum, so the next re-subscribe installs the edited tick source.
//
// Keying by the baked JSON (rather than a single global patch) confines an edit
// to the one entry whose datum it describes: a second entry with a different
// baked datum never sees it. The baked datum string is a compile-time constant,
// so the signature stays stable across re-subscribes.
//
// The overlay is inert unless [`dev_sub_active`] holds (flag on AND
// non-production). It shares the appearance overlay's `IPE_WATCH_HOT_APPEARANCE`
// gate — one dev-overlay switch for the whole program-as-data surface. In a
// production build the flag is off and the dev control path is never mounted, so
// no replacement is ever registered and `sub_every_hot` decodes and builds
// exactly the baked datum — one subscription semantics, dev == prod.

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::collections::HashMap;
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
use std::sync::{Mutex, OnceLock};

/// The registered dev replacements, keyed by an entry's baked-datum JSON
/// signature. Guarded by a `Mutex`; a poisoned lock is recovered (the map holds
/// only inert [`SubDescription`] data, so a panic mid-subscribe cannot leave it
/// unsound).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
type DevSubOverlay = HashMap<String, SubDescription>;

#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_sub_overlay() -> &'static Mutex<DevSubOverlay> {
    static OVERLAY: OnceLock<Mutex<DevSubOverlay>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether the sub-description hot-swap overlay may affect a re-subscribe: shares
/// the appearance overlay's [`super::literal_table::dev_overlay_active`] gate (the
/// `IPE_WATCH_HOT_APPEARANCE` flag set to a truthy value AND non-production).
///
/// With the flag off (the default) or in production this is `false`, so
/// [`sub_every_hot`] decodes and builds exactly the baked datum and the overlay is
/// never consulted — one subscription semantics, dev == prod.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn dev_sub_active() -> bool {
    super::literal_table::dev_overlay_active()
}

/// Register (or replace) the dev replacement for the entry whose baked datum JSON
/// is `default_json`. A subsequent [`sub_every_hot`] for that exact signature
/// builds from `replacement` instead of the baked datum. No-op when the overlay
/// is inactive, so a stray call in a production build changes nothing.
///
/// Replacing (not merging) means the most recent edit for an entry fully
/// describes its current subscription — the watch classifier sends the whole
/// description for the edited entry each time.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub fn register_dev_sub(default_json: &str, replacement: SubDescription) {
    if !dev_sub_active() {
        return;
    }
    let mut map = dev_sub_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(default_json.to_owned(), replacement);
}

/// The dev replacement registered for an entry's baked-datum signature, if any.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn dev_sub_replacement_for(default_json: &str) -> Option<SubDescription> {
    let map = dev_sub_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.get(default_json).cloned()
}

/// Clear all registered dev replacements. Test-support for asserting the
/// flag-off / inert path without cross-test overlay leakage.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
pub(crate) fn clear_dev_sub_for_test() {
    let mut map = dev_sub_overlay().lock().unwrap_or_else(|e| e.into_inner());
    map.clear();
}

/// Build a data-describable `subscriptions` entry's tick source, consulting the
/// dev overlay first. The compiler emits a classified entry as
/// `sub_every_hot("<baked datum JSON>")`.
///
/// The baked `default_json` is the compile-time constant describing the entry's
/// subscription. When the dev overlay is active AND a replacement is registered
/// for this exact baked signature, the replacement is built; otherwise the baked
/// datum is decoded and built. With the flag off / in production the overlay is
/// never consulted, so this decodes and builds exactly the baked datum —
/// byte-identical to a direct compiled `Sub.every`/`Time.every` (dev == prod).
///
/// Total and fail-closed at every seam: a baked datum that fails to decode (only
/// reachable on a codegen defect) returns [`IpeSub::None`], exactly as
/// [`build_sub`] refuses. The untrusted dev channel can register nothing but an
/// inert [`SubDescription`], which drives only the bounded [`build_sub`] routine.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
#[must_use]
pub fn sub_every_hot<Msg>(default_json: &str) -> IpeSub<Msg>
where
    Msg: serde::de::DeserializeOwned,
{
    // The overlay is consulted only under the dev gate; in production the branch
    // is never taken, so the baked datum path below is the sole behaviour.
    if dev_sub_active()
        && let Some(replacement) = dev_sub_replacement_for(default_json)
    {
        return build_sub(&replacement);
    }
    // Decode the baked datum. A decode failure is unreachable for compiler-emitted
    // JSON; it fails closed (no subscription) rather than panicking, so even a
    // corrupt constant can never crash the loop.
    match serde_json::from_str::<SubDescription>(default_json) {
        Ok(baked) => build_sub(&baked),
        Err(_) => IpeSub::None,
    }
}

#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod tests {
    use super::{SubDescription, build_sub};
    use crate::tea::IpeSub;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    enum Msg {
        Tick,
        SetTo(i64),
    }

    fn every(interval_ms: i64, msg_json: &str) -> SubDescription {
        SubDescription {
            interval_ms,
            msg_json: msg_json.to_owned(),
        }
    }

    // ── the data-describable shape builds exactly ─────────────────────────

    #[test]
    fn nullary_msg_builds_every() {
        // `Time.every 1000 Tick` — the message serializes to the JSON string
        // `"Tick"` (serde external tag for a nullary variant).
        let sub: IpeSub<Msg> = build_sub(&every(1000, "\"Tick\""));
        match sub {
            IpeSub::Every { ms, msg } => {
                assert_eq!(ms, 1000);
                assert_eq!(msg, Msg::Tick);
            }
            _ => panic!("expected Every"),
        }
    }

    #[test]
    fn payload_msg_builds_every() {
        // A single-int-payload tick message round-trips through its serde JSON.
        let sub: IpeSub<Msg> = build_sub(&every(500, "{\"SetTo\":7}"));
        match sub {
            IpeSub::Every { ms, msg } => {
                assert_eq!(ms, 500);
                assert_eq!(msg, Msg::SetTo(7));
            }
            _ => panic!("expected Every"),
        }
    }

    // ── refusal: every unprovable case installs NO subscription ────────────

    #[test]
    fn non_positive_interval_refuses() {
        assert!(matches!(
            build_sub::<Msg>(&every(0, "\"Tick\"")),
            IpeSub::None
        ));
        assert!(matches!(
            build_sub::<Msg>(&every(-5, "\"Tick\"")),
            IpeSub::None
        ));
    }

    #[test]
    fn undecodable_msg_refuses() {
        // A message JSON that does not describe a `Msg` variant refuses.
        assert!(matches!(
            build_sub::<Msg>(&every(1000, "\"Nonexistent\"")),
            IpeSub::None
        ));
        assert!(matches!(
            build_sub::<Msg>(&every(1000, "not json")),
            IpeSub::None
        ));
    }

    #[test]
    fn oversized_msg_json_refuses() {
        let big = format!("\"{}\"", "x".repeat(super::MAX_SUB_MSG_JSON_BYTES));
        assert!(matches!(build_sub::<Msg>(&every(1000, &big)), IpeSub::None));
    }
}

// ─── dev-only sub-description hot-swap overlay ──────────────────────────────
//
// The overlay + its gate are process-global, so these tests serialise on the
// appearance overlay's guard (the shared gate) and restore the override + clear
// the registry on the way out. Each proves the dev == prod crux at the reader
// seam: with the overlay OFF, `sub_every_hot` is byte-identical to decoding +
// building the baked datum; with it ON, a registered replacement swaps the
// entry's tick source with no recompile.
#[cfg(test)]
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
mod hot_tests {
    use super::super::literal_table::{overlay_test_lock, set_dev_overlay_active_for_test};
    use super::{SubDescription, clear_dev_sub_for_test, register_dev_sub, sub_every_hot};
    use crate::tea::IpeSub;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    enum Msg {
        Tick,
    }

    /// The baked datum for `Time.every 1000 Tick` (the shape the compiler bakes;
    /// exercised as a literal JSON constant here).
    fn baked_tick_1000() -> String {
        serde_json::to_string(&SubDescription {
            interval_ms: 1000,
            msg_json: "\"Tick\"".to_owned(),
        })
        .expect("serialize sub description")
    }

    #[test]
    fn overlay_off_builds_baked_datum_only() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_sub_for_test();

        // A registered replacement is IGNORED while inactive — the baked `1000ms`
        // builds, byte-identical to a direct compiled `Time.every`.
        register_dev_sub(
            &baked_tick_1000(),
            SubDescription {
                interval_ms: 500,
                msg_json: "\"Tick\"".to_owned(),
            },
        );
        match sub_every_hot::<Msg>(&baked_tick_1000()) {
            IpeSub::Every { ms, .. } => assert_eq!(ms, 1000, "inactive overlay uses baked datum"),
            _ => panic!("expected Every"),
        }

        clear_dev_sub_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn overlay_on_builds_registered_replacement() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(true));
        clear_dev_sub_for_test();

        // The interval SEAL: a `1000ms` entry hot-swapped to `500ms` with no
        // recompile.
        register_dev_sub(
            &baked_tick_1000(),
            SubDescription {
                interval_ms: 500,
                msg_json: "\"Tick\"".to_owned(),
            },
        );
        match sub_every_hot::<Msg>(&baked_tick_1000()) {
            IpeSub::Every { ms, .. } => assert_eq!(ms, 500, "active overlay uses replacement"),
            _ => panic!("expected Every"),
        }

        clear_dev_sub_for_test();
        set_dev_overlay_active_for_test(None);
    }

    #[test]
    fn corrupt_baked_json_refuses_total() {
        let _g = overlay_test_lock();
        set_dev_overlay_active_for_test(Some(false));
        clear_dev_sub_for_test();

        // A datum that does not decode (only reachable on a codegen defect)
        // installs no subscription — never a panic.
        assert!(matches!(sub_every_hot::<Msg>("not json"), IpeSub::None));

        set_dev_overlay_active_for_test(None);
    }
}
