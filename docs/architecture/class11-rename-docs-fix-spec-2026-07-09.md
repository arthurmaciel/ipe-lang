# Class 11 spec — rename + documentation accuracy (2026-07-09)

> Scope: `docs/architecture/campaign-classification-2026-07-09.md` Class 11,
> items **#75** and **#159** only. **#59** (the full pre-push Sky→Ipê rename)
> is explicitly OUT OF SCOPE for this spec and for this class's execution — it
> runs strictly solo, dead last in the whole campaign, touching every file in
> the repo. Do not schedule it concurrently with anything else, including the
> two items below. Nothing in this document specs #59.

---

## Item #159 — `docs/divergences-from-sky.md` A15 section is stale (doc-only fix)

### Verification performed (read-only)

The backlog claims `docs/divergences-from-sky.md` still describes #94/#95 as
"designed, not yet committed". I re-read the live file
(`docs/divergences-from-sky.md:505-524`, §A15) and confirmed the claim is
**still accurate as of this session** — the doc has NOT been fixed yet:

```
- **#94 (designed, not yet in code.rs):** `check_admissible_msg` — gates Msg
  at `skyc` … Emits the planned `SKY-L0121`. …
- **#95 (designed, not yet committed):** Lambda-aware `fn_param_ty(e, idx)` …
*Note:* `SKY-L0121` (InadmissibleAppMsg) is **designed but not yet in
`code.rs`** — mark as pending-implementation.
```

Cross-checked against the actual code and a live test run — both features are
fully implemented, wired, and regression-tested, and the diagnostic code is
different from what the doc says:

