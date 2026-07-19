# data-auth-money verdicts

Theme covers findings from rt-auth.md, rt-data.md, rt-tui.md, co-incr.md.
Read-only adversarial verification. Cited code re-read at `file:line`; vendored
`jsonwebtoken-9.3.1/src/validation.rs` and `serde-1.0.228` Visitor defaults read
to ground the JWT parse claims.

---

## RT-AUTH-001 · CONFIRMED
- final severity: medium (correctness, not an auth bypass — keep as filed)
- reachability: a token bearing a NEGATIVE-INTEGER `exp` (e.g. `-1`) AND a VALID
  signature under the app's own HMAC secret, decoded via the FLAT `Jwt.decodeHs256`
  / `Jwt.decodeRs256` kernels. No external forger: an attacker WITHOUT the signing
  key cannot pass the signature check, so this is NOT a remote bypass. The real
  trigger is a signer-side own-goal — an app minting `expiresAt (now - ttl)` by a
  sign error via the unguarded `ipe_jwt_expires_at : i64 -> …` (jwt.rs:406, no
  negative guard) — producing an intended-instantly-expired token that then never
  expires.
- reasoning: traced end to end. serde-1.0.228 `Visitor::visit_i64` default returns
  `Err(invalid_type)` (`core/de/mod.rs`); `numeric_type`'s `NumericType` visitor
  (validation.rs:334-364) overrides only `visit_u64`/`visit_f64`, so a JSON negative
  integer routes to the defaulted `visit_i64` → `Err` → caught at validation.rs:366-368
  → `Ok(TryParse::FailedToParse)`. At validation.rs:274 the expiry check is
  `matches!(claims.exp, TryParse::Parsed(exp) if …)`, false for `FailedToParse`, so
  the check is SKIPPED and the token is accepted as non-expiring. The flat decoders
  clear `required_spec_claims` (jwt.rs:186/273), so nothing re-rejects. Go's oracle
  evaluates `now >= exp` over the signed integer and rejects any negative `exp` as
  long-expired — a genuine P2 divergence. (`auth_verify_token` keeps the default
  `required_spec_claims={"exp"}`, so it is accidentally fail-closed here; and the
  BUILDER path `ipe_jwt_decode` uses `as_i64()` + `now >= exp`, which DOES reject a
  negative int — only the flat path is affected.)
- repro: mint `{"exp":-1,"sub":"x"}` HS256-signed under the app secret; feed to
  `Jwt.decodeHs256 secret token` → `Ok` (accepted). Go rejects the same token.
- dup-of: —

## RT-AUTH-002 · CONFIRMED
- final severity: medium (correctness + release-profile fail-open; keep as filed)
- reachability: a token with a FRACTIONAL `exp` in `[0, 0.5)` (e.g. `0.4` or `0.0`)
  and a valid signature under the app key, via the FLAT decoders. RFC 7519 NumericDate
  explicitly permits non-integer values, so this is reachable not only from the app's
  own flat encoder but from ANY RFC-legal external issuer sharing the secret (some JWT
  libraries emit fractional `iat`/`exp`) — a more plausible interop path than 001's
  negative case.
