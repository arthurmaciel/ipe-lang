# Plan — Verify & pin `stringify.rs` float scientific-notation threshold vs Go `%v` (item 27, task #52)

**Status:** ready to execute · **Kind:** correctness pin (before-push) · **Base:** HEAD `691e275`
**Source spec:** `docs/architecture/sky-rust-backend-reference-audit.md` (item 27 / §Open decisions) — *followed, not redesigned.*

---

## Goal

Confirm, against a live Go oracle, whether ipê's float-display scientific-notation
threshold (`!(-4..6)` — switch at decimal exponent ≥ 6) or the `../sky` reference's
(`!(-4..21)` — switch at ≥ 21) matches Go's `fmt %v` / `strconv.FormatFloat(f,'g',-1,64)`;
then keep whichever matches and **pin it with a discriminating regression test** so the
threshold cannot silently drift back to 21. This is the exact golden-mask trap class, so
the pin must *fail* if someone flips the constant to 21 — a tautological green is not
acceptable.

### Oracle ground truth (already probed on this machine — Go 1.26.2)

The probe below was run read-only in `/tmp` during planning (no repo touch, no repo build).
Its results are the evidence base for every assertion in this plan. **Re-running it is the
literal "re-probe before push" the source spec calls for** — it is copy-paste reproducible.

```go
// probe.go  — run: go run probe.go
package main

import ( "fmt"; "math"; "strconv" )

func main() {
    negZero := math.Copysign(0, -1) // TRUE IEEE-754 negative zero (a Go -0.0 *literal* folds to +0)
    show := func(f float64) {
        fmt.Printf("%%v=%-22q g=%-22q\n",
            fmt.Sprintf("%v", f), strconv.FormatFloat(f, 'g', -1, 64))
    }
    for _, f := range []float64{
        99999, 100000, 100001, 123456, 999999, 1000000, 1000001,
        1e15, 1e16, 1e20, 1e21, 1.5e20, 1234567, 123456.789,
        0.0001, 0.00001, 1.5, 1.0, 42.5, -1.5,
    } { show(f) }
    show(negZero); show(math.Inf(1)); show(math.Inf(-1)); show(math.NaN())
}
```

Captured output (the parity oracle — `%v` and `'g',-1` are identical for **every** input;
the cut to scientific is a **flat exponent ≥ 6**, with **no** exponent-21 behaviour anywhere):

| input | Go `%v` = `'g',-1` | branch | discriminates 6-vs-21? |
|---|---|---|---|
| `99999` | `99999` | positional (exp 4) | — |
| `100000` (1e5) | `100000` | positional (exp 5) | — |
| `999999` | `999999` | positional (exp 5) | **yes** — 21 would also say positional; keep as lower guard |
| `1000000` (1e6) | `1e+06` | **scientific (exp 6)** | **YES** — 21 wrongly prints `1000000` |
| `1000001` | `1.000001e+06` | scientific | **YES** — proves not a 1e6 special-case |
| `1234567` | `1.234567e+06` | scientific | **YES** |
| `1e15` | `1e+15` | scientific | **YES** — 21 wrongly prints `1000000000000000` |
| `1e20` | `1e+20` | scientific | **YES** — 21 wrongly prints `100000000000000000000` |
| `1e21` | `1e+21` | scientific | matches under both (≥21) |
| `123456.789` | `123456.789` | positional (exp 5) | — |
| `0.0001` | `0.0001` | positional (exp −4) | low boundary |
| `0.00001` (1e-5) | `1e-05` | scientific (exp −5) | low boundary |
| true `-0.0` | `-0` | positional | shared branch |
| `+Inf` / `-Inf` / `NaN` | `+Inf` / `-Inf` / `NaN` | non-finite | shared branch |

