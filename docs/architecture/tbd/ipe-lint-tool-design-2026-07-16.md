# `ipe lint` — the source-level lint tool (design + spec + plan)

Status: design-only (no code). Companion design:
`exhaustive-case-finite-adt-design-2026-07-16.md` (the closed-union catch-all
rule) — the two share the directive infrastructure, the fix machinery, and the
LSP quick-fix surface, and this document fixes the boundary between them.

Related specs: `ipe-lsp.md` (diagnostics publishing, code actions, the G2
`VerifiedEdit` gate, hazard L-A), `incremental-compilation-and-watch.md` /
`salsa-incremental-compilation-2026-07-11.md` (the query layer a lint query
joins), `divergence-policy.md`. Reference departure: the Ipê compiler
(`../sky`) ships **no lint subsystem** — no lint pass, no lint CLI command, no
per-site suppression (verified against `src/Ipê/` and the CLI surface; its
`Ipê/Lsp/Diag.hs` republishes compiler diagnostics only). This tool is an
intentional capability **beyond** the reference, ledgered in
`docs/divergences-from-sky.md` §6.

Naming: crates and codes use the current `sky_*` / `IPE-*` prefixes; they
rename with roadmap C.1 (`ipe_*`, `IPE-*`). CLI shown as `ipe lint`
(today spelled `ipe lint`).

---

## Executive summary

`ipe lint` is an opinionated, extensible static-analysis tool for `.ipe`/`.ipe`
**source** programs — the elm-review / clippy analogue for Ipê. Decisions:

| Concern | Decision |
|---|---|
| Analyzer identity | A **third consumer of the compiler's own artifacts** (parse AST + canon AST + `SolvedTypes`), never a second analyzer. A lint rule cannot parse, resolve, or infer anything itself — it reads what `ipe` computed. |
| Crate | New `src/compiler/lint`: rule trait, driver, registry, rules. CLI subcommand in `ipe`; LSP surfacing via `sky_lsp` (its Phase 3). |
| Rule API | elm-review-style **visitor schema over a single driver walk** (one AST traversal for N rules), with a typed `LintContext` exposing solved types by span. Rules are Rust impls registered in a closed `ALL_RULES` table with a drift test (the `sky_kernels` pattern). |
| Identity + levels | Dual identity per rule: stable code `IPE-W####` + kebab-case name (`unused-import`). clippy-style levels `allow`/`warn`/`deny`; defaults per rule; config in `sky.toml [lint]`; per-site `-- @allow(<rule>) <reason>` directive with mandatory reason. |
| Findings | Lint findings are `sky_diagnostics::Diagnostic`s (new `Lint` variant) — one rendering pipeline, one `explain` surface, one `Suggestion`/`Applicability` fix model, one LSP publishing path. |
| Autofix | Rules attach span-scoped `Suggestion`s; `ipe lint --fix` applies `MachineApplicable` edits and **re-checks before writing** (a fix that breaks the build is unrepresentable in the output — LSP G2 applied to the CLI). |
| Extensibility v1 | Adding a first-party rule = one file + one table row + one explain page + golden fixtures. Third-party rules (dylib loading or rules-written-in-Ipê) are **out of scope v1** — filed as a future phase, not half-shipped. |

---

## 1. Positioning — what kind of tool this is

**Source-level, not compiler-gated.** `ipe build` acceptance does not depend
on lint findings (the one rule that changes *acceptance* — the closed-union
catch-all check — therefore lives in the compiler, not here; see the companion
design). Lint is what a team turns on in CI and the editor: style, dead code,
pitfalls, security smells. Exit policy makes it CI-gateable: `ipe lint` exits
non-zero iff at least one `deny`-level finding fired.

**One analyzer (hazard L-A).** The LSP design already forecloses a divergent
second analyzer; the same rule binds here with the same structural mechanism:
`sky_lint` depends on `sky_parse`/`sky_canon`/`sky_types` and consumes their
outputs. A rule that calls a parser or a unifier is a review-rejectable defect.
The three inputs a rule may read:

| Input | Provides | Used for |
|---|---|---|
| Parse AST (`sky_parse`) | imports + exposing lists, source spans, pre-desugar surface shapes | unused-import, style rules that must see the literal source shape |
| Canon AST (`sky_canon::ast`) | resolved references (local / top-level / kernel / ctor), unions, defs | dead-code, reference counting, pattern rules |
| `SolvedTypes` (`sky_types`) | `env` (type per binding), `regions` (type per sub-expression span), `bounds` | every **typed** rule: `Task String` errors, Float-money, secret-typed flows |
| Source text + directive table | raw text, `@allow` directives (lexer trivia) | suppression, fix rendering, layout-sensitive style rules |

