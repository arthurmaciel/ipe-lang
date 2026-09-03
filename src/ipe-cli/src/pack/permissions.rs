//! Deriving a native shell's OS-permission declarations from the app's granted
//! web capabilities — the single source of truth for what a packaged app may do.
//!
//! A packaged Ipê app runs its client code inside a native shell (an iOS/macOS
//! bundle, an Android APK). Each native OS gates the device surfaces a page can
//! reach behind an explicit permission declaration: iOS/macOS require a
//! `Info.plist` usage-description key with a non-empty purpose string; Android
//! requires a `<uses-permission>` (and sometimes a `<uses-feature>`) manifest
//! entry. This module makes those declarations *derived*, never author-supplied:
//! the app's `[capabilities] accepts` set — the same grant the fail-closed
//! app-boundary consent gate reads ([`crate::web_consent`]) — is the sole input,
//! and the permission table here is the sole mapping.
//!
//! Two directions are fail-closed, and both are the security property:
//!
//! - **Accepted ⇒ merged.** Every granted web axis that needs an OS permission
//!   contributes it. The manifest is computed, so it can never under-declare
//!   relative to consent.
//! - **Merged ⇒ accepted.** An author-supplied override that adds an OS
//!   permission with no backing accepted axis is a hard, typed refusal naming
//!   the permission ([`reconcile_override`]). A package can never smuggle an OS
//!   permission the app did not consent to.
//!
//! The axis→permission table is exhaustive over the closed [`WebCapability`]
//! vocabulary: a new web axis added to the compiler forces a new arm here (there
//! is no wildcard), so a future device module cannot ship without its OS-permission
//! mapping considered.

use std::collections::{BTreeMap, BTreeSet};

use ipe_ir::{Capability, WebCapability};

use crate::CliError;

/// A native packaging target whose OS-permission model this module renders for.
///
/// `MacOs` is a distinct member from `Ios` even though both render `Info.plist`
/// usage-description keys: a desktop-mac bundle and an iOS bundle differ in which
/// keys the OS honours (e.g. location-when-in-use is iOS-shaped), so the target
/// is carried explicitly rather than collapsed into a single "Apple" case.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Platform {
    /// An iOS application bundle — `Info.plist` usage-description keys.
    Ios,
    /// A macOS desktop application bundle — `Info.plist` usage-description keys.
    MacOs,
    /// An Android application package — `<uses-permission>` / `<uses-feature>`.
    Android,
}

impl Platform {
    /// Whether this target renders an Apple `Info.plist` (iOS or macOS) rather
    /// than an Android manifest. The Apple targets require a non-empty
    /// usage-description purpose string for every plist key they declare.
    #[must_use]
    const fn is_apple(self) -> bool {
        matches!(self, Self::Ios | Self::MacOs)
    }

    /// The lowercase wire name of this platform, used in the CLI surface and
    /// diagnostics (`ios` / `macos` / `android`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::MacOs => "macos",
            Self::Android => "android",
        }
    }
}

impl std::str::FromStr for Platform {
    type Err = UnknownPlatform;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "ios" => Ok(Self::Ios),
            "macos" => Ok(Self::MacOs),
            "android" => Ok(Self::Android),
            other => Err(UnknownPlatform(other.to_owned())),
        }
    }
}

/// An unrecognised platform name, from [`Platform`]'s
/// [`FromStr`](std::str::FromStr). Carries the offending token so the caller can
/// name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPlatform(pub String);

impl std::fmt::Display for UnknownPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown platform {:?} (expected one of: ios, macos, android)",
            self.0
        )
    }
}

impl std::error::Error for UnknownPlatform {}

/// One `Info.plist` usage-description key and its default purpose string.
///
/// The key is a fixed `NS…UsageDescription` string the OS honours; `purpose` is
/// the human-readable reason shown at the permission prompt. The purpose is
/// invariantly non-empty (Apple rejects an empty usage description at review),
/// guaranteed by construction: every arm of the table supplies a literal.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PlistEntry {
    /// The `NS…UsageDescription` plist key.
    pub key: String,
    /// The non-empty default purpose string the OS displays at the prompt.
    pub purpose: String,
}

