# Kernel Row descriptor — one entry per kernel

## Problem

A single logical fact about a kernel — "`Random.shuffle` takes a list and a
seed, needs the `random` runtime module, exercises no capability, and emits
`random_shuffle`" — is today smeared across four methods in three crates with
nothing binding the fragments to a common row:

- `ipe_kernels::StdlibKernel::decl` — qualifier, name, arity, emit-class, emit
  symbol name.
- `ipe_kernels::StdlibKernel::capability` — the security axis.
- `ipe_kernels::StdlibKernel::required_runtime_module` — the vendored module a
  divergent symbol lives in (only `Web`/`Server` are expressible today).
- `ipe_types::constrain::Builder::stdlib_scheme` — the HM type scheme, built
  against the ~180-field `Builtins` cache of interned type-constructor symbols.
- `ipe_backend_rust::naming::kernel_name` and `ipe_ir::pretty::kernel_name` —
  the runtime symbol string and the qualified source name (the C1 pair).

Because the fragments are independent `match self { … }` arms, an *invalid row
is representable*: a kernel with a capability but no scheme; a scheme whose
arrow count disagrees with `decl().arity`; a `decl().emit` string that names no
runtime symbol; a runtime symbol whose module is never appended to the emitted
crate. Each such row is an exit-0-then-cargo-fail hole — `ipe` accepts the
program, the emitted crate fails `cargo build`.

Two concrete instances of this class already shipped and were fixed
reactively:

- A `Cache` kernel family whose emit symbols (`cache_*`, `CacheCfg`,
  `IpeCacheHandle`) had no way to declare their vendored module: the
  `RuntimeModule` enum has only `Web` and `Server`, so `required_runtime_module`
  could not name the `cache` module, the append never fired, and the emitted
  `main.rs` referenced undefined `cache_*`.
- A `Random` family where `shuffle`/`weighted`/`seed*` were declared in one
  site but had no `stdlib_scheme` row, so they were unresolvable — the scheme
  fragment silently lagged the `decl` fragment.

The `required_runtime_module` fact is itself already double-booked: `Web`/
`Server` flow through the `RuntimeModule` enum, but `random`'s module is instead
carried by an ad-hoc `kernel_usage.random` predicate in the lowerer. Two
mechanisms for one fact is the drift surface.

## The Kernel Row: `KernelDef`

Collapse the per-kernel facts into ONE descriptor, one entry per kernel, from
which every scattered method is *derived* (or is kept as a thin projection fed
by it). The descriptor is the single row; the five methods become views over
the table.

### Shape

Lives in `ipe_kernels` (the leaf crate: deps `ipe_intern` + `ipe_diagnostics`
only — no cycle with `types`/`backend`). It supersedes the current `StdlibDecl`
struct, absorbing `capability` and `required_runtime_module` and gaining the
emit/source-name/scheme-key fields.

Illustrative shape (not a runnable snippet — a sketch of the target struct):

```rust
pub struct KernelDef {
    // ── identity ────────────────────────────────────────────────
    pub qualifier: &'static str,   // canon QUALIFIERS key, e.g. "Random"
    pub name:      &'static str,   // source name, e.g. "shuffle"
    pub arity:     u8,             // Ipê-level arg count before the result

    // ── emit / dispatch ─────────────────────────────────────────
    pub class:     KernelClass,    // which subsystem owns emission
    pub emit:      &'static str,   // runtime symbol, e.g. "random_shuffle"

    // ── security ────────────────────────────────────────────────
    pub capability: Option<Capability>,

    // ── runtime residency ───────────────────────────────────────
    // The vendored module the emit symbol lives in, when it DIVERGES
    // from the module `class` already pulls in. `None` = symbol is in
    // the class's own module (no extra append). Replaces both the
    // Web/Server RuntimeModule enum AND the ad-hoc uses_* predicates.
    pub runtime_module: Option<RuntimeModule>,

    // ── type scheme ─────────────────────────────────────────────
    // How the HM scheme is produced. See "Carrying the scheme".
    pub scheme: SchemeSpec,
}
```

`RuntimeModule` grows from a 2-variant enum (`Web`, `Server`) to cover every
conditionally-vendored module: `Web`, `Server`, `Cache`, `Random`, `Codec`,
`Analytics`, … one variant per feature-module that a kernel symbol can reside
in. This is the change that makes the undefined-symbol class *unrepresentable*:
a kernel symbol with no declarable home is a compile error, not a silent
append-miss.