- reasoning: verified. `exp_is_zero` (jwt.rs:85-100) matches only
  `value.get("exp").and_then(JsonValue::as_u64) == Some(0)`; `as_u64` returns `None`
  for a JSON float `0.4`, so the pre-guard is BYPASSED. In jsonwebtoken, `visit_f64`
  (validation.rs:347-356) accepts `0.4` and returns `Parsed(0.4.round() as u64)` =
  `Parsed(0)`. Then validation.rs:274-275 computes `exp - reject_tokens_expiring_in_less_than`
  = `0u64 - 1`. The emitted-project profile pins `overflow-checks = false` for BOTH
  dev (project.rs:225, read directly) and release (default-off; release also
  `panic = "abort"`), so `0u64 - 1` WRAPS to `u64::MAX`, making `u64::MAX < now`
  false → the token is ACCEPTED as non-expiring. Under overflow-checks-on (the
  runtime's OWN test profile only, NOT emitted apps) the same input panics inside a
  Result-returning kernel. Either way a soundness/correctness defect; in shipped apps
  it is the fail-open acceptance, not a panic.
- repro: mint `{"exp":0.4}` signed under the app secret → `Jwt.decodeHs256` returns
  `Ok` (accepted forever). Go rejects (0 < now).
- dup-of: —

## RT-AUTH-003 · CONFIRMED
- final severity: low (keep)
- reachability: any token with a fractional NumericDate `exp`/`nbf` decoded on both
  documented surfaces. Flat `Jwt.decodeHs256` rounds+validates (rejects a past
  fractional exp); builder `Jwt.decode` reads `as_i64()` (jwt.rs:551) → `None` on a
  float → check skipped → accepts as non-expiring. Verified opposite outcomes for the
  same token across jwt.rs:551-562 (builder) vs jwt.rs:196/283 (flat).
- reasoning: sub-symptom of the same NumericDate-parse gap as 002; distinct because
  it is a flat-vs-builder DISAGREEMENT (migration hazard) rather than the underflow.
  Real, low.
- dup-of: partial-sibling of RT-AUTH-002 (shared root: fractional NumericDate handling);
  kept separate per its distinct symptom.

## RT-AUTH-004 · CONFIRMED (as smell/low)
- final severity: low
- reachability: NOT reachable from well-typed Ipê today. The `else` arm at jwt.rs:466
  / 527 requires an `algorithm_descriptor` that is neither `HS256:`- nor `RS256:`-
  prefixed; the only constructors are `ipe_jwt_hs256`/`ipe_jwt_rs256` (jwt.rs:358-365)
  which always emit those prefixes, so the branch is dead. The panic (`&s[..20]` on a
  char boundary) and the prefix-echo are one codegen drift away, not live.
- reasoning: verified the raw range-index `&algorithm_descriptor[..…min(20)]` at both
  sites — a byte slice that panics if byte 20 splits a multibyte char, against the
  crate's `.get()` discipline; and it would echo up to 20 descriptor bytes (which
  carry key material after the 6-byte tag). Correct as a latent-smell low.
- dup-of: —

## RT-AUTH-005 · CONFIRMED (as smell/low)
- final severity: low
- reachability: every builder-API program holds its signing secret inside a plain
  `String` (`"HS256:<secret>"`, jwt.rs:358-360) for the `Algorithm` value's lifetime.
  `IpeStringify`/`Display` for `String` means any generic show/log/interpolation sink
  reachable from that value prints the secret verbatim. No concrete live sink shown —
  hence low. Directly contrasts `secret.rs`'s sealed newtype design, verified present.
- reasoning: confirmed the descriptor scheme widens the secret into the most capable
  type in the system, keyed by a string prefix; RT-AUTH-004's error path shows how
  close descriptor bytes already sit to an error string. Legitimate make-invalid-states-
  unrepresentable smell.
- dup-of: —

---

## RT-DATA-001 · CONFIRMED
- final severity: medium (keep)
- reachability: no attacker needed — a latent data-correctness bug. `row_to_json`
  (db.rs:220-241, fallback `Err(_) => JsonVal::Null`) and `row_to_map` (db.rs:186-205,
  fallback `String::new()`) probe only `Option<String>→i64→f64` per column. On the
  Postgres driver (strict sqlx decode), a non-NULL BOOLEAN/BYTEA/NUMERIC/TIMESTAMP
  cell decodes as none of the three and collapses to Null/"" with no error. The
  runtime can itself write such a column (`SqlBytes` bindable, db.rs:1760, with no
  `Db.Decode.bytes` to read it back). SQLite's dynamic typing masks most of it; the
  bite is Postgres-specific.
- reasoning: verified both bridges and the doc comment (db.rs:218) that acknowledges
  the fallback. Composes badly with `Db.Decode.nullable`, turning a present value into
  a trusted `Nothing`. Medium is right (correctness/data-integrity, driver-dependent);
  not push-blocking on its own.
- dup-of: —

