# Rust abrupt-failure ledger

Ipê's own compiler and runtime code is **free of authored abrupt-failure
constructs** — `panic!`, `unwrap`, `expect`, `assert!`, `unreachable!`, `todo!`,
indexing panics, `process::abort`, and friends. Every failure path is a typed
`Result` or a diagnostic. This is enforced two ways:

- **clippy**, on our own workspace at compile time — `unwrap_used`, `expect_used`,
  `panic`, `indexing_slicing`, `unreachable`, `todo`, `unimplemented` are `deny`
  (`Cargo.toml` `[workspace.lints]`), plus `disallowed-methods` for the paths
  clippy has no native lint for (`clippy.toml`).
- **`tools/panic-scan`**, a token-level scanner — it lexes rather than
  greps, so a construct named in a string or comment is invisible and one split
  across lines is still found (no false positives, no false negatives). It runs
  where clippy can't reach: the `assert!` family in production (clippy has no
  assert lint and exempts tests), and generated / third-party FFI Rust.

The guarantee is about **authored** code. `std` and third-party crates can still
panic (slice indexing, integer overflow, allocation failure); we minimise the
reachable surface (`indexing_slicing` denied, checked arithmetic) but do not
claim their internals never panic.

## Accepted exceptions

Each genuinely-unavoidable site carries, at the call site, an
`// IPE-RUST-AUDIT:ACCEPTED (author, date) — reason [ledger #N]` comment and a
scoped `#[allow(clippy::…)]`. The classes below are the whole ledger; a periodic
review verdict for each is recorded in `docs/adr/0037-no-authored-abrupt-failure.md`.

| # | Sites | Construct | Justification |
|---|-------|-----------|---------------|
| 1 | `runtime/rust/src/crypto.rs` (×2) | `.expect` | `Hmac::new_from_slice` returns `Result`, but HMAC accepts a key of *any* length, so the ctor is genuinely infallible. A fallback MAC would be a security defect. |
| 2 | `runtime/rust/src/email.rs` | `.expect` | The same infallible HMAC ctor, in the SES request signer. |
| 3 | `runtime/rust/src/ffi_polyfills.rs` (×2) | `panic!` | Unconstrained generic `T` return with no total value; statically dead for valid Ipê (the peephole resolves the call before emit on `target=rust`). |
| 4 | `runtime/rust/src/cache.rs` (×2), `runtime/rust/src/live/pubsub.rs` | `.expect`/downcast | `dyn Any` registries keyed by `TypeId`/handle where the stored type is invariant, so the downcast cannot fail. |

This ledger is **tracked debt, not an accepted state.** A documented exception is
not conformance — the target is **zero**, and every entry is reworked to remove
the construct, even a provably-infallible one (via a `Result` channel or a
type-level guarantee) rather than left asserted behind an `#[allow]`:

- **#3** (`ffi_polyfills` `panic!`) — eliminate: drop the function or give it a
  `!`-typed dead branch so the impossibility is proven by types, not asserted.
- **#4** (`dyn Any` downcasts) — eliminate: replace the `TypeId`-keyed registry
  with a statically-typed one; the downcast and the exception vanish together.
- **#1/#2** (infallible HMAC `.expect`) — eliminate: thread a `Result` channel
  through the MAC kernels so the (dead) error propagates as a diagnostic rather
  than being asserted away, or encode the key so the ctor is infallible by type.
  Genuinely infallible today, but "tolerated because infallible" is exactly the
  bar this step raises.

No entry is kept because it was previously tolerated or because it is "only"
documented. The ledger exists to be emptied.