### Table

`KernelDef` is exposed as `StdlibKernel::def(self) -> KernelDef` (the existing
`decl` becomes a projection: `self.def().into()` returning the old `StdlibDecl`
subset, or is deleted once callers move to `def`). The authoritative
per-variant `match` stays in `ipe_kernels` — the same closed exhaustive match we
have now, just returning one richer struct instead of five thin ones. There is
exactly ONE `match self` over `StdlibKernel::ALL`; everything else reads its
result.

## Carrying the scheme (the hard half — interacts with C3 + row-polymorphism)

The type scheme cannot be a `&'static` literal: it is built from *interned*
`Symbol`s (`self.builtins.int`, `self.builtins.list`, …) that only exist after
the `Interner` runs, and some schemes are row-polymorphic (fresh unification
vars, open records). So the scheme is not *stored* in `KernelDef`; the
descriptor carries a **scheme key** and the scheme *builder* is a total function
of that key.

`SchemeSpec` is the bridge. Two candidate encodings, both keep the builder in
`ipe_types` (where `Builder`/`Builtins`/`UnionFind` live) — `ipe_kernels` must
not depend on `ipe_types`:

1. **Structural (preferred long-term).** A small closed ADT of type-shape nodes
   that name builtins *by tag*, not by interned `Symbol`. Illustrative sketch
   (not runnable):

   ```rust
   enum TyShape {
       Int, Float, Bool, String, Char, Bytes,
       List(Box<TyShape>), Maybe(Box<TyShape>),
       Fun(Box<TyShape>, Box<TyShape>),
       Var(u8),                 // scheme-local quantified var index
       RowOpen { fields: &'static [(&'static str, TyShape)] }, // row-poly
       Con(BuiltinTag, &'static [TyShape]),  // named builtin, resolved late
   }
   ```

   `SchemeSpec::Shape(&'static TyShape)` lives in `KernelDef` (all-`'static`,
   `const`-embeddable). `ipe_types` owns the single interpreter
   `TyShape -> Ty` that resolves each `BuiltinTag`/`Var`/`RowOpen` against
   `self.builtins` and a fresh-var allocator. This is the row-polymorphism
   carrier: `RowOpen`/`Var` are the only nodes that touch the union-find, and
   they do so in one place. This *is* the C3 fix — see below.

2. **Keyed (cheaper migration bridge).** `SchemeSpec::Key(SchemeKey)` where
   `SchemeKey` is a closed enum, and `ipe_types::stdlib_scheme` becomes
   `match def.scheme_key { … }` instead of `match kernel { … }`. Mechanically
   identical arm count, but the arm is now selected by the descriptor's key, so
   many kernels sharing a shape (`Int -> Int`, `String -> Bool`, …) collapse to
   one arm. This does NOT yet delete the scheme `match`, but it *co-locates the
   selection key with the row* so drift ("kernel added, key forgotten") is
   caught by the totality test at the descriptor, not deep in `constrain.rs`.

Sequencing: adopt the keyed bridge first (a rename of the match discriminant,
byte-identical schemes, no `Builtins` change), then migrate families to the
structural encoding incrementally. Under the structural encoding the ~180-field
`Builtins` (C3) shrinks: every field that exists only to build a kernel scheme
(`http_f_*`, `sqlvalue`, `migration_f_*`, …) is replaced by a `BuiltinTag`
resolved lazily by the interpreter, so `Builtins` keeps only the truly-shared
primitive symbols. That migration is per-family and additive: a family moved to
`TyShape` drops its bespoke fields; untouched families keep theirs. No big-bang
180-field rewrite.

### The C1 pair (`kernel_name` × 2)

`decl().emit` already holds the runtime symbol string; the backend's
`naming::kernel_name` is a near-duplicate table. Fold it: `naming::kernel_name(k)
= k.def().emit`. The IR pretty-printer's `kernel_name` returns the *source*
qualified name — derive it as `format!("{}.{}", def.qualifier, def.name)` (or a
cached `&'static` built once). Both C1 methods become one-line projections; the
two ~1,600-arm tables delete. A source-scan test asserts the delegating
one-liner persists (same pattern ADR 0009 used for `callee_arity`).

## How each scattered method becomes a derivation