**Verdict (resolves item 27 / #52):** ipê is **correct**. `!(-4..6)` reproduces Go byte-for-byte
across the whole range. The reference's `!(-4..21)` is **wrong** — it would emit `1000000`,
`1000000000000000`, `1e20`-as-`100000000000000000000`, etc., diverging from Go on every
`1e6..1e20` value. The existing `float_exponent_threshold_is_six` test is a *valid* pin, not a
false one. This plan **keeps ours** and **hardens the pin** so the 6-vs-21 discriminating cases
(currently only `1e6`/`1e5` are covered) can never regress silently. *(`../sky` is a
parity/capability reference; the 21 threshold is simply a value that does not match the Go oracle
— stated as a factual difference, nothing more.)*

---

## Architecture

Two independent Rust float renderers in the vendored runtime implement the same Go-`'g'` rule and
must stay in lockstep:

1. **`runtime/src/sky_runtime/stringify.rs` → `impl SkyStringify for f64::sky_show`** (lines
   118–160 at HEAD) — powers `toString` / `Debug`-style rendering. Threshold at line 145:
   `Ok(exp) if !(-4..6).contains(&exp) =>`. Uses `format!("{f:e}")` shortest-scientific, splits on
   `e`, re-emits Go `%e` shape (`{mantissa}e{sign}{mag:02}`) when out of `[-4,6)`, else
   `f.to_string()`.
2. **`runtime/src/sky_runtime/string.rs` → `pub fn string_from_float`** (lines 139–182 at HEAD) —
   powers `String.fromFloat`. Threshold at line 176: `if (-4..6).contains(&exp)` → positional via
   `fmt_g_positional`, else `fmt_g_exponent` (line 185).

Both are already `!(-4..6)`. The work is **evidence + pins**, not a code fix: expand each file's
test module with the discriminating boundary/high-magnitude/negative-zero/non-finite cases, prove
the pins actually fail under a 21-threshold (temporary flip → red → revert → green), align the
"Go said 21" hedge-comments to cite the concrete probe, and flip item 27 `OPEN → resolved` in the
audit doc.

## Tech stack

Rust (workspace member `runtime`, crate name **`sky-runtime-rust`**), stdlib `#[test]` unit tests
run under `cargo test` / nextest in CI (`.github/workflows/ci.yml`). Go 1.26.2 present at
`/usr/local/go/bin/go` for the one-shot oracle re-probe. No new crate deps, no new files in the
Rust tree, no Go file committed (the repo stays Go-free at test time — matches the audit's item-25
"inline Go expected values, no oracle needed" convention).

## Global constraints

- **PRINCIPLES order (strict):** security > correctness > soundness > efficiency > completeness >
  readability. This item is decided on **correctness** (byte-parity with the Go oracle).
- **Parse, don't validate.** The threshold decision is made once from a parsed `i32` exponent
  (`exp_str.parse::<i32>()`), not re-derived from string shape at each use. Keep it that way; do
  not add a second stringly-typed branch.
- **Make invalid states unrepresentable.** The parity invariant is encoded as executable
  assertions in *both* renderers so "renderer A says 6, renderer B drifted to 21" is a red build,
  not a latent divergence. The pin values are transcribed from the oracle table above, never from
  memory.
- **Fail-closed, no panics/wildcards.** `sky_show` totality contract (stringify.rs line 15: "NEVER
  panics — no `unwrap`/`expect`/indexing") is preserved. The exponent match already fails closed
  (`_ => f.to_string()`); do not widen it. No test uses `.unwrap()` on parse in a way that could
  panic the suite differently than production.
- **Doc-with-code.** Per CLAUDE.md template-sync rule, the audit doc's item 27 / §Open-decisions
  entry is updated in the same commit as the pins (Task 3).
- **Parallel-safety.** All edits are confined to `runtime/src/sky_runtime/{stringify,string}.rs`
  test modules + hedge-comments, and `docs/architecture/sky-rust-backend-reference-audit.md`.
  **No overlap** with the in-flight registry migration (#45: `crates/sky_types/src/constrain.rs`,
  `sky_kernels`, `crates/sky_lower/src/lower.rs` callee) or with **#49 TCO** (`sky_ir` +2 variants,
  `lower.rs`, `crates/sky_backend_rust/src/emit_expr.rs`). This task touches the vendored **runtime**
  crate only; it can land in any order relative to #45/#49 with zero merge risk. State this in the
  commit body.

---

## Task 1 — Harden the `stringify.rs` `f64::sky_show` Go-`%v` parity pin

**File:** `runtime/src/sky_runtime/stringify.rs`
**Anchor:** test module `mod tests` (line 316); existing pin `float_exponent_threshold_is_six`
(lines 349–357). Impl under test: `impl SkyStringify for f64::sky_show` (lines 118–160), threshold
constant at **line 145** (`!(-4..6)`).

### Interfaces

- **Consumes:** `f64::sky_show(&self) -> String` (already implemented, correct).
- **Produces:** an expanded regression test that pins Go-`%v` parity across the boundary,
  high-magnitude, negative-zero, and non-finite classes. No signature or impl change.

### Steps

1. **Prove the pin discriminates (genuine red).** Temporarily flip the threshold to the reference
   value to confirm the *new* assertions catch it. Edit line 145:

   ```rust
   // TEMPORARY — reference's (wrong) threshold, to prove the pin fails on it:
   Ok(exp) if !(-4..21).contains(&exp) => {
   ```

   Replace the body of `float_exponent_threshold_is_six` (lines 350–356) with the hardened test,
   renamed to reflect scope:

   ```rust
   #[test]
   fn float_go_v_parity() {
       // Byte-for-byte parity with Go `fmt %v` == `strconv.FormatFloat(f,'g',-1,64)`.
       // Oracle: Go 1.26.2 `go run probe.go` (see reference-audit.md item 27). The cut
       // to scientific notation is a FLAT decimal-exponent >= 6 (and < -4), NOT 21.
       // Positional class (exp in [-4, 6)):
       assert_eq!(99999.0f64.sky_show(), "99999"); // exp 4
       assert_eq!(1e5f64.sky_show(), "100000"); // exp 5
       assert_eq!(999999.0f64.sky_show(), "999999"); // exp 5 (lower guard)
       assert_eq!(123456.789f64.sky_show(), "123456.789");
       assert_eq!(0.0001f64.sky_show(), "0.0001"); // exp -4 boundary
       // Scientific class (exp >= 6) — these DISCRIMINATE 6 from 21:
       assert_eq!(1e6f64.sky_show(), "1e+06"); // exp 6 — 21 would print "1000000"
       assert_eq!(1000001.0f64.sky_show(), "1.000001e+06"); // not a 1e6 special-case
       assert_eq!(1234567.0f64.sky_show(), "1.234567e+06");
       assert_eq!(1e15f64.sky_show(), "1e+15"); // 21 would print 16 zeros
       assert_eq!(1e20f64.sky_show(), "1e+20"); // 21 would print 21 digits
       assert_eq!(1e21f64.sky_show(), "1e+21");
       // Scientific class (exp <= -5):
       assert_eq!(1e-5f64.sky_show(), "1e-05");
       // Negative zero (shared positional branch): Go true -0.0 -> "-0".
       assert_eq!((-0.0f64).sky_show(), "-0");
       // Non-finite (shared branch):
       assert_eq!(f64::INFINITY.sky_show(), "+Inf");
       assert_eq!(f64::NEG_INFINITY.sky_show(), "-Inf");
       assert_eq!(f64::NAN.sky_show(), "NaN");
       assert_eq!((-1.5f64).sky_show(), "-1.5");
   }
   ```

2. **Run — observe RED.**

   ```bash
   cargo test -p sky-runtime-rust --lib stringify::tests::float_go_v_parity
   ```

   Expected: the test FAILS with the 21-threshold in place, e.g.

   ```
   ---- stringify::tests::float_go_v_parity stdout ----
   assertion `left == right` failed
     left: "1000000"
    right: "1e+06"
   ...
   test result: FAILED. 0 passed; 1 failed; ...
   ```

   This confirms the pin actually guards the 6-vs-21 regression (not a tautology).

3. **Revert the impl to correct (green).** Restore line 145 to the shipped, Go-matching value:

   ```rust
   Ok(exp) if !(-4..6).contains(&exp) => {
   ```

4. **Align the hedge-comment.** Replace the vague "Go's older public comment said 21" note
   (lines 140–143) with a concrete, reproducible citation:

   ```rust
   // Go uses exponent form iff exp < -4 || exp >= 6 (Go `'g'` shortest-mode cut).
   // Verified against Go 1.26.2 `fmt %v` == `strconv.FormatFloat(f,'g',-1,64)`:
   // 1e6 -> "1e+06", 1e15 -> "1e+15", 999999 -> "999999" (see reference-audit.md
   // item 27 for the oracle probe). The `../sky` reference uses 21 here, which
   // diverges from the Go oracle on every 1e6..1e20 value.
   ```

5. **Run — observe GREEN.**

   ```bash
   cargo test -p sky-runtime-rust --lib stringify::tests
   ```

   Expected: `test result: ok. N passed; 0 failed; ...` (N = existing stringify tests, with
   `float_go_v_parity` replacing `float_exponent_threshold_is_six`).

6. **Commit.**

   ```bash
   git add runtime/src/sky_runtime/stringify.rs
   git commit -m "runtime(stringify): pin f64 sky_show Go-%v parity across 6-vs-21 boundary

Hardens the scientific-notation threshold pin: adds discriminating cases
(1e6/1e15/1e20 scientific, 999999 positional, -0.0, non-finite) transcribed
from a Go 1.26.2 oracle probe. Confirms !(-4..6) matches Go; the pin now fails
if the threshold drifts to 21. Runtime-crate-only; no overlap with #45/#49.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5zTa7nMvFij62AamtBj3H"
   ```

---

## Task 2 — Mirror the discriminating pin in `string.rs::string_from_float`

**File:** `runtime/src/sky_runtime/string.rs`
**Anchor:** test module `mod tests` (line 556); existing float pins `ff_e6_flips_to_exponent`
(lines 649–651) and `ff_e5_stays_positional` (lines 653–655). Impl: `pub fn string_from_float`
(lines 139–182), threshold at **line 176** (`if (-4..6).contains(&exp)`).

### Interfaces

- **Consumes:** `pub fn string_from_float(f: f64) -> String` (already correct).
- **Produces:** one new test locking the 6-vs-21 discriminators the current per-case tests miss
  (`ff_*` covers `1e5`/`1e6`/`1e21` but not `999999`, `1.000001e6`, `1e15`, `1e20` — the values
  where 6 and 21 visibly diverge).

### Steps

1. **Add the discriminating test** (append inside `mod tests`, after `ff_e5_stays_positional`,
   ~line 655):

   ```rust
   #[test]
   fn ff_go_g_threshold_is_six_not_twentyone() {
       // Discriminates Go's flat exp>=6 cut from the reference's 21. Oracle:
       // Go 1.26.2 strconv.FormatFloat(f,'g',-1,64) (see reference-audit.md item 27).
       assert_eq!(string_from_float(999999.0), "999999"); // exp 5 positional
       assert_eq!(string_from_float(1000001.0), "1.000001e+06"); // exp 6 scientific
       assert_eq!(string_from_float(1e15), "1e+15"); // 21 would print 16 zeros
       assert_eq!(string_from_float(1e20), "1e+20"); // 21 would print 21 digits
   }
   ```

2. **Prove RED (temporary flip).** Edit line 176 to the reference threshold and run:

   ```rust
   if (-4..21).contains(&exp) {
   ```

   ```bash
   cargo test -p sky-runtime-rust --lib string::tests::ff_go_g_threshold_is_six_not_twentyone
   ```

   Expected FAILED, e.g. `left: "1000000000000000"  right: "1e+15"`.

3. **Revert line 176** to `if (-4..6).contains(&exp) {` (the shipped value).

4. **Align the hedge-comment** at lines 173–176 to the same concrete-citation form used in Task 1
   step 4 (reference the oracle probe + item 27; note the reference uses 21 and diverges).

5. **Run — observe GREEN.**

   ```bash
   cargo test -p sky-runtime-rust --lib string::tests
   ```

   Expected `test result: ok.` with the new test passing alongside the existing `ff_*` block.

6. **Commit.**

   ```bash
   git add runtime/src/sky_runtime/string.rs
   git commit -m "runtime(string): pin string_from_float 6-vs-21 threshold discriminators

Adds 999999/1.000001e6/1e15/1e20 cases (Go 1.26.2 oracle) that visibly separate
the flat exp>=6 cut from the reference's 21, locking both float renderers to the
same verified invariant. Runtime-crate-only; no overlap with #45/#49.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5zTa7nMvFij62AamtBj3H"
   ```

---

## Task 3 — Resolve item 27 in the reference audit (doc, same series)

**File:** `docs/architecture/sky-rust-backend-reference-audit.md`
**Anchors:** comparison-table row 27 (line 59); §Runtime "One genuine cross-fork disagreement
(OPEN)" (lines 96–101); §Open decisions bullet "OPEN (item 27)" (lines 283–287); Roadmap
"Before push" bullet "Re-probe the `stringify.rs` float threshold" (line 225).

### Interfaces

- **Consumes:** the oracle table + verdict from this plan's header.
- **Produces:** item 27 flipped `OPEN → resolved`, with the probe recorded so the "before-push
  re-probe" is satisfied by a committed, reproducible recipe rather than tribal memory.

### Steps

1. **Table row 27** (line 59): change the last cell from
   `**OPEN** — one-line re-probe vs Go oracle before push` to
   `**resolved** — Go 1.26.2 oracle confirms flat exp>=6 (ours); reference's 21 diverges. Pinned
   in stringify.rs::float_go_v_parity + string.rs::ff_go_g_threshold_is_six_not_twentyone`.
   Keep the verdict `**O+**`.

2. **§Runtime lines 96–101:** replace the "(OPEN)" paragraph with a "(RESOLVED)" paragraph stating
   the Go 1.26.2 probe outcome (`1e6 -> "1e+06"`, `1e15 -> "1e+15"`, `999999 -> "999999"`, true
   `-0.0 -> "-0"`), that `%v` ≡ `'g',-1` everywhere with no exponent-21 behaviour, and that the
   reference's 21 is the diverging value. Embed the `probe.go` snippet from this plan so it is
   reproducible in-place.

3. **§Open decisions lines 283–287:** move the item-27 bullet out of OPEN — restate as resolved
   with the verdict and the two pinning test names. (Leave item 11 untouched.)

4. **Roadmap line 225:** strike the "Re-probe the `stringify.rs` float threshold (item 27, OPEN)"
   before-push bullet, or mark it done, since the pins from Tasks 1–2 discharge it.

5. **Verify no dangling "OPEN"/"21" claims remain for item 27.**

   ```bash
   rg -n "item 27|exp\W*21|-4\.\.21|OPEN" docs/architecture/sky-rust-backend-reference-audit.md
   ```

   Expected: only the historical "reference uses 21 / diverges" factual mentions remain; no
   "OPEN" tag survives against item 27.

6. **Commit.**

   ```bash
   git add docs/architecture/sky-rust-backend-reference-audit.md
   git commit -m "docs(audit): resolve item 27 — float sci-notation threshold verified vs Go

Go 1.26.2 oracle (fmt %v == strconv 'g',-1) confirms ipe's flat exp>=6 cut is
correct; the reference's 21 diverges on every 1e6..1e20 value. Pinned by the
two new runtime tests. Flips item 27 OPEN -> resolved.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5zTa7nMvFij62AamtBj3H"
   ```

---

## Definition of done

- `cargo test -p sky-runtime-rust --lib stringify::tests string::tests` → all green, including
  `float_go_v_parity` and `ff_go_g_threshold_is_six_not_twentyone`.
- Each pin proven to go RED under a temporary `!(-4..21)` flip (recorded in Tasks 1–2), then GREEN
  on revert — proof the guard is discriminating, not tautological.
- No `stringify.rs`/`string.rs` **impl** line changed (thresholds stay `!(-4..6)` / `(-4..6)`);
  only test modules + hedge-comments touched.
- Item 27 marked resolved in the audit doc, probe recipe embedded for reproducibility.
- Three small commits, runtime-crate + one doc file only — zero file overlap with #45 (constrain /
  sky_kernels / lower callee) or #49 TCO (sky_ir / lower / emit_expr). Close task #52.
