# Hardening Batch A — spec/plan (parallel, autopilot mechanical lane, Sonnet 4.6)

5 independent-file, genuinely mechanical fixes from `principles-audit-2026-07-09.md`.
Each item below is self-contained: root cause, exact touch points, the PICKED
approach (ambiguity resolved — this is a plan, not a menu), and the regression
test. Dispatched via `orchestrate.sh` (through `autopilot.sh`, capped to one
cycle — see dispatch note at the bottom).

Every item MUST end green on: `cargo build -p skyc` (or the crate touched) +
the crate's existing test suite + a rebuild of the named regression fixture.
Root-cause only — no fixture edits, no gate weakening (CLAUDE.md §3/§7, and the
6 principles + 2 rules + seal in `scripts/progressive-development/context.md`).

---

## A1 — AUD-15: promote `ALL_CODES` public, delete skyc's drifting mirror

**File:** `crates/sky_diagnostics/src/code.rs:451-469` (the `ALL` slice, currently
`#[cfg(test)]`) + `crates/skyc/src/lib.rs:41-110` (the 68-entry hand mirror).

**Root cause:** the diagnostic taxonomy's authoritative code list lives under
`#[cfg(test)]` and is never exported. `skyc` hand-maintains its own copy for
`skyc explain`/`explain_lookup`/`suggestions`, which drifted to 68 of 85 codes —
17 shipped, actively-produced codes (SKY-L0114..L0126, SKY-T0014/15, …) are
unresolvable by the exact command every diagnostic footer tells the user to run.

**Fix (picked):**
1. In `crates/sky_diagnostics/src/code.rs`, move the `ALL` slice out of
   `#[cfg(test)] mod tests` into module scope as `pub const ALL_CODES: &[Code]`
   (keep the existing 85-entry content verbatim — do not hand-retype it).
2. Re-export `ALL_CODES` from `sky_diagnostics::lib` (`pub use code::ALL_CODES;`
   if not already re-exported at that level).
3. In `crates/skyc/src/lib.rs`, delete the hand-mirrored code list (~lines
   41-110) entirely. Replace every call site that iterated the mirror
   (`run_explain`, `explain_lookup`, `suggestions`) to iterate
   `sky_diagnostics::ALL_CODES` instead.
4. Keep the existing 85-count + page-conformance tests in
   `sky_diagnostics::code` pointed at the one list (they already assert against
   `ALL` — just confirm they still compile against the renamed/relocated
   `ALL_CODES`).
5. Fix skyc's own test that currently asserts `SKY-T0014` is unknown to
   `explain` — it should now assert the OPPOSITE (resolvable), since T0014 is
   in the 85-entry taxonomy. Locate via `rg -n "SKY-T0014" crates/skyc/`.

**Regression test:** a test in `crates/skyc/` (or `sky_diagnostics`) that
iterates every entry of `ALL_CODES` and asserts `skyc explain <code>` (or the
in-process `explain_lookup` equivalent) resolves successfully for ALL 85 —
this is the direct proof the drift class is closed.

**Verify:** `cargo test -p sky_diagnostics -p skyc`, `cargo build -p skyc`,
`skyc explain SKY-L0115` (was previously unresolvable) now prints a page.

---

## A2 — AUD-14: qualifier member tables reject a second distinct import, don't overwrite

**File:** `crates/sky_canon/src/resolve.rs:1586-1592` (the `qual_map.insert(v, ...)`
silent-overwrite site).

**Root cause:** `import App.Utils` + `import Lib.Utils` (both default qualifier
`Utils`), or a user module whose last path segment collides with a stdlib
qualifier (`import App.Http` colliding with the kernel `Http` table): the merge
does `qual_map.insert(v, ...)`, which REPLACES any existing entry — so
`Utils.format` / `Http.get` silently resolves to whichever import came LAST in
source order, with zero diagnostic. The unqualified import path already has a
hard `AmbiguousImport` (SKY-N0024) gate for the equivalent case — the qualified
path is the one hole.

