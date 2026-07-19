# 13-skyshop — the FFI acceptance example (IN PROGRESS)

The full skyshop storefront transposed from the upstream Sky Rust-backend
variant, with the three shim crates replaced by the REAL SDK crates through
the shim-free auto-FFI:

| Was (shim crate)         | Now (real crate, auto-bound)            | State |
|---|---|---|
| `sky-firestore-shim`     | `firestore 0.49` (841 bindings)         | de-shimmed — `src/Lib/Db.ipe` |
| `sky-firebase-auth-shim` | `rs-firebase-admin-sdk 4.3` (304)       | de-shimmed — `src/Lib/Auth.ipe` |
| `sky-stripe-shim`        | `async-stripe 1.0.0-rc.6` (6 crates)    | BLOCKED — `src/Lib/Stripe.ipe` still names the shim |

`.ipe/cache/ffi/rust/` holds the verified install artifacts for all 8 crates
(one `ipe install --yes --allow-build-scripts` manifest run), so the build is
network-free.

## Why the tree does not build yet

The stripe checkout-session flow needs binding classes that the inspector
does not admit yet (the `stripe-builder-surface` wall in
`docs/architecture/async-ffi-bridge-impl-plan.md`):
`CreateCheckoutSession::line_items` (Vec-of-opaque param), `::mode`
(cross-crate enum param), `RetrieveCheckoutSession::new` (typed-ID param),
the `LineItemsPriceData` constructor (cross-crate `Currency` enum param), and
the `CheckoutSession.status` / `payment_status` accessors (enum-typed
fields). Everything else — the firestore document/query surface and the
firebase ID-token verification — binds and SEALs (probe projects verified
end-to-end, including live error-fold runs).
