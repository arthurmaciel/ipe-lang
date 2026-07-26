# Flat auto-imported stdlib namespace — design

## DECIDED SHAPE (user, 2026-07-03) — supersedes Q1/Q2 exploration below

The namespace is **`Ipe.<module>`, single-rooted and reserved**, not bare-flat:
- Unify `Ipe.*` + `Ipe.*` → one `Ipe.*` root, dropping both the `.Core`
  sub-tier and the `Std` tier: `Ipe.String`→`Ipe.String`, `Ipe.Db`→`Ipe.Db`,
  `Ipe.Ui`→`Ipe.Ui`. Deeper structure is kept (`Ipe.Ui.Background`,
  `Ipe.Http.Server`).
- **Reserve the `Ipe.` prefix**: a user `module Ipe.Foo` (or `import X as Ipe...`)
  is a hard compile error (parse-don't-validate). Stdlib owns the prefix and can
  grow without breaking user code.
- **Access stays QUALIFIED** (`Ipe.String.map`). This is the key simplification:
  the bare-name member-collision problem (`String.map` vs `List.map`) the swarm's
  Q1/Q2 wrestled with **does not exist** — every member is reached through its
  module qualifier. So N0026's user-vs-stdlib-*bare*-shadow machinery collapses to
  a single rule: reject a user module/alias in the reserved `Ipe.` root.
- **Auto-import + DCE is a SEPARATE, OPTIONAL layer** on top (auto-make the `Ipe.*`
  set available so an explicit `import` isn't required; DCE prunes the unused). It
  is decoupled from the unification+reservation and cannot block it.

Still in force from the swarm below: the **DCE-keeps-small guarantee** (Q3 + G2/G3),
the **migration/compat** path (Q4), the **rename-#59 coupling + upstream-Sky
preservation** (Q6), and harden **G1** (alias-table injectivity — stdlib-author
hygiene, still relevant). The bare-name collision analysis (G4/N0026-two-sites) is
mostly moot under qualified access — keep only the reserved-root rejection.

---

Status: DESIGN (locked Q1–Q6, open decisions listed). No code, no build.
Author: guardian design-swarm synthesis + user decided-shape (2026-07-03).
Supersedes the panel drafts.
Coupled to: rename #59 (Ipê → Ipê), memory `post-completion-rename-and-namespace`,
audit `docs/architecture/principled-decisions-audit.md` #11 (DCE).

Principle order governing every ruling below:
**security > correctness > soundness > efficiency > completeness > readability.**
Two invariants ride on top: *parse, don't validate* and *make invalid states
unrepresentable*. Corollary enforced throughout: **an ambiguous auto-imported
name is a loud compile error, never a silent pick.**

---

## Executive summary

Replace the two-tier `Ipe.Core.* / Ipe.*` import surface with a single **flat,
auto-imported namespace of module qualifiers**. You write `String.map`,
`List.map`, `Db.query` with **no import line**; the module prefix stays as the
mandatory disambiguator. This is *not* a fully-unqualified surface — bare `map`
would collide across 6–8 modules and an HM-only compiler cannot soundly
overload-resolve it. The design is mostly *formalising an invariant that already
half-ships*: `install_prelude_qualifiers` (`src/compiler/canon/src/env.rs:202`)
already auto-seeds kernel qualifiers, and kernels emit nothing unless referenced.

The real work is two hardening items, both code-confirmed this session:

1. **Close a live silent-merge hazard.** `qual_vars.entry(q).or_default().extend(m)`
   (`env.rs:1036` / `:1063`) is last-writer-wins on a colliding qualifier key.
   The flatten *widens* this table, so it must become a checked, erroring insert
   driven from one `STDLIB_MANIFEST`.
2. **Reseed reachability off referenced qualifiers, not import decls.** Ipê-level
   DCE is absent today (audit #11: emit-all + LLVM strip). Auto-import deletes the
   import edge that gates module inclusion (`src/ipe-cli/src/project.rs:285`), so
   inclusion must be re-rooted on the set of referenced qualifiers — otherwise the
   build either link-fails or compiles the whole stdlib every time.

Binary size is unaffected (LLVM link-strip already bounds it). Ambiguity is
foreclosed structurally (qualified members) or loudly (build tripwire / N0026 /
N0024). Migration is a **hard cut before the public push**, executed as bisectable
commits. Rename #59 lands **first**; flatten lands **second**.

---

## Ground truth (verified against the code)

- **F1. Qualifier auto-install already exists.** `Env::install_prelude_qualifiers`
  (`env.rs:202`) seeds a hardcoded `QUALIFIERS` table into
  `qual_vars: BTreeMap<Symbol, BTreeMap<Symbol, VarHome>>`. `String.fromInt`,
  `List.map`, `Math.pi` resolve with zero imports today. The explicit
  `import Ipe.Core.String as String` is near-redundant plumbing.
- **F2. Disambiguation is structural.** `qual_vars` is keyed `(qualifier → name →
  VarHome)`. The tripwire `no_colliding_qualifier_name_pairs`
  (`src/compiler/kernels/src/lib.rs`) already proves no two kernels share a
  `(qualifier, name)`. Collisions appear only if the qualifier dimension is
  collapsed.
- **F3. Silent qualifier merge is live.** `env.rs:1036` / `:1063` use
  `.extend(...)` on a shared qualifier key — last-writer-wins, no error.
- **F4. Ipê-level DCE is absent.** Audit #11: Ipê emits all defs
  (`for func in &module.funcs`) and relies on rustc/LLVM link-strip. Class "P4
  efficiency (build speed) only", priority low. Embedded stdlib
  (`src/ipe-cli/src/stdlib.rs`, fixed 18-module `include_str!` set) is pulled into
  the compile graph by import/reference resolution, then all its defs emit.
- **F5. Kernel-backed defs emit no Rust body.** `Ffi.kernel "String_fromInt"` →
  runtime dispatch. Auto-importing kernel modules costs zero emitted source; only
  the runtime crate carries them, and LLVM strips the unused.
- **F6. `Ipê`/`Std` are reserved first path segments** (IPE-N0025,
  `src/compiler/canon/src/resolve.rs:108`). `AmbiguousImport` (IPE-N0024,
  `resolve.rs`) already errors when two deps expose the same unqualified name.
- **F7. `Ipe.*` is not yet ported to the Rust fork.** Only 18 `Ipe.Core.*` modules
  ship. `Ipe.Db/Auth/Ui/Time` describe the Go/Haskell reference. This makes the
  `Time` collision (Q2) *latent*, not present — it must surface loudly at port
  time.

---

## Q1 — SHAPE

**Decision: flat-with-module-prefixes, auto-imported. NOT fully-unqualified.**

- Every stdlib module's qualifier is auto-in-scope under its short canonical name
  (`String`, `List`, `Dict`, `Set`, `Math`, `Db`, `Auth`, `Ui`, `Json.Encode`,
  `Json.Decode`, …) — no `import` line.
- Member access stays **qualified**: `String.map`, `List.foldl`, `Db.query`.
- Flattening rule: **strip exactly the tier-root prefix (`Ipe.Core.` / `Ipe.`),
  preserve the remaining qualifier path.** `Ipe.Core.String → String`;
  `Ipe.Core.Json.Encode → Json.Encode` (multi-segment leaf preserved — collapsing
  to bare `Json` would collide Encode/Decode); `Ipe.Ui.Background → Ui.Background`.
- The **unqualified surface stays the existing curated, CLOSED Prelude**:
  `identity`, `always`, `not`, `toString`, `fst`, `snd`, `clamp`, `modBy`,
  `errorToString`, `println`, plus ctors `Just/Nothing/Ok/Err/True/False` and the
  base types. Auto-imported qualifiers do **not** expose their members unqualified.

**Rationale.** A fully-flat bare surface makes `map` (List/Dict/Set/Maybe/Result/
Task/JsonDec), `empty` (Dict/Set/Bytes), `withDefault` (Maybe/Result),
`insert`/`get`/`member`/`union` (Dict/Set), `succeed`/`andThen` (Task/JsonDec),
`encode` (JsonEnc/Jwt), `send` (Email/WebSocket) collide. Under *ambiguity = error*
the only sound outcomes are error-everywhere (useless exactly where the stdlib is
richest) or silent-pick (forbidden). This fork is **HM-only** (limitation #1), so
type-directed overload resolution is unavailable. Keeping the qualifier makes
member collision *structurally impossible* — invalid states unrepresentable at the
syntax layer, not a runtime check. This is Elm's surface minus the `import`
ceremony; the divergence from Elm (dropping the explicit import) is recorded per
the sanctioned-divergence policy.

**The bare Prelude is never auto-widened.** A uniqueness predicate must not
*promote* a name into the bare surface: a name globally-unique today would silently
seize a slice of the stdlib's own future namespace the day a new module wants to
reuse it. Membership is a hand-reviewed, closed allowlist; global uniqueness is a
*guard on* membership (a promoted name must be unique), never an admission trigger.

---

## Q2 — COLLISION HANDLING

**Decision: the qualifier is the permanent disambiguator; every residual ambiguity
is a hard error sourced from a single manifest. No precedence, no silent shadow.**

Four layers, each making an ambiguity class unrepresentable or loud:

1. **Member level — impossible by construction.** Access is always
   `Qualifier.member`; `String.map` and `List.map` are distinct names. No change
   needed (F2).

2. **Stdlib qualifier uniqueness — build-time tripwire.** Replace the silent
   `.extend(...)` at `env.rs:1036` / `:1063` (F3) with a **checked insert** that
   errors when two stdlib modules claim the same qualifier with differing member
   sets. This is caught by a compiler unit test (`stdlib_qualifiers_unique`), so a
   *shipped* compiler cannot contain an ambiguous stdlib qualifier. **This is the
   highest-priority hardening — the flatten widens exactly this table, and the
   silent merge already exists.**

   The one genuine latent instance: `Ipe.Core.Time` (kernel) + a future `Ipe.Time`
   (IANA zones) both strip to `Time` (F7). The checked insert refuses to build when
   `Ipe.Time` ports, forcing a stdlib-author merge/rename decision at that moment
   rather than a silent last-writer-wins. Audit `Http` for the same overlap.

3. **User qualifier vs stdlib qualifier — hard error `IPE-N0026`.** `module Db
   exposing (..)` or `import MyLib as String` where the name equals an
   auto-imported stdlib qualifier → `NameError::QualifierShadowsStdlib` with a
   did-you-mean. This **generalises the existing N0025 `Ipê`/`Std` reservation**
   from 2 roots to the ~40 leaf qualifiers. Rationale for hard-error over
   warn-shadow: the stdlib qualifier is auto-imported *implicitly* — the user never
   opted into the collision, yet a warned shadow silently repoints *every* `Db.foo`
   call site in that module. Warnings are ignored; the blast radius is all
   references, not the one declaration. Fail-closed is the guardian choice. Escape
   hatch: rename the user module, or alias the *stdlib* module under a new
   qualifier — never the reverse. Lowercase locals (`string`, `db`) are a distinct
   lexical class and never collide.

4. **Bare-name user ambiguity — existing `IPE-N0024`.** Two user modules exposing
   the same unqualified name stays the current error, preserved and golden-tested
   against the auto-import widening.

**Single-source-of-truth mandate.** The qualifier map, `qual_vars`,
`stdlib_index`, and the reserved-qualifier set MUST all be derived from one
`STDLIB_MANIFEST` table, with a drift tripwire (extend the existing
`canon_equals_registry`) asserting the four agree. Four hand-maintained lists are
guaranteed drift; one generated source forecloses it. The checked insert of layer 2
is *how* the manifest build fails loudly.

---

## Q3 — DCE / EFFICIENCY (hello-world stays small)

**Decision: separate the two cost axes. Binary size is already bounded; the hard
prerequisite is module-inclusion-by-reference. Full def-level DCE is a
size-gate-enforced fast-follow, not a flatten blocker.**

**Axis A — final binary size — bounded, no new work.** Emission is a function of
*reachable call sites*, not *in-scope names*. Kernels emit only at referenced call
sites (`Callee::Kernel`); unreferenced runtime fns and emitted defs are never
referenced from the entry root → LLVM link-strip drops them (F4, F5). Auto-importing
the whole stdlib adds `BTreeMap` entries in the *compiler's* memory and zero bytes
to the *user's* binary. **This is exactly the guarantee Q3 asks for, and it holds
unconditionally.**

**Axis B — generated source + cargo compile time — HARD prerequisite.** Today an
embedded stdlib module enters the compile graph via import/reference resolution,
then all its defs emit (F4). Auto-import deletes the import line, so:

- **Module-inclusion-by-reference (blocking, cheap, mandatory with flatten).**
  Re-seed the entry-rooted DFS (`project.rs:285`) from the set of **referenced
  qualifiers** — a free by-product of canonicalisation, since access is always
  `Qualifier.member`. The used-module set is exactly the set of referenced
  qualifiers. Without this, auto-import either under-includes (`String.map`
  referenced but `String.ipe` never emitted → link failure) or, if naively
  "include all auto-imported", compiles all 18+ (soon 40+) modules every build (a
  real P4 regression). Enforce with a **hello-world compile-unit-count / binary-size
  regression test**: neither may grow when `qual_vars` grows.

- **Def-level DCE, audit #11 (staged, NOT blocking).** Pruning unreferenced defs
  *within* a referenced module (typed `Ref` / `FfiRef` / `CtorRef` reachability)
  trims how many defs cargo compiles. It becomes load-bearing only as pure-Ipê
  module def-counts grow — strong-recommend before a large pure-Ipê module (`Ipe.Ui`
  / `Ipe.Db`) lands. The size-regression gate self-enforces this: the moment
  auto-import bloats hello-world by one unused def, the gate goes red and #11
  becomes mandatory. No human "is it time yet" judgment.

**Soundness rule (both DCE layers).** **Keep all constructors of any referenced
type** — do not prune sister ctors. Sisters are tiny; pruning risks
exhaustiveness / `toString` / reflection paths. Do not port Haskell's ctor-closure
fixup. Top-level Ipê bindings are pure (effects are Tasks forced only at the entry
boundary), so a pruned unreferenced def has no observable effect — DCE is observably
sound. `let _ = TaskExpr` auto-force lives inside a reachable body, never a top-level
def, so DCE never touches it.

**Registry ↔ DCE separation (non-negotiable invariant).** The kernel registry /
`qual_vars` is a *resolution* surface and must **never seed inclusion**. Inclusion
is seeded solely by referenced qualifiers/names. Any future "preload/link-all-
kernels" shortcut breaks the guarantee and must be rejected.

---

## Q4 — MIGRATION

**Decision: hard cut of the two-tier long paths, completed before the public push;
executed as bounded, bisectable commits. Single-leaf explicit imports stay
permanently first-class. No permanent long-path compat shim.**

The fork is pre-public-push (`arthurmaciel/ipe-lang` is the endgame) → **no external
users to protect**. A permanent layer accepting both `import Ipe.Core.String as
String` *and* auto-`String.` creates two resolution paths for one name — a direct
make-invalid-states violation — for zero external benefit.

Sequence (each landing independently green; satisfies the green-everywhere gate and
keeps regressions bisectable):

1. **Flatten-additive.** Auto-import lands; old `Ipe.Core.*` / `Ipe.*` paths still
   resolve for this one window. **Differential resolution test**: for every example,
   the resolved `(qualifier, member)` set is byte-identical before/after
   auto-import — proves auto-import is purely additive and re-points nothing.
2. **Codemod.** Mechanically delete now-redundant stdlib imports and rewrite any
   fully-pathed `Ipe.Core.Foo.bar` / `Ipe.Foo.bar → Foo.bar` (strip tier root,
   preserve multi-segment leaves per Q1). Delete-lines refactor, `ipe fmt`-idempotent.
3. **Terminal hard cut.** Remove the long-form aliases → single resolution path.
   `import Ipe.Core.X` / `import Ipe.X` become errors. **This closes before the
   public push** — shipping a public language day-one with two ways to path the
   stdlib plus a pending deprecation is a self-inflicted make-invalid-states
   violation.
4. **Migration collision lint.** Flag every user module whose name equals an
   auto-installed qualifier during the sweep, so no silent behaviour change slips
   through (feeds Q2 layer 3).

**Single-leaf explicit imports remain first-class forever** — `import Db`, `import
Db as Database`, `exposing (query)`. This is Elm's legitimate aliasing/exposing
escape hatch, *not* a migration shim. `import Db as Database` for user renaming and
`exposing (query)` for pulling a name unqualified both stay.

Churn scope: because nearly all existing code already accesses `Qualifier.member`,
the delta is import-line deletion + root-prefix stripping. The one non-mechanical
change is the `Time` merge (Q2), and it surfaces mechanically via the tripwire.
Template-sync is a project non-negotiable: `AGENTS.md`, `templates/AGENTS.md`, and
`docs/stdlib.md` import examples are rewritten in the same commit as the terminal cut.

---

## Q5 — DE-ABBREVIATION (rides rename #59)

**Decision: expand a name iff it is (a) project-invented, (b) on a user-facing
surface or a place a newcomer reads first, and (c) not an established Elm / Rust /
compiler idiom. Failing any clause → keep. Readability is the lowest-priority
principle, so de-abbreviation must never touch a name that changes emitted output.**

- **Keep (Elm/Haskell-canonical — load-bearing for the "adopt Elm core" roadmap):**
  `fst`/`snd`, `Dict`, `Cmd`/`Sub`, `Msg`, `andThen`, `map2..5`, `foldl`/`foldr`,
  `modBy`. Expanding these breaks muscle memory and portability.
- **Keep (universal Rust/compiler idiom):** `env`, `decl`, `expr`, `stmt`, `ctor`,
  `impl`, `fn`, `str`, `Vec`, `Arc`. Expanding churns huge internal surface for no
  clarity gain.
- **Keep (domain / wire / protocol tokens — parity-locked):** `sha256`, `hmac`,
  `rsa`, `aes`, `jwt`, `uuid`, `utf8`, `iso8601`, `rfc3339`, `csv`, `toml`, `html`,
  `css`, `url`, `id`, `db`. These are the *correct* names.
- **Expand (project-invented, user-facing, cryptic):** e.g. `cfg → config`,
  `ctx → context`, `req`/`resp → request`/`response` in *public* signatures (keep in
  local params where idiomatic). Compiler-internal names (`qual`, `n_index`,
  `kernel_ty`) are in scope only if they surface in a user diagnostic.

**Guardrails.**
- De-abbreviation is a **pure, semantics-preserving rename** and MUST be
  **byte-neutral on all emitted artifacts** — JSON keys, SQL columns, HTTP headers,
  Go-parity goldens, `_fieldIndex` ordering. Any expansion that alters a serialised
  token is a correctness regression and is reverted. Verified by the equivalence
  harness.
- **ABI lockstep.** A rename that touches a kernel key (`Ffi.kernel "String_fromInt"`)
  must move the registry decl, the auto-install table, and the lower.rs match arm
  **in lockstep**, or it silently produces an unresolved name → the
  `qualifier-set-must-equal-constrain-set` desync class. Enforced by the
  `canon_equals_registry` tripwire. A rename that is *only* internal (never in a
  `.ipe` signature) can be a hard rename with no alias.
- User-visible public renames keep the old name as a deprecated alias through the
  flatten window (compat), removed at the terminal cut.

---

## Q6 — COUPLING TO RENAME #59

**Decision: rename #59 (+ de-abbreviation) FIRST, flatten SECOND. Two separate,
independently-green, bisectable passes. One qualifier registry, not two. Upstream-
Ipê provenance preserved throughout.**

Ordering rationale:

1. **Bisectability (soundness of review).** #59 is a mechanical, case-preserving,
   semantics-preserving global rename. Flatten is a *semantic* change to name
   resolution + reachability. Landing them separately makes any regression
   attributable to exactly one change; combined, a broken example is ambiguous and a
   resolution bug hides under thousands of path-rename lines.
2. **The flatten's tables are authored once in final vocabulary.** The single-segment
   qualifier registry, the reserved-qualifier list, and the de-abbreviated spellings
   (which ride #59) are computed from *final* `Ipe.Core.*` / `Ipe.*` names.
   Flatten-first would build them around `Ipê.*` and rewrite them again.
3. **De-abbreviation is part of the #59 pass** (Q5), so the flatten consumes
   already-final identifier names.

Concrete order:

1. **#59** — rename `Ipê → Ipê`/`Ipe`, extension `.ipe → .ipe`, `IPE-N00xx → IPE-N00xx`,
   reserved segments flip `{Ipê, Std} → {Ipe, Std}`, de-abbreviation. Old public
   names kept as deprecated aliases through the flatten window. Own commit, green
   against the sweep.
2. **Flatten** — build the `STDLIB_MANIFEST`; auto-install short qualifiers
   exhaustively from it; close the `extend` silent-merge into a checked insert; add
   `IPE-N0026`/`IPE-N0026`; reseed reachability off referenced qualifiers. Gated by
   new tests (uniqueness tripwire, N0026, differential-resolution, hello-world size
   gate).
3. **Codemod** import-strip + differential test (Q4 step 2).
4. **Terminal hard cut** of long-form paths (Q4 step 3), before push.

**Single registry.** The auto-installed manifest and `StdlibKernel::decl().qualifier`
strings are one source of truth feeding both the rename and the flatten. Do not fork
a second registry for flat mode — that reintroduces the two-source drift the tripwire
tests exist to prevent.

**Upstream-Ipê reference preservation (public-artifact rule).** The flatten touches
*code* only — the live reserved namespace drops `Ipê` and the qualifier registry
holds no `Sky` entry. It does **not** rename the curated upstream-Sky provenance:
the single README credit line, the `docs/divergences-from-sky.md` references, the
upstream Sky paths, and the embedded-source provenance comments in
`src/ipe-cli/src/stdlib.rs` are on the naive-sed exclusion list and are preserved
verbatim. No disparagement of the upstream project appears in code or docs.

---

## Locked decisions (spine)

| Q | Decision | Primary principle |
|---|---|---|
| Q1 Shape | Flat, auto-imported **module qualifiers**; `Qualifier.member` access; strip tier-root, preserve leaf path; bare Prelude = closed hand-curated allowlist | soundness > readability |
| Q2 Collision | Qualifier disambiguates (member collision impossible); one `STDLIB_MANIFEST`; `extend → checked erroring insert`; user-vs-stdlib = **hard error SKY/IPE-N0026**; bare = existing N0024; `Time` merge forced loudly at port time | make-invalid-states-unrepresentable |
| Q3 DCE | Binary bounded by LLVM (holds). **Module-inclusion-by-reference = hard prerequisite**; def-level #11 = staged, self-enforced by hello-world size-regression gate; keep all ctors of referenced types; registry never seeds inclusion | efficiency |
| Q4 Migration | **Hard cut before push**, no permanent long-path shim; bounded bisectable commits (additive → codemod → terminal cut); differential-resolution + collision-lint tests; single-leaf imports first-class forever | correctness (no silent re-resolution) |
| Q5 De-abbrev | Expand project-invented + user-facing + non-idiomatic only; keep Elm/Rust/domain/wire tokens; byte-neutral on all emitted artifacts; ABI lockstep via `canon_equals_registry` | correctness > readability |
| Q6 Sequence | Rename #59 + de-abbrev FIRST, flatten SECOND; separate green bisectable passes; one registry; upstream-Sky provenance preserved | soundness-of-review; public-artifact rule |

## Blocking prerequisites (treat as gates, not follow-ons)

1. **`STDLIB_MANIFEST` single-source + checked erroring insert** replacing the
   silent `extend` at `env.rs:1036` / `:1063`. Closes the live last-writer-wins
   hazard the flatten widens, and forces the latent `Time` collision loudly.
2. **Module-inclusion-by-reference** replacing import-edge gating in
   `project.rs:285`. Without it auto-import either link-fails or compiles the whole
   stdlib every build. Def-level #11 is a fast-follow, gated by the size-regression
   test.

## Enforcement tests (new)

- `stdlib_qualifiers_unique` — compiler unit test; a duplicate/differing-member
  qualifier fails the build.
- `canon_equals_registry` (extended) — manifest ↔ `qual_vars` ↔ `stdlib_index` ↔
  reserved-set drift tripwire; also guards ABI-key lockstep for Q5.
- Differential-resolution test — resolved `(qualifier, member)` set byte-identical
  before/after auto-import, per example.
- Hello-world compile-unit-count + binary-size regression gate — self-enforces the
  Q3 efficiency principle and promotes def-level #11 to mandatory exactly when it is
  needed.
- N0026 golden — user qualifier shadowing a stdlib qualifier errors; N0024 survives
  the widening.

---

## OPEN DECISIONS (unresolved forks)

1. **`Time` collision resolution (stdlib-author call, deferred to `Ipe.Time` port).**
   When `Ipe.Time` lands, the tripwire fires. Resolve by either folding the
   `Ipe.Core.Time` kernel entries into a single richer `Time`, or renaming the
   kernel-level module (candidate `Clock`). Audit `Http` for the same overlap.
   Mechanically forced, semantically open. (Same class: any future `Ipe.Core.X` vs
   `Ipe.X` leaf-name overlap.)

2. **User-vs-stdlib qualifier: hard-error vs warn-shadow — LOCKED to hard-error,
   minority dissent noted.** Two of three reconciled panelists and the fail-closed
   guardian principle land on hard error (N0026); one original draft preferred a
   deterministic warn-shadow (user intent wins locally). Locked hard-error; reopen
   only if a concrete ergonomic need for local shadowing surfaces post-push.

3. **Redundant single-segment `import Foo` after the terminal cut: no-op vs error.**
   A leaf-level `import Db` for an already-auto-imported qualifier could resolve as a
   harmless no-op (lets the codemod leave leaf imports untouched) or be an error (one
   resolution path, cleanest make-invalid-states). Leaning error for the long
   `Ipe.Core.*`/`Ipe.*` forms (never a no-op); the single-leaf no-op is a genuine
   micro-decision. Note: `import Db as X` and `exposing (...)` are always valid
   regardless.

4. **Exact membership of the closed bare Prelude.** The *rule* is locked (closed,
   hand-reviewed, uniqueness-guarded). The *contents* beyond today's set require
   per-name human review — no name is admitted by a predicate.

5. **De-abbreviation concrete per-identifier list.** The *rule* is locked (Q5). The
   concrete list of project-invented user-facing names to expand rides the #59 pass
   and is reviewed per-identifier under the byte-neutrality + ABI-lockstep guardrails.
