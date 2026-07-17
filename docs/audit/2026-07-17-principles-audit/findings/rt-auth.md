# rt-auth findings

5 findings: 0 critical, 0 high, 2 medium, 3 low.

Audited: `src/runtime/rust/src/auth.rs`, `src/runtime/rust/src/jwt.rs`,
`src/runtime/rust/src/crypto.rs`, `src/runtime/rust/src/secret.rs`
(+ vendored `jsonwebtoken-9.3.1/src/validation.rs` to ground the exp-parsing
claims, and `src/compiler/backend/rust/src/project.rs` for the emitted-profile
`overflow-checks = false` fact).

Prior-audit status checked: `runtime-audit-verdict.md`'s auth-email findings
(user-0 auth via `try_get(0).unwrap_or(0)`; `signToken` negative-expiry
underflow) are FIXED in the current `auth.rs` with regression tests
(`auth.rs:354-361` + `test_login_id_decode_failure_yields_err_not_user_zero`;
`auth.rs:137-143` + `test_sign_token_negative_expiry_rejected`). The AUD-02
leeway-60 window is closed (`auth.rs:201-202`). Not re-filed.

## RT-AUTH-001 · Negative `exp` silently skips expiry validation → non-expiring token (flat Jwt decoders)
- severity: medium
- axis: correctness
- principle: P2 correctness (unsanctioned Go divergence) + "parse, don't validate" (exp enters validation as a partial `u64` parse whose failure branch is accept)
- location: `src/runtime/rust/src/jwt.rs:196` (`jwt_decode_hs256`), `src/runtime/rust/src/jwt.rs:283` (`jwt_decode_rs256`); root cause in `jsonwebtoken-9.3.1/src/validation.rs:334-369` (`numeric_type` has `visit_u64`/`visit_f64` but no `visit_i64` → a negative integer `exp` becomes `TryParse::FailedToParse`) + `validation.rs:274` (`matches!(… TryParse::Parsed …)` is false for `FailedToParse` → the expiry check is skipped entirely)
- reachability: a token signed under the app's own key with `"exp": -1` (or any negative integer). The mint side can produce this from well-typed Ipê: the flat `Jwt.encodeHs256 secret claimsJson` accepts arbitrary claims JSON, and the builder `ipe_jwt_expires_at : i64 -> …` (`jwt.rs:406`) has no negative guard — an app computing `expiresAt (now - ttl)` by sign error mints it. The flat decoders clear `required_spec_claims` (`jwt.rs:186/273`), so nothing else rejects it. (`auth_verify_token` is accidentally fail-closed here: it keeps the default `required_spec_claims = {"exp"}`, so `FailedToParse` trips `MissingRequiredClaim`.)
- problem: Go's oracle evaluates `now >= exp` over the signed integer and rejects any negative `exp` as long-expired. The Rust flat decoders silently skip the check and ACCEPT the token as non-expiring — an intended-instantly-expired token becomes a forever-valid credential (fail-open inversion of a signer-side bug). The prior audit recorded this as an optional Go-parity guard on the acceptable-as-is crypto-jwt group; it is still present and its blast radius grew with the builder API's unguarded `expiresAt`.
- fix direction: pre-parse `exp` from the payload as `i64`/`f64` (extending `exp_is_zero` into an `exp_already_expired(now)` guard) and reject `exp <= now` before handing the token to jsonwebtoken; optionally also reject negative TTL at `ipe_jwt_expires_at` mint time (mirror `auth_sign_token`'s guard).
- prior: `runtime-audit-verdict.md` crypto-jwt residual LOW ("negative/fractional `exp` is accepted by Rust where Go rejects") — still present; re-filed per this partition's focus.

## RT-AUTH-002 · Fractional `exp` in [0, 0.5) rounds to 0, bypasses the `exp_is_zero` guard → u64 underflow → always-expired token accepted as non-expiring
- severity: medium
- axis: correctness
- principle: P2 correctness + P3 soundness (comment-asserted invariant "the exp == 0 underflow … is guarded above" is not enforced for float-encoded exp)
- location: `src/runtime/rust/src/jwt.rs:85-100` (`exp_is_zero` matches only via `JsonValue::as_u64`, which returns `None` for JSON floats), used at `jwt.rs:161`, `jwt.rs:243`, and `src/runtime/rust/src/auth.rs:189`; underflow site `jsonwebtoken-9.3.1/src/validation.rs:275` (`exp - reject_tokens_expiring_in_less_than` in plain u64 with `reject_tokens_expiring_in_less_than = 1`); float rounding at `validation.rs:347-356` (`visit_f64` → `Parsed(value.round() as u64)`, so `0.0`–`0.49…` → `Parsed(0)`)
- reachability: a token signed under the app's key with `"exp": 0.4` (or `0.0`) — RFC 7519 NumericDate explicitly permits non-integer values, and the flat `Jwt.encodeHs256` accepts arbitrary claims JSON from well-typed Ipê (`payload_json` round-trips the float). Reaches `jwt_decode_hs256`, `jwt_decode_rs256`, AND `auth_verify_token` (the float parses as `Parsed(0)`, so it also passes `auth_verify_token`'s required-exp presence check).
- problem: `exp_is_zero` was built precisely to keep `exp - 1` from underflowing, but it only recognises the integer spelling of zero. A float `exp` that rounds to 0 slips past it into jsonwebtoken's non-saturating subtraction: `0u64 - 1` wraps to `u64::MAX` under `overflow-checks = false` (which the emitted-project profile pins — `src/compiler/backend/rust/src/project.rs:225`), making the reject condition `u64::MAX < now` false — an unconditionally-expired token is ACCEPTED as non-expiring. Under any build with overflow-checks on (the runtime's own test profile), the same input is a reachable panic inside a Result-returning kernel.
- fix direction: make the pre-guard total over the claim's numeric domain — parse `exp` as `f64`/`i64` and short-circuit-reject anything `< 1` (or normalise the payload's `exp` before validation), instead of pattern-matching the single integer-zero spelling.
- prior: sibling of the crypto-jwt residual LOW (same "negative/fractional exp" clause), but the guard-bypass-into-underflow mechanism is new — the prior audit judged the `exp_is_zero` pre-guard "necessary and correct"; it is necessary but incomplete.

