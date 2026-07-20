# SP5 — `ipe diff` + enforced semver: spec + implementation plan

> **For agentic workers:** implement task-by-task, TDD, one commit per task. Steps
> use checkbox (`- [ ]`). All fenced blocks are **illustrative targets, not commands
> to run** unless the surrounding text says a command was actually run — bind exact
> tokens against the real tree; `cargo` lines are the TDD loop.

**Goal:** `ipe diff <old> <new>` computes the public-API delta between two package
versions from their **typed module interfaces**, classifies each change, and derives
the required semver bump; a `pub fn` the gate consumes rejects a wrong bump. Mirrors
Elm's `elm diff`, mapped to Ipê's pre-1.0 semver.

**Architecture:** analysis + CLI only — no emission, SEAL untouched. Two source
trees are each parsed → canonicalised → per-module scoped-inferred; each module's
`TypedInterface` (the exported-name → generalized-scheme boundary) is projected into
a canonical, order-independent `PublicApi` value; the two `PublicApi`s are diffed;
the diff is classified (breaking / compatible / none); the classification maps to a
required bump.

**Tech Stack:** `ipe-cli` (`src/ipe-cli`), the compiler front-end crates already
in the CLI's dependency set (`ipe_parse`, `ipe_canon`, `ipe_types`, `ipe_diagnostics`,
`ipe_intern`), and `semver` (already a dep, used by SP2/SP3).

## Global Constraints

- Principle order (strict tie-breaker): Security > Correctness > Soundness >
  Efficiency > Completeness > Readability.
- The SEAL is untouched: SP5 is read-only analysis + a CLI surface; it emits no Rust
  and writes no source. The golden suite (`golden_basics`) is unchanged.
- **Parse, don't validate:** the two typed interfaces are projected ONCE into a typed
  `PublicApi` (a canonical, owned, `Ord`-keyed structure); the classifier walks that
  typed value, never a stringly-typed map. The required bump is a closed `enum`
  (`Compatibility` / `RequiredBump`), never a magic string.
- **Make invalid states unrepresentable:** the diff is a sum over the finite change
  kinds (`ApiChange`); classification is an exhaustive `match` (no wildcard swallowing
  a new change kind); the bump derivation is total over `Compatibility`.
- **Fail closed / conservative (Security first):** the classifier is conservative — a
  change it cannot prove compatible is **breaking**. "changed ⇒ breaking" is the
  default; a false-breaking wastes a version number, a false-compatible ships a
  silent break. An interface that is `Open` (a scheme reaches a residual variable an
  importer could pin) or that fails to typecheck is treated as a hard, typed error —
  never silently skipped.
- Comments say WHAT not HOW; no archaeology outside `docs/adr/`; self-explaining
  names. Commits scoped, plain messages, no AI attribution / no trailer (hook-enforced).

## Elm / Sky reference

Elm's `elm diff` compares the **exposed** modules of two package versions, classifying
each change and taking the **maximum** magnitude across all changes as the required
bump:

- **MAJOR** — a breaking change: an exposed value/type/constructor **removed** or
  **renamed**; an exposed value's **type changed**; a constructor removed from an
  exposed union; an exposed union's arity/parameters changed.
- **MINOR** — a purely **additive** change: a new exposed value, a new exposed type,
  a new constructor added to an exposed union.
- **PATCH** — no change to the exposed API.

Sky's Haskell compiler carries the same design (an `elm diff`-derived
`Elm.Compiler.Type` comparison + a `magnitude` = max over per-change magnitudes); the
Ipê `../sky` checkout available in this environment does not expose those Haskell
sources, so this spec follows Elm's published `elm diff` semantics directly, which
Sky mirrors.

### Ipê pre-1.0 mapping (major is reserved)

Ipê is pre-1.0; a major bump is reserved. The magnitude collapses to two reachable
outcomes, matching the release-please config (feat→patch, breaking→minor pre-1.0):

