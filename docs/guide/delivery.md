# Delivering an app

You have a working Ipê program; now you want to hand it to someone. This page
takes a `Web` app from source to a desktop bundle and to a mobile shell. Every
command here has been run as written.

## The mental model: shape, then delivery

Two independent questions decide how a program ships:

- **Shape** — what `view` renders. The head of `main` fixes it: `Web.app` is a
  DOM app, `Tui.app` terminal cells, `Cli.app` terminal lines, `Server.listen`
  an HTTP server, and a bare `main : Task Error ()` renders nothing. The shape is
  never written in `package.ipe`; it is read from `main`.
- **Delivery** — for a `Web` app only, *how* the DOM app runs and *where* it is
  hosted. This is two sub-axes:
  - **runtime** — `live` (a co-located server loop; the unnamed default) or
    `spa` (a sandboxed client compiled to WebAssembly).
  - **host** — where a resolved shape × runtime runs: served, `desktop`, `ios`,
    or `android`.

The knot to spot: **shape is not host.** A "desktop app" and a "mobile app" are
both the *`Web` shape* delivered to a different host — the same `main`, the same
`update`/`view` loop, packaged differently. You do not write a separate program
for each; you name the host in one uniform grammar: `ipe <verb> <shape> <host>`.
`build web <host>` lays out a fast development bundle; `release web <host>` the
production distributable.

Only the `Web` shape has these axes. A `tui`, `cli`, `server`, or `script` app
builds one way, so it has no runtime or host to choose.

The full rationale — why webview is a host and not a shape, why `live` is never
spelled out — is [ADR 0069](../adr/0069-runtime-host-delivery-model.md).

## The `delivery` record

Per-host settings (a window title, a mobile bundle id) live in the `delivery`
record of `package.ipe`. Every field is optional; omit the record for the
defaults.

```ipe
package : Package
package =
    { name = "ui-layout"
    , version = "0.1.0"
    , delivery =
        { desktop = { title = "UI Layout", width = 1024, height = 768 }
        , mobile = { bundleId = "com.example.ui-layout", orientation = Portrait }
        , browser = { basePath = "/" }
        }
    }
```

Run `ipe doc Ipe.Package` for every field.

## Desktop: a webview-native bundle

A desktop app is the `Web` shape run `live` inside a native window (a system
webview over a local bridge, not a browser tab). It needs no extra manifest —
the `main` head `Web.app` is enough.

Package it:

```
ipe release web desktop
```

This compiles the app and lays out a bundle for the host OS:

```
packaged `ui-layout` for linux → dist/linux/ui-layout
  This app requires WebKitGTK at runtime (Debian/Ubuntu: libwebkit2gtk-4.1-0).
```

The Linux bundle is a self-contained tree:

```
dist/linux/ui-layout/
  bin/                 the compiled binary
  ui-layout.desktop    the desktop-entry launcher
  RUNTIME.txt          the runtime dependency note
```

`build web desktop` lays out the same bundle from a fast, unoptimised build for
the inner loop; `release web desktop` produces the optimised distributable. The
bundle is always the **host OS's** — the **Linux** artifact is built end to end on
a Linux host; a macOS `.app` or a Windows `.exe` + zip has its *layout and
manifest* written for inspection here, but the signed, runnable artifact must be
finished on that OS's own runner (cross-OS toolchains are out of scope).

## Mobile: a wasm SPA in a system-webview shell

A mobile app is the `Web` shape delivered as an `spa` — the DOM app compiled to
WebAssembly and hosted offline from app assets inside a native iOS/Android
webview. Because it is the `spa` runtime, the app must enable the wasm client in
`package.ipe`:

```ipe
package : Package
package =
    { name = "ui-layout"
    , version = "0.1.0"
    , wasm = On { mode = Spa }
    }
```

Then package for a device OS. Because mobile is the `spa` runtime, the host is
spelled `web spa <os>`:

```
ipe release web spa android
```

This builds the wasm bundle and materialises a native shell:

```
  wasm bundle ready at out/rust/www/
  bundle size: 196 KB (out/rust/www/pkg/ipe_app_bg.wasm)
packaged `ui-layout` for android → dist/android/ui-layout-android
  note: an Android shell project is written here; run `./gradlew assembleDebug`
        inside it with the Android SDK to produce an APK.
```

The Android shell is a ready-to-build Gradle project; the SPA rides under
`app/src/main/assets/www/` and a `WebViewAssetLoader` serves it same-origin, so
there is no remote host and no `file://` access. Finish the APK with
`./gradlew assembleDebug` where the Android SDK is present.

`ipe release web spa ios` writes the equivalent Xcode project (`WKWebView` +
`WKURLSchemeHandler`). Its layout and derived-permission manifest are written for
inspection, but a signed `.ipa` must be produced on a macOS runner with Xcode and
a signing identity. As with desktop, `build web spa <os>` lays out the same shell
around a fast dev SPA; `release web spa <os>` hosts the production SPA.

## OS permissions come from your capabilities

A packaged app may only touch an OS capability the app itself accepted. The
packager *derives* every iOS `Info.plist` key and Android `<uses-permission>`
line from the app's `[capabilities] accepts` set — it never hand-authors one, so
a bundle can neither under-declare nor smuggle a permission the app never took.

See exactly what a consent set yields, without building, with a read-only
dry-run:

```
ipe build --emit-permissions android
```

For an app that accepts `JsPort Geolocation`, that prints:

```
OS permissions for `geo-clipboard` on android
  js-port:geolocation → android.permission.ACCESS_FINE_LOCATION, android.permission.ACCESS_COARSE_LOCATION

AndroidManifest.xml fragment:
  <uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION" />
  <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />
```

Pass `ios` or `macos` for the `Info.plist` keys instead. The optional trailing
path selects a project other than the current directory.

## Where to go next

- [ADR 0069](../adr/0069-runtime-host-delivery-model.md) — the two-axis delivery
  model in full: the five shapes, the two web runtimes, and why each is where it
  is.
- `ipe doc Ipe.Package` — every `delivery`, `wasm`, and `capabilities` field.
- `ipe build --help` / `ipe release --help` — the one delivery grammar (`ipe
  <verb> [shape] [runtime] [host]`): `build` compiles or bundles a single
  delivery for the inner loop, `release` produces the production distributable.