- `crates/sky_backend_rust/src/emit_model_gate.rs:105` — `pub fn
  check_admissible_msg(ctx: &EmitCtx, msg_ty: &IrType, app: AppShape) ->
  DResult<()>` — fully implemented (#94).
- `crates/sky_backend_rust/src/emit_model_gate.rs:46` — `pub fn fn_param_ty(e:
  &Expr, idx: usize) -> Option<&IrType>` handles both `Expr::FuncValue` and
  `Expr::Lambda` — fully implemented (#95).
- Wired from all three app-shape emit sites: `crates/sky_backend_rust/src/
  emit_live.rs:349`, `emit_tui.rs:182`, `emit_webview.rs:137`.
- Diagnostic code is **`SKY-L0125`** (`InadmissibleAppMsg`), not the
  originally-planned `SKY-L0121`. `crates/sky_diagnostics/src/code.rs:204`
  shows `SKY_L0121` was reassigned in the interim to an unrelated gate
  (`JsonDec.succeed`/`Db.Decode.succeed` curry-arity, "constructor arity
  exceeds 10"). `SKY_L0125` is declared at `code.rs:222` (doc comment
  `code.rs:217-221`) and its explain page exists at
  `crates/sky_diagnostics/explain/SKY-L0125.md`.
- Regression suite: `crates/skyc/tests/msg_admissibility.rs`, 7 tests, all
  passing:

  ```
  $ timeout 300 cargo test -p skyc --test msg_admissibility
  running 7 tests
  test live_msg_with_fn_is_rejected ... ok
  test live_lambda_update_with_cmd_msg_is_rejected ... ok
  test live_msg_with_cmd_is_rejected ... ok
  test tui_msg_with_fn_is_rejected ... ok
  test tui_msg_with_cmd_is_rejected ... ok
  test live_html_msg_is_accepted ... ok
  test live_plain_msg_is_accepted ... ok
  test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
  ```

So the backlog's characterization of #159 is correct: the doc genuinely is
stale and genuinely needs fixing (this was **not** already fixed this
session before I started). Proceed with the fix below.

### Exact fix

File: `docs/divergences-from-sky.md`, §A15, lines 505–524 (the `#91`/`#94`/
`#95` bullet list plus the trailing `*Rationale*`/`*Note*` paragraph).

**Replace this block** (`sed -n '505,524p' docs/divergences-from-sky.md` to
confirm exact current text before editing):

```markdown
- **#91 (shipped):** `check_admissible_model` in `emit_model_gate.rs:62` — gates
  Model at `skyc`, emits `SKY-L0120` on a non-serde/non-Clone leaf. Verified:
  `code.rs:198-200`, `emit_model_gate.rs`.
- **#94 (designed, not yet in code.rs):** `check_admissible_msg` — gates Msg
  at `skyc` using `ir_type_is_derivable` for all three app shapes (NOT serde —
  Html is derivable and thus admissible as a Live Msg payload, unlike Live Model).
  Emits the planned `SKY-L0121`. Designed in
  `docs/architecture/seal-gates-msg-lambda-view-design.md §2`.
- **#95 (designed, not yet committed):** Lambda-aware `fn_param_ty(e, idx)` in
  `emit_model_gate.rs:38` — closes the fail-open gap where `view = \m -> …`
  (an `Expr::Lambda`) bypassed the `FuncValue`-only model recovery and silently
  skipped the gate. Designed in §3 of the same doc.

*Rationale:* seal-forced divergence. The Go backend's dynamic path is correct for
Go; the Rust backend's static bounds make the Go-dynamic path a `cargo`-fail.
Gates at `skyc` convert the `cargo`-fail class into a clear user diagnostic.
See `docs/architecture/seal-gates-msg-lambda-view-design.md §4`.
*Note:* `SKY-L0121` (InadmissibleAppMsg) is **designed but not yet in
`code.rs`** — mark as pending-implementation.
```

**With:**

```markdown
- **#91 (shipped):** `check_admissible_model` in `emit_model_gate.rs:142` —
  gates Model at `skyc`, emits `SKY-L0120` on a non-serde/non-Clone leaf.
  Verified: `code.rs:199-201`, `emit_model_gate.rs`.
- **#94 (shipped):** `check_admissible_msg` (`emit_model_gate.rs:105`) — gates
  Msg at `skyc` using `ir_type_is_derivable` for all three app shapes (NOT
  serde — Html is derivable and thus admissible as a Live Msg payload, unlike
  Live Model). Wired from all three emit sites: `emit_live.rs:349`,
  `emit_tui.rs:182`, `emit_webview.rs:137`. Designed in
  `docs/architecture/seal-gates-msg-lambda-view-design.md §2`.
- **#95 (shipped):** Lambda-aware `fn_param_ty(e, idx)` in
  `emit_model_gate.rs:46` — closes the fail-open gap where `view = \m -> …`
  (an `Expr::Lambda`) bypassed the `FuncValue`-only model recovery and
  silently skipped the gate. Shared by the #91 Model gate, the #94 Msg gate,
  and #108's routed-page-field detection. Designed in §3 of the same doc.

*Rationale:* seal-forced divergence. The Go backend's dynamic path is correct
for Go; the Rust backend's static bounds make the Go-dynamic path a
`cargo`-fail. Gates at `skyc` convert the `cargo`-fail class into a clear
user diagnostic. See `docs/architecture/seal-gates-msg-lambda-view-design.md
§4`.
*Note:* `SKY-L0121` was reassigned before #94 landed — it now names the
unrelated `JsonDec.succeed`/`Db.Decode.succeed` curry-arity gate
(`code.rs:202-204`). `InadmissibleAppMsg` ships as **`SKY-L0125`** instead
(`code.rs:217-222`, `explain/SKY-L0125.md`). #91/#94/#95 are all shipped and
regression-tested as of 2026-07-09 (`crates/skyc/tests/msg_admissibility.rs`,
7/7 passing) — none is pending.
```

### Verification commands (after the edit)

```bash
# The doc no longer claims #94/#95 are pending.
rg -n "designed, not yet|pending-implementation" docs/divergences-from-sky.md
# expect: no hits inside the A15 section (grep the whole file — 0 hits total
# is fine unless some OTHER divergence legitimately uses that phrasing; if so,
# confirm by line number that none are in the #94/#95 block).

# Line numbers cited in the new text still resolve to the right functions.
sed -n '105p' crates/sky_backend_rust/src/emit_model_gate.rs   # check_admissible_msg
sed -n '46p'  crates/sky_backend_rust/src/emit_model_gate.rs   # fn_param_ty
sed -n '142p' crates/sky_backend_rust/src/emit_model_gate.rs   # check_admissible_model
sed -n '199,204p;217,222p' crates/sky_diagnostics/src/code.rs  # L0120/L0121/L0125

# Regression suite still green.
timeout 300 cargo test -p skyc --test msg_admissibility
```

### Regression test requirement

None — this is a pure prose fix to an already-correct, already-tested
implementation. No code changes, so no new test is required. The existing
`msg_admissibility.rs` (7 cases) is the test that already proves the doc's
new claim; do not add a duplicate.

---

## Item #75 — `type Color` rename + `RESERVED_BUILTIN_TYPES` addition

### Verification performed — **the backlog's premise is stale; do not execute the literal plan**

The task brief asks to "understand why the rename is needed" and "write the
exact rename plan." Research shows the rename is **no longer needed** — the
naming-collision problem #75 was filed against was already solved by a
different, already-shipped mechanism, and executing #75 literally would
actively regress that mechanism's own regression tests. Full evidence below;
the recommendation is to close #75 as obsolete, not to perform the rename.

**Where #75 came from.** The backlog entry originates from the 2026-07-06
task-board migration (`d0284c5`), where it was carried over verbatim from an
older in-session task list. At that time the concern was real: a future
built-in `Color` type (for `Std.Ui` colours) was expected to collide with any
user- or fixture-defined `type Color`, and the anticipated fix was "rename the
user-facing samples out of the way, then reserve the name" (the same pattern
already used for `Int`/`Task`/`List`/etc. in `RESERVED_BUILTIN_TYPES`).

**What actually landed since, in order:**

1. `fc3455c` (2026-07-03, **before** the backlog migration) — "home-aware
   reorder — program `type Color`/`Length` resolves to own enum, not
   UiPlain (#101)". This changed `ir_type_from_ty`/`ir_type_from_canon` so the
   `(home, name)` `enum_variants` lookup runs **before** the nullary
   `Color`/`Length`/… builtin arms. A program-defined `type Color` (user OR
   stdlib) now resolves to its own enum by construction; only a genuine
   opaque builtin reference (no matching program enum) falls through to
   `UiPlain::Color`. This is the actual fix for the collision #75 worried
   about — and it is the opposite of reservation: it makes the name safe to
   share.
2. `5fe3f7a` (2026-07-03) — "`Std.Css` as compiled-Sky-source module (#47)".
   `Std.Css` **is** the anticipated "future built-in `Color` type" — it ships
   `type Color = Hex String | Rgb Int Int Int | … ` at
   `crates/skyc/stdlib/Std/Css.sky:121-129`, as a real, already-committed
   stdlib ADT. It relies on the #101 home-aware resolution to coexist with
   user code that also declares `type Color`.

**Current state of `crates/sky_canon/src/resolve.rs` (read directly, not from
docs):**

- `RESERVED_BUILTIN_TYPES` (`resolve.rs:64-102`) does **not** contain
  `"Color"` — and the block comment directly above it (`resolve.rs:49-63`)
  explains this is deliberate: `Color`, `Length`, `HAlign`, `VAlign`,
  `Location`, `PseudoClass`, `Description`, `LayoutContext`, `LiveReq` are
  "the nullary Std.Ui / Sky.Live opaque names. Leaving them UNRESERVED is
  what lets a user ADT — and, crucially, a compiled-source `Std.Css` type
  (`Color`/`Length`/…) — declare them", and it names the exact fixtures
  (`m4d_dict_adt_gate`, `m4d_set_adt_fn_gate`, `mm_local_pkg`) as intentional,
  already-working examples.
- `STDLIB_DEFINABLE_UI_TYPES` (`resolve.rs:215-224`, the carve-out that lets a
  trusted `ModuleOrigin::EmbeddedStdlib` module define an otherwise-reserved
  name) also does **not** list `Color` — because `Color` was never moved into
  `RESERVED_BUILTIN_TYPES` in the first place, so no carve-out is needed for
  it. (It lists `Length`/`HAlign`/`VAlign`/`Location`/`PseudoClass`/
  `Description`/`LayoutContext`/`LiveReq` — the same family, same reasoning.)
- Two dedicated golden regression tests exist specifically to prove a user
  `type Color` and the stdlib's own `Color` concept coexist correctly:
  `tests/golden/i101_user_color_hof/Main.sky` and
  `tests/golden/i101_user_color_record/Main.sky`, backed by
  `crates/skyc/tests/golden_i101_color_seal.rs`. Both pass today:

  ```
  $ timeout 300 cargo test -p skyc --test golden_i101_color_seal
  running 2 tests
  test user_color_in_record_field_agrees_across_paths ... ok
  test user_color_via_hof_resolves_to_own_enum ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
  ```

**Consequence.** If `Color` were added to `RESERVED_BUILTIN_TYPES` as #75
literally asks:

- Every fixture below would immediately fail canonicalisation with
  `SKY-N0026 ReservedBuiltinType`, **including** `crates/skyc/stdlib/Std/
  Css.sky` itself (its `type Color` declaration) — unless `Color` were
  *also* added to `STDLIB_DEFINABLE_UI_TYPES`, which would then make the
  "reservation" a no-op for the one module that actually defines the
  builtin, while only blocking ordinary user code from ever writing `type
  Color` again.
- That last effect — permanently forbidding user code from naming an ADT
  `Color` — is a strictly worse outcome than today's behaviour (home-aware
  resolution lets a user's `type Color` and `Std.Css`'s `type Color` both
  work, each resolving to its own enum). It would also delete the very
  regression coverage (`i101_user_color_hof`/`i101_user_color_record`) that
  exists to prove that capability.

**Recommendation: mark #75 OBSOLETE, do not implement the literal plan.**

### Fixture inventory (for the record — also corrects the backlog's count)

The backlog says "5 fixtures + 2 canon tests". A full-repo search for actual
`type Color` **declarations** (not comment mentions) found **10 fixtures**,
not 5, plus the 2 canon unit tests. None of this changes the recommendation
above, but the spec should record the accurate count in case #75 is
revisited later:

| # | File | Line | Role |
|---|---|---|---|
| 1 | `tests/golden/i101_user_color_hof/Main.sky` | 15 | #101 regression — user `Color` through HOF/inferred path |
| 2 | `tests/golden/i101_user_color_record/Main.sky` | 12 | #101 regression — user `Color` in a record field |
| 3 | `tests/golden/i130_enum_capture/Main.sky` | 15 | unrelated feature test, `Color` used as a convenient sample enum |
| 4 | `tests/golden/i136_alias_catchall/Main.sky` | 6 | unrelated feature test (alias catch-all), same |
| 5 | `tests/golden/i136_underscore_alias/Main.sky` | 6 | unrelated feature test (underscore alias), same |
| 6 | `tests/golden/m4d_dict_adt_gate/Main.sky` | 6 | Dict-ADT-key gate test, `Color` as sample non-comparable ADT |
| 7 | `tests/golden/m4d_dict_adt_fn_gate/Main.sky` | 14 | same, forwarder-function variant |
| 8 | `tests/golden/m4d_set_adt_fn_gate/Main.sky` | 12 | Set-ADT-key gate test, same pattern |
| 9 | `tests/golden/mm_local_pkg/src/Lib.sky` | 5 | multi-module test — local package re-export of `Color(..)` |
| 10 | `tests/golden/mm_neg_ambigctor/src/ModA.sky` | 2 | multi-module negative test — ambiguous-ctor-across-modules |

Plus 2 canon unit tests (`crates/sky_canon/src/lib.rs`):

- Line 1449, `alias_to_local_union_preserves_home` — `type Color = Red |
  Green` is an arbitrary sample union used to test that a `type alias`
  pointing at a local union preserves the union's home. `Color` is
  incidental; any other name would serve identically.
- Line 1613, `alias_colliding_with_a_union_is_a_duplicate_type` — `type Color
  = Red` likewise incidental, testing `DuplicateType` detection between a
  union and a colliding alias.

Only fixtures **1** and **2** (the `i101_user_color_*` pair) are actually
*about* the Color-naming question; the rest use `Color` as an arbitrary
throwaway ADT name for unrelated features and could be renamed with zero
semantic loss — but renaming them buys nothing either, since they are not in
anyone's way. `crates/skyc/stdlib/Std/Css.sky` (the real, shipped stdlib
`Color` type) is not a fixture and must never be renamed regardless of what
happens to #75 — it is the production definition the whole #101/#103 design
exists to protect.