| Elm magnitude | Ipê `Compatibility` | Ipê `RequiredBump` (pre-1.0) |
|---------------|---------------------|------------------------------|
| MAJOR         | `Breaking`          | **minor** (`0.Y.0`)          |
| MINOR         | `Compatible`        | **patch** (`0.y.Z`)          |
| PATCH         | `Compatible`        | **patch** (`0.y.Z`)          |

A `Breaking` delta requires at least a minor bump; a `Compatible` delta requires at
least a patch bump; an empty delta still requires a patch bump (a re-release is a new
version). The gate rejects a `<new>` version that does not clear the required floor
over `<old>`.

## Design decisions (the three questions, resolved)

### D1 — Interface representation

The comparable surface is derived from each module's `TypedInterface`
(`src/compiler/types/src/lib.rs:210`), whose `values: BTreeMap<Symbol, TypedScheme>`
is exactly the exported public value API, plus `unions: Vec<canon::Union>` for the
exported type/constructor API. Extraction, per source tree:

1. Read each `.ipe` module; parse (`ipe_parse::parse_module`) → canonicalise
   (`ipe_canon::canonicalise_module`, which returns `(ast::Module, ModuleExports)`).
2. Scoped-infer each module (`ipe_types::infer_module`, deps threaded in dependency
   order) to obtain its `InterfaceStatus`.
   - `InterfaceStatus::Closed(TypedInterface)` — usable.
   - `InterfaceStatus::Open` — a hard, typed error for SP5: an open scheme cannot be
     faithfully compared (an importer could pin its residual variable), so we fail
     closed rather than guess. (Reported as `DiffError::OpenInterface { module }`.)
3. Project the `TypedInterface` into a canonical `PublicApi`:
   - Each exported value → `(name, signature)` where `signature` is the **owned,
     interner-free, variable-name-canonicalised** rendering of its `Ty`. Produce it by
     `ty_to_doc` (`src/compiler/types/src/doc.rs:93`) with a **fresh** `VarNamer` per
     scheme (first-seen-order letters ⇒ two schemes render identically **iff**
     structurally equal up to variable renaming — exactly the α-equivalence a type
     signature must be compared under), then `render_ty`
     (`src/compiler/diagnostics/src/render.rs:977`) to a `String`. `TyDoc` derives
     `Eq`, so the pre-`render_ty` `TyDoc` is itself a valid comparison key; the string
     is the human-facing form the diff report prints.
   - Each exported union → `(name, params, constructors)` where each constructor is
     `(ctor_name, arg_signatures)`, arg types rendered the same canonical way against
     the union's own `vars` for variable naming.
   - Keys sorted (`BTreeMap` / sorted `Vec`) so the structure is **order-independent**:
     the same public API yields byte-equal `PublicApi` regardless of source order.

`PublicApi` is `#[derive(Clone, PartialEq, Eq)]` with `BTreeMap`/sorted-`Vec` fields —
canonical by construction. It is the single typed boundary the classifier consumes.

Illustrative shape (not code to paste verbatim):

```rust
// Illustrative — the canonical, order-independent public-API surface of one package.
struct PublicApi {
    modules: BTreeMap<ModulePath, ModuleApi>,
}
struct ModuleApi {
    values: BTreeMap<String, String>,          // name -> canonical signature
    unions: BTreeMap<String, UnionApi>,         // name -> its exported shape
}
struct UnionApi {
    params: usize,                              // arity of the type constructor
    ctors: BTreeMap<String, Vec<String>>,       // ctor name -> arg signatures
}
```

### D2 — Change classification (Elm's rules, conservative)

Diff `old: &PublicApi` against `new: &PublicApi` into a `Vec<ApiChange>`; each change
carries its `Compatibility`. The finite change kinds and their classification:

| Change | Compatibility | Rationale |
|--------|---------------|-----------|
| Exported module removed | `Breaking` | its whole surface disappears |
| Exported module added | `Compatible` | additive |
| Exported value removed / renamed | `Breaking` | a rename = removal + addition |
| Exported value added | `Compatible` | additive |
| Exported value signature changed | `Breaking` | conservative default (see below) |
| Exported union removed / renamed | `Breaking` | |
| Exported union added | `Compatible` | additive |
| Union type-parameter arity changed | `Breaking` | every use site's arity breaks |
| Constructor removed from exported union | `Breaking` | pattern matches break |
| Constructor added to exported union | `Breaking` | exhaustive `match`es over it break |
| Constructor argument types changed | `Breaking` | payload shape breaks |

