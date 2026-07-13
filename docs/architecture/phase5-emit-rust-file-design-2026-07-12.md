# Phase 5 residual — `emit_rust_file(RustFileId)` design spec (2026-07-12)

> **Status: DESIGN SPEC ONLY. Nothing in this document is implemented.**
> Downstream of `docs/architecture/salsa-incremental-compilation-2026-07-11.md`
> §10.2 ("Task 15 — `emit_rust_file(RustFileId)`: genuinely blocked, not
> forced"), which this doc does NOT reopen or re-litigate — it is the
> authoritative statement of *why* the gap exists. This doc answers the
> question §10.2 left open: **what is the concrete, buildable design that
> closes it**, staged so THE SEAL (`skyc` exit-0 ⇒ `cargo build` succeeds)
> never has a half-split intermediate state, and the 155-file golden-oracle
> suite is migrated rather than broken.
>
> **Principles order (hard, unchanged):** security > correctness > soundness
> > efficiency > completeness > readability. Every design choice below is
> justified against this order when it trades one property for another.

## Revision log

**2026-07-12 (same-day revision).** This revision addresses 4 findings from
an independent adversarial review, re-verified by hand against `HEAD`
before editing (not taken on the review's word alone):

1. **SqlValue/SqlField's `home` was misstated** (§2.2 said the lowerer sets
   it "to the entry module"; it is actually the empty `ModPath` — a
   documented Prelude built-in) **and the interaction was unverified.**
   Resolved by choosing option (a) — `partition_items` now force-routes
   these two synthetic enums to `Spine` by name, structurally, rather than
   letting them fall through the generic empty-home fallback. See the
   revised §2.2, the revised Task 4 (partition function), the revised
   Task 5 (route `emit_program`, now with an explicit ordering-
   preservation rule for the DB-using goldens), and Task 11's pilot
   fixture, now extended to exercise `Std.Db` alongside the multi-module
   split.
2. **`RecordStruct` names were never cross-checked against
   `enum_names`/`func_names`**, a pre-existing gap whose failure mode the
   glob barrel turns from a loud `E0428` into a silent shadow. Resolved by
   a new Task 3, folding `record_struct_name` into a shared type-namespace
   collision registry with the enum/func/mod-ident names, fail-closed via
   `Diagnostic::Name::DuplicateValue`, landed BEFORE the partition function
   (Task 4) so "the flat namespace is sound" is true before Milestone A
   proceeds.
3. **The coverage gate (formerly Task 8, now Task 9) was a negative
   heuristic**, not a positive structural proof. Rewritten to assert every
   `golden_*.rs` file (except a small, explicitly-named allowlist of
   exit-0/substring-only tests that never did a byte-diff assert in the
   first place — verified by hand, not guessed) calls the shared
   `support::assert_emitted_project_matches_golden_dir` helper.
4. **Task 7 (now Task 8) was mis-sized** — one task standing in for ~6
   batches of real work. Split into Task 8a (one hand-migrated file +
   script authoring) and Task 8b–8g (six ~23-24-file batches), each ending
   in its own commit, so a session-budget interruption has an unambiguous
   stopping point.

Every task after Task 2 shifts by at least one position; every
cross-reference (decisions ledger, milestone summaries, the proof-test
inventory) is updated in place. References to task numbers from the
**other**, historical design doc (`salsa-incremental-compilation-2026-07-11.md`
§10.2/§10.4/§10.5, and the "original 26-task plan") are a different
numbering scheme and are deliberately left untouched.

---

## 1. Re-survey against HEAD — confirming, refining, and adding to §10.2

§10.2 was written from a read of `sky_backend_rust::project::emit_program`
and `sky_lower::lower`. This section re-verifies that survey against the
current `HEAD` (`crates/sky_backend_rust/src/{project,lib}.rs`,
`crates/sky_ir/src/ir.rs`, `crates/sky_backend/src/lib.rs`,
`crates/sky_db/src/lib.rs`, `crates/skyc/src/lib.rs`,
`crates/skyc/tests/golden_*.rs`) and records **three findings that
materially change the shape of the fix**, plus two that confirm §10.2
verbatim. Nothing has drifted since 2026-07-11; these are refinements, not
corrections.

### 1.1 CONFIRMED — `sky_lower::lower` still produces exactly one `sky_ir::Module`

`sky_db::lower_program` (`crates/sky_db/src/lib.rs:729`) still calls
`sky_lower::lower(&linked.module, &types, &mut interner)` on the SINGULAR
`linked.module` — `Program { modules: vec![module] }`, unchanged since Phase
4 §9.1. There is still no `program_ir_module(ModuleId)` salsa domain.

### 1.2 NEW — origin tracking SURVIVES lowering, via `home: ModPath` on every item

This is the load-bearing finding of this spec, and it refines §10.2's
finding (1) rather than contradicting it. `sky_ir::Func` (`crates/sky_ir/src/
ir.rs:413-423`) and `sky_ir::EnumDef` (`crates/sky_ir/src/ir.rs:152-163`)
each carry a `home: ModPath` field, with an explicit doc comment on `Func`:

> "After `link::link` merges several modules into one this field retains the
> original source module path, so the backend can prefix the emitted Rust
> function name with the correct module segment... instead of always using
> the merged entry module's name."

The backend already USES this field today, for naming: `EmitCtx::enum_names:
BTreeMap<(ModPath, Symbol), String>` and `EmitCtx::func_names:
BTreeMap<FuncId, String>` are built by walking `program.modules[..].types /
.funcs` and keying/naming off `def.home` / `func.home`
(`crates/sky_backend_rust/src/lib.rs:293-397`) — e.g. `(["Std","Palette"],
Shade)` → `StdPaletteShade`, `update` in `["Lib"]` → `lib_update`.

**Consequence: §10.2's finding (1) — "no `program_ir_module(ModuleId)` to
key `emit_rust_file`'s `owner` dependency on" — is true as stated (that
salsa domain genuinely does not exist) but it is NOT a precondition for a
per-file EMIT split.** The task prompt that commissioned this spec asked
this exact question and the answer is confirmed by reading the code: a
per-file split can be built by **partitioning the ONE already-lowered
`Module`'s `types`/`funcs` by each item's own `home` field, entirely at the
emit layer**, with zero change to `sky_lower` or a new IR domain. This
downgrades Task 15 from "needs a lowering redesign" (which would have been
a Phase-4-continuation-sized effort) to "needs an emit-layer partition
function + a file-writing redesign" — still real work (§2), but a
qualitatively smaller and safer slice.

### 1.3 NEW — names are ALREADY globally mangled and collision-checked; visibility is smaller than it looks

`naming::enum_name` / `naming::module_value` (`crates/sky_backend_rust/src/
naming.rs:180-191`) fold `(home, name)` to a flat, home-qualified Rust
identifier (`main_update`, `LibColor`, `StdPaletteShade`). Critically, this
folding is **not claimed to be injective** — the code says so explicitly:

> "AUD-08: `naming::module_value`'s snake_case fold is not injective over the
> (home, name) split — `["Std","Ui"]/borderRounded` and
> `["Std","Ui","Border"]/rounded` both fold to `std_ui_border_rounded`... Fail
> closed with the same duplicate-value diagnostic rather than emit a broken
> crate." (`crates/sky_backend_rust/src/lib.rs:378-394`, mirrored for enums
> at `lib.rs:~325`)

Both the func-name and enum-name builders already run
`func_names.values().any(|n| n == &rust_name)` /
`enum_names.values().any(...)` and fail closed with
`Diagnostic::Name::DuplicateValue` (a `skyc`-time, not `cargo`-time, error)
the moment two *distinct* items would mangle to the same Rust identifier.

**Consequence:** the pre-existing single-file backend ALREADY depends on
global name uniqueness — a flat Rust namespace requires it — and ALREADY
enforces it at `skyc` time with a fail-closed gate. §10.2's sub-problem (a)
("reintroduces a whole visibility design") is therefore not "design
`pub`/`use` per item from scratch" — it is "decide which file each
already-globally-unique-named item is DECLARED in, and make it visible to
every other file." Because names are unique, a single flat glob re-export
barrel is a sound mechanism for the second half (§2.1) — no per-item
`pub`/`use` graph is needed, and the existing duplicate-name gate is exactly
the invariant that keeps a glob-reexport free of Rust's `E0659` ambiguous-
glob error (two conflicting glob-imported items sharing one name).

### 1.4 CONFIRMED — `record_structs` is genuinely home-less; ownership needs an explicit new rule

`RecordStruct` (`crates/sky_backend_rust/src/lib.rs:115-153`) carries no
`home`/origin field. It is built from `shapes`, a **program-wide**
first-occurrence-order map (`record_by_fieldset`,
`crates/sky_backend_rust/src/lib.rs:521-565`) keyed purely by the
canonicalised field-name set — `naming::record_struct_name(&key)` derives
the Rust name from the SHAPE, not from any originating Sky module. §10.2's
sub-problem (b) is real, not speculative: a synthesised record struct has no
single natural "owning" Sky module by construction, because the whole point
of the table is cross-module deduplication. §2.2 resolves this with an
explicit, principle-ranked decision.

### 1.5 NEW — the emit→cargo bridge is ALREADY file-count-agnostic; nothing there needs to change

`EmittedProject.files: BTreeMap<RelPath, String>` (`crates/sky_backend/src/
lib.rs:142-147`) already holds 3 entries today (`src/main.rs`,
`src/sky_runtime/mod.rs`, `src/sky_runtime/config.rs`) and Phase 5 §10.3 /
§10.5 already recorded, and Task 16's own decisions ledger confirms, that
`build_emit_manifest` / `reconcile_emitted_project` / `prune_orphaned_files`
(`crates/skyc/src/lib.rs:826-935`) are already generic over the file count —
"Task 16 needed zero backend changes... the manifest shape is agnostic to
how many files it contains" (§10.5 item 4). Re-verified against current
`HEAD`: still true, no code has changed there. **Consequence: this spec's
job is entirely upstream of the bridge** — make `emit_program` (or its
salsa-wrapped successor) populate MORE keys in `EmittedProject.files` when
appropriate; the write/reconcile/prune machinery needs no design work.

### 1.6 CONFIRMED — the golden-oracle SEAL is a real, simultaneous, 155-test blocker

`ls crates/skyc/tests | grep -c '^golden'` returns **155** (the doc's "140+"
estimate is accurate, slightly conservative). Every one of them duplicates
its own `repo_root()`/`ldir()` helper and does a direct
`assert_eq!(read_to_string(out.join("src").join("main.rs")),
read_to_string(golden.join("main.rs")))` (representative:
`crates/skyc/tests/golden_m0.rs:34-41`). A SECOND, independent oracle exists
at the crate level: `crates/sky_backend_rust/tests/golden.rs` builds a
`Program` by hand and asserts `RustBackend::emit` reproduces
`GOLDEN_MAIN`/`GOLDEN_CARGO` byte-for-byte. **Both harnesses hard-code the
single-path assumption** (`out/src/main.rs`, one string). Splitting the file
boundary breaks all 156 test bodies' assertion shape at once, confirming
§10.2's own framing: "a breaking change to the golden-oracle SEAL itself,
not an additive one." §2.4 gives the concrete migration mechanism the task
brief asked for.

### 1.7 CONFIRMED — `sky_backend_rust` is `#![forbid(unsafe_code)]`, workspace deny-lints apply

`crates/sky_backend_rust/src/lib.rs:1` is `#![forbid(unsafe_code)]`. The
crate inherits `[workspace.lints.clippy]`'s deny table (`unwrap_used`,
`expect_used`, `panic`, `indexing_slicing`, `unreachable`, `todo`,
`unimplemented`, `pedantic`, `nursery`). Every new function this spec
proposes (§2, §4) must be written against that gate: `BTreeMap`/`.get()`
lookups with typed fallbacks, no `.unwrap()`/`.expect()`, exhaustive
`match`es on `RustFileId` and any new enum, `DResult<T>` return types
threading `Diagnostic::CompilerBug` for internal-invariant violations
exactly as the existing anchor-assertion code in `project.rs` already does
(`anchor_missing`, the G3 `block_on` replace-or-fail pattern).

---

## 2. Design — closing all four sub-problems

### 2.1 (a) The mod/visibility scheme — "Spine" + per-module files + a flat glob barrel

**Decision.** Introduce a two-tier file taxonomy instead of the naive "N
Sky modules → N Rust files, symmetrically":

- **`Spine`** — exactly one file, `src/main.rs`. Holds everything that is
  program-wide rather than module-owned: the preamble, the fixed
  kernel-wrapper prelude (`runtime_bindings()`), the TEA/Auth type-alias
  blocks, **all synthesised record structs** (§2.2), **the DB
  boundary-projection impl blocks** (§2.3), the epilogue (Ffi-kernel
  polyfill, list helpers), and `fn main()`. This is the exact same content
  `emit_program` writes today MINUS the per-module `types`/`funcs` loops
  (`project.rs:342-347` and `:383-387`).