## RT-AUTH-003 · Flat vs builder decode paths disagree on fractional `exp`/`nbf` (same token, opposite outcomes)
- severity: low
- axis: correctness
- principle: P2 correctness — one runtime, two documented decode surfaces, divergent time-claim semantics
- location: `src/runtime/rust/src/jwt.rs:551-562` (`ipe_jwt_decode` manual checks use `as_i64`, `None` on any JSON float → check silently skipped) vs `jwt.rs:196/283` (flat decoders → jsonwebtoken `visit_f64` rounds and validates)
- reachability: any token carrying a fractional NumericDate (RFC 7519-legal, e.g. `"exp": 1752771600.5` minted by an external system sharing the secret, or by the flat encode kernel): `Jwt.decodeHs256` validates it (rounds), the builder `Jwt.decode` treats it as absent and accepts regardless of `now`.
- problem: an app migrating between the two documented API surfaces (the divergence-policy doc names both) silently changes whether fractional exp/nbf are enforced — a past fractional `exp` is rejected on one path and accepted as non-expiring on the other.
- fix direction: in `ipe_jwt_decode`, read exp/nbf via `as_f64().or(as_i64 as f64)` and compare against `now` with the reference's `now >= exp` / `now < nbf` semantics, so both paths share one NumericDate parse.
- prior: new.

