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

## Sanction mechanism

`tools/panic-scan` suppresses a hit when an
`// IPE-RUST-AUDIT:ACCEPTED (author) — reason [ledger #N]` comment appears in the
annotation block directly attached to the construct (the hit line, or any line
above it up to the nearest blank line — the audit comment, `#[allow(…)]`, and any
statement continuation). The lexer drops comments, so this check runs against the
raw source lines. It is per-site and explicit: an unannotated new `panic!`,
`.expect()`, `assert!`, `debug_assert!`, or `process::exit` still fails the scan,
so the gate is never weakened by the marker.

Two site families carry the marker:

- **Boundary constructs** — `process::exit` at a binary `main`, the `System.exit`
  kernel, the `ipe db migrate` CLI-op, the `Ipe.Web` shutdown-signal handlers, the
  jail exec process-control, the emitted-program TEMPLATE (`backend/rust/templates/
  main.rs`, which is the *user's generated* binary, not compiler code), the
  compile-time `const _: () = assert!(…)` Win32-flag lockstep, and the
  golden-tested integer division-by-zero abort. These are the sanctioned boundary
  set of ADR-0037.
- **Ledger #1/#2** — the structurally-dead HMAC `.expect` sites below.

## Remaining exceptions

Each remaining site carries, at the call site, an
`// IPE-RUST-AUDIT:ACCEPTED (author, date) — reason [ledger #N]` comment and a
scoped `#[allow(clippy::…)]`. Only classes #1/#2 remain; a review verdict for
each is recorded in `docs/adr/0037-no-authored-abrupt-failure.md`.

| # | Sites | Construct | Justification |
|---|-------|-----------|---------------|
| 1 | `runtime/rust/src/crypto.rs` (×2) | `.expect` | `Hmac::new_from_slice` returns `Result`, but `Hmac<D>`'s impl pads/hashes any key internally and returns `Ok` unconditionally, so the `InvalidLength` branch is structurally dead. See "Why #1/#2 stay" below. |
| 2 | `runtime/rust/src/email.rs` | `.expect` | The same structurally-dead HMAC ctor, in the SES SigV4 request signer's key-derivation chain. |

The ledger is reduced to **two permanently-retained, security-reviewed HMAC
assertions** — everything that could be removed without weakening security has
been. Retaining #1/#2 is a deliberate project decision on Security-first grounds
(Security > Correctness > Soundness: a weaker design is never shipped to chase a
zero count), not a tolerated leftover. The two remaining classes are documented
below and were driven from four down to two.

### Eliminated

- **#3 — `ffi_polyfills` `panic!` (×2): removed.** The dynamic-dispatch
  `Ffi.callPure` / `Ffi.callTask` shape has no denotation on `target=rust`: no
  stdlib or example `.ipe` names it (the FFI surface exposed to Ipê is
  `Ffi.kernel "Name"` direct dispatch), and the Rust backend's IR `Callee` enum
  has no dynamic/reflective variant, so the codegen emits no call to these
  polyfills. The two guards returned an unconstrained generic `T` and could
  signal the refusal only by panicking; being unreachable dead code, they were
  deleted outright. The impossibility is now structural (no runtime symbol to
  reach), not asserted behind an `#[allow(clippy::panic)]`.

- **#4 — `dyn Any` downcast registries: reclassified to zero.** These sites carry
  **no authored-abrupt-failure construct**: every downcast in
  `cache.rs` and `live/pubsub.rs` already resolves through a total fallback (a
  `None => miss / no-op / rebuild-fresh` arm), and lock poisoning is handled with
  `unwrap_or_else(|e| e.into_inner())` — no `.expect`, no `panic!`, no
  `#[allow]` for an abrupt-failure lint. What remained was only an
  `IPE-RUST-AUDIT:ACCEPTED` comment documenting a `dyn Any` type-erasure seam,
  which is a soundness note, not a failure construct. A fully-typed registry is
  legitimately blocked — `pubsub`'s per-`T` broker would need an external crate
  (`generic_singleton`/`once_map`, a supply-chain surface on crypto-adjacent
  runtime), and `cache`'s `Cache k v` lowers to a non-generic `i64` handle that
  carries no type args, so a typed registry is structurally infeasible without
  changing the handle representation. Attempting either would add risk for zero
  abrupt-failure reduction. The class is closed.

### Why #1/#2 stay (justified remains)

The `hmac 0.12` crate's only variable-length-key constructor is
`Mac::new_from_slice(key) -> Result<_, InvalidLength>`; for `Hmac<D>` the impl
runs the key through `get_der_key` (padding/hashing over-long keys) and returns
`Ok` unconditionally, so `InvalidLength` is never produced. The crate's own
infallible `KeyInit::new` is defined as `new_from_slice(...).unwrap()` — our
`.expect` mirrors that. No infallible-by-type constructor exists that does not
require caller-side key preparation, and **hand-reimplementing HMAC key prep is a
security regression risk**, so that path is refused.

Threading a `Result Error String` channel through the MAC kernels (the other
candidate) is *worse*, not better: `email.rs::hmac_bytes` is called five times in
the SES SigV4 key-derivation chain, each output keying the next call. A dead
`Err` channel there invites a caller to `.unwrap_or(vec![])` a "can't-happen"
error and substitute an **empty/wrong MAC** that flows silently into a
plausible-but-invalid AWS signature — a silent-wrong-crypto defect. The same
applies to the pure `hmacSha256`/`hmacSha512` kernels: making `String -> String`
into `Result`-returning forces every caller to handle a never-occurring `Err`
whose mishandling is a wrong hash. A **loud** `.expect` on a provably-dead branch
is safer and more honest than a `Result` carrying a provably-dead `Err`.

These two are **retained by decision** as permanent, security-justified
exceptions — not kept because they were previously tolerated, but because a loud
assertion on a provably-dead branch is the most secure option available. Any new
abrupt-failure site is still forbidden; these two are the sole, reviewed
exceptions, and the enforcement gates (clippy + `tools/panic-scan`) hold for
everything else.