## RT-DATA-002 · CONFIRMED (as smell/low)
- final severity: low
- reachability: no current `Db.Decode` Ipê program hits a drifted decoder — the
  exposed surface composes only field-preserving combinators under `nullable`. The
  `pub run`/`pub fields` struct (json.rs:21-30) is openly constructible and three
  shipped combinators (`decode_list`/`decode_one_of`/`decode_at`) already break the
  documented fields-mirror-run invariant; `db_decode_nullable` gates NULL on
  `inner.fields`. First future addition of `oneOf`/`list` to the `Db.Decode` kernel
  surface turns it into a silent NULL-gating bug. Structural smell today.
- reasoning: matches prior-audit item (3), still present. Correct as low.
- dup-of: —

## RT-DATA-003 · CONFIRMED
- final severity: low (keep)
- reachability: `Db.findByConditions conn table dict` where `dict` is built from
  request-derived filters and comes back EMPTY → `keys.is_empty()` branch at
  db.rs:1475-1476 emits `SELECT * FROM {table}`, returning every row (cross-tenant
  read in a multi-tenant app). Requires the app to (a) derive conditions from
  untrusted input and (b) permit an empty set — a real but narrower path than a mass
  write. Asymmetric with the already-fixed fail-closed `db_update_fields`
  (db.rs:2299-2305).
- reasoning: verified the fail-open SELECT and the contrast with the UPDATE fix.
  Correct as low (read exposure, precondition-gated).
- dup-of: —

## RT-DATA-004 · CONFIRMED (as completeness/low)
- final severity: low
- reachability: `Config.decodeYaml`/`loadFromFile` on attacker YAML. The 4 MiB source
  cap (config_decode.rs:69-97) bounds raw input; the anti-expansion leg rests on
  serde_yaml 0.9's internal alias-repetition limit, asserted "(verified)" in a comment
  with no in-repo bomb fixture (confirmed none under src/runtime/rust/). serde_yaml is
  archived. The untested load-bearing property is a real completeness gap; a future
  crate swap silently drops it.
- reasoning: verified the comment-only assertion and the deprecated dependency. Correct
  as low (add a bomb fixture pinning `Err`).
- dup-of: —

---

## RT-TUI-001 · CONFIRMED
- final severity: high on the soundness axis; practical impact LOCAL-ONLY (see note)
- reachability: a well-typed `Ui.row` with ≥3 fill children carrying huge
  `Ui.fillPortion` weights, on every render. `distribute_row_fill` (layout.rs:2230)
  sums portions as a PLAIN `i64` (`.sum()`, NOT saturating) — verified. `[i64::MAX,
  i64::MAX, 4]` wraps (overflow-checks off in emitted release/dev, project.rs:225) to
  `total_portion = 2`, passes the `<= 0` guard (line 2231), then line 2255
  `remaining.saturating_mul(MAX as usize) / 2` ≈ `usize::MAX/2`, becomes `target`, and
  `set_width(target.max(1), …)` (line 2272) hits `" ".repeat(w - lw)` (layout.rs:404)
  with ~9.2e18 → `str::repeat` capacity-overflow panic / OOM. Same defect class as the
  FIXED `fr_total` HIGH; the fix clamped `Grid.fr` tracks but not `fillPortion`.
  NOTE: `distribute_col_fill` (layout.rs:2100) already sums in `usize` with `.max(1)`
  and its shares feed a `while len < share` push loop → OOM/hang rather than the repeat
  panic; the row path is the sharp one. The col `.max(1)` does NOT bound the wrapped
  total, so the col OOM is still real.