| Method | After |
|---|---|
| `StdlibKernel::decl` | projection of `def()` (or deleted; callers read `def()`) |
| `StdlibKernel::capability` | `self.def().capability` |
| `StdlibKernel::required_runtime_module` | `self.def().runtime_module` — now total over ALL feature modules, subsumes the `uses_random`/`uses_cache` ad-hoc predicates |
| `constrain::stdlib_scheme` | interpret `self.def().scheme` (structural `TyShape`; bridge: `match scheme_key`) |
| `naming::kernel_name` | `self.def().emit` |
| `ir::pretty::kernel_name` | `"{qualifier}.{name}"` from `def()` |

The lowerer's per-program `uses_*` scan reads `def().runtime_module` for EVERY
module (it already does for `Web`/`Server`); the separate `kernel_usage.random`
/ cache predicates delete.

## The load-bearing invariant tests

Two anti-drift tests, both cheap (no `cargo build`), both in `ipe_kernels` or
`crates/ipe/tests`:

1. **Every emit symbol its declared module defines (the undefined-symbol
   catcher).** For every `k in StdlibKernel::ALL`: `k.def().emit` must be a
   symbol the vendored crate actually defines, in the module
   `k.def().runtime_module` (or the class's own module when `None`).
   Implementation: scan the vendored `runtime/src/ipe_runtime/**` source for a
   `pub fn`/`pub struct`/`pub const` named `emit` under the expected module path
   (a source-symbol index, not a build). A `cache_get` declared with
   `runtime_module = Some(Cache)` but absent from `ipe_runtime::cache` FAILS the
   test — this is exactly the check that would have red-flagged the Cache family
   in CI before it shipped. Pairs with the E2E SEAL (`IPE_E2E=1`) which is the
   ground truth; this test is the fast pre-cargo tripwire.

2. **Every kernel row is internally coherent.** For every `k in ALL`:
   - `scheme(k)`'s arrow-count == `def().arity` (arity ↔ scheme agreement;
     ADR 0009 made `callee_arity` derive from `decl().arity` — this extends the
     same rule to the *scheme*, catching the "declared but unschemed" drift as a
     coherence failure, not a silent `None`).
   - `def().capability.is_some()` ⇒ the kernel's class is one that the
     capability gate actually inspects (no capability on a `Pure` kernel the gate
     never sees).
   - `scheme(k)` is `Some` for every `k` not in `KNOWN_UNBACKED` /
     `REACHABLE_BUT_UNLOWERED` (subsumes `stdlib_scheme_total_over_reachable`).
   - `def().emit` is unique per `class` (no two kernels in one module aliasing
     one symbol unintentionally) and a valid Rust identifier.

Both run in the normal (fast, no-E2E) `cargo nextest -p ipe` path so they gate
every PR.

## Implementation plan (sliceable kernel-family by kernel-family)

The project runs parallel kernel-family batches; each stage is independently
landable, green, and byte-identical-golden wherever the emit does not change.
Cheapest-highest-certainty first.

**Stage A — introduce `KernelDef`, no behavior change.**
Add the `KernelDef` struct + `StdlibKernel::def()` that *initially delegates to
the existing `decl`/`capability`/`required_runtime_module`* (three reads folded
into one struct). No caller moves yet. Golden-identical (no emit change).

**Stage B — co-locate name + capability + arity + runtime_module (the cheap
high-certainty slice).**
Make `def()` the authoritative `match`; rewrite `decl`/`capability`/
`required_runtime_module` as projections *of* `def()`. Grow `RuntimeModule` to
all feature modules; move `random`/`cache` off their ad-hoc `uses_*` predicates
onto `runtime_module`. Fold both `kernel_name` methods (C1) to `def().emit` /
`"{q}.{n}"`. Land invariant test 1 (emit-symbol-defined) and the arity-only half
of test 2. The scheme is still referenced by the *existing* `stdlib_scheme`
match (untouched) — this stage leaves the scheme where it is. Golden-identical.
Sliceable per family: a family is "on the row" once its arms in the folded
tables are removed.

**Stage C — scheme by key (bridge).**
Add `SchemeSpec::Key` to `KernelDef`; rewrite `stdlib_scheme` as
`match def.scheme_key`. Byte-identical schemes → golden-identical. Land the full
coherence test 2 (arity ↔ scheme arrow-count). Per-family sliceable.

