# T4 — JWT-exp + Money correctness

Findings: CO-INCR-001, CO-INCR-002, CO-INCR-003 (Money); RT-AUTH-001, RT-AUTH-002,
RT-AUTH-003 (JWT `exp`/`nbf` NumericDate).

Two independent root causes, one theme (both are money/credit correctness on a
value that enters from untrusted-or-request data through a boundary that fails to
parse it precisely):

- **Root cause A (Money) — a pure-Ipê port silently dropped the two invariants
  the runtime kernel it replaced enforces.** `../ipe/ipe-stdlib/Std/Money.ipe:341`
  routes `allocate` through `Ffi.callPure "Money_allocate"` — the guarded,
  sign-correct kernel (`src/runtime/rust/src/money.rs:271`, `parts <= 0 → []`,
  residue distributed *toward zero by sign*). The fork's `src/stdlib/Ipe/Money.ipe`
  reimplements `allocate` (lines 432-453) in pure Ipê and drops **both**: no
  `parts <= 0` guard (`totalMinor // 0` → `ipe_int_div` divide-by-zero abort,
  `math.rs:78-83`, exit 101), and it pairs trunc-toward-zero `//` with
  Euclidean-nonneg `modBy` (`basics.rs:16-23`) so negative totals mint cents.
  CO-INCR-003 is the same module's *documented* currency-check being a silent
  left-operand fallthrough (`else a`) rather than a typed error.

- **Root cause B (JWT) — the `exp`/`nbf` pre-guard parses only ONE spelling of
  the claim's numeric domain (`as_u64` integer-zero), not the whole RFC 7519
  NumericDate domain (negative int, fractional, integer, on both surfaces).**
  RFC 7519 §2 defines NumericDate as *any* JSON number (may be non-integer).
  `exp_is_zero` (`jwt.rs:85-100`) matches `value.get("exp").and_then(as_u64) ==
  Some(0)` — `as_u64` is `None` for a negative int and for any float, so both slip
  past the guard into jsonwebtoken's non-saturating `exp - 1` (`u64`) under the
  emitted profile's `overflow-checks = false` (`project.rs:225`), wrapping to
  `u64::MAX` (accepted non-expiring). The builder path (`ipe_jwt_decode:551`) reads
  `as_i64()`, `None` on a float → the check is *skipped* — the flat-vs-builder
  disagreement (RT-AUTH-003). `ipe_jwt_expires_at` (`jwt.rs:406`) has no negative
  guard, unlike `auth_sign_token` (`auth.rs:137-143`) which does.

The theme is fixed by re-establishing the dropped invariant in each port at the
one boundary where the value is first turned into a decision — not by scattering
new runtime checks at call sites.

---

## CO-INCR-001 + CO-INCR-002 — `Money.allocate` guard + sign correctness

### Root cause
`src/stdlib/Ipe/Money.ipe:432-453` is a pure-Ipê reimplementation of a guarded,
sign-correct runtime kernel. The header comment (lines 4-8) records the choice to
replace `Ffi.callPure` with pure Ipe "because `ex00-standard-libs` doesn't exercise
the kernel" — that is exactly the §0 shortcut: the port satisfied the example but
regressed the contract the module's own doc states (line 429: "The sum of returned
parts equals the input exactly").

Two structural options, pick per the kernel-availability finding below:

**The kernel IS reachable from the backend.** `scripts/ipe-index locate
Money.allocate` → `parity=ok route=ipe:…Kernel.hs:294 rust=ipe:money.rs:271`. The
Rust runtime and the reference backend both carry `Money_allocate`; only the fork's
`KernelFn` enum (`src/compiler/kernels/src/lib.rs`) lacks a `Money.allocate` arm
(grep shows only `Db.Decode.n` money-adjacent). So there are two principled
resolutions, ordered by preference:

### Design — preferred: restore the invariant in the pure `.ipe` body
Keep the port pure (no new kernel-registry surface, no FFI dependency in a
value-type stdlib module), but make it *match the kernel's arithmetic exactly*.
This makes the invalid state unrepresentable in the source that ships.

1. **`parts <= 0` guard** (closes CO-INCR-001):

   ```elm
   allocate : Int -> Money -> List Money
   allocate parts m =
       if parts <= 0 then
           []
       else
           let
               c = currency m
               dp = minorUnits c
               totalMinor = Dec.toMinor dp (amount m)
               base = totalMinor // parts
               remainder = totalMinor - base * parts   -- exact, sign-carrying
           in
           List.reverse (allocateHelp c parts base remainder [])
   ```

   `parts <= 0 → []` mirrors `money_allocate` (money.rs:272-274) and upstream's
   kernel. `[]` is the total, honest answer: "split into ≤0 parts yields nothing,"
   and the sum-equals-input invariant holds vacuously.