### Recommended action (replaces the literal rename plan)

1. **Do not** add `"Color"` to `RESERVED_BUILTIN_TYPES`.
2. **Do not** rename any of the 10 fixtures or 2 canon tests above.
3. **Update `docs/architecture/backlog.md`** — change the `#75` line (in the
   `## Rename (pre-push, per memory pre-push-rename-sky-to-ipe)` section)
   from:

   ```markdown
   - **#75** Rename `type Color` → `Swatch` in 5 fixtures + 2 canon tests, then add `Color` to `RESERVED_BUILTIN_TYPES`.
   ```

   to:

   ```markdown
   - **#75** ✅ **OBSOLETE, closed without code changes (2026-07-09).** Filed
     before `fc3455c`/`5fe3f7a` (#101/#47) landed the home-aware `(home,
     name)` resolution + `Std.Css` compiled-source module. The anticipated
     collision between a future built-in `Color` and user/fixture `type
     Color` declarations is already solved by that mechanism (a
     program-defined `type Color`, user or stdlib, resolves to its own enum;
     `RESERVED_BUILTIN_TYPES` deliberately excludes `Color` — see
     `resolve.rs:49-63`). Adding `Color` to `RESERVED_BUILTIN_TYPES` now
     would break `Std.Css`'s own `type Color` (`crates/skyc/stdlib/Std/
     Css.sky:121`) and would delete the regression coverage proving user/
     stdlib `Color` coexistence (`golden_i101_color_seal.rs`, 2/2 green).
     Rename plan and 10-fixture/2-canon-test inventory preserved in
     `docs/architecture/class11-rename-docs-fix-spec-2026-07-09.md` in case
     this is ever revisited under different premises.
   ```