/// One Android manifest element derived for a web axis: a `<uses-permission>`.
///
/// A typed kind rather than a raw XML string, so the renderer — not the table —
/// owns the XML surface, and so an entry is matched/compared/deduped by its
/// structured identity rather than by fragile text. It is an enum (not a bare
/// struct) so a future feature-gated axis can add a `<uses-feature>` element as a
/// second variant without touching this one's call sites.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum AndroidEntry {
    /// `<uses-permission android:name="android.permission.NAME"/>`. `name` is the
    /// bare permission constant (e.g. `ACCESS_FINE_LOCATION`).
    UsesPermission {
        /// The permission constant, without the `android.permission.` prefix.
        name: String,
    },
}

/// The per-axis OS-permission requirement across all platforms — the closed SSOT
/// row for one [`WebCapability`].
///
/// `plist` is the Apple usage-description key (absent when the axis needs no
/// plist entry — some web surfaces are user-initiated on Apple platforms and
/// require no static declaration); `android` is the (possibly empty) set of
/// Android manifest elements the axis contributes.
struct AxisRequirement {
    /// The Apple `Info.plist` usage-description entry this axis contributes, or
    /// `None` when the axis needs no static plist declaration on Apple platforms.
    plist: Option<PlistEntry>,
    /// The Android manifest elements this axis contributes (may be empty).
    android: Vec<AndroidEntry>,
}

/// The single source of truth: the OS-permission requirement for one web axis.
///
/// Exhaustive over the closed [`WebCapability`] vocabulary — there is no
/// wildcard arm, so adding a web axis to the compiler forces a new arm here and
/// the crate will not compile until its OS-permission mapping is decided. This is
/// the make-invalid-states-unrepresentable guarantee: no web axis can silently
/// ship un-permissioned.
fn requirement_for(axis: WebCapability) -> AxisRequirement {
    // Helpers keep each arm one line and its intent legible.
    let plist = |key: &str, purpose: &str| PlistEntry {
        key: key.to_owned(),
        purpose: purpose.to_owned(),
    };
    let perm = |name: &str| AndroidEntry::UsesPermission {
        name: name.to_owned(),
    };

    let no_os_permission = || AxisRequirement {
        plist: None,
        android: Vec::new(),
    };

    match axis {
        WebCapability::Geolocation => AxisRequirement {
            plist: Some(plist(
                "NSLocationWhenInUseUsageDescription",
                "This app uses your location to provide location-based features.",
            )),
            android: vec![perm("ACCESS_FINE_LOCATION"), perm("ACCESS_COARSE_LOCATION")],
        },
        WebCapability::Notification => AxisRequirement {
            // Apple: notification authorisation is a runtime prompt, not a static
            // plist key. Android 13+ requires the POST_NOTIFICATIONS permission.
            plist: None,
            android: vec![perm("POST_NOTIFICATIONS")],
        },
        WebCapability::Vibration => AxisRequirement {
            // Apple has no vibration permission. Android gates the vibrator.
            plist: None,
            android: vec![perm("VIBRATE")],
        },
        WebCapability::NetworkInfo => AxisRequirement {
            // Network-information hints. Android reads them behind ACCESS_NETWORK_STATE;
            // Apple exposes them without a usage-description key.
            plist: None,
            android: vec![perm("ACCESS_NETWORK_STATE")],
        },
        // The axes that reach no permission-gated OS surface on any target:
        // clipboard (user-initiated paste), storage (in-sandbox persistence),
        // share (user-initiated share sheet), battery (ungated status), file and
        // camera (both reach the OS only through a user-gesture picker / `<input
        // capture>`, which needs no static usage-description declaration — unlike
        // getUserMedia), and the `raw` floor (an uncharacterised port reaches no
        // compiler-known surface; its disclosure is enforced by the consent gate,
        // not an OS declaration). Named explicitly — no wildcard — so a new web
        // axis stays unmatched and forces a deliberate permission decision here.
        WebCapability::Clipboard
        | WebCapability::Storage
        | WebCapability::Share
        | WebCapability::Battery
        | WebCapability::File
        | WebCapability::Camera
        | WebCapability::Raw => no_os_permission(),
    }
}