Notes:

- **"Signature changed ⇒ breaking" is the deliberate conservative default.** A truly
  compatible generalization (a signature getting *strictly more general*, so every old
  use still type-checks) is in principle a `Compatible` change; proving that direction
  requires a subsumption check the first cut does **not** attempt. Per Security-first
  fail-closed, an unproven-compatible signature change is `Breaking`. (Deferred: a
  subsumption-based `Compatible`-generalization refinement — see Out of scope.)
- **A new constructor to an *exposed* union is `Breaking`** (matches Elm: an exposed
  union whose constructors are exported lets importers write exhaustive matches, so a
  new variant breaks them). This is stricter than "added ⇒ compatible" and is correct
  for Ipê's exhaustive `case`.
- Classification is an **exhaustive `match` over `ApiChange`** — a new change kind is a
  compile error at the classifier, never a silently-defaulted magnitude.

### D3 — Two-version input + required-bump derivation

- `ipe diff <old-path> <new-path>`: two directories (or `.ipe` entries), each a
  resolved package version's source tree. Each is loaded, typechecked, and projected to
  a `PublicApi` independently (D1). A typecheck failure in either tree is a hard, typed
  `DiffError` (we cannot diff an API we cannot type).
- `magnitude(changes) -> Compatibility` = `Breaking` if any change is `Breaking`, else
  `Compatible` (the max-magnitude fold, collapsed to two outcomes).
- `required_bump(compat) -> RequiredBump` = `Minor` for `Breaking`, `Patch` for
  `Compatible` (pre-1.0 mapping, D-table above).
- The gate primitive (the `pub fn` SP4 consumes):

```rust
// Illustrative signature — the gate calls this with the two trees + the proposed
// new version, and gets back either Ok (bump clears the floor) or the required floor.
pub fn check_semver_bump(
    old_tree: &Path,
    new_tree: &Path,
    old_version: &semver::Version,
    new_version: &semver::Version,
) -> Result<SemverReport, DiffError>;
```

`SemverReport` carries the classified changes, the required floor version, and whether
`new_version` clears it. The CLI prints the report; the gate reads
`report.satisfied` and rejects on `false`.

## Bump-floor semantics (pre-1.0)

Given `old_version` and a `RequiredBump`, the **minimum acceptable** `new_version`:

- `Patch` floor: `new_version > old_version` (any increase; the smallest is a patch
  bump `0.y.(z+1)`).
- `Minor` floor: `new_version >= 0.(y+1).0` (a minor bump; a patch bump is rejected).

`new_version` **satisfies** the floor iff `new_version >= floor`. A larger bump than
required always satisfies (over-bumping is allowed; under-bumping is rejected). This
matches Elm, where the tool reports the *minimum* required magnitude and a larger one
is fine.

## File structure

- `src/ipe-cli/src/api_surface.rs` (**create**) — `PublicApi` + extraction: load a
  source tree, typecheck each module, project each `TypedInterface` into the canonical
  structure. Typed `DiffError` for load/typecheck/open-interface failures.
- `src/ipe-cli/src/diff.rs` (**create**) — `ApiChange`, `Compatibility`,
  `RequiredBump`, `SemverReport`; `diff_api`, `classify`/`magnitude`, `required_bump`,
  the floor computation, and `check_semver_bump` (the gate `pub fn`) + `run_diff` (the
  CLI).
- `src/ipe-cli/src/lib.rs` (**modify**) — dispatch `"diff"` → `diff::run_diff`; a
  `DiffError` → `CliError` arm.
- `src/ipe-cli/src/help.rs` (**modify**) — a `diff` `Command` entry + add it to the
  `Tools` section.

## Out of scope (later / deliberate)

- **Compatible-generalization refinement:** proving a signature change is a strict
  generalization (every old use still checks) ⇒ `Compatible`. The first cut is
  conservative (`Breaking`); the refinement is a subsumption check, filed separately.