- **`SkyModule(ModPath)`** — one file per DISTINCT `home` observed across
  `program.modules[..].types` ∪ `.funcs` (using the SAME `effective_home`
  fallback the naming layer already uses for empty `home` — see §2.1.1),
  written to `src/sky_mods/<mod_ident>.rs`. Holds exactly that module's
  `EnumDef` and `Func` declarations, each declared `pub(crate)` (not bare
  top-level as today, since they now live in a `mod` block).

**Cross-file visibility mechanism.** `main.rs` gains, for every distinct
`SkyModule(home)` present:

```rust
mod sky_mod_std_palette;
pub(crate) use sky_mod_std_palette::*;
mod sky_mod_lib;
pub(crate) use sky_mod_lib::*;
```

and every per-module file opens with `use crate::*;`. Because (i) every
name is already globally unique (§1.3) and (ii) `main.rs` re-exports every
module's items at the crate root via the barrel, `use crate::*;` inside
`sky_mod_lib.rs` sees: everything declared directly in `main.rs` (Spine
content — record structs, aliases, kernel wrappers) AND everything
re-exported from every OTHER `sky_mod_*` file, with zero per-symbol
bookkeeping. A module's `use crate::*;` picking up its OWN already-locally-
declared items via the glob is harmless — Rust's name-resolution precedence
(local definition > explicit `use` > glob `use`) means the local items
always win, never a conflict.

**Why this is sound, not just convenient (principles order).** This is
NOT "a real per-module unqualified-namespace design" in the rust-analyzer
sense — it is deliberately the SMALLEST correct mechanism given the
flat-mangled-name invariant this backend ALREADY relies on for its
single-file output. A hand-rolled selective `pub`/`use` graph (computing
exactly which items module B's functions reference and importing only
those) would be strictly more Rust-idiomatic but is unjustified extra
complexity here: it is a NEW analysis pass with its OWN soundness
obligation (miss one reference → `E0433` unresolved-name → SEAL violation
at `cargo` time, not `skyc` time), buying nothing the glob barrel doesn't
already buy for free, given names are already unique. Under
efficiency-vs-soundness (efficiency ranks below soundness), the simpler,
already-proven-sound mechanism wins.

**2.1.1 The `mod_ident` naming gate — a NEW namespace, needs its OWN
fail-closed uniqueness check.** Unlike func/enum names (§1.3), a
`ModPath → Rust mod identifier` folding is new surface this task
introduces — nothing today needs one, because there is only one file.
Reuse the SAME base fold `naming.rs` already has for the value/type case
(`module_prefix`, `crates/sky_backend_rust/src/naming.rs:81-83`, currently
private — promote to `pub(crate)` or add a thin `mod_ident(home: &[&str]) ->
String` wrapper), producing e.g. `["Std","Palette"]` → `sky_mod_std_palette`
(snake_case, `sky_mod_` prefix to keep it visually distinct from
`sky_runtime`). **Mirror the exact pattern already established for
func/enum names**: build a `BTreeSet<String>` of every `SkyFileId`'s
`mod_ident` and fail closed with a NEW, precisely-named diagnostic
(`Diagnostic::Name::DuplicateValue` is reusable verbatim — same shape,
different `name` — no new `Diagnostic` variant needed) the moment two
DISTINCT `home`s fold to the same identifier. This closes THE SEAL for the
one genuinely new namespace this design introduces.

### 2.2 (b) Record-struct ownership — fixed to Spine, not module-partitioned

**Decision.** Every `RecordStruct`, the DB boundary-projection impls, and
the `SqlValue`/`SqlField` enum declarations those impls project onto are
emitted in `Spine`, never split by module.

**Correction to an earlier draft of this section (independent-review
finding).** `SqlValue`/`SqlField` do NOT carry "a home set by the lowerer
to the entry module." `synthetic_sqlvalue_enum` / `synthetic_sqlfield_enum`
(`crates/sky_lower/src/lower.rs:4023`, `:4094-4118`) set
`home: ModPath(Vec::new())` on both — the SAME empty canonical home the
code's own doc comment calls out explicitly: "`SqlValue` is a Prelude
built-in (not a user `type`): its constructors carry the empty canon
home... The backend's empty-home→entry-module naming fallback reproduces
the pre-#100 Rust name byte-for-byte." That "empty-home→entry-module
fallback" is real, but it lives in `partition_items` (§2.1, Task 4) — it is
the SAME fallback Task 4's own test exercises for generic hand-built IR
with an empty `home`. Left unpatched, that generic fallback would route
`SqlValue`/`SqlField` into whichever `SkyModule(entry)` bucket the program's
entry-point module happens to own once 2+ distinct `home`s exist — a
`SkyModule` file, not `Spine` — silently contradicting this section's own
decision text. The fixture that would have caught this (Task 10, now
Task 11) had no `Std.Db` usage, so the interaction shipped unverified.

**Fix (option (a) — force `Spine`-routing inside `partition_items`,
chosen over option (b) — leave the fallback and document + test it).**
`partition_items` (Task 4) special-cases these two enums BY NAME, before
the generic empty-home fallback runs: an `EnumDef` whose interner-resolved
name is exactly `"SqlValue"` or `"SqlField"` is inserted into the
`RustFileId::Spine` bucket unconditionally, never into a `SkyModule`
bucket. This reuses the exact detection idiom the backend already applies
elsewhere for these two names (`uses_db`'s scan at
`crates/sky_backend_rust/src/lib.rs:571-576`, and the
`sqlvalue_rust_name`/`sqlfield_rust_name` lookups at `:577-606` — both
match on `interner.resolve(def.name) == Some("SqlValue" | "SqlField")`), so
it is not new machinery, just a new call site for an established pattern.
See Task 4 for the exact code location and the ordering rule this
introduces for Milestone A's byte-identical proof.

**Why (a), not (b), under the principles order.** Option (b) — leave
`partition_items`'s generic fallback in place, correct this section's prose
to say the enums live in the entry module's `SkyModule` file, and add a
golden fixture proving that's still sound — was rejected. It is not
unsound (the review confirms it compiles via the glob barrel), but it
reintroduces, for a SECOND time in this same section, the exact
usage-dependent instability §2.2's own decision below already rejects for
`RecordStruct`s: which file `SqlValue`/`SqlField` declare in would depend
on which module happens to be the *entry point* of a given build, an
accident of invocation rather than a property of the type itself — and it
would leave the DB-projection impl blocks (fixed to `Spine`, unconditionally,
by this section's own decision) referencing a type declared in a DIFFERENT,
program-dependent file, for no reason connected to what the type IS.
Structurally guaranteeing `Spine`-placement inside `partition_items` costs
one small, well-scoped `partition_items` special case (documented exactly
in Task 4) and makes "these two Prelude built-ins live in Spine" true by
construction rather than true by convention + a regression test — the
"make invalid states unrepresentable" rule from `PRINCIPLES.md` applied
directly. Correctness/soundness rank above the completeness/efficiency
argument for option (b) (skip the `partition_items` change, just test it),
so (a) wins. Task 11's pilot fixture is ALSO extended to exercise
`Std.Db` alongside the multi-module split (not merely documented as sound)
— belt-and-braces, since even a structurally-guaranteed placement benefits
from one concrete build-and-run proof before Milestone C is considered
proven, matching this project's own TDD discipline throughout §5.

**Why record structs specifically (principles order: soundness/correctness
before efficiency).** Two options were considered for record-struct
ownership:

1. **Fixed home (Spine)** — chosen. A record shape shared by two modules
   has exactly one stable location, always, regardless of which modules
   currently construct it. A body-only edit to module A that changes which
   *shapes* A constructs can only ever touch `Spine`'s content (in addition
   to `SkyModule(A)`'s own content) — it can never move a struct's
   declaration into or out of some OTHER unrelated module's file.
2. **First-occurrence-owner (assign a shape to whichever module first
   constructs it)** — rejected. This reintroduces exactly the ownership
   instability §10.2 flagged as unresolved: if module A stops using a
   shared shape while module B still does, the shape's file would migrate
   from A's to B's file across builds — an edit to module A that has
   NOTHING to do with module B's own code would still change B's emitted
   file's *set of declarations* (though not B's own functions), a
   correctness-adjacent instability with no compensating benefit, since
   the glob barrel (§2.1) already makes any struct visible everywhere
   regardless of which file declares it. The only claimed win — "slightly
   finer diffing" — is an efficiency concern, and efficiency yields to
   soundness/correctness under the principles order when the two conflict.

This is recorded as a **documented, deliberate divergence from finest-
possible granularity**, matching this project's own established pattern of
shipping the sound floor and recording a finer refinement as explicitly
out-of-scope (Phase 5 §10.5 item 2 did the identical thing for
`reachable_types`). A future session MAY revisit first-occurrence ownership
if profiling ever shows Spine-file churn dominating incremental-build cost
at Stripe-SDK scale (76k symbols) — not attempted here.

### 2.3 (c) Relocating the fixed anchors — they do not relocate

**Decision.** The kernel-wrapper prelude, TEA/Auth alias blocks, epilogue
(Ffi-kernel polyfill, list helpers), and `fn main()` are ALREADY
program-wide, never per-Sky-module, content. Under the `Spine`/`SkyModule`
split (§2.1) they simply... stay exactly where they are, in `main.rs` —
`Spine` IS `main.rs`, and it is the "spine" precisely because it is what
survives once the module-owned `types`/`funcs` loops are extracted. There
is no relocation design needed; §10.2's framing of this as a distinct
sub-problem is resolved as a consequence of the `Spine`/`SkyModule` split,
not by any separate mechanism.