/// The typed, per-platform OS-permission declarations derived for an app.
///
/// Constructed only by [`derive_permissions`] from a granted-capability set, so a
/// `PermissionSet` value is always a faithful projection of consent — it cannot
/// be built with a permission the app did not accept. The stored entries are
/// de-duplicated and deterministically ordered (via `BTreeSet`), so two derivations
/// of the same accept-set render byte-identically.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PermissionSet {
    /// The platform this set was derived for; the renderers project accordingly.
    platform: Platform,
    /// The Apple `Info.plist` usage-description entries (empty on Android).
    plist: BTreeSet<PlistEntry>,
    /// The Android manifest elements (empty on Apple platforms).
    android: BTreeSet<AndroidEntry>,
}

impl PermissionSet {
    /// The platform this set targets.
    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    /// Whether this set declares no OS permissions at all (a pure app, or one
    /// whose granted axes need none on this platform).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plist.is_empty() && self.android.is_empty()
    }

    /// The Apple `Info.plist` key/purpose pairs, sorted and de-duplicated.
    ///
    /// Empty on [`Platform::Android`]. Every returned purpose string is non-empty
    /// (an Apple-review invariant guaranteed by the table).
    #[must_use]
    pub fn to_info_plist_entries(&self) -> Vec<(String, String)> {
        self.plist
            .iter()
            .map(|e| (e.key.clone(), e.purpose.clone()))
            .collect()
    }

    /// The Android manifest fragment (the `<uses-permission>` / `<uses-feature>`
    /// lines), sorted, de-duplicated, and rendered as XML.
    ///
    /// Empty on the Apple platforms.
    #[must_use]
    pub fn to_android_manifest_entries(&self) -> AndroidManifestFragment {
        let lines = self
            .android
            .iter()
            .map(render_android_entry)
            .collect::<Vec<_>>();
        AndroidManifestFragment { lines }
    }
}

/// A rendered Android manifest fragment: the ordered `<uses-permission>` /
/// `<uses-feature>` lines a packager splices under `<manifest>`.
///
/// A typed wrapper rather than a bare `String`, so a caller cannot confuse it
/// with arbitrary text and the line list stays inspectable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AndroidManifestFragment {
    lines: Vec<String>,
}

impl AndroidManifestFragment {
    /// The individual manifest element lines, in deterministic order.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Whether the fragment declares no elements.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The fragment as a single newline-joined XML block (no trailing newline).
    #[must_use]
    pub fn to_xml(&self) -> String {
        self.lines.join("\n")
    }
}

/// Render one [`AndroidEntry`] to its XML manifest line. The `android:name`
/// value is a fixed constant from the closed table, never user text, so no XML
/// escaping of untrusted input is required; the constants are ASCII identifiers.
fn render_android_entry(entry: &AndroidEntry) -> String {
    match entry {
        AndroidEntry::UsesPermission { name } => {
            format!("<uses-permission android:name=\"android.permission.{name}\" />")
        }
    }
}