- **Multi-file dependency graphs across the two trees:** the first cut handles a
  package as its set of `.ipe` modules under a `src/` root, inferred in topological
  order using the tree's own modules as deps. A package that pulls *external* index
  deps for typechecking is deferred to the SP3-resolver integration.
- **The gate CI wiring** (SP4): SP5 ships the `check_semver_bump` primitive; the
  workflow that calls it on a submission PR is SP4.
- **`.ipei` on-disk interface files:** none exist yet; SP5 recomputes interfaces from
  source. A serialized-interface fast path is a later optimisation.

## Bite-sized TDD plan

Each task: write the test(s) first (red), implement (green), one commit. Run from the
worktree with `CARGO_TARGET_DIR` isolated.

- [ ] **T1 — `PublicApi` + single-module extraction.** `api_surface.rs`: define
  `PublicApi`/`ModuleApi`/`UnionApi` and `extract_tree(root) -> Result<PublicApi,
  DiffError>` for a single-file/single-module tree. Test: a fixture `.ipe` with two
  exported values + one exported union projects to the expected canonical `PublicApi`;
  source-order permutations of the fixture yield an **equal** `PublicApi`.
- [ ] **T2 — canonical signature rendering + α-equivalence.** Signatures render via a
  fresh `VarNamer` per scheme so `map : (a -> b) -> List a -> List b` and the same
  binding written with different type-var letters project to the **same** signature
  string. Test: two fixtures differing only in type-variable spelling produce equal
  `PublicApi`.
- [ ] **T3 — open / un-typecheckable interface fails closed.** A fixture whose exported
  binding yields `InterfaceStatus::Open`, and a fixture that fails to typecheck, each
  return the corresponding `DiffError` (`OpenInterface` / `Typecheck`), never a partial
  `PublicApi`. Test asserts the error variant.
- [ ] **T4 — `diff_api` change detection.** `diff.rs`: `ApiChange` + `diff_api(old,
  new) -> Vec<ApiChange>`. Tests, one per row of the D2 table: removed value, added
  value, changed signature, removed/added union, changed union arity, removed/added
  constructor, changed constructor args, removed/added module — each yields exactly the
  expected `ApiChange`(s).
- [ ] **T5 — classification + magnitude.** `classify`/`magnitude`: each `ApiChange`
  maps to the D2 `Compatibility`; `magnitude` is `Breaking` if any change is breaking.
  Tests: a pure-addition delta is `Compatible`; any removal/change is `Breaking`; the
  empty delta is `Compatible`.
- [ ] **T6 — required bump + floor + satisfaction.** `required_bump`, the floor
  computation, `SemverReport`. Tests: `Breaking` ⇒ `Minor` floor `0.(y+1).0`;
  `Compatible` ⇒ `Patch` floor `> old`; a `new_version` at/above the floor satisfies,
  below rejects; over-bumping satisfies.
- [ ] **T7 — `check_semver_bump` end-to-end + `run_diff` CLI.** The gate `pub fn` over
  two fixture trees + versions returns the right `SemverReport`; `ipe diff <old> <new>`
  prints the classified changes + required bump and exits 0 (report), while a
  standalone `--check <old-ver> <new-ver>` mode (or equivalent) exits non-zero on an
  unsatisfied floor. Wire dispatch in `lib.rs`, help entry in `help.rs`.
- [ ] **T8 — gate green.** `cargo nextest run -p ipe`, `cargo clippy --all-targets
  --workspace -- -D warnings`, `rustfmt --edition 2024` on touched files, and confirm
  `golden_basics` unchanged.

## Guards

- SEAL untouched (analysis + CLI only; no emission).
- `golden_basics` unchanged (`cargo nextest run -p ipe --test golden_basics`).
- Full local gate green before push: `cargo nextest run -p ipe`, `cargo clippy
  --all-targets --workspace -- -D warnings`, `rustfmt --edition 2024 <touched files>`
  (the `cargo fmt` hook is blocked; format per-file).
- `CARGO_TARGET_DIR` isolated to the lane; no in-tree `target/`.
