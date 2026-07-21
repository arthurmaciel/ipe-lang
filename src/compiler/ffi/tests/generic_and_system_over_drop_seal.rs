//! Negative SEAL fixtures: two auto-binding shapes that MUST stay over-dropping.
//!
//! Coverage widening admits more shapes only where they are provably sound. Two
//! Bevy-shaped surfaces are deliberately NOT admitted, and this fixture proves
//! each over-drops fail-closed (absent from the Ipê interface, so no program can
//! name it, so the DCE tree-shake removes its wrapper before cargo sees it):
//!
//! * a Bundle-generic method (`Commands::spawn<B: Bundle>(b: B)`) — an OPEN
//!   generic whose `Bundle` bound is not modellable and whose argument would be a
//!   provide-nominal outside the closed instance set. Admitting it would surface a
//!   forwarder onto a generic wrapper the emitter degrades to broken code
//!   (`String::spawn()`), an `ipe`-exit-0 ⇒ cargo-fail breach. The demand-driven
//!   generic path binds a generic FFI call ONLY at a concrete, closed-set,
//!   modellable-bound instantiation — a Bundle instantiation is neither.
//!
//! * a `dyn Fn`/`FnMut` SYSTEM registration whose closure signature is outside the
//!   sound `provide.closure` carrier envelope. The landed closure→run handoff
//!   admits a `provide.closure` adapter (`Fn(A, B) -> R`) as an Ipê forwarder, but
//!   only when every param/return is a closed carrier, the return is total-scalar
//!   or `Result`/`Option`, and the bounds are exactly `{Send, Sync, 'static}`. A
//!   signature outside that envelope (an opaque TOTAL return with no error channel)
//!   over-drops the whole adapter at decode — never emit-and-cargo-fail.
//!
//! All assertions run in the DEFAULT gate — an over-drop is proven by ABSENCE
//! from the interface + a recorded reason, so no cargo build is needed.
#![allow(clippy::expect_used)] // test setup: a failed decode IS the failure

use ipe_ffi::interface::crate_interface;
use ipe_ffi::pkginfo::PkgInfo;

/// A Bundle-generic method: `Commands::spawn<B: Bundle>(bundle: B)`. The open
/// generic block carries the non-modellable `Bundle` bound and a `param 0` arg.
fn bundle_generic_pkg() -> PkgInfo {
    let doc = serde_json::json!({
        "pkg": "bevy_app", "name": "bevy_app", "version": "0.1.0",
        "functions": [{
            "name": "spawn", "effect": "effectful",
            "recvType": "Commands", "methodName": "spawn",
            "generic": {
                "params": ["b"],
                "bounds": {"b": ["Bundle"]},
                "call": {
                    "kind": "method", "path": ["::bevy_app", "Commands"], "method": "spawn",
                    "receiver": {"arg": 0, "by": "refmut"}, "args": [1],
                    "argTypes": [{"ctor": "::bevy_app::Commands"}, {"param": 0}],
                    "ret": {"ctor": "()"}
                }
            }
        }],
        "errors": []
    })
    .to_string();
    PkgInfo::decode_json(&doc).expect("bundle-generic surface decodes")
}

/// The Bundle-generic method over-drops from the interface: an OPEN parametric
/// generic is never surfaced as a static forwarder (its instances are wired
/// demand-driven at concrete, closed-set, modellable-bound call sites — a
/// `Bundle` instantiation is none of those). Admitting it would expose the
/// emitter's degraded generic wrapper to cargo (a SEAL breach); the over-drop is
/// the seal.
#[test]
fn bundle_generic_method_over_drops() {
    let iface = crate_interface(&bundle_generic_pkg());
    assert!(
        iface
            .bindings
            .iter()
            .all(|b| b.ref_name != "spawn_from_commands"),
        "a Bundle-generic method must not be admitted:\n{:?}",
        iface.bindings
    );
    assert!(
        iface.skipped.iter().any(|s| s.ref_name == "spawn_from_commands"
            && s.reason.contains("parametric generic")),
        "the over-drop must record the parametric-generic reason:\n{:?}",
        iface.skipped
    );
}

/// A `dyn Fn` system closure whose return is an opaque TOTAL (`Fn(Query) -> World`)
/// — outside the sound carrier envelope: an opaque total return has no error
/// channel to fold a per-call panic into, so `ClosureSig::parse` refuses it and
/// the whole `provide.closure` adapter over-drops at decode.
fn opaque_total_system_pkg() -> PkgInfo {
    let doc = serde_json::json!({
        "pkg": "bevy_app", "name": "bevy_app", "version": "0.1.0",
        "functions": [{
            "name": "add_system", "effect": "pure", "isClosureAdapter": true,
            "closureSig": "Fn(Query) -> World + Send + Sync + 'static"
        }],
        "errors": []
    })
    .to_string();
    PkgInfo::decode_json(&doc).expect("system surface decodes even when the adapter drops")
}

/// The opaque-total system closure over-drops: `ClosureSig::parse` refuses an
/// opaque total return at decode, so the adapter is never a `FnShape` the
/// interface can surface. Proven by absence of the `add_system` binding + a
/// recorded decode drop.
#[test]
fn opaque_total_system_closure_over_drops() {
    let pkg = opaque_total_system_pkg();
    // The whole closure entry is refused at the decode boundary (an ill-formed
    // signature drops the binding into `dropped`, never a `FnShape`).
    assert!(
        pkg.fns().iter().all(|f| f.name() != "add_system"),
        "the opaque-total system closure must not decode into a binding:\n{:?}",
        pkg.fns()
            .iter()
            .map(ipe_ffi::pkginfo::FnInfo::name)
            .collect::<Vec<_>>()
    );
    assert!(
        !pkg.dropped().is_empty(),
        "the decode boundary must record the refusal"
    );

    let iface = crate_interface(&pkg);
    assert!(
        iface.bindings.iter().all(|b| b.ref_name != "add_system"),
        "the system closure must not be admitted:\n{:?}",
        iface.bindings
    );
}

/// A SOUND multi-arg system closure — the boundary case that DOES bind — anchors
/// the over-drop above as a genuine limit, not a blanket refusal: a
/// `Fn(Model, Msg) -> Result<Model, Error>` (the landed closure→run handoff
/// shape) admits as an arity-1 forwarder. The refusal is precise.
#[test]
fn a_sound_multi_arg_system_closure_still_admits() {
    let doc = serde_json::json!({
        "pkg": "app", "name": "app", "version": "0.1.0",
        "functions": [
            {
                "name": "model_new", "effect": "pure", "isStructCtor": true,
                "structName": "Model",
                "structFields": [{ "name": "value", "type": "i64" }],
                "structDerives": ["Clone"]
            },
            {
                "name": "msg_new", "effect": "pure", "isEnumDef": true,
                "enumName": "Msg",
                "enumVariants": [{ "name": "Tick", "payload": [] }],
                "enumDerives": ["Clone"]
            },
            {
                "name": "update_fn", "effect": "pure", "isClosureAdapter": true,
                "closureSig": "Fn(Model, Msg) -> Result<Model, Error> + Send + Sync + 'static"
            }
        ],
        "errors": []
    })
    .to_string();
    let iface = crate_interface(&PkgInfo::decode_json(&doc).expect("sound surface decodes"));
    let uf = iface.bindings.iter().find(|b| b.ref_name == "update_fn");
    assert!(
        uf.is_some(),
        "the sound multi-arg system closure must admit:\n{:?}",
        iface.skipped
    );
    assert_eq!(
        uf.expect("asserted present").sig,
        "(Model -> Msg -> Model) -> UpdateFnClosure"
    );
}