/// Derive the OS-permission declarations for `platform` from the app's granted
/// capability set `accepts` — the fail-closed, deny-by-default derivation.
///
/// Only the [`Capability::JsPort`] members of `accepts` contribute: those are the
/// browser web axes a native shell must map to OS permissions. Every other
/// capability (network, filesystem, native-ffi, …) is a server/Ipê-side axis with
/// no native-shell OS-permission surface and is ignored here. Each granted web
/// axis contributes exactly the entries [`requirement_for`] assigns it on
/// `platform`; an axis that needs no permission contributes nothing. Nothing is
/// contributed for an axis the app did not accept (deny-by-default).
///
/// # Errors
/// Infallible over the closed vocabulary today — the signature returns
/// `Result<_, CliError>` so a future web axis whose derivation *can* fail (an
/// axis needing a per-platform decision the app must resolve) has a channel
/// without changing this public signature.
pub fn derive_permissions(
    accepts: &BTreeSet<Capability>,
    platform: Platform,
) -> Result<PermissionSet, CliError> {
    let mut plist = BTreeSet::new();
    let mut android = BTreeSet::new();

    for cap in accepts {
        let Capability::JsPort(axis) = cap else {
            // A non-web capability has no native-shell OS-permission surface.
            continue;
        };
        let requirement = requirement_for(*axis);
        if platform.is_apple() {
            if let Some(entry) = requirement.plist {
                plist.insert(entry);
            }
        } else {
            for entry in requirement.android {
                android.insert(entry);
            }
        }
    }

    Ok(PermissionSet {
        platform,
        plist,
        android,
    })
}

/// The set of OS permissions the app is entitled to on `platform`, as bare
/// identifiers: Apple plist keys, or Android permission/feature `android:name`s.
///
/// This is the completeness anchor for the fail-closed reconciliation: a merged
/// manifest may declare only permissions in this set. Built from the same
/// derivation as [`derive_permissions`], so it can never drift from what an app
/// actually derives.
fn entitled_permission_names(
    accepts: &BTreeSet<Capability>,
    platform: Platform,
) -> Result<BTreeSet<String>, CliError> {
    let derived = derive_permissions(accepts, platform)?;
    let mut names = BTreeSet::new();
    if platform.is_apple() {
        for (key, _purpose) in derived.to_info_plist_entries() {
            names.insert(key);
        }
    } else {
        for entry in &derived.android {
            names.insert(android_entry_name(entry));
        }
    }
    Ok(names)
}

/// The bare `android:name` identifier of an Android entry, used to reconcile an
/// override against the entitled set.
fn android_entry_name(entry: &AndroidEntry) -> String {
    match entry {
        AndroidEntry::UsesPermission { name } => format!("android.permission.{name}"),
    }
}

/// Reconcile an author-supplied override manifest against the app's granted
/// capabilities — the **merged ⇒ accepted** fail-closed gate.
///
/// `override_permissions` is the set of OS-permission identifiers an author
/// hand-added to the packaged manifest (plist keys on Apple, `android:name`s on
/// Android). Any of them with no backing accepted web axis is a hard refusal
/// naming exactly which permission is un-consented and how to remedy it. A
/// package can therefore never declare an OS permission the app did not accept —
/// the derivation is the single source of truth, and an override may only
/// *annotate* (e.g. a custom purpose string for) a permission the app already
/// derives, never *introduce* one.
///
/// # Errors
/// [`CliError::UsageOwned`] carrying the `IPE-P0001` refusal when any override
/// permission has no backing accepted axis.
pub fn reconcile_override(
    accepts: &BTreeSet<Capability>,
    platform: Platform,
    override_permissions: &BTreeSet<String>,
) -> Result<(), CliError> {
    let entitled = entitled_permission_names(accepts, platform)?;
    let unbacked: Vec<&String> = override_permissions
        .iter()
        .filter(|name| !entitled.contains(*name))
        .collect();
    if unbacked.is_empty() {
        return Ok(());
    }
    Err(unbacked_permission_refusal(platform, &unbacked))
}