## RT-AUTH-004 · Unknown-algorithm-descriptor error byte-slices at index 20 (char-boundary panic site) and echoes a key-bearing string's prefix
- severity: low
- axis: soundness
- principle: P3 soundness (no reachable panic; clippy `indexing_slicing` deny-set intent) — filed as a smell: not reachable from well-typed Ipê
- location: `src/runtime/rust/src/jwt.rs:466` and `jwt.rs:527` (`&algorithm_descriptor[..algorithm_descriptor.len().min(20)]`)
- reachability: requires an `Algorithm` value that is neither `HS256:`- nor `RS256:`-prefixed. `Algorithm` is opaque at the Ipê level and only constructible via `ipe_jwt_hs256`/`ipe_jwt_rs256`, so well-typed programs cannot reach the branch today — the panic/leak arms are one codegen drift away, not live.
- problem: (a) `&s[..20]` on a `String` panics when byte 20 falls inside a multi-byte UTF-8 char — a raw range-index in a Result-returning kernel, contrary to the crate's `.get()`-everywhere discipline; (b) the echoed prefix is 20 bytes of a string whose format-by-design carries key material after a 6-byte tag — a mis-cased or drifted descriptor (`"hs256:<secret>"`) would put 14 secret bytes into the Ipê-visible error.
- fix direction: replace the slice with a fixed message (or `s.chars().take(20)`) and never echo descriptor content — the tag alone identifies the failure.
- prior: new.

## RT-AUTH-005 · HS256 secret / RSA key embedded in a plain `String` algorithm descriptor (`"HS256:<secret>"`)
- severity: low
- axis: security
- principle: P1 no secret leakage + "make invalid states unrepresentable" (a secret-bearing value with every `String` capability is the anti-pattern `secret.rs` exists to close)
- location: `src/runtime/rust/src/jwt.rs:358-366` (`ipe_jwt_hs256`/`ipe_jwt_rs256`), consumed at `jwt.rs:458-461`, `jwt.rs:495-511`
- reachability: every builder-API program holds its signing secret inside an ordinary runtime `String` for the lifetime of the `Algorithm` value; any present-or-future stringification of that value (`IpeStringify` is implemented for `String`, so a generic show/log/interpolation path that receives the descriptor prints the secret verbatim) is a leak. No concrete live sink shown — hence low, a smell.
- problem: the same runtime ships `secret.rs`, a sealed newtype whose whole design is that secret-bearing values have NO Display/stringify/serde surface; the JWT builder API goes the opposite way and widens the secret into the most capable type in the system, keyed only by a string prefix. The RT-AUTH-004 error path already demonstrates how close descriptor bytes sit to an error message.
- fix direction: make the runtime `Algorithm` a real struct (enum `{ Hs256(Secret), Rs256(String) }` or at minimum a sealed newtype over the descriptor) with redacted `Debug`/no stringify, mirroring `secret.rs`.
- prior: new (the flat-kernel stringly-claims shape was noted as parse-don't-validate-compliant in the prior audit; the secret-in-descriptor builder encoding postdates it).

## Clean notes (no finding)
- `crypto.rs`: entropy from `OsRng` (getrandom CSPRNG) with 1..=1024 size bounds; AEAD nonces are fresh 96-bit `OsRng` values per encryption (no reuse; random-nonce collision bound applies, matching the Go format); `constantTimeEqual` uses `subtle` with the documented length short-circuit; sha1/md5 weak-hash exposure and PBKDF2 100k iterations are documented, parity-locked prior-audit acceptances — not re-filed.
- `secret.rs`: sealed constructor, constant-time `PartialEq`, redacted `Debug`/`IpeStringify`, absent `Display`/`Hash`/`Ord`/serde, zeroize-on-drop — the design holds as documented.
- `auth.rs`: prior critical-path findings (user-0 authentication, negative signToken TTL, 60s leeway) verified FIXED with regression tests; bcrypt cost clamp [4,15] with cost-12 default and equal-cost dummy verify on the unknown-email path are sound; algorithm pinned HS256 (no alg-confusion); email canonicalisation applied symmetrically.
- Algorithm confusion: all decoders pin the expected algorithm via `Validation::new(...)` (jsonwebtoken rejects header/expected mismatch), and the builder dispatches on the caller's descriptor, never the token header — `alg=none`/HS-vs-RS confusion is closed.