4. Move the item out of the `## Rename` section entirely (it's the only
   thing keeping that section alive besides #59) — either delete the line
   after archiving it in this spec, or leave the "closed" line as a durable
   marker so nobody re-files the same misunderstanding. Prefer leaving the
   marker: `backlog.md`'s own preamble says "update as items land," and a
   silently-deleted line invites re-discovery of the same stale premise from
   old notes/memory.

### Verification commands

```bash
# Confirm Color really is deliberately absent from RESERVED_BUILTIN_TYPES today.
sed -n '64,102p' crates/sky_canon/src/resolve.rs | rg -n '"Color"'
# expect: no output (0 hits) — Color is NOT in the list.

# Confirm Std.Css really does define type Color as shipped stdlib source.
sed -n '120,129p' crates/skyc/stdlib/Std/Css.sky

# Confirm the #101 collision regression tests are green (no code touched).
timeout 300 cargo test -p skyc --test golden_i101_color_seal

# Confirm the full canon test suite (including the 2 incidental Color tests)
# is green — again, no code touched, this is a pre-existing-green check.
timeout 600 cargo test -p sky_canon
```

### Regression test requirement

None — this is a backlog-bookkeeping fix (mark #75 obsolete with a documented
rationale), not a code or behavior change. No new test is required; the
existing `golden_i101_color_seal.rs` (2 cases) and the incidental canon tests
already cover everything #75 could plausibly have wanted covered.

If a future session decides the collision concern should be revisited under
*different* premises (e.g. a brand-new top-level, non-`Std`-namespaced
built-in named `Color` that isn't home-scoped the way `Std.Css`'s is), that
is a new design question — not a resurrection of #75's literal rename plan,
which this investigation shows would be a regression against already-shipped,
tested behaviour.

---

## Ordering note (from the campaign classification)

Per `campaign-classification-2026-07-09.md`, Class 11's #75+#159 are listed
as parallel-safe within the "mechanical wave" (alongside Classes 7/8/9/2).
Both items in this spec are pure-documentation changes touching only
`docs/divergences-from-sky.md` and `docs/architecture/backlog.md` — they do
not touch `sky_canon`/`sky_lower`/`sky_backend_rust` source, so they carry no
merge-conflict risk against any other Class's concurrent lane. **#59 remains
excluded from all of this** and must not run concurrently with anything,
per the classification doc and per memory `pre-push-rename-sky-to-ipe`.
