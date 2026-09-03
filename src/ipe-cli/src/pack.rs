//! The native-shell packager: turning a built Ipê app into an OS application
//! bundle.
//!
//! [`permissions`] derives a native shell's OS-permission declarations (iOS/macOS
//! `Info.plist` usage-description keys, Android `<uses-permission>`) from the
//! app's granted web capabilities, fail-closed in both directions. It is the
//! security boundary of the whole packager: the derivation is the single source
//! of truth for what a packaged app may do, so a package can neither
//! under-declare relative to consent nor smuggle an OS permission the app never
//! accepted.
//!
//! [`desktop`] turns a built `Ipe.WebView` app into a distributable per-OS
//! desktop bundle (a macOS `.app`, a Linux tarball, a Windows `.exe` + zip),
//! assembling the macOS `Info.plist` around [`permissions`]'s derivation — never
//! authoring a permission itself.
//!
//! [`mobile`] wraps the client-wasm `Web` SPA (the `--target wasm` bundle) in a
//! thin iOS/Android system-webview shell that loads the bundle offline from app
//! assets, assembling the `Info.plist` / `AndroidManifest.xml` around
//! [`permissions`]'s derivation — never authoring a permission itself.

pub mod desktop;
pub mod mobile;
pub mod permissions;
