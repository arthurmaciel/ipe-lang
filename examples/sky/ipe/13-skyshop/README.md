# 13-skyshop — the FFI acceptance example

The full skyshop storefront transposed from the upstream Sky Rust-backend
variant, with the three shim crates replaced by the REAL SDK crates through
the shim-free auto-FFI:

| Was (shim crate)         | Now (real crate, auto-bound)            | State |
|---|---|---|
| `sky-firestore-shim`     | `firestore 0.49`                        | de-shimmed — `src/Lib/Db.ipe` |
| `sky-firebase-auth-shim` | `rs-firebase-admin-sdk 4.3`             | de-shimmed — `src/Lib/Auth.ipe` |
| `sky-stripe-shim`        | `async-stripe 1.0.0-rc.6` (6 crates)    | de-shimmed — `src/Lib/Stripe.ipe` |

SHIM-FREE SEAL: `ipe build` exit 0 → the emitted crate `cargo build`s exit 0.
The checkout flow rides the real builder surface — conversion-bound params
(`impl Into<Currency>`), Vec-of-opaque `line_items`, the cross-crate
`CheckoutSessionMode` enum, the typed-ID `RetrieveCheckoutSession::new`
(surfaced as a plain `String` via its `From<String>` proof), and the
`status`/`payment_status` enum-typed field accessors. Sentinel DCE keeps only
the reached wrappers in the emitted `src/ffi.rs` (51 of the ~32.5k catalog
bindings).

`.ipe/cache/ffi/rust/` is regenerable with
`ipe install --yes --allow-build-scripts`; `<slug>.pkg.json` is the sole
catalog source the build re-derives every consumer view from. `async-stripe`
pins `features = ["default-tls"]` in `sky.toml` so the inspection binds the
default tokio-hyper client surface (all-features surfaces several client
concretes and drops the async `send`s as ambiguous).