**Fix (picked — reject immediately, matching the unqualified path's existing
shape; NOT the deferred-ambiguity-at-use-site alternative — simpler, symmetric,
lower mechanical risk):**
1. Track qualifier→owning-dep_path alongside the existing `qual_map`
   construction (a sibling `BTreeMap<Symbol, DepPath>` or extend the map's
   value type with the owning dep path if it doesn't already carry one — check
   what `qual_map`'s value type currently is before choosing).
2. Before `qual_map.insert(v, ...)`, check whether `v` (the qualifier symbol)
   already has an entry from a DIFFERENT dep_path (re-importing the SAME
   dep_path under the same qualifier — e.g. `sky.toml`-driven re-resolution —
   must stay idempotent/accepted, mirroring how `inject_dep_type`'s
   diamond-dependency rule already treats identical re-injection as a no-op).
3. On a genuine clash (different dep_path claiming an existing qualifier), do
   NOT insert; instead emit a new `NameError` variant — add
   `NameError::DuplicateQualifier { qualifier: Symbol, first_origin: DepPath,
   second_origin: DepPath, span: Span }` (follow the existing `NameError` enum's
   field/span conventions — model it directly on the neighboring
   `DuplicateType`/`DuplicateValue` variants) at the SECOND import's span.
4. Wire the new diagnostic code (`SKY-N00XX` — check
   `sky_diagnostics::code::ALL_CODES` after A1 lands, or check the current max
   `SKY-N00xx` in use via `rg -n 'SKY-N00' crates/sky_diagnostics/src/code.rs`
   for the next free number) end-to-end: `Diagnostic`, `code.rs` `ALL_CODES`
   entry + explain page, `render.rs` if it needs a custom render arm (most
   `NameError` variants share a generic render — check first).

**Regression test:** two fixture modules `A.sky`/`B.sky` each declaring
`module A.Utils exposing (format)` / `module B.Utils exposing (format)`
(divergent bodies so a wrong pick is observable), a third module importing
both (`import A.Utils` + `import B.Utils`), asserting the compile fails with
the new `DuplicateQualifier` diagnostic, NOT a silent last-wins resolution.
Also test the stdlib-collision shape: a user module named to collide with a
kernel qualifier (e.g. `App.Http`) imported alongside `Sky.Core.Http`-adjacent
usage — same diagnostic.

**Verify:** `cargo test -p sky_canon`, `cargo build -p skyc`.

---

## A3 — AUD-12: gate numeric-var defaulting on the class's accumulated `Append` obligation

**File:** `crates/sky_types/src/lib.rs:261-273` (the post-solve `Super`
defaulting arm).

**Root cause:** the arm matches `Content::Super { rigid: false, bounds } if
bounds.has_number()` and pins the class root to `Int` WITHOUT checking whether
the class's UNION bounds also carry an `Append` obligation (which `Int` cannot
satisfy). `f x = (x ++ x) + 1` creates the `Append` super first, then the
`Number` super on the same var; defaulting pins to `Int`, skyc accepts, `cargo
build` fails on the emitted `x + x` where `x: i64` doesn't implement whatever
the `++` lowering expected. Order-dependent exit-0-then-cargo-fail.

**Fix (picked + API-verified — `concrete_super_ok` and `super_unsatisfied`
both already exist in this file at `lib.rs:444` and `lib.rs:492`; this is a
pure call-site fix, no new helper needed):**
1. The enclosing loop is `for (v, orig_bounds, span) in &generated.super_vars`
   (`lib.rs:249`) — `span` is already in scope at the match arm.
2. Build `let int_ty = Ty::Con { module: Vec::new(), name: int_sym, args:
   Vec::new() };` (matches the `Ty::Con` shape `concrete_super_ok` itself
   matches on — `int_sym` is already bound just above the loop, `lib.rs:248`).
3. In the `Content::Super { rigid: false, bounds } if bounds.has_number()`
   arm, BEFORE the `uf.set_content(root, Content::Structure(...))` pin, insert:
   `if !concrete_super_ok(interner, bounds.clone(), &int_ty) { return
   Err(super_unsatisfied(interner, bounds.clone(), &int_ty, *span)); }` (adjust
   to match this function's actual return type / the `lift!` macro's error
   convention used everywhere else in this function — every other fallible
   step here goes through `lift!(...)`, so this new check should propagate the
   same way, not diverge with a bare `return Err`).