- reasoning: the values are Ipê SOURCE `Int`s (author/Model-derived), never runtime
  request-derived — a TUI app has no remote party — so this is a local self-inflicted
  crash, not a remotely-exploitable DoS. Under P3 ("a well-typed program can never
  trigger a runtime failure") it is a genuine soundness hole and the auditor's HIGH is
  justified on that axis (parity with the accepted-HIGH `fr_total`). I keep HIGH but
  record that reachability is local-author-only with no data/security blast radius.
- repro: `Ui.row [] [ Ui.el [Ui.width (Ui.fillPortion 9223372036854775807)] …, Ui.el
  [Ui.width (Ui.fillPortion 9223372036854775807)] …, Ui.el [Ui.width (Ui.fillPortion 4)] … ]`
  → panic/OOM on render.
- dup-of: —

## RT-TUI-002 · CONFIRMED
- final severity: medium (keep)
- reachability: `Ui.paddingEach`/`Ui.spacing`/`Ui.height (vh …)` with large Ipê Ints,
  every render. `apply_padding` (layout.rs:906-943): `top`/`bottom` = `cells_y(pad)`
  each clamped to MAX_CELLS=100k (layout.rs:96), and `total_w` clamped to MAX_CELLS
  (line 919) — but the AREA `top × total_w` is the product = up to 100k rows ×
  `" ".repeat(100k)` ≈ 10^10 cells ≈ 10-20 GB → OOM. Per-dimension clamp only; product
  unbounded. Verified.
- reasoning: same local-author-only reachability as 001 (no remote party). Correct as
  medium (local DoS). fix = bound the area to a terminal-proportional multiple.
- dup-of: —

## RT-TUI-003 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: any focused input/textarea holding wide chars. `cursor_line_col`
  (layout.rs:1274-1286) counts `col += 1` per char; consumer `reverse_cell_at`
  (layout.rs:521-556) accumulates `UnicodeWidthChar::width`. Verified mismatch →
  cursor misplaced/invisible for CJK/emoji. Visual defect only, no panic. Low correct.
- dup-of: —

## RT-TUI-004 · CONFIRMED (as smell/low)
- final severity: low
- reachability: `BorderSpec` style is an open `String` (layout.rs:195) matched against
  a closed set in `border_glyphs` (layout.rs:1958-1974); an arbitrary style silently
  degrades to solid. Parse-don't-validate smell, no crash. Low correct.
- dup-of: —

## RT-TUI-005 · CONFIRMED (as smell/low)
- final severity: low
- reachability: `Rendered.hits` first tuple field must index `ctx.focusables`; the
  type does not enforce it (layout.rs:575-578). Sole consumer uses `.get_mut` (total),
  so a drifted index is a silent no-op, not a panic. Smell. Low correct.
- dup-of: —

## RT-TUI-006 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: every non-mouse key event. Shift-Tab dispatch is the heuristic
  `kind == "other" && value.contains('Z')` (app.rs:529) over stringly-typed `TuiKey`;
  an unrelated `ESC O Z` (SS3) sequence also matches → misfires focus-back. Low correct.
- dup-of: —

## RT-TUI-007 · CONFIRMED (as completeness/low)
- final severity: low
- reachability: n/a (the point is non-reachability). No module outside tui/{cell,diff}
  constructs a `Grid` or calls `tui::diff::diff`; the paint path (app.rs:103-108)
  clear-and-repaints the whole frame per event. The module docs (diff.rs:3-6) assert an
  incremental diff pipeline that does not exist; `Cell`/`Grid`/`diff` are dead. Harmless
  but a false doc claim (P5). Low correct.
- dup-of: subsumes the prior `Cell::width` open-`u8` concern (moot: no consumer).

---

## CO-INCR-001 · CONFIRMED
- final severity: high (keep)
- reachability: YES, `parts` is reachably request-derived. `Money.allocate` is pure-Ipê
  embedded stdlib (Money.ipe:432-441), injected into every build. `Money.allocate
  (List.length participants) total` with an empty request-derived `participants`
  yields `parts = 0` → `totalMinor // 0` → `ipe_int_div(x, 0)` panics (math.rs:78-83,
  the intentional DivisionByZero abort, exit 101). In an Ipe.Live/Http.Server app this
  kills the whole server process from one request.
- reasoning: verified the port dropped the guard. The RUNTIME kernel `money_allocate`
  (money.rs:271-274) HAS `if parts <= 0 { return Vec::new() }`, but `Money.ipe:allocate`
  is a full pure-Ipê reimplementation (NOT an `Ffi.kernel` alias to it), so the guard
  is gone. Upstream routes `allocate` to the guarded kernel; the pure port regressed it.
  P3 soundness breach with a plausible request path → high is right.
- repro: `Money.allocate (List.length []) (Money.usd 100)` → process abort (exit 101).
- dup-of: —

## CO-INCR-002 · CONFIRMED
- final severity: high as filed; defensible DOWNGRADE to medium (see note) — I keep HIGH
- reachability: any refund/credit flow — `Money.allocate 3 (Money -100 minor USD)`.
- reasoning: verified the trunc-div / Euclidean-modBy mismatch. `//` is `ipe_int_div`
  = `wrapping_div`, truncates toward zero (math.rs:78-83): `-100 // 3 = -33`. `modBy 3
  (-100)` (basics.rs:16-23): `r = -100 % 3 = -1`, `r < 0` → `+3` → `extra = 2`.
  `allocateHelp` gives first two parts `base+1 = -32`, last `base = -33` →
  `[-32,-32,-33]` sum **-97 ≠ -100** — 3 cents minted, violating the module's own stated
  invariant (Money.ipe:429) and the "never wrong money silently" pinned default. The
  runtime kernel distributes residue toward zero by sign correctly (money.rs:310-322);
  the port does not. NOTE: severity could be argued medium (correctness, no crash/no
  security, narrower than 001's abort), but a flagship money module silently minting
  currency against its own documented contract warrants high — I keep high.
- repro: `Money.allocate 3 (negative $1.00)` → parts sum to -$0.97.
- dup-of: same port-regression family as CO-INCR-001 (distinct symptom; not merged).

## CO-INCR-003 · CONFIRMED (as correctness/medium)
- final severity: medium (keep)
- reachability: any app mixing currencies. `Money.add`/`sub` (Money.ipe:395-408) return
  the LEFT operand on currency mismatch (verified — `else a`); `sumOf` (457-469) inherits
  it, silently dropping non-matching entries; `compare`/`lt`/`lte`/`gt`/`gte` (474-506)
  ignore currency and compare raw decimals. Silent-wrong-money default.
- reasoning: verified. This MATCHES upstream `../sky` (parity), but no divergence record
  sanctions it and no module doc warns the caller — exactly the swallowed-error class the
  correctness axis names, in the "never raw Float for currency" flagship. Medium correct.
- dup-of: —

## CO-INCR-004 · CONFIRMED (as completeness/low)
- final severity: low
- reachability: no end-user bypass today — CLI (one-shot + watch) always routes through
  `compile_prepared` where the IPE-N0029 gate lives (lib.rs:800-816), both cache tiers
  are target-keyed (cache.rs), FFI disables cache. The bypass is STRUCTURAL: a direct
  demand of `emit_project`/`emit_manifest` (db/src/lib.rs:829/993) with a `WasmClient`
  config — tests do this; a future warm driver / LSP code-action / alternate front-end
  would — emits a full wasm bundle with server-only kernels and NO diagnostic. The gate
  is convention-attached, not a dependency of the emit query graph. THE SEAL "fails
  closed by construction" property is not upheld structurally. Correct as low (no live
  bypass; architectural fail-open-if-reordered).
- reasoning: verified the gate lives outside the emit queries it protects. Low correct.
- dup-of: —

## CO-INCR-005 · CONFIRMED (as completeness/medium)
- final severity: medium (keep)
- reachability: any project with installed FFI crates (`.ipe/cache/ffi/rust`). `ipe
  build` loads the FFI catalog + `inject_interfaces` + `assemble_emit` (lib.rs:429-446);
  `ipe watch` FsBatch arm (watch.rs:718-765) injects only the std closure and builds
  `BuildConfig::new(…, None, Target::Native)` — `ffi = None`, no interface injection —
  so every `import Rust.Foo` resolves Unresolved → IPE-N0020, red-loop. Watch silently a
  non-FFI-only feature. INV-3 keeps the last-good binary up (no soundness impact).
- reasoning: verified the contrast between the two drivers. Medium correct (capability
  partially works; watch diverges from build).
- dup-of: —

## CO-INCR-006 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: every `ipe watch` rebuild after the first + every LSP session reuse a
  warm `db_main` (watch.rs:667-798; lsp main_loop.rs:33). The doc (db/src/lib.rs:30-35)
  reserves warm reuse to tests "until the clean-vs-incremental parity gate exists," yet
  production warm reuse ships and watch writes warm-emitted bytes to disk and runs them.
  No concrete wrong-byte output demonstrated (resolved strings dominate emission), so a
  smell: stated invariant unenforced + doc claim false as written. LSP never emits →
  unaffected beyond diagnostics.
- reasoning: verified the doc-vs-code drift and the absence of a parity gate. Low correct.
- dup-of: —

## CO-INCR-007 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: any `DidSave`/`DidClose`/`DidChangeWatchedFiles` during a transient
  loader failure (mid-rename, momentary I/O, branch switch). `ensure_project_fresh`
  error arm (main_loop.rs:367-400) degrades to a one-module fallback; `sync_inputs`
  rebuilds `desired` from the shrunken layout (422-454); `publish` clears diagnostics
  for URIs that left the layout (564-593) and the fallback file gains bogus unresolved-
  import diagnostics — a false clean/false red picture until the next edit.
- reasoning: verified the error arm collapses the layout instead of retaining the last
  good one. Low correct (LSP UX correctness; recovers on next edit).
- dup-of: —

## CO-INCR-008 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: an FS batch landing while a source file is mid-write in no-manifest mode
  (or a momentary read failure). watch.rs:687-716 bumps the generation and KILLS
  `cargo_child` BEFORE `resolve_project_sources`; the error arm `continue`s. The
  superseded in-flight build now fails the `g != generation` staleness check and is
  dropped, the almost-done cargo build was already killed, and no new cycle is scheduled
  — the save is lost until the next FS event. INV-3 holds (last-good binary up), but the
  pipeline wedges on a red note that never self-retries.
- reasoning: verified the kill-before-resolve ordering and the non-retrying error arm.
  Low correct.
- dup-of: —

## CO-INCR-009 · CONFIRMED (as correctness/low)
- final severity: low
- reachability: a supervised app (or test run) that writes any file under an in-root
  `tests/` directory while `ipe watch` runs. `is_watchable_leaf` (scope.rs:283-292)
  accepts ANY component named `tests`, any extension, anywhere under root; the exclusion
  list (`target`/`out`/`.ipe`/…) doesn't cover it → write → relevant → rebuild →
  restart → app writes again → loop. `count_source_files` (scope.rs:294-334) also counts
  every file, miscounting assets toward MAX_WATCHED_FILES.
- reasoning: verified the any-component `tests` match with no extension filter. Low
  correct (self-triggering rebuild loop; observe-own-output class).
- dup-of: —

---

Confirmed: 20 (2 crit/0 · high 3 [RT-TUI-001, CO-INCR-001, CO-INCR-002] · med 5
[RT-AUTH-001, RT-AUTH-002, RT-DATA-001, RT-TUI-002, CO-INCR-003, CO-INCR-005] —
6 med, correcting the tally: med = RT-AUTH-001, RT-AUTH-002, RT-DATA-001, RT-TUI-002,
CO-INCR-003, CO-INCR-005 · low 11) · Refuted: 0 · Downgraded: 0 · Dup: 0

Count (canonical): Confirmed: 20 (0 crit / 3 high / 6 med / 11 low) · Refuted: 0 ·
Downgraded: 0 · Dup: 0.

PUSH-BLOCKING candidates:
- CO-INCR-001 (high): request-derived `parts=0` aborts a live server process from one
  request — a reachable remote-triggerable crash in embedded stdlib. Strongest
  push-blocker of the set.
- CO-INCR-002 (high): silent money-minting against the module's own contract in the
  flagship Money module — correctness, not a crash, but ships wrong currency amounts.
- RT-AUTH-001 / RT-AUTH-002 (med each): fail-open acceptance of intended-expired tokens
  on the flat decode surface. NOT a remote auth bypass (signature under the app key is
  required), so not blocking on a security axis, but a real Go-parity correctness
  regression on a credential path — fix before push is strongly advised.
- RT-TUI-001 (high on soundness axis) is local-author-only (no remote party, TUI-only,
  no data/security blast radius) — a real P3 hole worth fixing but NOT push-blocking.