One concrete, small, positive side effect: the G3 Webview main-thread-entry
anchor replace (`project.rs:404-420`, `out.replacen(BLOCK_ON_ANCHOR,
BLOCK_ON_THREAD_REPLACEMENT, 1)`) currently scans the ENTIRE concatenated
`main.rs` string (preamble + every module's types/funcs + epilogue) for its
anchor. Under this design it only needs to scan `Spine`'s text (the anchor
lives in the epilogue, which is Spine-only) — a strictly smaller haystack,
marginally safer (lower chance of an accidental anchor-text collision
inside emitted user code) and free (no new code, just a smaller input to
the same `replacen` call once `Spine`'s text is a distinct `String` from
the module files').

**The "must be visible to every subsequent function body" ordering
constraint is retired.** `project.rs`'s current doc comment on the
DB-projection-impls placement ("emitted immediately after the user types...
so they are visible to every subsequent function body") is a single-file
top-to-bottom-visibility artifact. Real Rust modules are order-independent
within a crate — once Spine and every `SkyModule` file are glob-visible to
each other (§2.1), this ordering constraint is void. Worth noting in the
implementation comments as a now-historical constraint, not something that
needs to be preserved.

### 2.4 (d) Golden-suite migration — an additive directory-diff harness, landed BEFORE any real split

This directly answers the task brief's ask for "a NEW variant of the
golden-test harness that diffs a WHOLE DIRECTORY tree instead of one file,
landing that harness change FIRST as its own safe, additive step."

**Mechanism, concretely.** Add ONE shared helper (not 155 individually
hand-rolled ones):

```rust
/// crates/skyc/tests/support/mod.rs (extended)

/// Assert every file `emitted` claims (`files` + the root `Cargo.toml`)
/// matches its counterpart under `golden_dir` byte-for-byte, AND that
/// `golden_dir` contains no *additional* `.rs`/`Cargo.toml` file `emitted`
/// does not claim — catches both under-emission (a missing file) and
/// over-emission (a stray file) symmetrically, mirroring `prune_orphaned_
/// files`'s own manifest-is-authoritative discipline one layer up, in the
/// TEST harness rather than the driver.
///
/// `golden_dir` may contain OTHER, non-emitted fixture files (`Main.sky`,
/// `expected_go.txt`, `oracle.meta`) — the walk is scoped to exactly the
/// relative-path KEYS `emitted` declares (`src/**/*.rs` + root
/// `Cargo.toml`), never a blind recursive diff of the whole directory, so
/// adding an unrelated fixture file to a golden dir can never be
/// misread as a missing-emit failure.
pub fn assert_emitted_project_matches_golden_dir(
    emitted: &sky_backend::EmittedProject,
    golden_dir: &std::path::Path,
) {
    let mut mismatches = Vec::new();
    for (rel, want_text) in &emitted.files {
        let want_path = golden_dir.join(rel.as_str());
        match std::fs::read_to_string(&want_path) {
            Ok(golden_text) if &golden_text == want_text => {}
            Ok(golden_text) => mismatches.push(format!(
                "{}: emitted != golden ({} vs {} bytes)",
                rel.as_str(), want_text.len(), golden_text.len()
            )),
            Err(e) => mismatches.push(format!(
                "{}: golden file missing or unreadable at {}: {e}",
                rel.as_str(), want_path.display()
            )),
        }
    }
    let want_cargo = golden_dir.join("Cargo.toml");
    match std::fs::read_to_string(&want_cargo) {
        Ok(golden_cargo) if golden_cargo == emitted.cargo_toml => {}
        Ok(golden_cargo) => mismatches.push(format!(
            "Cargo.toml: emitted != golden ({} vs {} bytes)",
            emitted.cargo_toml.len(), golden_cargo.len()
        )),
        Err(e) => mismatches.push(format!("Cargo.toml: golden missing: {e}")),
    }
    assert!(mismatches.is_empty(), "golden mismatch:\n{}", mismatches.join("\n"));
}
```

**Staged landing (mirrors the "gate first" discipline this project already
uses — Phase 3 §8.4 decision 1):**

1. Add the helper. It has zero callers — dead code is acceptable for ONE
   commit because the very next step gives it a real caller (unlike the
   "reserved input, zero consumers" anti-pattern Phase 1 §3.2 names, which
   is about *design surface left indefinitely unconsumed*, not a
   same-session two-step land).
2. Add it as a SECOND, additional assertion inside the EXISTING
   `golden_m0.rs`'s `emits_byte_identical_main_rs_and_vendors_runtime` test
   (call both the old `assert_eq!` AND the new helper, side by side) — proves
   the new helper has IDENTICAL discriminating power to the old assertion
   on TODAY's single-file output, with a real (not hypothetical) green run.
3. Delete the OLD hand-rolled assertion from that ONE test, leaving only the
   shared helper — now the test is strictly shorter and behaviourally
   identical.
4. Migrate the remaining 154 golden tests in reviewable batches (a scripted,
   mechanical `sed`/codemod pass is appropriate here — the transformation is
   syntactically uniform: replace each file's ad hoc `read_to_string` +
   `assert_eq!` pair with one call to the shared helper, and delete that
   file's now-redundant local `ldir`/`repo_root` duplicate in favour of
   `support::repo_root`). Land in batches (e.g. 20-30 files per commit) so
   review stays tractable and any one batch's regression is easy to bisect.
5. Add a COVERAGE gate: a new test (`crates/skyc/tests/golden_harness_
   coverage.rs`) that greps `crates/skyc/tests/golden_*.rs` for the
   retired ad hoc pattern (`out.join("src").join("main.rs")` compared via
   `assert_eq!` outside `support::`) and fails if any instance remains —
   makes "migration complete" a machine-checked property, not a claim.

**Only after step 5 is green does any REAL per-file split happen — and even
then, first against ONE pilot fixture** (§3, Milestone C), never a
blanket change across all 155 goldens simultaneously. Every existing
**single-home** golden (one user module importing only kernel modules — the
vast majority) needs **zero golden-file changes** at all, by construction
(the Spine-collapse invariant, §3.3). The originally-claimed "154 of the 155,
everything except the new pilot" was a FALSE premise: ~6 tests / 5 binaries
are genuinely multi-home and legitimately split alongside the pilot — see the
corrected §3.3 blast-radius table and §5 Task 13.

---

## 3. Staged rollout — four milestones, each independently landable and testable

This section is the "small, quick steps" backbone; §5 turns it into exact
TDD tasks. High-level shape first.

### 3.1 Milestone A — `RustFileId` + partition function, behaviour-preserving

Introduce the `RustFileId` type and the `home`-keyed partition function
INSIDE `sky_backend_rust`, and re-route `emit_program`'s existing
single-file concatenation THROUGH it — output stays byte-identical for
every existing golden. This is pure internal refactor risk (compile-time
and unit-test provable), zero golden-file risk.

### 3.2 Milestone B — the directory-diff harness (§2.4), landed and adopted

Exactly the 5-step sequence in §2.4. Zero backend changes. Zero golden-file
changes. Pure test-infrastructure risk.

### 3.3 Milestone C — the real split, ONE pilot fixture, with the Spine-collapse invariant

> **DESIGN-PREMISE CORRECTION (2026-07-13, Task 13 execution).** This
> section's original text asserted the Spine-collapse invariant would fire
> for "every existing golden today, since none of the 155 fixtures are
> deliberately multi-module at the Rust-emission-relevant level," and that
> the Milestone C blast radius was therefore ZERO existing goldens (only the
> new pilot). **That premise was FALSE and is corrected in place below.**
> Executing Task 13 (the machine-checked blast-radius gate) revealed that
> **6 tests across 5 golden binaries are genuinely multi-home** and the
> per-Sky-module split CORRECTLY fires for them — the actual blast radius is
> ~6 goldens, not zero. Two classes were missed by the original survey: (a)
> genuine USER multi-module fixtures (`mm_diamond` = B/C/D/Main, `mm_local_pkg`
> = Lib/Main, `class1_boundary_scheme_field_result` = Lib1/Lib2/Main), each
> carrying 2+ distinct user `home`s; and (b) programs importing a Layer-3
> **stdlib** module compiled to Sky source (`Std.Css`, `Std.Ui.Grid`,
> `Std.Ui.Transition`) — the stdlib module carries its OWN `home` distinct
> from `Main`, so `partition_items` sees 2 distinct `SkyModule` homes and the
> split fires. **The decision on discovering this was to ACCEPT the wider-
> but-correct blast radius, NOT to narrow the split trigger** (e.g. by
> collapsing stdlib homes into `Main`) — a multi-home program SHOULD emit
> multi-file Rust; that per-module granularity is the entire point of Phase 5
> (salsa incremental recompilation in Milestone D). The 6 goldens were
> regenerated to their correct multi-file shape and SEAL-verified (each split
> project actually `cargo build`s + runs), not narrowed away. Task 12's code
> (`ffec21f`) stays as-is; it is sound. See the corrected blast-radius
> paragraph below and §5 Task 13 for the enumerated list.

**The Spine-collapse invariant (the key correctness rule that keeps this
narrow):** when a program has exactly ONE distinct `home` across all its
`SkyModule`-bucketed items (i.e. `partition_items`'s `RustFileId::SkyModule`
keys partition to a single bucket — true for a SINGLE-home program: one
user module that imports only kernel modules, which is the majority of the
155 fixtures but NOT all of them, see the corrected blast-radius paragraph
below), `emit_program` collapses
back to the CURRENT shape: that one module's types/funcs are inlined into
`Spine` at the SAME position as today (`project.rs`'s existing lines
342-347/383-387), producing a byte-identical single `src/main.rs`. The real
multi-file split only MATERIALISES as separate files when 2+ distinct
`SkyModule` `home`s are present. This is not a special case bolted on
afterwards — it falls out naturally from "iterate the `SkyModule` partition
buckets; if there is only one, write it to `Spine`'s `RelPath`; otherwise
write each to its own `RelPath` and add the barrel lines to `Spine`."

**The trigger condition counts `SkyModule` buckets only, never `Spine`.**
`partition_items` (Task 4) ALWAYS produces a `Spine` bucket for any program
that uses `Std.Db` — that is where `SqlValue`/`SqlField` now route
unconditionally (§2.2's fix). A DB-using single-Sky-module golden therefore
has exactly one `SkyModule` bucket (collapse fires, unchanged single-file
output) alongside a non-empty `Spine` bucket (which was ALREADY part of
`Spine`'s content today, per §2.2) — the presence of `Spine`-bucket content
does not, by itself, materialise a second file. Only 2+ distinct
`SkyModule` `home`s trigger the real split. See Task 5 for the exact
ordering rule that keeps this collapse byte-identical for the existing
`Std.Db` goldens (`golden_m5b_db`, `golden_m5b_db_gates`, and friends).

This invariant is what keeps Milestone C's blast radius small — but **small
is not zero** (corrected premise, verified by executing Task 13's
machine-checked gate). The VAST MAJORITY of the 155 goldens are single-home
programs (one user module importing only kernel modules) and need ZERO
golden-file changes, exactly as designed. But **~6 tests across 5 golden
binaries are genuinely multi-home and legitimately split**, each regenerated
+ SEAL-verified in Task 13:

| Golden binary | Homes | Kind | Fix applied |
|---|---|---|---|
| `golden_mm::mm_diamond_emits_byte_identical_main_rs` | B, C, D, Main | user multi-module (dir-diff) | regenerated golden DIR: Spine-only `main.rs` + barrel + `sky_mods/sky_mod_{b,c,d,main}.rs` |
| `golden_mm::mm_local_pkg_emits_byte_identical_main_rs` | Lib, Main | user multi-module (dir-diff) | regenerated golden DIR: Spine-only `main.rs` + barrel + `sky_mods/sky_mod_{lib,main}.rs` |
| `golden_class1_boundary_scheme_field_result` | Lib1, Lib2, Main | user multi-module (substring) | assertion widened to scan whole emitted `src/` tree; visibility prefix dropped (`pub(crate) fn`) |
| `golden_css_source` | Main, Std.Css | stdlib-source import (substring) | assertion widened to scan whole emitted `src/` tree |
| `golden_stdui_grid_seal` | Main, Std.Ui.Grid | stdlib-source import (substring) | assertion widened to scan whole emitted `src/` tree |
| `golden_stdui_transition_seal` | Main, Std.Ui.Transition | stdlib-source import (substring) | assertion widened to scan whole emitted `src/` tree |

(That is 6 tests / 5 binaries — `golden_mm` carries two of the affected
tests.) The pilot fixture (§5 Tasks 8/11/12) additionally exercises the
real split against a Db-using user-module program; the stdlib-source split
(css/grid/transition) is a NEW SEAL surface the pilot did NOT cover, closed
in Task 13 by building each of those emitted projects under `SKY_E2E` and
confirming the relocated leaf kernels still resolve via `use crate::*;`.

The `>= 2` split trigger was deliberately NOT narrowed to exclude stdlib
homes: a Layer-3 stdlib module compiled to Sky source is a real, own-home
compilation unit, and giving it its own `sky_mods/<mod>.rs` file is exactly
the per-module granularity Milestone D's salsa incrementality is built on.
Collapsing stdlib homes into `Main` would be a papering-over workaround that
throws away that granularity for the sole benefit of a smaller golden diff —
rejected under the no-deferral / root-cause principle.

**Note on `golden_cross_module_type_res.rs`.** This existing test (multi-
`.sky`-file examples 16/17) asserts `skyc::build` exits 0 — it does NOT
byte-compare `main.rs`. It is UNAFFECTED by this spec either way (it never
touches the golden-dir byte-comparison harness), but it IS a genuine
existing multi-module program at the SOURCE level. Worth a follow-up note
in §5's proof-test inventory: once Milestone C lands, this test's build
output legitimately gains multiple `.rs` files under `src/sky_mods/` for
the first time — its own assertion (`exit 0`) does not need to change, but
a NEW assertion could be added confirming the split actually materialised
(catches a regression where the split silently stops happening). Recorded
as an optional strengthening, not required for THE SEAL.

### 3.4 Milestone D — the salsa query-graph wiring (the compiler-side incrementality tracking)

**Milestones A-C already stand alone as a real, non-regressive increment —
this is not merely a staging convenience.** `EmittedProject.files:
BTreeMap<RelPath, String>` (§1.5) was ALREADY file-count-agnostic before
this spec, and `build_emit_manifest` / `reconcile_emitted_project` /
`prune_orphaned_files` (`crates/skyc/src/lib.rs:826-935`) ALREADY do
per-file content-gated writes (`write_if_changed`) and orphan-pruning for
however many files a manifest declares — that machinery needed zero
changes for Task 16 and needs zero changes here. So the moment Milestone C
lands, an N-file emitted project already gets `cargo`-visible, per-file,
mtime-based incremental rebuilds on disk **regardless of whether Milestone
D ever lands** — a body edit to module A writes only `sky_mods/<A>.rs`
(content differs) and leaves every OTHER module's `.rs` file's mtime
untouched (content-identical, `write_if_changed` skips the write), so
`cargo build`'s own dependency-graph incrementality already does the right
thing per compilation unit. Milestone D's job is narrower and more
specific than "the payoff": it makes the COMPILER's OWN re-derivation of
each file's content salsa-tracked and memoized (so `skyc` itself, not just
the downstream `cargo build`, skips re-rendering an unaffected module's
text) — a real, additional win, but layered on top of a floor that is
already sound and already delivers most of the user-visible benefit
(faster `cargo build`) without it.

Milestones A–C land the BACKEND capability (N-file emission) as a plain,
non-salsa-wrapped function, exactly the same relationship Phase 4's
`sky_types::infer_attributed`/`sky_lower::lower` had to `sky_db::typecheck`/
`lower_program` before Phase 4 wrapped them. Milestone D wraps the new
capability in tracked queries: `program_rust_file_ids(root, entry)`,
`emit_spine_file(root, entry, config)`, `emit_rust_file(root, entry, config,
file: RustFileId)`, `emit_manifest(root, entry, config)` — see §4 for the
exact query graph and dependency shape, and why it deliberately does NOT
match the original design doc's `program_ir_module(owner)` dependency edge
(§4.3 explains the honest divergence).

**Milestone D is the natural session boundary.** Mirroring how this
project staged Task 17 after Task 15's own survey (§10.4 → §11: "not
attempted this session; landed next session"), Milestones A–C are scoped
to fit inside one focused session's sound-review budget (they are, in
total, a large but mechanically bounded refactor + a test migration).
Milestone D is a genuinely separate salsa-design pass with its own proof-
test obligations and should be reviewed against the Task-18 clean-vs-
incremental parity gate on its own, not bundled into the same review as
the backend split. This spec sketches it fully (§4, §5 Tasks 14-17) so the
NEXT session has a concrete plan, not a re-derivation task — exactly the
discipline §10.2 itself modelled for this session.

---

## 4. The salsa query graph — target shape and its honest divergence from the design doc

### 4.1 New interned key

```rust
/// crates/sky_db/src/lib.rs (new)

/// One Rust source file the backend emits for a Sky module's OWN
/// declarations (`Spine` — the fixed-anchor + entry file — is NOT a
/// `RustFileId`; it is produced by the separate `emit_spine_file` query,
/// §4.2, because it is always-present and never added/removed by a
/// module add/delete the way a `SkyModule` file is).
#[salsa::interned]
pub struct RustFileId {
    /// The Sky module's defining path — `sky_ir::Func::home` /
    /// `EnumDef::home`'s value, never empty on the real driver path (only
    /// backend unit-test IR built by hand skips it, and that path never
    /// reaches salsa).
    pub home: sky_ir::ModPath,
}
```

`ModPath` already derives `Clone + PartialEq + Eq + PartialOrd + Ord + Hash`
(`crates/sky_ir/src/ir.rs:12-15`) — it is usable as a `#[salsa::interned]`
field with no new trait work.

### 4.2 New tracked queries

| Query | Depends on | Returns | Role |
|---|---|---|---|
| `program_rust_file_ids(root, entry)` | `lower_program(root, entry)` | `Arc<BTreeSet<RustFileId>>` | The `home`-set quantifier — mirrors `program_metadata`'s `program_modules()` role in the ORIGINAL design doc: makes the "which files exist" question a first-class, salsa-tracked value so an add/delete of a Sky module (which changes the `home` set) is a visible dependency edge, not an implicit side effect of `lower_program` re-running. |
| `emit_spine_file(root, entry, config)` | `lower_program(root, entry)`, `config.db_driver(db)` | `EmitResult` (reuses the existing `Result<Arc<...>, Diagnostic>` shape, `Arc<String>`) | `Spine`'s text — preamble, kernel-wrapper prelude, record structs, DB-projection impls, TEA/Auth aliases, epilogue, `fn main()`, and the `mod`/`pub(crate) use` barrel lines for every id in `program_rust_file_ids`. |
| `emit_rust_file(root, entry, config, file: RustFileId)` | `lower_program(root, entry)`, `config.db_driver(db)`, `file` | `EmitResult` | One `SkyModule` file's text — that `home`'s `EnumDef`s + `Func`s only. |
| `emit_manifest(root, entry, config)` | `emit_spine_file(...)`, `emit_rust_file(f)` for every `f` in `program_rust_file_ids(...)`, plus the existing runtime-shim/`Cargo.toml` construction `emit_project` already does | `Result<Arc<sky_backend::EmittedProject>, Diagnostic>` | The complete intended project — the top-level driver demand, replacing `emit_project` as `compile_prepared`'s call site (§4.4). |

### 4.3 The honest divergence from the original design-doc table, and why it is sound

`docs/architecture/incremental-compilation-and-watch.md`'s table sketches
`emit_rust_file(RustFileId)` depending on `program_ir_module(owner) +
program_metadata()` — i.e. it assumed per-module LOWERING
(`program_ir_module(ModuleId)`) would exist by the time per-file emission
landed. Phase 4's own continuation scope (§9.4) — true per-module
`typecheck`/`lower` — is **still not shipped**, unchanged status per §10.4.
This spec's `emit_rust_file` therefore depends on the COARSE
`lower_program(root, entry)` (whole-program, Phase-4-shaped) instead, plus
the query's own `file` key to select which slice of that one `Program` to
render. **This is a deliberate, recorded divergence, not an oversight** —
matching exactly the pattern this project used for `program_metadata`
depending directly on `lower_program` rather than being firewalled (Phase 5
§10.1: "This gets... H6 lock... by construction: because `lower_program` is
itself the coarse whole-program spine, ANY semantic edit anywhere already
re-executes it").

**Why this still delivers a real incrementality win despite `lower_program`
staying whole-program-coarse.** A body edit to module A forces
`lower_program` to re-execute in full (Phase 4's coarse floor, unchanged)
— its OUTPUT VALUE genuinely differs (module A's function body changed), so
salsa cannot early-cut `lower_program` itself. But `emit_rust_file(B)` for
an UNRELATED module B is its OWN separately-memoized salsa node, keyed on
`(root, entry, config, file=B)`. When it re-executes (forced to, because
its `lower_program` dependency's value changed) it reads ONLY module B's
slice of the freshly-lowered `Program` — which is byte-identical to before,
since B's own `EnumDef`s/`Func`s did not change. Its rendered `String`
output therefore comes out byte-identical, salsa backdates `emit_rust_file
(B)`'s memo, and `emit_manifest`'s dependency on it early-cuts — meaning
the on-disk write for B's `.rs` file (§4.4, the Task-16 bridge's
content-gated `write_if_changed`) sees no diff and skips the write, so
`cargo`'s own mtime-based incrementality is preserved for B's compilation
unit even though the compiler-side query genuinely re-ran. This is the
exact "red-green" pattern already used throughout this project's salsa
work (Phase 5 §10.1's own framing, verbatim: "What downstream consumers
gain from this being a *salsa* query... is exactly what the design doc
promises: early-cutting on a byte-identical output even though the query
itself always re-executes"). **The win is real and does not require Phase
4's per-module lowering continuation as a prerequisite** — it is available
today, standing on Phase 4's coarse floor exactly as-is.

### 4.4 Wiring `compile_prepared` and the Task-16 bridge

`compile_prepared` (`crates/skyc/src/lib.rs:516`) currently demands
`sky_db::emit_project(root, entry, config)` (a single `EmitResult`) and
passes its `EmittedProject` to `write_emitted_project`
(`crates/skyc/src/lib.rs:790`). Milestone D changes this call to demand
`sky_db::emit_manifest(root, entry, config)` instead — same `EmitResult`
return SHAPE (`Result<Arc<EmittedProject>, Diagnostic>`), so
`write_emitted_project` / `build_emit_manifest` / `reconcile_emitted_
project` / `prune_orphaned_files` need **zero changes**, exactly repeating
Task 16's own "manifest shape is agnostic to file count" finding (§1.5)
one layer up. `emit_project` itself is NOT deleted — it stays as the
whole-program, non-split entry point `sky_backend_rust/tests/golden.rs`'s
crate-level byte-oracle and any future non-incremental caller can keep
using; `emit_manifest` is additive, not a breaking rename.

---

## 5. TDD step list

Each task is scoped to be completable in well under an hour. Every step:
write the failing test → confirm it fails for the right reason → implement
the minimal change → confirm it passes → commit. File paths are exact;
Rust is illustrative-but-concrete (adjust to compile against the real
surrounding code at implementation time — signatures here are the
CONTRACT, not necessarily the literal final tokens).

### Task 1 — `RustFileId` (backend-internal, non-salsa) + `mod_ident`

**Files:**
- create `crates/sky_backend_rust/src/rust_file.rs`
- edit `crates/sky_backend_rust/src/lib.rs` (register the module)
- edit `crates/sky_backend_rust/src/naming.rs` (promote `module_prefix` to
  `pub(crate)`, or add a thin wrapper)

**Steps:**

1. Write the failing test first, in the new file:

```rust
//! `crates/sky_backend_rust/src/rust_file.rs`
//! Backend-internal file-id domain for per-Sky-module Rust emission
//! (Phase-5 continuation — see `docs/architecture/
//! phase5-emit-rust-file-design-2026-07-12.md` §2.1). NOT yet a salsa
//! type — that is Milestone D (§4.1).

use sky_ir::ModPath;

/// Which Rust file a program's item (an `EnumDef` or `Func`) is declared
/// in. `Spine` is the always-present entry file (`src/main.rs`); a
/// `SkyModule` is one Sky module's OWN file (`src/sky_mods/<ident>.rs`),
/// materialised only when 2+ distinct homes are present (the Spine-
/// collapse invariant — see the design doc §3.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum RustFileId {
    Spine,
    SkyModule(ModPath),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_is_not_a_sky_module() {
        assert_ne!(RustFileId::Spine, RustFileId::SkyModule(ModPath(vec![])));
    }
}
```

2. Run it — compiles and passes trivially (this step exists to prove the
   module wires into the crate, not to test real logic yet):

```bash
cargo test -p sky_backend_rust rust_file::
```

3. Add `mod rust_file;` to `crates/sky_backend_rust/src/lib.rs` near the
   other `mod` declarations if the test in step 2 did not already require
   it to compile.

**Verify.** `cargo test -p sky_backend_rust rust_file::` green; `cargo
clippy -p sky_backend_rust` clean.
**Done when.** `RustFileId` exists, compiles, is `pub(crate)`, and the
crate builds with zero behavioural change (nothing calls it yet).

### Task 2 — `mod_ident` + the fail-closed duplicate-mod-name gate

**Files:** edit `crates/sky_backend_rust/src/rust_file.rs`; edit
`crates/sky_backend_rust/src/naming.rs`.

**Steps:**

1. Failing test in `rust_file.rs`, proving the injective-enough happy path
   AND the fail-closed collision path:

```rust
#[test]
fn mod_ident_is_stable_and_distinct_for_distinct_homes() -> sky_diagnostics::DResult<()> {
    let a = mod_ident(&["Std", "Palette"]);
    let b = mod_ident(&["Lib"]);
    assert_ne!(a, b);
    assert_eq!(a, "sky_mod_std_palette");
    Ok(())
}

#[test]
fn duplicate_mod_idents_fail_closed() {
    // Two distinct ModPaths that fold to the same identifier under the
    // current naive fold — construct via whatever two segments the real
    // `module_prefix` fold is known to collide on once implemented; if no
    // such pair exists for the CURRENT fold, this test still documents the
    // gate's presence by asserting the checker function's shape/behaviour
    // on a synthetic forced collision (inject two RustFileId::SkyModule
    // values that map to the same ident via a test-only override point).
    let ids = vec![
        RustFileId::SkyModule(ModPath(vec![/* Std */])),
        RustFileId::SkyModule(ModPath(vec![/* Std again, distinct symbol interning */])),
    ];
    let result = assert_mod_idents_unique(&ids, /* interner */ todo_interner());
    assert!(result.is_err());
}
```

   (The exact synthetic-collision construction depends on `Symbol`
   interning specifics available at implementation time — the CONTRACT
   under test is "two distinct `ModPath`s that fold to the same
   `mod_ident` must produce `Err(Diagnostic::Name::DuplicateValue)`", not
   the literal collision pair above.)

2. Confirm both tests fail to compile (functions don't exist yet).

3. Implement `mod_ident` (wrapping `naming::module_prefix`, prefixed
   `"sky_mod_"`, snake-cased) and `assert_mod_idents_unique(ids: &[RustFileId],
   interner: &Interner) -> DResult<()>` mirroring the EXISTING
   `func_names.values().any(...)` fail-closed pattern
   (`crates/sky_backend_rust/src/lib.rs:386-394`) — same
   `Diagnostic::Name::DuplicateValue` shape.

4. Run the tests; both green.

**Verify.** `cargo test -p sky_backend_rust rust_file::`; `cargo clippy -p
sky_backend_rust` clean (no `unwrap`/`expect`, exhaustive matches).
**Done when.** `mod_ident` is deterministic and the collision gate fails
closed with a typed diagnostic, never a panic.

### Task 3 — fold `record_struct_name` into the shared type-namespace collision registry (independent-review finding)

**Why this task exists, and why it lands here.** `unique_struct_name`
(`crates/sky_backend_rust/src/lib.rs:1779-1791`), called from
`EmitCtx::build` at `lib.rs:523-526`, dedupes a synthesised record struct's
name against a LOCAL `used_names: BTreeSet<String>` populated purely from
OTHER record-struct names — it is never checked against `enum_names` or
`func_names`, both already built earlier in the SAME `EmitCtx::build`
function (`lib.rs:289-397`) and both ALREADY self-checked with a
fail-closed `Diagnostic::Name::DuplicateValue` gate
(`enum_names.values().any(...)` at `:342`,
`func_names.values().any(...)` at `:386`). This is a PRE-EXISTING gap in
TODAY's single-file backend — `RecordStruct` and `EnumDef` both render as
Rust `struct`/`enum` items and share Rust's TYPE namespace (per the
language reference, `mod` items share it too — the same namespace Task 2's
`mod_ident` gate polices), while `Func` items render in the disjoint VALUE
namespace, so a record-struct/func collision was never a real risk. A
record-struct/enum collision, however, is real: today it surfaces as a
loud `cargo`-time `E0428` (annoying, never silent-wrong). Under the flat
glob-reexport barrel (§2.1), once `EnumDef`s and `RecordStruct`s can live
in DIFFERENT files, the SAME collision stops erroring — Rust's
name-resolution precedence (local definition wins over a glob `use`) means
the record struct's local declaration SILENTLY SHADOWS the glob-reexported
enum, surfacing only as a confusing type mismatch at some unrelated third
file's use site, or not at all if nothing else references the shadowed
enum. This undermines §2.1's "already enforced at `skyc` time, so the flat
glob barrel is sound" argument, so this task lands BEFORE Task 4
(partition function) and Milestone A is considered to begin — the flat
namespace must actually BE sound before anything is built on top of it.

**Files:** edit `crates/sky_backend_rust/src/lib.rs` (new `EmitCtx` method;
the `unique_struct_name` call site at `:523-526`).

**Design.** Add `EmitCtx::assert_record_structs_disjoint_from_type_namespace
(&self, mod_idents: &BTreeSet<String>) -> DResult<()>`, called from
`emit_program` immediately after `EmitCtx::build` returns. It walks
`self.record_structs()` and, for each struct's ALREADY-CHOSEN name (i.e.
AFTER `unique_struct_name`'s existing intra-category `_2`/`_3` bumping —
that mechanism is UNCHANGED and still the right tool for two record
shapes that coincidentally camel-case to the same base, since a synthetic
name has no user-facing identity to preserve by failing instead of
renaming), checks it against `self`'s existing `enum_names.values()` (a
NEW `pub(crate) fn contains_type_name(&self, name: &str) -> bool` accessor
avoids leaking the map's internal shape), `self`'s `func_names.values()`
(included for defense-in-depth even though the value/type namespace split
means it is not strictly load-bearing today — cheap, and it stops relying
on an implicit "func-name casing convention never collides" invariant that
a future `naming.rs` change could silently violate), and the caller-supplied
`mod_idents` set. ANY hit fails closed with `Diagnostic::Name::DuplicateValue`
— mirroring Task 2's own choice for `mod_ident` collisions (mirror, not
auto-rename, for a namespace-wide collision), rather than the intra-category
auto-suffix behaviour `unique_struct_name` already has for record-struct-vs-
record-struct collisions.

Milestone A (Task 5) calls this with `mod_idents = &BTreeSet::new()` — at
that point no `mod` items exist in the emitted output at all (Milestone A
never writes more than one file), so an empty set is not a loophole, it is
the honest state of the world. Milestone C (Task 12) updates the call site
to pass the REAL `mod_idents` set once `partition_items`'s buckets, and
therefore real `mod` declarations, exist.

**Steps:**

1. Write the failing test first — a REAL, constructible collision, not a
   hedged placeholder: hand-build a `Program` with one module whose Sky
   module path is `["Rec"]`, declaring a user enum `type XY = A | B` (so
   `EnumDef { home: ModPath(["Rec"]), name: "XY", .. }`). `naming::enum_name`
   folds `(home, name)` via `to_camel_case(format!("{}_{}", module_prefix(
   home), ty))` (`naming.rs:181`) — `module_prefix(["Rec"])` is `"Rec"`, so
   this enum's Rust name is `"RecXY"`. Separately, construct a record
   literal with fields `{ x, y }` — `naming::record_struct_name(&["x",
   "y"])` is ALREADY asserted to be `"RecXY"` byte-for-byte by the existing
   unit test `record_struct_names_from_field_sets` (`naming.rs:1299`). Build
   `EmitCtx` (or call the new method directly against a hand-assembled
   `enum_names`/`record_structs` pair) and assert
   `assert_record_structs_disjoint_from_type_namespace` returns
   `Err(Diagnostic::Name::DuplicateValue)`.

2. Confirm the test fails to compile (method doesn't exist yet).

3. Implement the method and the `contains_type_name` accessor; wire the
   call site into `emit_program` right after `EmitCtx::build`.

4. Run; green. Also re-run the FULL existing golden suite
   (`cargo test -p sky_backend_rust golden`, `cargo test -p skyc --test
   golden_m0`) to confirm zero behavioural change for every program that
   does NOT hit this collision — the new check is purely additive for the
   common case.

**Verify.** `cargo test -p sky_backend_rust record_struct_namespace::` (or
wherever the test lands) green; `cargo test -p sky_backend_rust golden`
and `cargo test -p skyc --test golden_m0` unaffected; `cargo clippy -p
sky_backend_rust` clean.
**Done when.** A record struct can never silently shadow an enum, func, or
(once Task 12 wires the real set through) a Sky-module file — the flat
namespace §2.1 depends on is enforced across EVERY category that shares a
Rust namespace, not just within each category independently.

### Task 4 — the partition function, proven total, with the `SqlValue`/`SqlField` Spine special case

**Files:** edit `crates/sky_backend_rust/src/rust_file.rs`.

**Steps:**

1. Failing tests: build a small hand-constructed `sky_ir::Program` with 2
   modules' worth of `EnumDef`/`Func` (reuse the `build_m0`-style
   hand-construction pattern from `crates/sky_backend_rust/tests/
   golden.rs`, extended with a second `home`), call the not-yet-existing
   `partition_items`, and assert:
   - every `EnumDef` and every `Func` in the input appears in EXACTLY one
     output bucket (no drop, no duplicate) — count-based assertion, not a
     spot check;
   - a `home`-empty item NAMED something other than `"SqlValue"`/
     `"SqlField"` (simulating hand-built test IR) falls into the SAME
     bucket as the containing `Module.name` — the existing naming layer's
     fallback behaviour, preserved;
   - a `home`-empty `EnumDef` named EXACTLY `"SqlValue"` or `"SqlField"`
     (matching what `synthetic_sqlvalue_enum`/`synthetic_sqlfield_enum`,
     `crates/sky_lower/src/lower.rs:4023`/`:4094-4118`, actually produce)
     falls into `RustFileId::Spine`, NEVER the `Module.name` fallback
     bucket — this is the §2.2 fix (independent-review finding 1); assert
     it explicitly, do not rely on the generic empty-home test above to
     cover it.

2. Confirm all three fail to compile.

3. Implement, now taking `interner: &sky_intern::Interner` so the name
   check in step (a) below can resolve `Symbol -> &str` (`EmitCtx` already
   has an interner in scope at every real call site — see Task 5):

```rust
pub(crate) fn partition_items<'p>(
    program: &'p sky_ir::Program,
    interner: &sky_intern::Interner,
) -> std::collections::BTreeMap<RustFileId, (Vec<&'p sky_ir::EnumDef>, Vec<&'p sky_ir::Func>)> {
    let mut out: std::collections::BTreeMap<_, (Vec<_>, Vec<_>)> = std::collections::BTreeMap::new();
    for module in &program.modules {
        for ty in &module.types {
            let sky_ir::TypeDef::Enum(def) = ty;
            // §2.2 fix: SqlValue/SqlField are Prelude built-ins (empty
            // canon home, see lower.rs's own doc comment on
            // synthetic_sqlvalue_enum) that the DB-projection impl blocks
            // (ALWAYS Spine, per §2.2) reference. Force them into Spine
            // BY NAME, before the generic empty-home fallback below would
            // otherwise route them into whichever module happens to be
            // `Module.name` — reuses the exact detection idiom `uses_db`
            // already applies at `lib.rs:571-576`.
            let resolved = interner.resolve(def.name);
            if matches!(resolved, Some("SqlValue" | "SqlField")) {
                out.entry(RustFileId::Spine).or_default().0.push(def);
                continue;
            }
            let home = if def.home.0.is_empty() { module.name.clone() } else { def.home.clone() };
            out.entry(RustFileId::SkyModule(home)).or_default().0.push(def);
        }
        for func in &module.funcs {
            let home = if func.home.0.is_empty() { module.name.clone() } else { func.home.clone() };
            out.entry(RustFileId::SkyModule(home)).or_default().1.push(func);
        }
    }
    out
}
```

4. Run; all three green.

**Verify.** `cargo test -p sky_backend_rust rust_file::partition`; totality
assertion passes for the 2-module fixture, the existing `build_m0`
single-module fixture (reused, imported from the golden test's helper —
extract `build_m0` into a `pub(crate)` test-support function if it is not
already reachable across test binaries), AND a `build_m0`-derived fixture
with `Std.Db` usage (`SqlValue`/`SqlField` present) proving the Spine
special case.
**Done when.** `partition_items` is proven total (no item lost or
duplicated) against at least one multi-module and one single-module
fixture, and `SqlValue`/`SqlField` are proven to route to `Spine`
regardless of how many `SkyModule` homes are present.

### Task 5 — route `emit_program` through `partition_items`, byte-identical proof

**Files:** edit `crates/sky_backend_rust/src/project.rs`.

**Steps:**

1. Failing test: none new — this step's SUCCESS CRITERION is that ALL
   EXISTING golden tests stay green. Run the baseline first to record the
   starting state, INCLUDING the `Std.Db` goldens (the ones this task's
   ordering rule below exists to protect):

```bash
cargo test -p sky_backend_rust golden
cargo test -p skyc --test golden_m0
cargo test -p skyc --test golden_m5b_db
cargo test -p skyc --test golden_m5b_db_gates
cargo test -p skyc --test golden_db_wrapper_empty_params_165
```

2. Refactor `emit_program`'s type/func emission loops
   (`project.rs:342-347`, `:383-387`) to iterate
   `crate::rust_file::partition_items(program, ctx.interner)` (Task 4)
   instead of `program.modules` directly, but keep writing everything into
   the SAME single growing `out: String` exactly as today — Milestone A
   does not yet write multiple files, it only proves the partition
   function produces the IDENTICAL emission order and content as the
   current `program.modules` walk for every existing (single-module)
   fixture.

   **Ordering rule (load-bearing for the `Std.Db` goldens).** Do NOT
   iterate the returned `BTreeMap<RustFileId, _>` in its raw derived-`Ord`
   order for the types pass — `RustFileId::Spine < RustFileId::SkyModule(_)`
   under the enum's declaration order, so a blind full-map iteration would
   emit `SqlValue`/`SqlField` FIRST, ahead of every user type, which is NOT
   today's position (`synthetic_sqlvalue_enum`/`synthetic_sqlfield_enum`
   are pushed onto `types_ir` in `sky_lower::lower` AFTER every user union,
   `crates/sky_lower/src/lower.rs:3899-3949` — i.e. LAST among a module's
   types, immediately before the record structs that follow in
   `emit_program`'s existing sequence). Instead: for the types pass, walk
   the `SkyModule(_)` buckets first (`BTreeMap` order over `ModPath` — a
   single bucket for every existing golden today, so this is a no-op
   reordering for them), THEN append the `Spine` bucket's `EnumDef`s (if
   any — i.e. `SqlValue` then `SqlField`, in the insertion order
   `partition_items` preserves) immediately after. This reproduces today's
   exact byte sequence: user types, then `SqlValue`, then `SqlField`, then
   (unchanged) record structs, then (unchanged) the DB-projection impls.

3. Re-run the SAME commands from step 1.

**Verify.** Every command from step 1 green, byte-for-byte, with ZERO
golden-file changes. If any goes red, the partition function's ORDER or
CONTENT diverges from the current per-module walk — fix `partition_items`
or the ordering rule above, not the goldens.
**Done when.** `emit_program` is internally refactored to route through
`partition_items` with proven zero behavioural change, INCLUDING for every
existing `Std.Db`-using golden.

### Task 6 — the directory-diff golden harness helper (additive)

**Files:** edit `crates/skyc/tests/support/mod.rs`.

**Steps:**

1. Add `assert_emitted_project_matches_golden_dir` exactly as specified in
   §2.4's code block. It has no test of its OWN yet in this step (support
   modules are not directly `#[test]`-annotated); its correctness is
   proven by Task 7.

2. `cargo build --tests -p skyc` — confirm it compiles (unused-function
   warning is expected and fine at this point; do not silence it with
   `#[allow(dead_code)]`, since Task 7 removes the warning by using it).

**Verify.** `cargo build --tests -p skyc` succeeds.
**Done when.** The helper exists, typed, unused.

### Task 7 — adopt the helper in `golden_m0.rs`, side by side then exclusively

**Files:** edit `crates/skyc/tests/golden_m0.rs`.

**Steps:**

1. In `emits_byte_identical_main_rs_and_vendors_runtime`, ADD (do not yet
   remove the old assertion) a call:

```rust
support::assert_emitted_project_matches_golden_dir(
    &sky_backend_rust::RustBackend::new(/* interner */).emit(&/* program */)?,
    &golden.parent().expect("golden has a parent dir"),
);
```

   (Exact construction depends on whether `emitted` is already available
   as an in-memory `EmittedProject` at this point in the test, or only as
   written-to-disk output — prefer calling the helper against the
   IN-MEMORY value if `skyc::build`'s API exposes it, else adapt the
   helper to accept a `&Path` for the emitted `out/src` tree too; either
   is a sound instantiation of the same contract.)

2. Run: `cargo test -p skyc --test golden_m0` — both the old `assert_eq!`
   and the new helper assert on the SAME data; confirm both pass.

3. Delete the OLD hand-rolled `assert_eq!` block, keeping only the new
   helper call.

4. Re-run: `cargo test -p skyc --test golden_m0` — still green.

**Verify.** `cargo test -p skyc --test golden_m0` green after step 4.
**Done when.** `golden_m0.rs` uses ONLY the shared directory-diff helper,
proven equivalent in discriminating power to the assertion it replaced.

### Task 8 — migrate the remaining golden tests (split into per-batch sub-tasks — independent-review finding)

**Scope, precisely (re-verified against `HEAD`, not the doc's earlier
"154" estimate).** `ls crates/skyc/tests/golden_*.rs` is 155 files. 13 of
them were checked by hand and confirmed to contain NO `assert_eq!` at
all — they are exit-0-only or `.contains(...)` substring-pattern tests
that never did a byte-diff against a golden `main.rs`, so the migration
does not touch them:

```
golden_cross_module_type_res.rs   golden_l0114_server_handler_arc.rs
golden_i136_alias_truncation.rs   golden_m102_local_type_shadows_dep.rs
golden_i138_total_resolution.rs   golden_m6_middleware_csrf.rs
golden_i148_http_stream_id.rs     golden_m6_server.rs
golden_i148_input_slider.rs       golden_m7_live_lambda_view_routed.rs
golden_i155_input_radio_row.rs    golden_m7_live_let_bound_routes.rs
golden_t0012_cross_module_attr.rs
```

`golden_m0.rs` is already done (Task 7). That leaves **141 files** that
actually byte-compare a golden `main.rs` and need migrating — realistically
~6 batches of ~24 files each, each batch needing its own test run, manual
fixups for nonstandard shapes, and a commit: hours to a day as ONE task,
not the "well under an hour" granularity the rest of this plan uses. Split
accordingly, each sub-task ending in its own commit so a session-budget
interruption has an unambiguous stopping point.

**Files:** every `crates/skyc/tests/golden_*.rs` NOT in the 13-file
allowlist above and not `golden_m0.rs`.

#### Task 8a — hand-migrate one more file, then write the migration script

1. Pick ONE file by hand (`golden_m1_tuples.rs`) and apply the identical
   Task-7 transformation manually, to confirm the pattern generalises
   beyond M0 (M0 is special — the FIRST golden, worth double-checking
   manually before scripting).
2. Run: `cargo test -p skyc --test golden_m1_tuples` — green.
3. Write a small migration script (`scripts/migrate-golden-harness.sh` or
   inline `sed`/`ripgrep` invocation, not committed as permanent tooling
   unless the project wants it kept) that finds the common
   `read_to_string(...).../assert_eq!(...)` pattern and replaces it with
   the shared helper call, ALSO deleting each file's local
   `ldir`/`repo_root` duplicate in favour of `support::repo_root`.
4. Commit (hand migration + script, 1 file done, 140 remaining).

**Verify.** `cargo test -p skyc --test golden_m1_tuples` green.
**Done when.** The script exists and is proven correct on one real file.

#### Task 8b through 8g — six scripted batches, ~23-24 files each

Repeat for six batches (`8b`, `8c`, `8d`, `8e`, `8f`, `8g`; adjust the
final batch's size so all 140 remaining files are covered — exact batch
membership is an implementation-time choice, not fixed by this spec):

1. Apply the Task 8a script to the batch. Run:

```bash
cargo test -p skyc --test 'golden_*' 2>&1 | tee /tmp/golden_batch_N.log
```

2. Fix any file the script mis-transformed (some goldens have nonstandard
   shapes — e.g. multiple assertions per test, or a `golden.join(...)`
   path built differently).
3. Re-run until green; commit the batch.

**Verify (per batch).** `cargo test -p skyc --test 'golden_*'` green after
that batch's commit, zero golden-file changes (Milestone B never touches
the split, only the test HARNESS).
**Done when (8g, the final batch).** All 141 byte-comparison
`golden_*.rs` files use the shared helper; `cargo test -p skyc --test
'golden_*'` fully green.

### Task 9 — the coverage gate (machine-checks migration completeness — rewritten, independent-review finding)

**Why this was rewritten.** The earlier draft of this task greped for the
RETIRED pattern (`out.join("src").join("main.rs")` + `assert_eq!`) — a
NEGATIVE heuristic. A hand-fixed golden file using a syntactically
different but still-stale single-file check (a bare `.join("src/main.rs")`,
a `format!`-built path, an `include_str!`-based comparison) could pass that
gate while remaining silently unmigrated. Rewritten as a POSITIVE,
structural check: every `golden_*.rs` file, except the 13-file allowlist
named in Task 8 (verified by hand to have never done a byte-diff in the
first place), MUST contain a call to the shared
`support::assert_emitted_project_matches_golden_dir` helper (Task 6).
Adding a new golden test file that is neither on the allowlist nor calling
the helper now fails this gate by construction — a deliberate choice
requires either using the helper or adding the file to the (small,
reviewed) allowlist, never a silent third option.

**Files:** create `crates/skyc/tests/golden_harness_coverage.rs`.

**Steps:**

1. Write the failing test first (it should currently FAIL if Task 8 is
   incomplete, or PASS trivially once Task 8g finishes — write it
   immediately after Task 8g's commit so it starts green, then
   deliberately break it once to confirm it CAN fail):

```rust
//! Machine-checked proof that every golden test uses the shared
//! directory-diff harness (`support::assert_emitted_project_matches_
//! golden_dir`) — see design doc §2.4 step 5 / Task 9. A POSITIVE check:
//! we assert the NEW helper is present, not merely that some retired
//! pattern is absent (a syntactically different but still-stale hand-roll
//! would pass a negative grep while staying unmigrated).

use std::path::PathBuf;

/// Exit-0-only or `.contains(...)`-substring tests that never did a
/// byte-diff against a golden `main.rs` in the first place — verified by
/// hand against `HEAD` when this gate was authored (Task 8's own scope
/// note lists the same 13 files with the same justification). Extending
/// this list requires the SAME hand-verification, not a guess.
const NEVER_BYTE_DIFFED: &[&str] = &[
    "golden_cross_module_type_res.rs",
    "golden_i136_alias_truncation.rs",
    "golden_i138_total_resolution.rs",
    "golden_i148_http_stream_id.rs",
    "golden_i148_input_slider.rs",
    "golden_i155_input_radio_row.rs",
    "golden_l0114_server_handler_arc.rs",
    "golden_m102_local_type_shadows_dep.rs",
    "golden_m6_middleware_csrf.rs",
    "golden_m6_server.rs",
    "golden_m7_live_lambda_view_routed.rs",
    "golden_m7_live_let_bound_routes.rs",
    "golden_t0012_cross_module_attr.rs",
];

fn repo_root() -> PathBuf {
    let joined = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn every_non_allowlisted_golden_test_calls_the_shared_helper() {
    let dir = repo_root();
    let mut offenders = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.starts_with("golden_") || !name.ends_with(".rs") {
            continue;
        }
        // This gate's own file never calls the helper — it checks for it.
        if name == "golden_harness_coverage.rs" || NEVER_BYTE_DIFFED.contains(&name) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        if !src.contains("assert_emitted_project_matches_golden_dir") {
            offenders.push(name.to_owned());
        }
    }
    assert!(
        offenders.is_empty(),
        "golden tests not yet migrated to the shared directory-diff helper \
         (and not on the allowlist): {offenders:?}"
    );
}
```

   (Exact helper name per Task 6/7 — confirm it did not change during
   implementation before hard-coding the substring check above.)

2. Run it against the pre-Task-8 state (temporarily, in a scratch branch
   or by checking out one un-migrated file) to confirm it FAILS for the
   right reason (lists the un-migrated file names) — do not skip this
   negative-case confirmation.

3. Run it against the post-Task-8g (fully migrated) state — green.

**Verify.** `cargo test -p skyc --test golden_harness_coverage` green;
manually confirmed to fail correctly against an un-migrated fixture.
**Done when.** Migration completeness is a CI-enforced, POSITIVE,
structural invariant, not a one-time claim and not a negative grep a
syntactically-different stale pattern could dodge.

### Task 10 — `RustBackend` gains `emit_spine`/`emit_module_file` methods (still unused by `project.rs`'s public path)

**Files:** edit `crates/sky_backend_rust/src/project.rs`.

**Steps:**

1. Failing test in `crates/sky_backend_rust/tests/golden.rs` (or a new
   `crates/sky_backend_rust/tests/split_emit.rs`): build the same
   hand-constructed 2-module `Program` from Task 4, call a not-yet-existing
   `sky_backend_rust::project::emit_spine(ctx, program)` and
   `emit_module_file(ctx, program, &home)`, and assert:
   - `emit_spine`'s output contains the preamble banner, the kernel-wrapper
     prelude, `fn main()`, and record structs, but NOT either module's
     `Func`/`EnumDef` declarations;
   - `emit_module_file(program, &home_a)`'s output contains ONLY home A's
     items, `pub(crate)`-prefixed, and opens with `use crate::*;`;
   - for a `Std.Db`-using fixture, `emit_spine`'s output ALSO contains the
     `SqlValue`/`SqlField` enum declarations (§2.2's fix, Task 4), in that
     relative order, positioned before the record structs — NOT in either
     module's `emit_module_file` output.

2. Confirm it fails to compile.

3. Implement `emit_spine` and `emit_module_file` as thin wrappers that call
   `partition_items` (Task 4) and reuse the EXISTING `emit_enum`/`emit_func`
   rendering functions per-item, just routed to two different output
   buffers instead of one — `emit_spine` renders the `Spine` bucket's
   `EnumDef`s (if any) immediately before the record structs, matching the
   ordering rule Task 5 established. `emit_program` (the current public,
   whole-project entry point) is UNCHANGED in this task — still
   single-file, still calling the ORIGINAL loops (Task 5 already proved
   those equivalent to `partition_items`, but `emit_program` itself is not
   yet switched to call `emit_spine`/`emit_module_file`; that is Task 12).

4. Run; green.

**Verify.** `cargo test -p sky_backend_rust split_emit::` green.
**Done when.** The two new rendering entry points exist, are individually
testable, and are provably not yet wired into any golden-affecting path
(Task 5's tests are unaffected — re-run them to confirm: `cargo test -p
sky_backend_rust golden`).

### Task 11 — a NEW multi-module + `Std.Db` pilot fixture, at TODAY's (pre-split) single-file shape

**Files:** create `tests/golden/multi_mod_split_pilot/Main.sky`; create
`tests/golden/multi_mod_split_pilot/Lib.sky`; create
`tests/golden/multi_mod_split_pilot/main.rs` (golden, single-file shape);
create `tests/golden/multi_mod_split_pilot/Cargo.toml`; create
`crates/skyc/tests/golden_multi_mod_split_pilot.rs`.

**Steps:**

1. Write minimal two-module Sky source: `Lib.sky` declares one small
   function, one small enum, AND one `Std.Db` call (e.g. `Db.query`/
   `Db.exec` against a trivial table) so `uses_db` is true; `Main.sky`
   imports and uses both, plus defines `main`. The `Std.Db` call is
   deliberate, not incidental — it makes this the ONE fixture that
   exercises the multi-module split AND the `SqlValue`/`SqlField`
   Spine-routing fix (§2.2) TOGETHER, closing the gap the independent
   review flagged: the original draft of this fixture had no `Db` usage,
   so that interaction would have shipped unverified through Milestone C.

2. Write the new golden test file, modelled EXACTLY on `golden_m0.rs`'s
   structure but using `support::assert_emitted_project_matches_golden_dir`
   from the start (this fixture is NEW, never used the retired pattern).

3. Run `skyc::build` by hand (or via a throwaway `cargo run --bin skyc --
   build ...` invocation) against the two-module source, capture its
   CURRENT (pre-split, Milestone C not yet landed) single-file `main.rs`
   output — including the `db`-enabled manifest fragments
   (`RUNTIME_MOD_RS_DB_APPEND` etc., `project.rs`'s existing M5b-db logic)
   — and commit it as the golden — i.e. this task establishes the "before"
   baseline HONESTLY, at whatever the compiler produces TODAY
   (home-qualified names like `lib_helper`/`main_helper` already present,
   per §1.3 — this is not new emission behaviour, just a new fixture
   exercising existing multi-module + `Std.Db` naming).

4. Run the new test; green.

**Verify.** `cargo test -p skyc --test golden_multi_mod_split_pilot` green.
**Done when.** A deliberately-multi-module, `Std.Db`-using fixture exists
with a byte-verified TODAY baseline, ready to be the ONE fixture Task 12
flips to the real split.

### Task 12 — flip `emit_program` to real multi-file output; update the pilot fixture's golden

**Files:** edit `crates/sky_backend_rust/src/project.rs`; edit
`tests/golden/multi_mod_split_pilot/` (delete the single-file `main.rs`
golden, add `tests/golden/multi_mod_split_pilot/main.rs` (Spine-only
content — includes the preamble, the `SqlValue`/`SqlField` enum
declarations, record structs, DB-projection impls, kernel-wrapper prelude,
epilogue, and `fn main()`) and
`tests/golden/multi_mod_split_pilot/sky_mods/sky_mod_lib.rs` +
`.../sky_mods/sky_mod_main.rs`).

**Steps:**

1. In `golden_multi_mod_split_pilot.rs`, the directory-diff helper (Task 7)
   will now FAIL once step 2 lands, because the golden dir still has the
   OLD single-file shape — this is the "confirm it fails for the right
   reason" checkpoint. Run it BEFORE step 2 to record the still-green
   baseline, then again AFTER step 2 to confirm it goes red with a clear
   "golden file missing" (for the new per-module paths) / "emitted !=
   golden" (for `main.rs`, now Spine-only) message — not a panic, not a
   silent pass.

2. Change `emit_program` (`project.rs:332-`) to branch on the number of
   DISTINCT `RustFileId::SkyModule` buckets `partition_items(program,
   ctx.interner)` returns (§3.3's "counts `SkyModule` buckets only, never
   `Spine`" rule):
   - `== 1` → EXACTLY today's behaviour (Spine-collapse invariant, §3.3):
     inline that one bucket's items into the single `src/main.rs`
     `RelPath`, unchanged from Milestone A — the `Spine` bucket's
     `SqlValue`/`SqlField` content (if any) is ALREADY part of `Spine`'s
     inlined output, per Task 5's ordering rule.
   - `>= 2` → call `emit_spine` (Task 10) for `src/main.rs` — which now
     also carries the `Spine` bucket's `SqlValue`/`SqlField` declarations
     — plus `emit_module_file` per `SkyModule` bucket for
     `src/sky_mods/<mod_ident>.rs` (using `mod_ident`, Task 2), and append
     the `mod`/`pub(crate) use` barrel lines (§2.1) to Spine's text before
     inserting it into `EmittedProject.files`. Also update the
     `assert_record_structs_disjoint_from_type_namespace` call site (Task 3)
     to pass the REAL `mod_idents` set — the `BTreeSet<String>` of every
     `SkyModule` bucket's `mod_ident` this same branch just computed —
     instead of Milestone A's empty placeholder, now that real `mod`
     declarations exist in the output.

3. Run the pilot test — confirm the failure from step 1 is now the ONLY
   thing between here and green (i.e. the CODE is producing the new
   shape; only the GOLDEN FILES need updating).

4. Regenerate and commit the new golden files
   (`tests/golden/multi_mod_split_pilot/main.rs`,
   `.../sky_mods/sky_mod_lib.rs`, `.../sky_mods/sky_mod_main.rs`) from the
   now-correct emitted output — inspect them by hand once before
   committing (never blindly copy a compiler's output into a golden
   without a human read, per this project's own golden-authoring norm).
   Confirm by hand that `sky_mod_lib.rs`'s `Db.query`/`Db.exec` call site
   references `SqlValue`/`SqlField` variants that resolve via the
   `use crate::*;` glob back to `Spine`'s declarations — the concrete,
   file-level proof that §2.2's fix is correct, not merely that it compiles.

5. Run the pilot test again — green.

6. Gated by `SKY_E2E=1`: run the pilot fixture's end-to-end
   build-and-run test (modelled on `golden_m0.rs`'s
   `end_to_end_builds_and_prints_one`) — confirms THE SEAL: the newly
   multi-file, `Std.Db`-using emitted project actually `cargo build`s and
   runs correctly, not just that its TEXT matches a golden.

**Verify.** `cargo test -p skyc --test golden_multi_mod_split_pilot` green;
`SKY_E2E=1 cargo test -p skyc --test golden_multi_mod_split_pilot
end_to_end` green.
**Done when.** The real per-Sky-module split exists, is proven correct
against ONE pilot fixture at both the byte level and the `cargo build`
level, INCLUDING the multi-module + `Std.Db` interaction §2.2's fix exists
to make sound.

### Task 13 — measure the ACTUAL blast radius; regenerate + SEAL-verify every genuinely-multi-home golden

> **CORRECTED SCOPE (2026-07-13).** This task was ORIGINALLY specified as
> "confirm ZERO blast radius on every OTHER existing golden," on the §3.3
> premise that only the pilot was multi-module. Executing it disproved that
> premise: the FULL golden suite, rebuilt against Task 12's real
> `emit_program` split, red-fails **6 tests across 5 golden binaries** whose
> programs are genuinely multi-home (§3.3's corrected blast-radius table).
> The task's real job is therefore NOT "prove zero changes" but "measure the
> true blast radius, and for each genuinely-multi-home golden regenerate it
> to the correct multi-file shape and SEAL-verify it." A red multi-home
> golden here is the split CORRECTLY firing, not a `partition_items` bug — do
> NOT narrow the trigger; regenerate the golden. (A red SINGLE-home golden
> WOULD still be a Spine-collapse-invariant violation — step 4 keeps that
> distinction.)

**Files:** `tests/golden/mm_diamond/` (regenerate: Spine-only `main.rs` +
`sky_mods/*.rs`), `tests/golden/mm_local_pkg/` (same), plus test-body edits to
`crates/skyc/tests/{golden_mm,golden_css_source,golden_class1_boundary_scheme_field_result,golden_stdui_grid_seal,golden_stdui_transition_seal}.rs`
and the shared `crates/skyc/tests/support/mod.rs` helper.

**Steps:**

1. Run the FULL golden suite against the Task-12 backend:

```bash
cargo test -p sky_backend_rust golden
cargo test -p skyc --test 'golden_*' --no-fail-fast
```

2. Enumerate every red test. Partition each into single-home (a real bug —
   step 4) vs genuinely-multi-home (the split correctly firing — step 3).
   The verified multi-home set is the §3.3 table: `mm_diamond`, `mm_local_pkg`,
   `class1_boundary_scheme_field_result` (user multi-module); `css_source`,
   `stdui_grid_seal`, `stdui_transition_seal` (stdlib-source import).

3. For each genuinely-multi-home golden, regenerate to the correct new shape
   and SEAL-verify (NEVER blind-copy compiler output into a golden):
   - **dir-diff goldens** (`mm_diamond`, `mm_local_pkg`): regenerate the golden
     DIR — `main.rs` now Spine-only + barrel, plus one `sky_mods/sky_mod_<mod>.rs`
     per module. Hand-read each regenerated file once (confirm each per-module
     file's cross-module + Spine-kernel calls resolve via `use crate::*;`).
     Extend `support::assert_emitted_project_matches_golden_dir` to compare
     `sky_mods/*.rs` symmetrically so the split's per-module content is
     byte-locked. `cargo build` + run the emitted project by hand to confirm
     THE SEAL (both have runnable `main`s: `mm_diamond` prints 87,
     `mm_local_pkg` prints "hello from Lib").
   - **substring goldens** (`css_source`, `class1_*`, `stdui_grid_seal`,
     `stdui_transition_seal`): the asserted symbol moved from `main.rs` to
     `sky_mods/<mod>.rs`. Widen each assertion to scan the WHOLE emitted `src/`
     tree (`main.rs` + `sky_mods/*.rs`) via a new shared
     `support::read_all_emitted_src` helper — robust to future placement,
     preferred over hard-coding the new path. Build the css/grid/transition
     emitted projects under `SKY_E2E=1` to confirm the relocated leaf kernels
     resolve via `use crate::*;` and the split project cargo-builds — the SEAL
     for the stdlib-home split, which the pilot (a Db/user-module case) did NOT
     cover.

4. If ANY genuinely-SINGLE-home golden goes red or needs a file change, THAT
   is a Spine-collapse-invariant violation (§3.3) — STOP, do not "fix" the
   golden; find and fix the bug in Task 12's `partition_items` branch (the
   `<= 1` distinct-`SkyModule`-bucket collapse case) instead. This is exactly
   the kind of finding CLAUDE.md's no-deferral principle requires entering the
   pipeline immediately, not worked around. (In practice Task 13's execution
   found ZERO single-home regressions — every red test was a correctly-firing
   multi-home split.)

**Verify.** Full `golden_*` suite green (0 failed). `git diff --stat
tests/golden/` shows changes under `multi_mod_split_pilot/` (Task 12) AND the
regenerated `mm_diamond/` + `mm_local_pkg/` dirs (Task 13) — all hand-read +
SEAL-verified.
**Done when.** The true blast radius (~6 goldens) is measured, every
genuinely-multi-home golden is regenerated + SEAL-verified, and the full suite
is green — a verified fact backed by a `git diff` artifact and a per-project
`cargo build` proof, not an assumption.

### Task 14 — `RustFileId` as a real salsa-interned domain

**Files:** edit `crates/sky_db/src/lib.rs`.

**Steps:**

1. Failing test in a new `crates/sky_db/tests/phase9_emit_rust_file.rs`:
   intern two `RustFileId`s with different `home`s via
   `RustFileId::new(db, home)`, assert they compare unequal and that
   interning the SAME `home` twice returns the SAME salsa `Id` (standard
   salsa-interned-key smoke test, mirrors this project's own `Task 1`-
   style smoke tests for prior interned/tracked types).

2. Confirm it fails to compile (`RustFileId` doesn't exist in `sky_db`
   yet — Task 1's `RustFileId` lives in `sky_backend_rust` and is a
   DIFFERENT, non-interned type; `sky_db`'s is new, per §4.1).

3. Add the `#[salsa::interned] pub struct RustFileId { pub home: ModPath }`
   definition from §4.1.

4. Run; green.

**Verify.** `cargo test -p sky_db phase9_emit_rust_file::`.
**Done when.** `RustFileId` is a genuine salsa key.

### Task 15 — `program_rust_file_ids`, `emit_spine_file`, `emit_rust_file` tracked queries

**Files:** edit `crates/sky_db/src/lib.rs`.

**Steps:**

1. Failing tests (add to `phase9_emit_rust_file.rs`):
   - `emit_spine_file_memoized_coarse_floor` — same shape as every prior
     seam's memoization proof (`typecheck_memoized_coarse_floor` etc.):
     repeat demand + byte-equal re-save execute zero `WillExecute` events;
     a dependency (source) edit re-executes.
   - `emit_rust_file_memoized_per_file` — for a 2-module warm session,
     demand `emit_rust_file(file_a)` and `emit_rust_file(file_b)`; edit
     module A's body; re-demand both; assert `emit_rust_file(file_a)`'s
     `WillExecute` fires (or its value differs) while `emit_rust_file
     (file_b)`'s produced STRING is unchanged (the §4.3 red-green proof —
     assert on the VALUE, not the execution-count, since the underlying
     node is still expected to re-run given `lower_program`'s coarse
     re-execution; the useful assertion is value-equality, not
     zero-executions).
   - `program_rust_file_ids_tracks_module_add_delete` — add a third `.sky`
     module (with a distinct `home`) to the warm session's `SourceRoot`;
     assert the returned `BTreeSet<RustFileId>` grows by one.

2. Confirm all three fail to compile / fail red for the right reason.

3. Implement `program_rust_file_ids`, `emit_spine_file`, `emit_rust_file`
   per §4.2's table, calling into `sky_backend_rust::project::{emit_spine,
   emit_module_file}` (Task 10) and `partition_items` (Task 4) — or, more
   precisely, a thin `sky_backend_rust`-exposed function that returns the
   `home` set directly (`rust_file_homes(program) -> BTreeSet<ModPath>`)
   for `program_rust_file_ids` to wrap.

4. Run; all green.

**Verify.** `cargo test -p sky_db phase9_emit_rust_file::`.
**Done when.** All three proof tests pass, matching the exact rigor of
every prior phase's memoization + coarseness-regression-proof pair.

### Task 16 — `emit_manifest`, wired to replace `emit_project` at the `compile_prepared` call site

**Files:** edit `crates/sky_db/src/lib.rs`; edit `crates/skyc/src/lib.rs`.

**Steps:**

1. Failing test: `emit_manifest_matches_emit_project_for_single_module` —
   for a SINGLE-module program, assert `emit_manifest(...)`'s
   `EmittedProject` is BYTE-IDENTICAL to `emit_project(...)`'s (the
   Spine-collapse invariant, proven at the salsa layer now, not just the
   backend layer).

2. Confirm it fails to compile (`emit_manifest` doesn't exist yet).

3. Implement `emit_manifest` per §4.2's table (assembling `emit_spine_file`
   + every `emit_rust_file` + the existing runtime-shim/`Cargo.toml`
   construction `emit_project` already performs — factor THAT part into a
   small shared helper both `emit_project` and `emit_manifest` call, so it
   is not duplicated).

4. Run; green.

5. Change `compile_prepared` (`crates/skyc/src/lib.rs:516`) to demand
   `sky_db::emit_manifest` instead of `sky_db::emit_project`.

6. Re-run the FULL existing test suite (`cargo test -p skyc`,
   `cargo test -p sky_db`) — must stay fully green, including the
   Task-18 clean-vs-incremental parity gate
   (`crates/skyc/tests/clean_vs_incremental_parity.rs`) and the
   adversarial probe (`adversarial_review_parity_probe.rs`).

**Verify.** Full `cargo test` workspace run green; `git diff --stat
tests/golden/` still shows no unexpected changes.
**Done when.** `compile_prepared` runs on the new fine-grained query graph
end to end, with THE SEAL and the parity gate both still holding.

### Task 17 — the actual incrementality proof, end to end

**Files:** create `crates/sky_db/tests/phase9_incrementality_e2e.rs`.

**Steps:**

1. Failing test: build a warm session (mirroring
   `clean_vs_incremental_parity.rs`'s `WarmSession` shape) over the Task-11
   pilot fixture's TWO modules; demand `emit_manifest`; edit ONLY
   `Lib.sky`'s body (no signature/export change); re-demand
   `emit_manifest`; assert via the `SkyDatabase::with_event_callback`
   mechanism (Phase 1 §3.1's own memo-hit proof mechanism) that
   `emit_rust_file` fires a `WillExecute` for `Lib`'s `RustFileId` but
   `emit_spine_file`'s OUTPUT VALUE is unchanged (backdated) — the actual
   "body edit → only that module's compiled-unit-relevant output changes"
   property this whole task exists to deliver.

2. Confirm it fails for the right reason before the implementation (should
   already pass once Task 16 lands correctly — if it does NOT, that is
   itself the discovery this test exists to make; do not weaken the
   assertion to make it pass).

3. Run; green.

**Verify.** `cargo test -p sky_db phase9_incrementality_e2e::`.
**Done when.** The end-to-end incrementality property Task 15 (the
original 26-task plan's numbering) exists to deliver is proven by a real
salsa event-stream assertion, not inferred from the query graph's shape.

---

## 6. Decisions ledger

1. **Per-file emit is built by partitioning the ONE lowered `Module` by
   `home`, not by inventing a `program_ir_module(ModuleId)` IR domain** —
   the load-bearing finding of this spec (§1.2); strictly smaller and
   safer than the redesign §10.2's pessimistic reading implied was
   required, while being faithful to what §10.2 actually verified (no
   such IR domain exists — true; not needed for THIS problem — new
   finding).
2. **Cross-file visibility is a flat glob-reexport barrel, not a
   hand-built selective `pub`/`use` graph** — sound because names are
   ALREADY globally unique and fail-closed-checked (§1.3); a bespoke
   reference-analysis pass would be new soundness surface for no
   compensating benefit, rejected under efficiency-yields-to-soundness.
3. **Record structs, DB-projection impls, AND the `SqlValue`/`SqlField`
   enum declarations those impls project onto are fixed to `Spine`, never
   module-partitioned** — avoids the ownership-instability class §10.2
   flagged (a shared shape's file migrating across builds as usage shifts
   between modules); recorded as a deliberate coarser-than-optimal floor,
   matching this project's own established "sound floor now, finer
   refinement later, explicitly out of scope" pattern. `SqlValue`/`SqlField`
   were added to this decision (not merely documented as already covered)
   by an independent-review finding: `partition_items` (Task 4)
   structurally forces them into `Spine` by name — see §2.2.
4. **The Spine-collapse invariant (a program's `partition_items` output
   has exactly ONE distinct `RustFileId::SkyModule` bucket ⇒ byte-identical
   to today's single-file output; the `Spine` bucket's presence/absence
   never gates this) keeps the rollout narrow but NOT zero** — the vast
   majority of the 155 goldens are single-home (one user module + kernel
   imports) and need zero changes, but ~6 tests / 5 binaries are genuinely
   multi-home and legitimately split (Task 13's corrected finding — §3.3's
   blast-radius table). Those were regenerated + SEAL-verified, NOT narrowed
   away; the pilot (a `Std.Db` user-module case) plus the stdlib-source
   splits (`Std.Css`/`Std.Ui.Grid`/`Std.Ui.Transition`) together exercise the
   real split.
5. **The golden-harness migration is staged as its own additive,
   independently-provable step (Milestone B) BEFORE any real split
   (Milestone C)** — directly answers the task brief's request for a
   concrete, testable de-risking mechanism, not "update the tests."
6. **`emit_manifest`/`emit_rust_file`/`emit_spine_file` depend on the
   COARSE `lower_program`, not a hypothetical per-module lowering** — an
   honest, recorded divergence from the original design doc's dependency
   sketch, justified by Phase 4's per-module-lowering continuation still
   being unshipped (§10.4) and by the win being real anyway (§4.3's
   red-green argument) — this task does not block on, or require,
   Phase 4's own still-open continuation.
7. **`emit_project` is kept, not deleted, once `emit_manifest` lands** —
   `sky_backend_rust/tests/golden.rs`'s crate-level byte-oracle and any
   future non-incremental caller keep a whole-program, non-split entry
   point; `emit_manifest` is additive.
8. **Milestones A-C (backend split + harness migration) are scoped to one
   session; Milestone D (salsa wiring) is explicitly staged as the next
   session's work** — mirrors this project's own Task-15-survey → Task-17-
   next-session precedent (§10.4 → §11), rather than forcing a
   under-reviewed salsa design into the same session as a large mechanical
   test migration.
9. **`SqlValue`/`SqlField`'s `Spine` placement is enforced structurally
   inside `partition_items` (option (a)), not merely documented and tested
   (option (b))** — independent-review finding 1. Rejected leaving the
   generic empty-home fallback in place because it reintroduces, for a
   second type, the exact usage/invocation-dependent instability decision
   3 above already rejects for record structs; a `partition_items` special
   case (Task 4) costs little and makes the property true by construction.
   The pilot fixture (Task 11) is additionally extended to exercise
   `Std.Db`, so the interaction is proven, not merely argued sound.
10. **`RecordStruct` names are folded into the SAME shared uniqueness
    registry `enum_names`/`func_names`/`mod_ident`s already use, fail-closed
    on any cross-category collision** — independent-review finding 2, new
    Task 3, landed BEFORE the partition function (Task 4) so "the flat
    glob-reexport barrel is sound because names are already globally
    unique" (§1.3, §2.1) is true of EVERY name-producing path before
    Milestone A begins, not just the two paths (enum, func) that already
    self-checked. `func_names` is included in the shared check even though
    it occupies a namespace (Rust's value namespace) that cannot ACTUALLY
    collide with `RecordStruct`/`EnumDef`/`mod` (Rust's type namespace) —
    a deliberate over-inclusive margin, since the check is cheap and it
    removes a dependency on today's naming-convention casing split
    (CamelCase types vs snake_case funcs) staying that way forever.

## 7. Proof-test inventory (once implemented)

| Test | Asserts |
|---|---|
| `sky_backend_rust::record_struct_namespace::*` (Task 3) | A synthesised record struct can never silently shadow an enum/func/mod-ident Rust name — fails closed with `Diagnostic::Name::DuplicateValue` on a REAL constructed collision (`module ["Rec"] { type XY }` vs. record fields `{x, y}`, both folding to `"RecXY"`) |
| `sky_backend_rust::rust_file::partition::*` (Task 4) | `partition_items` is total: no item lost or duplicated, across a multi-module and a single-module fixture; `SqlValue`/`SqlField` route to `Spine` regardless of the `home` set's shape |
| `sky_backend_rust::rust_file::mod_ident_is_stable_and_distinct_for_distinct_homes` / `duplicate_mod_idents_fail_closed` (Task 2) | The new mod-name namespace is deterministic and fails closed (typed diagnostic, never a panic) on collision |
| `sky_backend_rust::golden::*` (re-run, Task 5) | Routing `emit_program` through `partition_items` changes zero emitted bytes for every existing single-module golden, INCLUDING the `Std.Db` goldens (`golden_m5b_db`, `golden_m5b_db_gates`, `golden_db_wrapper_empty_params_165`) the Task-5 ordering rule protects |
| `skyc::golden_m0::emits_byte_identical_main_rs_and_vendors_runtime` (Task 7) | The new directory-diff helper has identical discriminating power to the retired single-file `assert_eq!` |
| `skyc::golden_harness_coverage::every_non_allowlisted_golden_test_calls_the_shared_helper` (Task 9) | Migration completeness is a machine-checked POSITIVE structural proof (every non-allowlisted golden calls the shared helper), not a negative grep a syntactically-different stale pattern could dodge, and not a one-time claim |
| `skyc::golden_multi_mod_split_pilot::*` (Tasks 11-12) | The real split's byte-level output, the multi-module + `Std.Db` `SqlValue`/`SqlField`-routing interaction (§2.2's fix, previously unverified per independent review), AND (`SKY_E2E`-gated) `cargo build` success on the FIRST genuinely-split fixture |
| Full golden suite re-run, `git diff --stat tests/golden/` (Task 13) | Measured blast radius: the vast majority of the 155 goldens need no file changes, but ~6 tests / 5 binaries are genuinely multi-home (§3.3 table) and legitimately split — each regenerated to its correct multi-file shape and SEAL-verified (`cargo build` + run of the split project), not narrowed away. The ORIGINAL "zero blast radius / 154 of 155" claim was a false premise, corrected here in place. |
| `skyc::{golden_mm,golden_class1_boundary_scheme_field_result,golden_css_source,golden_stdui_grid_seal,golden_stdui_transition_seal}` (Task 13, corrected) | Each genuinely-multi-home golden's post-split output: dir-diff goldens (`mm_diamond`/`mm_local_pkg`) byte-lock Spine-only `main.rs` + barrel + every `sky_mods/*.rs` (symmetric compare); substring goldens scan the whole emitted `src/` tree via `support::read_all_emitted_src`; the css/grid/transition splits `cargo build` under `SKY_E2E` (the stdlib-home SEAL the pilot did not cover) |
| `sky_db::phase9_emit_rust_file::emit_spine_file_memoized_coarse_floor` / `emit_rust_file_memoized_per_file` / `program_rust_file_ids_tracks_module_add_delete` (Task 15) | Real salsa memoization + the module-add/delete visibility edge, matching every prior phase's proof-test rigor |
| `sky_db::phase9_emit_rust_file::emit_manifest_matches_emit_project_for_single_module` (Task 16) | The Spine-collapse invariant holds at the SALSA layer, not just the backend layer |
| `skyc::clean_vs_incremental_parity` + `adversarial_review_parity_probe` (re-run, Task 16) | THE SEAL and the parity gate hold after `compile_prepared` switches to `emit_manifest` |
| `sky_db::phase9_incrementality_e2e::*` (Task 17) | The actual, end-to-end "body edit to one module → only that module's compiled-unit-relevant output changes" property, proven via salsa's own `WillExecute` event stream — the mission proof this whole spec exists to deliver |