2. **Sign-correct residue** (closes CO-INCR-002): replace `extra = modBy parts
   totalMinor` (Euclidean, always ≥0) with `remainder = totalMinor - base * parts`
   (mirrors money.rs:301, exact and sign-carrying), and step each of the first
   `abs remainder` slots by `sign remainder` rather than always `+1`:

   ```elm
   allocateHelp : Currency -> Int -> Int -> Int -> List Money -> List Money
   allocateHelp c remaining base remainder acc =
       if remaining <= 0 then
           acc
       else
           let
               step = if remainder < 0 then -1 else 1
               piece = if remainder /= 0 then base + step else base
               newRem = if remainder > 0 then remainder - 1
                        else if remainder < 0 then remainder + 1
                        else 0
           in
           allocateHelp c (remaining - 1) base newRem (fromMinor c piece :: acc)
   ```

   For `totalMinor = -100, parts = 3`: `base = -33`, `remainder = -100 - (-99) =
   -1`, `step = -1` → first slot `-34`, rest `-33` → `[-34,-33,-33]` sum `-100`
   exactly (matches the kernel's `test_allocate_negative_total_shares_sum_to_input`,
   money.rs:379). For `+100/3`: `base = 33`, `remainder = 1`, `step = 1` →
   `[34,33,33]` sum `100`. Both directions satisfy the module's own invariant.

   Note the port used `modBy` for `extra`; `remainder = totalMinor - base * parts`
   is the correct pairing with truncating `//` (both truncate toward zero, so the
   residue is exact and sign-consistent) — this is the structural fix, not a
   band-aid on `modBy`.

### Design — alternative: register `Money.allocate` as a kernel
Add a `KernelFn::MoneyAllocate` arm (kernels/src/lib.rs, lower.rs, naming.rs,
constrain.rs type scheme `Int -> Money -> List Money` lowering to
`money_allocate(places, parts, amount)`), and rewrite `Money.ipe:allocate` as an
`Ffi.kernel`-style Ipe decl delegating to it — byte-identical to upstream's
`Ffi.callPure` routing. This is the *most* faithful to parity and reuses the
already-tested, overflow-checked kernel (`checked_mul`/`checked_div`, the 100k
parts cap, the `to_i64()` residue fix). **Rejected as the primary fix** because it
adds a new acceptance path (a new kernel scheme) that must fail-closed under THE
SEAL, widening blast radius for two lines of arithmetic the pure port can carry
correctly. Recommend the pure fix; hold the kernel route as the fallback if the
pure body cannot reach byte-parity with the kernel on the `money_parity` fixtures.

### Impl plan
1. Edit `src/stdlib/Ipe/Money.ipe:432-453` per the pure design above (guard +
   sign-correct residue). Run `ipe fmt` on the file.
2. **Regression test — allocate zero / negative parts** (the abort case). Home:
   `src/runtime/rust/tests/money_parity.rs` already has `test_money_allocate_zero_parts`
   for the kernel; add an **end-to-end `.ipe` example** under
   `examples/` or a stdlib behaviour test that calls `Money.allocate 0 (Money.fromMajor
   USD 100)` and `Money.allocate (List.length []) …` and asserts `[]` (compiles,
   runs, exit 0 — NOT exit 101). This is the missing coverage the audit flagged:
   the kernel test passed while the shipped `.ipe` path aborted.
3. **Regression test — negative-amount sum-equals-input**: a `.ipe` test asserting
   `Money.allocate 3 (Money.neg (Money.fromMajor USD 1))` sums (via `sumOf`) back to
   `-$1.00`, and each part rendered. Mirror `money_parity.rs:379`'s expected
   `[-34,-33,-33]`-cents shape at the Money layer.
4. Re-run the `money_parity` runtime suite unchanged (kernel untouched) + the new
   `.ipe` behaviour tests + the standard-libs example sweep.

---

## CO-INCR-003 — currency-mismatch silent left-operand

### Root cause
`Money.add`/`sub` (`Money.ipe:395-408`) return `a` on mismatch; `sumOf` inherits it
(silently drops non-matching entries); `compare`/`lt`/`lte`/`gt`/`gte` (474-506)
ignore currency entirely. This is byte-parity with `../ipe/…/Money.ipe:304-317` and
the comparison functions there — but it is an *unflagged* silent-wrong-money default
in the flagship "never raw Float for currency" module, exactly the swallowed-error
class the correctness axis names.