4. `concrete_super_ok` ALREADY implements the exact check needed
   (`!bounds.has_append() || appendable_ok` is one of its ANDed conditions) —
   do not reimplement the `has_append()` logic inline; call the existing
   function as a single boolean gate covering append AND every other bound
   the class might carry (ord/eq/show/comparable-key), which is MORE correct
   than a narrow `has_append()`-only check would be.

**Regression test:** `f x = (x ++ x) + 1` (from the audit's own witness) as a
`crates/skyc/tests/` fixture — assert `skyc` REJECTS with a type error
(SKY-T0014 or whatever the existing unsatisfied-super code is), NOT silent
accept. Also a positive control: `f x = String.length x + 1` (Number-only,
no Append) must still default to `Int` and build clean — proves the fix
doesn't over-tighten.

**Verify:** `cargo test -p sky_types -p skyc`.

---

## A4 — AUD-07: `SKY_DB_URL` resolves from a real config source, never a hardcoded `sqlite::memory:`

**Files:** `runtime/src/sky_runtime/config.rs:10` (the hardcoded const) +
`crates/sky_backend_rust/src/project.rs:284` (verbatim `include_str!` of
`config.rs` into every generated project) + `runtime/src/sky_runtime/db.rs:646-648`
(`db_connect` consumer) + `db.rs:560` (`url_is_cacheable`, related but see the
note below).

**Root cause:** every generated project embeds `config.rs` VERBATIM (`const
RUNTIME_CONFIG_RS_DB: &str = include_str!(...)`); `SKY_DB_URL` is hardcoded to
`"sqlite::memory:"`; `sky.toml [database] url` / `DATABASE_URL` is never wired
in anywhere in the Rust pipeline. `url_is_cacheable` excludes memory URLs from
the pool cache, so each `Db.connect()` call re-entry builds a fresh pool — a
DISTINCT empty in-memory database each time. Silent data loss on any program
using `Db.connect ()` more than once (the documented per-call lowering shape).

**Fix (picked — runtime env-var read with a project-supplied non-memory
default, matching the project's OWN existing precedent for other config
surfaces, e.g. `SKY_LIVE_STORE_PATH` falling back to `DATABASE_URL`; NOT
per-driver `config.rs` codegen — smaller, safer surface for a mechanical lane):**
1. In `runtime/src/sky_runtime/config.rs`, change `SKY_DB_URL` from a bare
   `pub const SKY_DB_URL: &str = "sqlite::memory:";` to a runtime resolution
   function `pub fn sky_db_url() -> String` that reads `DATABASE_URL` (or
   whatever env var name the project's documented `[database] url` indirection
   uses — check `docs/sky-toml.md` / CLAUDE.md `[database]` section for the
   exact precedent before picking the var name) via the SAME `read_env_var`
   helper `db.rs` already uses elsewhere (do not hand-roll a new env-read),
   falling back to a NON-memory default: `"sqlite://sky.db?mode=rwc"` (a real
   file, created on first connect — never `:memory:`).
2. Update every call site of the old `SKY_DB_URL` const (`db.rs:646-648` and
   any others — `rg -n "SKY_DB_URL" runtime/ crates/`) to call `sky_db_url()`
   instead of referencing the const directly.
3. Do NOT touch `project.rs:284`'s verbatim `include_str!` mechanism itself —
   the fix is entirely inside `config.rs`'s own logic, so the verbatim-embed
   approach is now safe (the embedded file resolves correctly at runtime
   regardless of per-project customization).
4. Leave `url_is_cacheable`'s `contains("memory")` substring check as-is for
   THIS item (it's AUD-09, a separate lower-severity finding, out of this
   batch's scope) — do not conflate the two fixes.

**Regression test:** a `runtime/src/sky_runtime/db.rs` (or wherever
`sky_db_url`/`db_connect` tests live) test asserting: (a) with `DATABASE_URL`
unset, `sky_db_url()` returns the sqlite file default (never contains
`:memory:`); (b) with `DATABASE_URL` set, it returns that value verbatim; (c) a
"shared-connection" behavioural test — two sequential `Db.connect()`-equivalent
calls (or two `connect_cached()` calls with the same resolved URL) observe the
SAME data (write via one, read via the other, assert non-empty).

**Verify:** `cargo test -p sky_runtime` (or wherever `runtime` tests are named
in this workspace — check `runtime/Cargo.toml`'s package name), `cargo build
-p skyc`.

---

## A5 — AUD-11: dotted Access chains counted against `MAX_DEPTH`

**Files:** `crates/sky_parse/src/parser.rs:1109-1113` (`parse_atom_postfix`'s
segment loop) + `:1414-1418` (`ident_expr`'s equivalent loop).

**Root cause:** every genuinely recursive parse path checks `depth >
MAX_DEPTH` (256), but `Expr_::Access` nesting from a DOTTED IDENTIFIER TOKEN is
built ITERATIVELY (`for seg in text.split('.')`, one `Access(Box<...>)` wrap
per segment) with no bound at either site — because the lexer already merged
the whole dotted run into ONE token with no segment cap. A single token with
500k `.a` segments produces a 500k-deep AST in one iteration, defeating the
`MAX_DEPTH` guarantee lib.rs documents ("recursion is bounded... adversarial
input cannot overflow the stack") — first recursive AST traversal (canon, type
solve, or even `Drop` on the deeply-nested boxed `Expr` chain) overflows the
stack. Adversarial-input DoS, reachable from any parsed source file.

**Fix (picked — count segments against the existing `MAX_DEPTH` budget and
reuse the EXISTING `too_deep` error path, at BOTH loop sites; do not add a new
diagnostic variant, do not touch the lexer):**
1. In `parse_atom_postfix`'s segment loop (`parser.rs:1109-1113`), track a
   segment counter as the loop builds `Access` wraps; if the counter exceeds
   `MAX_DEPTH` (the same constant the rest of the file uses — `rg -n
   "MAX_DEPTH" crates/sky_parse/src/parser.rs` for the exact name/value),
   return `Err(self.too_deep(Construct::Expression))` (the existing helper at
   `parser.rs:168`, already used at the neighboring recursive-descent sites)
   instead of continuing to wrap.
2. Apply the identical counter+check to `ident_expr`'s loop
   (`parser.rs:1414-1418`) — same pattern, same constant, same error path.
3. Do not touch the lexer's dotted-continuation loop (`lexer.rs:616-627`) —
   the audit's cheaper "cap at lex time" alternative is explicitly NOT the
   pick here (two independent guard sites at the parser level, matching how
   every other `MAX_DEPTH` check in this file already works, is the lower-risk
   change for a mechanical lane; a lexer-level change touches token shape and
   needs broader re-validation).

**Regression test:** a fixture (or inline `#[test]` in `crates/sky_parse/src/`)
constructing a source string `"x = y" + ".a".repeat(300_000)` (or however many
segments comfortably exceed `MAX_DEPTH`), asserting the parser returns
`Err(...)` (the `too_deep`/`Construct::Expression` diagnostic) rather than
panicking or hanging. Run it under a reasonable timeout in CI (a stack
overflow before the fix would crash the TEST PROCESS, not return an `Err` — so
this test, run pre-fix, should be observed to crash/hang, confirming it
exercises the real bug, then pass clean post-fix).

**Verify:** `cargo test -p sky_parse`, `cargo build -p skyc`.

---

## Dispatch note (for whoever wires the queue — see the companion runbook)

Each item above becomes ONE `PENDING\tmechanical\t<desc>` line in
`docs/architecture/progressive-development-queue.tsv`, where `<desc>` points
the lane at THIS file's matching section by absolute path and explicitly
overrides `prompt.md`'s security/soundness-gate exclusion instinct (several of
these items are security-flavored findings even though the FIX itself is a
clean, bounded, reference-free mechanical wire — pre-vetted by this audit +
spec, not a lane-time design decision). See
`docs/architecture/hardening-2026-07-09-runbook.md` for the exact queue lines
and the `autopilot.sh` invocation.