**Stage D — scheme by structure + `Builtins` shrink (the C3 slice).**
Introduce `TyShape` + the single `ipe_types` interpreter. Migrate families off
`SchemeSpec::Key` onto `SchemeSpec::Shape`; each migrated family drops its
bespoke `Builtins` fields. Row-polymorphic families (`Http`, `Server`, records)
migrate last, using `RowOpen`/`Var`. Schemes stay byte-identical (the
interpreter reproduces the exact `Ty`), so goldens re-bless only if a family's
scheme was subtly wrong before (a *fix*, not a regression). Per-family
sliceable; `Builtins` shrinks monotonically.

Each stage ends green under the full CI-replica gate (unfiltered
`-p ipe_backend_rust --lib` + full golden + `IPE_E2E=1` SEAL + clippy/fmt), not a
filtered subset.

## Risks + mitigations

- **SEAL: exit-0-then-cargo-fail.** The whole point of the row is to *close*
  this class, but a botched runtime-module grow could itself open it (a wrong
  module append). Mitigation: invariant test 1 runs pre-cargo and is the gate;
  every stage additionally runs the `IPE_E2E=1` SEAL as ground truth before
  merge.
- **Golden re-bless.** The `KernelDef`-introduction, method-fold, and scheme-key
  stages are byte-identical by construction (same strings, same schemes) — a
  dirty `git status` after `regen-goldens` means the refactor changed emit and
  must be investigated, NOT blessed. The structural-scheme stage may legitimately
  re-bless where it *fixes* a wrong scheme; re-bless is cheap and automated
  (`regen-goldens`) and must never be weighed as a cost — only the diff's
  *correctness* is reviewed.
- **The ~180-field `Builtins` migration (C3).** A big-bang rewrite would be
  un-reviewable and merge-conflict-prone across parallel family batches.
  Mitigation: `Builtins` shrinks *per family* in the structural-scheme stage,
  additively; a family not yet migrated keeps its fields. No stage touches all
  180 fields.
- **Parallel-batch merge conflicts on the one `def()` match.** All families edit
  the same `match self` — the classic append hotspot. Mitigation: the match is
  append-ordered by family (already the case for the enum); batches partition by
  family and touch disjoint arm ranges, reconciled by union of arm-appends (the
  existing kernel-batch reconcile discipline).
- **`ipe_kernels` must not gain a `types` dep.** `TyShape` is all-`'static` and
  lives in `ipe_kernels`; the *interpreter* lives in `ipe_types`. Mitigation: a
  crate-dep assertion (the leaf-crate invariant from ADR 0028) plus the fact that
  `TyShape` references builtins by `BuiltinTag`, never by `Symbol`.
- **Row-polymorphism fidelity.** A `TyShape::RowOpen` must reproduce the exact
  fresh-var + open-tail behavior of the current hand-built `Ty::Record`.
  Mitigation: migrate row-poly families LAST, each pinned by a scheme-equality
  test against the pre-migration `Ty` before the old arm is deleted.

## Affected work

The Kernel Row de-risks every "add a kernel/module" task by making the add a
single coherent row that the invariant tests gate. Live tracker items and their
relationship to this design are recorded in the delivering pull request's
description (the tracker cross-reference belongs there, not in this timeless
doc). In summary:

- The Cache-codegen undefined-symbol bug and the Random source-vs-kernel drift
  bug are *subsumed*: the first by `runtime_module` on `KernelDef` plus the
  emit-symbol-defined invariant test, the second by the arity↔scheme coherence
  test.
- New-kernel / new-stdlib-module tasks (external-DB `Db.open`, `Std.Codec`,
  `Std.Analytics`, `Html.raw`/`render`) *coordinate*: any kernels they add
  should be authored as Kernel Rows, and any vendored module they ship becomes a
  new `RuntimeModule` variant covered by the invariant test. Compiled-source
  modules (ADR 0029) that use `Ffi.kernel` aliases still route to existing rows.
- The always-failing-to-lower `Task.retryOn`/`withRetryOn` kernels would be
  classified `REACHABLE_BUT_UNLOWERED` rows explicitly by the coherence test
  rather than failing silently.
- The is_* classification-partition tests align with the coherence suite
  (classification derived from the descriptor's `class`/`capability`, per
  ADR 0028's removal of the `is_db()`/`is_ui()` predicates).
- The whole-compiler readability audit should credit the five-methods-into-one-row
  consolidation rather than re-flag it.
- Out of scope: FFI kernels are the open `Ffi(FfiKernelId)` tier (they share the
  `KernelId` sum per ADR 0028 but not the closed-`StdlibKernel` row); and the
  direct-WASM backend would *read* `def().emit`/`class` (benefiting from the row)
  but is not blocked by it.