/// The typed, fail-closed refusal naming each override permission with no backing
/// accepted axis, and the remedy.
fn unbacked_permission_refusal(platform: Platform, unbacked: &[&String]) -> CliError {
    let mut body = format!(
        "the packaged {} manifest declares OS permission(s) the app has not accepted\n",
        platform.as_str()
    );
    for name in unbacked {
        body.push_str("  = ");
        body.push_str(name);
        body.push('\n');
    }
    body.push_str(
        "  = an OS permission is DERIVED from the app's `accepts` set, never hand-added; a \n\
         \x20   permission with no backing accepted web capability cannot ship. Grant the backing \n\
         \x20   capability after review by adding the axis to `accepts = [ … ]` under \n\
         \x20   [capabilities] in package.ipe, or remove the permission from the override.\n",
    );
    CliError::UsageOwned(format!("error[IPE-P0001]: {body}"))
}

/// A structured, per-axis view of a derivation for the CLI surface — which web
/// axes were granted and, for each, the OS entries it contributed on the target.
///
/// Consumed by the `ipe pack --emit-permissions` dry-run to print a legible,
/// deterministic report without the CLI reaching into the private table.
#[must_use]
pub fn per_axis_breakdown(
    accepts: &BTreeSet<Capability>,
    platform: Platform,
) -> BTreeMap<&'static str, Vec<String>> {
    let mut by_axis: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for cap in accepts {
        let Capability::JsPort(axis) = cap else {
            continue;
        };
        let requirement = requirement_for(*axis);
        let mut entries = Vec::new();
        if platform.is_apple() {
            if let Some(e) = requirement.plist {
                entries.push(e.key);
            }
        } else {
            for e in requirement.android {
                entries.push(android_entry_name(&e));
            }
        }
        by_axis.insert(axis.as_str(), entries);
    }
    by_axis
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(items: &[Capability]) -> BTreeSet<Capability> {
        items.iter().copied().collect()
    }

    fn web(axis: WebCapability) -> Capability {
        Capability::JsPort(axis)
    }

    /// A one-element permission-override set, for the fail-closed reconciliation
    /// tests.
    fn one_override(name: &str) -> BTreeSet<String> {
        std::iter::once(name.to_owned()).collect()
    }

    // ── The table is exhaustive over the closed web vocabulary ────────────────

    #[test]
    fn every_web_axis_has_a_requirement_on_every_platform() {
        // Coverage guard: iterating `WebCapability::ALL` exercises every arm of
        // the exhaustive `requirement_for` match. A new axis added to the compiler
        // without an arm here fails to compile (no wildcard); this test then also
        // pins that the derivation runs for it on all three platforms.
        for &axis in WebCapability::ALL {
            for platform in [Platform::Ios, Platform::MacOs, Platform::Android] {
                let set = derive_permissions(&accepts(&[web(axis)]), platform)
                    .expect("derivation is total");
                assert_eq!(set.platform(), platform);
            }
        }
    }

    #[test]
    fn geolocation_derives_the_expected_entries_per_platform() {
        let a = accepts(&[web(WebCapability::Geolocation)]);

        let ios = derive_permissions(&a, Platform::Ios).expect("ios");
        assert_eq!(
            ios.to_info_plist_entries(),
            vec![(
                "NSLocationWhenInUseUsageDescription".to_owned(),
                "This app uses your location to provide location-based features.".to_owned(),
            )]
        );
        assert!(ios.to_android_manifest_entries().is_empty());

        let android = derive_permissions(&a, Platform::Android).expect("android");
        assert!(android.to_info_plist_entries().is_empty());
        let xml = android.to_android_manifest_entries().to_xml();
        assert!(
            xml.contains("android.permission.ACCESS_FINE_LOCATION"),
            "fine location: {xml}"
        );
        assert!(
            xml.contains("android.permission.ACCESS_COARSE_LOCATION"),
            "coarse location: {xml}"
        );
    }

    #[test]
    fn apple_purpose_strings_are_non_empty() {
        // The Apple-review invariant: every derived plist key carries a non-empty
        // purpose. Exercised over every axis on both Apple platforms.
        for &axis in WebCapability::ALL {
            for platform in [Platform::Ios, Platform::MacOs] {
                let set = derive_permissions(&accepts(&[web(axis)]), platform).expect("derive");
                for (key, purpose) in set.to_info_plist_entries() {
                    assert!(
                        !purpose.is_empty(),
                        "{key} on {platform:?} has empty purpose"
                    );
                }
            }
        }
    }

    #[test]
    fn a_pure_or_non_web_app_derives_no_permissions() {
        // deny-by-default: no accepted web axis ⇒ no OS permission. Non-web
        // capabilities (network, native-ffi) never contribute.
        let a = accepts(&[Capability::Network, Capability::NativeFfi]);
        for platform in [Platform::Ios, Platform::MacOs, Platform::Android] {
            let set = derive_permissions(&a, platform).expect("derive");
            assert!(
                set.is_empty(),
                "non-web app derived a permission on {platform:?}"
            );
        }
        let empty = accepts(&[]);
        for platform in [Platform::Ios, Platform::MacOs, Platform::Android] {
            assert!(
                derive_permissions(&empty, platform)
                    .expect("derive")
                    .is_empty()
            );
        }
    }

    #[test]
    fn a_multi_axis_app_derives_exactly_those_permissions() {
        // The geo-clipboard shape: geolocation + clipboard + raw. Clipboard and
        // raw contribute nothing; geolocation contributes location.
        let a = accepts(&[
            web(WebCapability::Geolocation),
            web(WebCapability::Clipboard),
            web(WebCapability::Raw),
        ]);
        let ios = derive_permissions(&a, Platform::Ios).expect("ios");
        assert_eq!(
            ios.to_info_plist_entries(),
            vec![(
                "NSLocationWhenInUseUsageDescription".to_owned(),
                "This app uses your location to provide location-based features.".to_owned(),
            )]
        );
        let android = derive_permissions(&a, Platform::Android).expect("android");
        let names: BTreeSet<String> = android
            .to_android_manifest_entries()
            .lines()
            .iter()
            .cloned()
            .collect();
        // Only the two location permissions; clipboard/raw add nothing.
        assert_eq!(names.len(), 2, "unexpected android entries: {names:?}");
    }

    #[test]
    fn derivation_is_deterministic() {
        // Two derivations of the same accept-set render byte-identically.
        let a = accepts(&[
            web(WebCapability::Geolocation),
            web(WebCapability::Notification),
            web(WebCapability::Vibration),
        ]);
        let first = derive_permissions(&a, Platform::Android).expect("first");
        let second = derive_permissions(&a, Platform::Android).expect("second");
        assert_eq!(
            first.to_android_manifest_entries(),
            second.to_android_manifest_entries()
        );
    }

    // ── Fail-closed: accepted ⇒ merged (completeness) ─────────────────────────

    #[test]
    fn accepted_axis_appears_in_the_derived_manifest() {
        // Completeness: a granted axis with an OS requirement MUST surface in the
        // entitled set the reconciler accepts.
        let a = accepts(&[web(WebCapability::Geolocation)]);
        let entitled_android = entitled_permission_names(&a, Platform::Android).expect("android");
        assert!(entitled_android.contains("android.permission.ACCESS_FINE_LOCATION"));
        let entitled_ios = entitled_permission_names(&a, Platform::Ios).expect("ios");
        assert!(entitled_ios.contains("NSLocationWhenInUseUsageDescription"));
    }

    #[test]
    fn a_backed_override_is_accepted() {
        // An override that only names permissions the app derives is accepted —
        // the author may annotate (e.g. a custom purpose) a derived permission.
        let a = accepts(&[web(WebCapability::Geolocation)]);
        let over = one_override("NSLocationWhenInUseUsageDescription");
        reconcile_override(&a, Platform::Ios, &over).expect("a backed override is accepted");
    }

    // ── Fail-closed: merged ⇒ accepted (the pinned security refusal) ──────────

    #[test]
    fn an_unbacked_camera_override_is_refused_naming_it() {
        // THE security property. An author hand-adds NSCameraUsageDescription to
        // the packaged Apple manifest, but the app never accepted a camera axis
        // (indeed no `camera` axis exists in the vocabulary). This MUST be a hard,
        // typed refusal naming the smuggled permission — a package can never
        // declare an OS permission with no backing consent.
        let a = accepts(&[web(WebCapability::Geolocation)]);
        let over = one_override("NSCameraUsageDescription");
        let err = reconcile_override(&a, Platform::Ios, &over)
            .expect_err("an unbacked camera permission must be refused");
        let msg = err.to_string();
        assert!(msg.contains("IPE-P0001"), "carries the code: {msg}");
        assert!(
            msg.contains("NSCameraUsageDescription"),
            "names the smuggled permission: {msg}"
        );
    }

    #[test]
    fn an_unbacked_android_camera_permission_is_refused() {
        // The Android face of the same property: a hand-added CAMERA permission
        // with no accepted backing axis is refused naming it.
        let a = accepts(&[web(WebCapability::Geolocation)]);
        let over = one_override("android.permission.CAMERA");
        let err = reconcile_override(&a, Platform::Android, &over)
            .expect_err("an unbacked android camera permission must be refused");
        assert!(err.to_string().contains("android.permission.CAMERA"));
    }

    #[test]
    fn a_clipboard_grant_does_not_back_a_location_permission() {
        // A granted axis does not entitle a *different* axis's permission: the
        // grant is per-axis, so a clipboard grant cannot back a location key.
        let a = accepts(&[web(WebCapability::Clipboard)]);
        let over = one_override("NSLocationWhenInUseUsageDescription");
        let err = reconcile_override(&a, Platform::Ios, &over)
            .expect_err("a clipboard grant does not back a location permission");
        assert!(
            err.to_string()
                .contains("NSLocationWhenInUseUsageDescription")
        );
    }

    // ── Platform parsing ──────────────────────────────────────────────────────

    #[test]
    fn platform_round_trips_its_wire_name() {
        for platform in [Platform::Ios, Platform::MacOs, Platform::Android] {
            assert_eq!(platform.as_str().parse::<Platform>(), Ok(platform));
        }
    }

    #[test]
    fn an_unknown_platform_is_rejected() {
        assert_eq!(
            "windows".parse::<Platform>(),
            Err(UnknownPlatform("windows".to_owned()))
        );
    }

    // ── End-to-end on the real geo-clipboard fixture ──────────────────────────

    #[test]
    fn geo_clipboard_fixture_derives_the_expected_permissions_end_to_end() {
        // The derivation runs from a real project's `package.ipe` through the
        // actual manifest reader — proving the accept-set the reader produces is
        // exactly the input `derive_permissions` consumes. `geo-clipboard`
        // accepts `JsPort Geolocation`, `JsPort Clipboard`, `JsPort Raw`; only
        // geolocation carries an OS permission.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/shapes/web/geo-clipboard/package.ipe");
        let manifest = crate::project::parse_manifest(&fixture)
            .expect("the geo-clipboard fixture manifest must parse");
        let accepts = &manifest.capabilities_accept;

        let ios = derive_permissions(accepts, Platform::Ios).expect("ios");
        assert_eq!(
            ios.to_info_plist_entries(),
            vec![(
                "NSLocationWhenInUseUsageDescription".to_owned(),
                "This app uses your location to provide location-based features.".to_owned(),
            )],
            "geo-clipboard on iOS declares exactly the location usage key"
        );

        let android = derive_permissions(accepts, Platform::Android).expect("android");
        let names: BTreeSet<String> = android
            .to_android_manifest_entries()
            .lines()
            .iter()
            .cloned()
            .collect();
        assert!(
            names
                .iter()
                .any(|l| l.contains("android.permission.ACCESS_FINE_LOCATION")),
            "geo-clipboard on Android declares fine location: {names:?}"
        );
        assert_eq!(
            names.len(),
            2,
            "only the two location permissions; clipboard/raw add nothing: {names:?}"
        );
    }
}