### Design
The principled fix is `Result Error Money` arithmetic (or a same-currency witness
type) so a mismatch is a typed `Err`, not a trusted wrong value. That is a
**breaking API change** to a parity-locked module and to every downstream caller,
and it diverges from upstream. Under the ordering (P2 correctness) it is warranted,
but it must be a deliberate, recorded divergence, not smuggled in with the allocate
fix. Two-step:

- **Now (this theme):** record the parity behaviour as a **sanctioned divergence
  candidate / documented limitation** — add the mismatch semantics to the module
  doc (a `-- WARNING:` on `add`/`sub`/`sumOf`/`compare` stating "same-currency
  only; cross-currency `add`/`sub` return the left operand, comparisons ignore
  currency — convert first") AND a `docs/divergences-from-sky.md` note that this
  matches upstream and is *knowingly* retained pending the typed-Result redesign.
  This closes the "no divergence record sanctions it, no doc warns the caller"
  half of the finding without a breaking change.
- **Backlog (separate, larger):** a typed `add`/`sub`/`sumOf : … -> Result Error
  Money` redesign (or a `SameCurrency` witness). Filed as its own unit; NOT part
  of the push-blocking allocate fix.

### Impl plan
1. Add doc warnings to `Money.ipe` `add`/`sub`/`sumOf`/`compare`/`lt`/… stating the
   same-currency precondition and the mismatch behaviour (WHAT, no archaeology).
2. Add a `docs/divergences-from-sky.md` entry: "Money cross-currency arithmetic
   returns left operand (parity with upstream); comparisons ignore currency.
   Retained pending typed-`Result` redesign; convert first." with its own note.
3. (Backlog unit, not here) the typed-Result redesign + migration.

---

## RT-AUTH-001 + RT-AUTH-002 + RT-AUTH-003 — align `exp`/`nbf` NumericDate to Go

### Root cause
The pre-guard and the two decode surfaces each parse a *different subset* of the
RFC 7519 NumericDate domain:
- `exp_is_zero` (`jwt.rs:99`): `as_u64 == Some(0)` — misses negatives and all
  floats.
- flat decoders hand the token to jsonwebtoken, whose `numeric_type` visitor has
  `visit_u64`/`visit_f64` but no `visit_i64` (validation.rs:334-369) → a negative
  int becomes `FailedToParse` → the `matches!(…Parsed…)` expiry check (validation.rs:274)
  is *skipped*; a float `0.4` rounds to `Parsed(0)` → `0u64 - 1` underflow.
- builder `ipe_jwt_decode` (`jwt.rs:551/557`): `as_i64()` — misses floats → check
  skipped.

### Design — one total NumericDate parse, applied before validation on every path
Replace the single-spelling `exp_is_zero` with a **total pre-normalisation** that
reads `exp`/`nbf` over the whole numeric domain and reduces the decision to Go's
`now >= exp` / `now < nbf`. Two coordinated changes:

1. **A total claim reader** in `jwt.rs`, replacing `exp_is_zero`:

   ```rust
   /// Reads a NumericDate claim (RFC 7519 §2: any JSON number, may be fractional)
   /// as an integer second count, truncating toward −∞ so a token is never treated
   /// as valid LONGER than its fractional exp states. Returns None when the claim
   /// is absent or not a number (→ claim treated as absent = accepted, per Go).
   fn numeric_date(value: &JsonValue, claim: &str) -> Option<i64> {
       match value.get(claim) {
           Some(JsonValue::Number(n)) => n
               .as_i64()
               .or_else(|| n.as_f64().map(|f| f.floor() as i64)),
           _ => None,
       }
   }
   ```

   `floor` for `exp` is the conservative direction (`0.4 → 0`, already-expired;
   `-0.1 → -1`). For `nbf` the reference accepts at `now == nbf`; flooring a
   fractional `nbf` is also conservative (accepts slightly earlier, matching the
   integer-second oracle). Keep it uniform: floor both, then compare with Go's
   integer `now`.

2. **A pre-validation reject on the flat decoders** (`jwt_decode_hs256:161`,
   `jwt_decode_rs256:243`) — replace the `exp_is_zero` call with:

   ```rust
   let now = now_unix_seconds();          // the wall-clock the flat path already uses
   if let Some(exp) = numeric_date(&payload, "exp") {
       if now >= exp {                    // Go's pastClaim, including negatives & floors
           return IpeResult::Err("jwt-decode: token has expired".into());
       }
   }
   if let Some(nbf) = numeric_date(&payload, "nbf") {
       if now < nbf {
           return IpeResult::Err("jwt-decode: token is not yet valid".into());
       }
   }
   ```

   This makes the flat path evaluate the SAME `now >= exp` / `now < nbf` the
   builder path already does (`ipe_jwt_decode:551-562`), so the negative and the
   float cases both reject before ever reaching jsonwebtoken's `exp - 1`
   subtraction — closing RT-AUTH-001, RT-AUTH-002 (the underflow can no longer be
   reached), and RT-AUTH-003 (both paths now share one NumericDate parse). Keep the
   downstream `validation.reject_tokens_expiring_in_less_than = 1` for the
   integer-boundary parity it already provides; the pre-reject only *adds* the
   domain the guard missed. **Note:** the flat decoders currently take no `now` — if
   they must stay deterministic-on-wall-clock like the builder, this pre-reject
   reads the system clock exactly where jsonwebtoken already would; that is the
   existing behaviour of the flat path (it uses jsonwebtoken's internal `now`), so
   no new nondeterminism is introduced. Confirm at impl time whether to read the
   clock once and thread it into both the pre-guard and (via `Validation`) the
   crate.

3. **Fix the builder path** (`ipe_jwt_decode:551,557`): replace `as_i64()` with the
   same `numeric_date(&claims_val, "exp")` / `"nbf"` reader so a fractional claim is
   honoured, not silently skipped.

4. **Mint-side guard** (mirror `auth_sign_token`): in `ipe_jwt_expires_at`
   (`jwt.rs:406`), the reference `Jwt.ipe` passes the value straight through, so a
   hard reject here would diverge from the builder contract. Prefer the *decode-side*
   fix as the safe outcome (a mis-minted negative/expired token is rejected on
   read). Optionally add a debug-only log or leave `expiresAt` transparent —
   document that the safety boundary is decode, not mint (unlike `auth.rs`, whose
   `signToken` owns both ends). Do NOT silently clamp a negative `exp` at mint —
   that would hide the signer's bug and diverge from the pass-through contract.

### Parity / divergence
This RESTORES Go parity (Go evaluates `now >= exp` over the signed value and
rejects negatives/past-floors); no divergence record needed — it removes a
`../ipe` divergence. Note in the fix that `exp_is_zero`'s replacement is a superset
(handles integer-zero plus the missed domain), so no behaviour regresses on the
cases the old guard covered.

### Impl plan
1. Add `numeric_date` to `jwt.rs`; remove `exp_is_zero` (or reduce it to a thin
   caller of `numeric_date`). Update `auth.rs:189`'s call site to the new reader
   (it currently reuses `exp_is_zero`).
2. Wire the pre-validation reject into `jwt_decode_hs256` and `jwt_decode_rs256`.
3. Replace `as_i64()` with `numeric_date` in `ipe_jwt_decode`.
4. **Regression tests** (runtime `#[cfg(test)]` in `jwt.rs`; the audit found the
   suite missed these):
   - `test_flat_decode_negative_exp_rejected`: HS256-sign `{"exp":-1,"sub":"x"}`
     under a ≥32-byte secret → `jwt_decode_hs256` returns `Err` (was `Ok`).
   - `test_flat_decode_fractional_exp_zero_rejected`: `{"exp":0.4}` → `Err` (was
     `Ok`; and no underflow). Add `{"exp":0.0}` too.
   - `test_flat_vs_builder_fractional_exp_agree`: a token with `"exp":<past>.5`
     rejected by BOTH `jwt_decode_hs256` and `ipe_jwt_decode` (RT-AUTH-003).
   - `test_flat_decode_fractional_future_exp_accepted`: `{"exp":<far-future>.5}`
     still `Ok` (no over-rejection).
   - Keep the existing zero-exp and negative-TTL `auth.rs` tests green.
5. **Compiler-rejection tests:** none — these are runtime-decode-time, not a
   compile-time acceptance path, so `negative_suite.rs` is not the home. State this
   in the plan so the implementer doesn't add a spurious compiler test.

---

## Risk / blast radius

- **Money.ipe** is embedded into every build (`src/ipe-cli/src/stdlib.rs`
   injection). A change to its emitted body affects every downstream program's
   golden output that renders a `Money.allocate` result. **Re-gate:** the full
   examples sweep + any goldens that pin `Money.allocate`/`format` output +
   `money_parity.rs`. The guard/sign fix changes output ONLY for the previously-buggy
   inputs (`parts<=0` was an abort; negatives summed wrong) — positive same-currency
   allocations are unchanged, so most goldens should be stable. Verify no golden
   encoded the buggy `[-32,-32,-33]` shape.
- **JWT decoders** are runtime kernels behind `Jwt.decodeHs256`/`decodeRs256`/
   `Jwt.decode` and `Auth.verifyToken`. **Re-gate:** `cargo nextest run -p
   ipe-runtime-rust --features full` (the jwt module tests), plus any auth E2E in
   the examples sweep. The change is strictly *more rejecting* on the previously-
   accepted bad tokens; a valid future-dated integer/float `exp` still passes
   (covered by the accept test). Watch for any fixture token in the suite that
   carried a negative/zero/fractional `exp` and *expected* acceptance — that would
   be a golden encoding the bug (§0: fix the fixture, don't relax the guard).
- **THE SEAL:** the pure-`.ipe` Money fix adds no new acceptance path (same
   `allocate` signature), so no SEAL surface changes. The rejected kernel-registration
   alternative WOULD add a scheme and must fail-closed — another reason to prefer the
   pure fix.
- **CO-INCR-003 doc/divergence step** is doc-only (no code behaviour change) — zero
   runtime blast radius; the typed-Result redesign is deferred and separately gated.

---

## Proposed backlog entries

```json
{"id":"TBD","priority":"high","phase":"principles-audit-fix","task":"Restore Money.allocate parts<=0 guard and sign-correct residue in the pure .ipe port","notes":"src/stdlib/Ipe/Money.ipe:432-453 dropped the runtime kernel's guard (parts<=0 -> divide-by-zero abort, CO-INCR-001) and pairs trunc-div // with Euclidean modBy so negative totals mint cents (CO-INCR-002). Fix: `if parts<=0 then []`, and `remainder = totalMinor - base*parts` stepped by sign(remainder) mirroring money.rs:271-322. Add .ipe regression tests: allocate 0 -> [] not exit-101; allocate 3 (neg $1) sums to -$1.00. Re-run money_parity + examples sweep; verify no golden pinned the buggy shape.","spec":"docs/audit/2026-07-17-principles-audit/specs/t4-jwt-money.md","blocked_by":[],"status":"pending"}
{"id":"TBD","priority":"medium","phase":"principles-audit-fix","task":"Document Money cross-currency mismatch semantics + record divergence (CO-INCR-003)","notes":"Money.add/sub/sumOf return left operand on currency mismatch; compare/lt/... ignore currency (Money.ipe:395-506). Parity with ../ipe but unflagged. Add -- WARNING doc on the affected fns stating same-currency precondition + mismatch behaviour, and a docs/divergences-from-sky.md entry noting it is knowingly retained pending the typed-Result redesign. Doc-only, no behaviour change.","spec":"docs/audit/2026-07-17-principles-audit/specs/t4-jwt-money.md","blocked_by":[],"status":"pending"}
{"id":"TBD","priority":"low","phase":"principles-audit-fix","task":"Typed Result Money arithmetic redesign (add/sub/sumOf -> Result Error Money)","notes":"Follow-up to CO-INCR-003: make cross-currency add/sub/sumOf return a typed Err (or a SameCurrency witness) instead of a silent left-operand fallthrough. Breaking API change + upstream divergence; caller migration required. Separate from the doc/divergence step.","spec":"docs/audit/2026-07-17-principles-audit/specs/t4-jwt-money.md","blocked_by":[],"status":"pending"}
{"id":"TBD","priority":"medium","phase":"principles-audit-fix","task":"Align JWT exp/nbf NumericDate parsing to Go across flat + builder decoders","notes":"Replace exp_is_zero (as_u64 integer-zero only) with a total numeric_date reader (i64 or floor(f64)) covering negative + fractional exp/nbf (RFC 7519 NumericDate). Pre-reject now>=exp / now<nbf on jwt_decode_hs256 (jwt.rs:161) and jwt_decode_rs256 (jwt.rs:243) before jsonwebtoken's exp-1 underflow; replace as_i64() with numeric_date in ipe_jwt_decode (jwt.rs:551,557); update auth.rs:189 call site. Closes RT-AUTH-001 (neg exp accepted), RT-AUTH-002 (fractional exp underflow under overflow-checks=false), RT-AUTH-003 (flat/builder disagree). Restores Go parity (no divergence). Add runtime jwt tests: neg exp, exp 0.4/0.0, flat-vs-builder agreement, future-fractional still accepted. NOT a negative_suite.rs (compile-time) test.","spec":"docs/audit/2026-07-17-principles-audit/specs/t4-jwt-money.md","blocked_by":[],"status":"pending"}
```