**Why post-`sky_types` (typed AST) and not post-parse.** The high-value rules
are type-directed: "this `Task`'s error slot is `String`" is unanswerable
without `SolvedTypes.regions`. Running after inference also means lint only
ever sees programs the compiler accepted — rules never need error-recovery
logic. (In-editor, the LSP feeds lint from the same last-good snapshot
discipline it uses for everything else.)

## 2. Prior art — what is adopted, what is rejected

**From elm-review** (rule = visitor over the AST with a folded context):
- ADOPT: the visitor schema — a rule declares hooks, the driver walks once.
- ADOPT: project rules with a final-evaluation step (cross-module dead code).
- ADOPT: fixes as first-class rule output.
- REJECT: rules as user packages in the source language (v1). elm-review's
  killer feature is writing rules in Elm; the Ipê analogue (rules in Ipê,
  running compiled) needs the FFI story and a sandbox stance — filed as
  Phase 5, not faked.
- REJECT: elm-review's *no in-source suppression* stance. It optimizes for
  review-time visibility of exceptions but forces config-file churn for every
  legitimate local exception; clippy's per-site model with a **mandatory
  reason** keeps the exception next to the code it excuses, greppable, and
  itself lintable (`unused-allow`).

**From clippy** (lint pass registry, levels, `#[allow]`):
- ADOPT: `allow`/`warn`/`deny` levels; per-rule defaults; category-level
  configuration; dual stable-code + human-name identity; `Applicability` (the
  crate already has clippy/rustc's model verbatim in `sky_diagnostics`).
- ADOPT: the "unused allow" self-lint (clippy's `#[expect]` benefit without a
  second directive form).
- REJECT: attribute syntax. Ipê has no attributes; the directive is a comment
  (§6) parsed by the one lexer.

## 3. Architecture

```
sky_parse ──► parse AST ─┐
sky_canon ──► canon AST ─┼──► sky_lint ──► Vec<Diagnostic::Lint>
sky_types ──► SolvedTypes┘        │
   (all compiler-owned)           ├── driver: single walk, dispatch to rule hooks
                                  ├── registry: ALL_RULES + drift test
                                  ├── rules/: one file per rule
                                  └── config: LintConfig (from sky.toml + CLI)
consumers:  ipe lint (CLI)  ·  sky_lsp (diagnostics + quick-fixes)  ·  salsa query (later)
```

- `sky_lint` is a leaf of the front-end DAG: it depends on
  `sky_parse` + `sky_canon` + `sky_types` + `sky_diagnostics` and nothing
  depends on it except `ipe` and (later) `sky_lsp`.
- **No I/O in the crate** (the LSP INV-1 discipline): `sky_lint` takes ASTs,
  types, source text, and a parsed `LintConfig`; file reading and `sky.toml`
  parsing stay in `ipe`. A rule that reads the filesystem is a
  compile-time-visible design error.
- Determinism: findings are sorted (file, span, code) before emission; rule
  iteration order is the registry order. Same input ⇒ byte-identical output.

## 4. The rule API

### 4.1 Identity and metadata

```rust
/// Compile-time metadata for one lint rule. All fields are &'static:
/// the registry is a const table, misregistration is a compile error
/// or a drift-test failure, never a runtime surprise.
pub struct RuleMeta {
    /// Stable diagnostic code, e.g. IPE_W0101. Never reused, never renumbered.
    pub code: Code,
    /// Kebab-case human name, e.g. "unused-import" — the `@allow` / config key.
    pub name: &'static str,
    pub category: Category,     // Correctness | DeadCode | Pitfall | Style | Security
    pub default_level: Level,   // Allow | Warn | Deny
    /// One-line summary (the `explain` index line).
    pub summary: &'static str,
}
```

`Category` is a closed enum; every rule belongs to exactly one. Category is a
configuration handle (`deny = ["security"]`) and a documentation grouping — it
carries no behavior of its own.

### 4.2 The visitor schema (one walk, N rules)

A naive `fn check_module(&Module)` per rule costs N full traversals and invites
each rule to grow its own walker (the drift surface hazard L-J warns about).
Instead the driver owns the single canonical walk and rules subscribe:

```rust
/// A lint rule: default-empty hooks called by the one driver walk.
/// Rules keep per-module state in `self` (fresh instance per module).
pub trait Rule {
    fn meta(&self) -> &'static RuleMeta;

    // -- module-shape hooks (parse AST side)
    fn import(&mut self, cx: &Cx, import: &parse::Import) {}
    fn union_decl(&mut self, cx: &Cx, union: &canon::Union) {}
    fn def(&mut self, cx: &Cx, def: &canon::Def) {}

    // -- expression walk (canon AST side; enter/exit for scope-tracking rules)
    fn enter_expr(&mut self, cx: &Cx, expr: &canon::Expr) {}
    fn exit_expr(&mut self, cx: &Cx, expr: &canon::Expr) {}
    fn pattern(&mut self, cx: &Cx, pat: &canon::Pattern) {}

    // -- evaluation points
    fn finish_module(&mut self, cx: &Cx, out: &mut Findings) {}
    /// Cross-module rules only (dead exported bindings). Receives the
    /// per-module state of every instance of this rule.
    fn finish_project(&mut self, cx: &ProjectCx, out: &mut Findings) {}
}
```

`Cx` (the lint context) is the read-only window onto the compiler artifacts:

```rust
impl Cx<'_> {
    /// Solved type of the sub-expression at `span`, from SolvedTypes.regions.
    pub fn ty_of(&self, span: Span) -> Option<&Ty>;
    /// Solved type of a top-level binding.
    pub fn def_ty(&self, home: &[Symbol], name: Symbol) -> Option<&Ty>;
    /// The interner (resolve Symbols to names — read-only).
    pub fn name(&self, sym: Symbol) -> Option<&str>;
    /// Source text slice for a span (fix construction, style rules).
    pub fn src(&self, span: Span) -> &str;
    /// Is this span suppressed for `rule` by an @allow directive?
    pub fn allowed(&self, rule: &RuleMeta, span: Span) -> bool;
}
```

Emission: `Findings::push(meta, span, message, help, suggestions)` — the driver
(not the rule) applies level resolution and `@allow` suppression, so a rule
cannot forget either. A `Finding` becomes `Diagnostic::Lint` (§5).

### 4.3 Registration + anti-drift

```rust
pub static ALL_RULES: &[fn() -> Box<dyn Rule>] = &[ /* one ctor per rule */ ];
```

Drift tests (the kernel-registry pattern):
- every `RuleMeta.code` is in `sky_diagnostics::ALL_CODES` and vice versa for
  the `IPE-W` range;
- every rule has an explain page (`explain/IPE-W####.md` non-empty);
- rule `name`s are unique, kebab-case, and stable (snapshot test);
- every rule has at least one flagged-fixture and one clean-fixture golden.

Adding a rule = rule file + `ALL_RULES` row + `Code` constant + explain page +
fixtures; missing any one is a failing test, not a latent gap.

## 5. Findings are Diagnostics — one pipeline

`sky_diagnostics::Diagnostic` gains one variant:

```rust
Lint {
    span: Span,
    code: Code,             // the rule's IPE-W#### (validated: W range only)
    rule: &'static str,     // kebab-case name, shown as `[unused-import]`
    message: Box<str>,
    help: Vec<HelpLine>,    // reuses Suggest(Suggestion) for fixes
}
```

What this buys, structurally:
- **Rendering** — the existing rustc/Elm-style report renderer works unchanged
  (caret snippet, help lines, `ipe explain IPE-W0101` pointer).
- **`explain`** — lint explain pages join `src/compiler/diagnostics/explain/`
  and the `ipe explain` index; they follow the compiler-as-kind-teacher
  standard (progressive ELI10→deep, runnable before/after snippets).
- **Fixes** — `Suggestion { span, replacement, applicability }` is already the
  crate's fix model and `ipe fix` already applies it; `ipe lint --fix` reuses
  the same application machinery.
- **LSP** — `sky_lsp` publishes `Diagnostic`s; lint findings arrive on the
  same channel with severity mapped from the resolved level
  (deny → Error, warn → Warning, Style/allow-but-requested → Hint) and
  `MachineApplicable` suggestions surfaced as quick-fix code actions behind
  the G2 `VerifiedEdit` gate. No new LSP machinery.

Severity: `Diagnostic::severity()` for `Lint` reads the **resolved level**
carried at construction (the driver resolves config before emitting), keeping
the accessor total and config-free.

Code range: `IPE-W####` (new family, W = warning/lint). Sub-ranges by category
for greppability, not behavior: W01xx dead code, W02xx pitfalls, W03xx style,
W04xx security. Codes never renumber; a removed rule's code is retired, not
reused.

## 6. Levels, configuration, and per-site suppression

### 6.1 Levels

`allow` (off), `warn` (report, exit 0), `deny` (report, exit non-zero).
Resolution precedence, highest first:

1. CLI (`--deny <rule|category>`, `--allow …`, `--warn …`)
2. per-site `@allow` directive (can only *lower* to allow, never raise)
3. `sky.toml [lint.rules] <name> = "<level>"`
4. `sky.toml [lint] deny/warn/allow = ["<category>|<name>", …]`
5. the rule's `default_level`

### 6.2 `sky.toml`

```toml
[lint]
# level floor applied to every rule that is not `allow` by default
level = "warn"
deny  = ["security", "unused-import"]
allow = ["case-bool-to-if"]

[lint.rules]
float-money = "deny"
```

Parsed in `ipe` (`project.rs`) into a typed `LintConfig` at the boundary —
unknown rule/category names are a **configuration error** (parse, don't
validate: a typo'd rule name silently linting nothing is the invalid state).

### 6.3 Per-site suppression — the `@allow` directive

```elm
-- @allow(unused-binding) kept for the public API surface
legacyHelper : Int -> Int
```

Grammar: a line comment whose first token is `@allow(<rule-name>)` followed by
a **mandatory non-empty reason**. It scopes to the next declaration when it
stands alone on the line(s) immediately above one, or to the enclosing
expression line when trailing. Directive parsing lives in the **lexer's**
trivia skip (`skip_trivia` already sees every comment): comments matching the
`@allow` shape are collected into a side table `Vec<(Span, Directive)>` carried
on the parse output. One lexer, no second scanner re-tokenizing comments, no
divergence about what is or is not a comment. A malformed directive
(`@allow` with no rule, empty reason, unknown rule name) is itself a finding
(`malformed-allow`, deny) — never silently inert.

Self-lint: an `@allow` that suppressed nothing this run fires `unused-allow`
(warn) — suppressions cannot rot silently.

Security-category rules are suppressible like any other (with reason) — and
this is stated honestly: **the lint tool is advisory, not a security
boundary**. Anything that must be *impossible* (SQL injection shapes, secret
logging the runtime can prevent) belongs in the compiler/runtime gates, not in
a suppressible lint. A security lint here is an early-warning smell detector.

This same directive table is the escape-hatch carrier for the compiler-level
closed-union rule (companion design) — one grammar, one parser, one table.

## 7. Autofix

- A rule attaches `Suggestion`s via `HelpLine::Suggest` with an honest
  `Applicability` (the existing enum: `MachineApplicable` / `MaybeIncorrect` /
  `HasPlaceholders`).
- `ipe lint --fix` applies `MachineApplicable` edits (others are printed, or
  applied per-edit interactively like `ipe fix`).
- **Verify-before-write** (LSP G2 applied to the CLI): edits are applied to an
  in-memory copy, the copy is re-run through parse→canon→types (+ a re-lint),
  and only a clean result is written to disk. On failure nothing is written
  and the tool reports which fix broke the round-trip as a lint-tool bug
  (IPE-I range). A `--fix` that leaves the tree broken is unrepresentable.
- Overlapping edits within one file are applied in one pass sorted by span,
  rejecting overlaps (the second overlapping fix is deferred to the next run —
  fixpoint by iteration, never a corrupted splice).

## 8. LSP integration

Per `ipe-lsp.md` Q3(c): compiler-sourced lints surface directly; `sky_lint`
extends that set without changing the mechanism. The LSP calls the same
`sky_lint::run(module_inputs, config)` entry the CLI uses (v0: on the batch
backend, after each successful check; salsa: as a derived `lint(file)` query
downstream of `typecheck`, incremental and cancellable for free).

- Findings → `textDocument/publishDiagnostics` with `code` = `IPE-W####`,
  `codeDescription` → the explain page, severity per §5.
- `MachineApplicable` suggestions → quick-fix code actions through the
  existing `VerifiedEdit` gate (verify-on-apply in v0, verify-on-offer under
  salsa) — an LSP lint fix can never introduce a build break.
- The `@allow` directive gets its own code action: *"Suppress
  `<rule>` here (requires reason)"* inserting the directive with a
  placeholder-tabstop reason (`HasPlaceholders` — never auto-applied).

## 9. CLI surface

```
ipe lint <entry.ipe | project-dir | sky.toml>
    [--fix]                    apply MachineApplicable fixes (verify-then-write)
    [--deny|--warn|--allow <rule|category>]...
    [--rule <name>]...         run only the named rules
    [--format human|json]      json = one finding per line (CI/tooling)
    [--list]                   print the rule index (name, code, category, level)
```

Exit codes: `0` clean or warn-only · `1` ≥1 deny finding · `2` usage/config
error. `ipe lint --list` and `ipe explain IPE-W####` are the discovery
surface.

## 10. Rule catalogue v1

Every rule names its inputs (P = parse AST, C = canon AST, T = SolvedTypes).
Fix column: MA = MachineApplicable, MI = MaybeIncorrect, HP = HasPlaceholders,
— = none.

### Dead code (W01xx) — default `warn`

| Rule | Detects | Inputs | Fix |
|---|---|---|---|
| `unused-import` | imported module/member never referenced | P+C | MA remove |
| `unused-binding` | top-level def neither exported nor referenced (project-wide via `finish_project`) | C | MI remove |
| `unused-let-binding` | `let` binder never used in body (except auto-forced `_ = task`) | C | MA remove |
| `unused-pattern-binding` | pattern variable never read | C | MA rename to `_` |
| `unused-variant` | constructor never constructed nor matched outside its decl (project-wide) | C | — (advisory: removing changes the type's public shape) |
| `unused-allow` | `@allow` directive that suppressed nothing | directive table | MA remove |

### Case hygiene (W02xx) — default `warn`

| Rule | Detects | Inputs | Fix |
|---|---|---|---|
| `case-bool-to-if` | `case b of True -> … ; False -> …` | C | MA rewrite to `if` |
| `comparison-to-bool` | `x == True`, `x /= False` | C+T | MA drop the comparison |
| `length-zero-is-empty` | `List.length xs == 0`, `String.length s == 0` | C | MA `List.isEmpty` / `String.isEmpty` |
| `wildcard-absorbs-variants` | catch-all over a closed union **in nested pattern positions** (the top-level-column case is the compiler's IPE-T0018 — companion design; this rule covers the columns the compiler rule deliberately leaves open) | C+T | HP expand |

(The compiler already owns redundant arms — IPE-T0011 — and non-exhaustive
`case` — IPE-T0010; lint never duplicates a compiler diagnostic.)

### Elm-family pitfalls (W02xx) — default `warn`

| Rule | Detects | Inputs | Fix |
|---|---|---|---|
| `pointless-lambda` | `\x -> f x` where `x` free-count is exactly the application | C | MA eta-reduce |
| `redundant-pipe` | piping into / applying `identity` (`x \|> identity`, `identity <\| e`) | C | MA simplify |
| `nested-case-same-scrutinee` | inner `case` over the same already-matched scrutinee | C | — |

### Security (W04xx) — default `deny` where typed, `warn` where heuristic

| Rule | Detects | Inputs | Level | Fix |
|---|---|---|---|---|
| `task-string-error` | a binding/region typed `Task String a` or `Result String a` (AGENTS.md: never `String` as error type) | T | deny | — (points at `Error`) |
| `float-money` | `Float`-typed binding whose name matches money vocabulary (`price`, `amount`, `total`, `balance`, …) | C+T | warn (heuristic — honest: name matching can false-positive, so it may never be deny) | — (points at `Ipe.Money`) |
| `password-oninput` | `onInput` handler attached alongside `type "password"` on the same input attr list | P+C | deny | — (teaches the `onSubmit` pattern) |
| `secret-in-log` | identifier with secret vocabulary (`password`, `token`, `secret`, `apiKey`) flowing as a `Log.*` / `println` argument | C | warn (heuristic) | — |
| `data-sky-eval` | the forbidden `data-sky-eval` attribute string in a view | P | deny | MA remove |

**Honest limits, stated in each rule's explain page:** the two heuristic rules
(`float-money`, `secret-in-log`) match *names*, not information flow — they
catch the common blunder, not a determined mistake, and are capped below
`deny` by policy until a typed carrier (e.g. `Ipe.Secret`, ledgered
B-Secret) lets them become type-directed.

### Style (W03xx) — default `allow` (opt-in)

| Rule | Detects | Fix |
|---|---|---|
| `exposing-everything` | `exposing (..)` on a non-Prelude import | MI enumerate used members |
| `long-function` | def body over N lines (configurable) | — |

v1 ships the tables above and nothing speculative; every additional rule
follows the §4.3 checklist.

## 11. Soundness corners (surfaced, not papered over)

1. **Cross-module dead code needs the whole program.** `unused-binding` /
   `unused-variant` are only sound against the full linked module set (the
   compiler's `link::link` output — already the artifact lint receives).
   Entry-point reachability is the criterion; a library-style module with no
   entry cannot support these rules and they are skipped there (a skipped rule
   is reported in `--format json` metadata, never silently).
2. **Heuristic security rules can false-positive and false-negative** — §10's
   level cap + explain-page honesty is the mitigation; the roadmap fix is
   typed carriers, not smarter regexes.
3. **Suppress-by-comment can be committed thoughtlessly.** Mandatory reason +
   `unused-allow` + CI-visible `--format json` (suppression counts) keep it
   auditable. Teams that want elm-review's zero-suppression stance set
   `[lint] forbidSuppression = true` (a config key that turns any `@allow`
   into a deny finding).
4. **Rule state across the driver walk is per-rule mutable state** — a rule
   with a scope-tracking bug produces wrong findings, not unsoundness (lint
   never changes acceptance). Golden fixtures per rule are the gate.

## 12. Phased implementation plan

Each phase lands green (fmt, clippy pedantic/nursery, tests) and is
independently useful. Gates listed per phase.

- **Phase 0 — skeleton + pipeline seam.**
  `src/compiler/lint` (RuleMeta/Rule/Cx/Findings/registry + drift tests);
  `Diagnostic::Lint` + `IPE-W` range in `sky_diagnostics` (+ `ALL_CODES`
  count test update); driver walk over parse+canon+`SolvedTypes`;
  `ipe lint` CLI (human format, exit policy); **two seed rules**
  (`unused-import`, `case-bool-to-if`) with explain pages + fixtures.
  *Gate:* corpus run over `examples/` + `src/stdlib` — every finding
  on our own corpus is triaged: real (fixed in the same change) or a
  false-positive (rule fixed). A rule that cannot go corpus-clean does not
  ship.
- **Phase 1 — dead code + suppression.**
  Lexer trivia directive table (`@allow` grammar, malformed-directive
  finding); `sky.toml [lint]` parsing in `skyc/project.rs`; level resolution;
  dead-code rule family; `unused-allow`.
  *Gate:* directive round-trip goldens (suppressed finding disappears; unused
  directive fires); config-precedence unit matrix.
- **Phase 2 — typed rules + autofix.**
  Security + pitfall rules; `--fix` with the verify-then-write loop;
  `--format json`.
  *Gate:* per-fix round-trip test — apply fix → re-parse/re-check clean →
  re-lint quiet; property test: `--fix` output always re-parses.
- **Phase 3 — LSP surfacing.** Wire `sky_lint::run` into `sky_lsp` diagnostics
  + quick-fix actions (lands with/after the LSP plan's Phase 3; behind the
  same `VerifiedEdit` gate). Salsa: register `lint(file)` as a derived query
  when the LSP moves to the salsa backend.
  *Gate:* the LSP plan's own G2 verification tests extended with a lint fix.
- **Phase 4 — manifest lints (extension).** Rules over `sky.toml` itself
  (`memory` session store flagged for production, missing `[lint]` floor) —
  a distinct input class, added only once source rules are stable.
- **Phase 5 (filed, not committed) — rules in Ipê.** elm-review's user-rule
  model: rules authored in Ipê compiled against a typed AST surface. Blocked
  on the FFI sandbox stance and a stable public AST — explicitly out of scope
  until then.

## 13. Open decisions (need a user call before implementation)

1. **Should `warn` findings gate the examples sweep?** (Proposal: no — sweep
   gates on deny only; a warn-storm is a campaign, not a red build.)
2. **`forbidSuppression` default** for the security category (proposal: off;
   teams opt in).
3. **Rule-name prefix for style rules** (flat `long-function` vs namespaced
   `style/long-function` in config keys — proposal: flat names, category as a
   separate axis).
