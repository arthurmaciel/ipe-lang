# Class 3 — kernel-registry & emitted-name integrity: implementation spec

Status: read-only research complete, no code changed. Companion to
`docs/architecture/campaign-classification-2026-07-09.md` (Class 3) and
`docs/architecture/backlog.md`. AUD-08 (the other original Class-3 member) is
**already landed** (`crates/sky_backend_rust/src/lib.rs`'s function-name
collision guard, `crates/skyc/tests/golden_aud08_function_name_collision.rs`,
commit `8cc5cab`) — not re-specced here.

This spec covers the four remaining items: **#45**, **#70**, **#71**, and
AUD-09's `Match::from_parts_unchecked` pub-visibility finding. Verified against
HEAD as of 2026-07-09 (branch `master`).

Sequencing note (per the classification doc): land these AFTER Class 1
(type-system inference) lands, since #45/#70 touch `sky_kernels` /
`crates/sky_types/src/constrain.rs` / `crates/sky_lower/src/lower.rs` — the
same files Class 1's inference fix touches. Do #45 → #70 → the AUD-09 fix →
#71 in that order (#71 requires no code, so it can land any time as a
doc-only commit; listing it last is just bookkeeping cleanliness).

---

## Item 1 — #71: `explain_lookup` UnknownCode for 8 real page-backed codes

### Finding: already fixed. This is a doc/backlog bookkeeping fix, not a code fix.

`#71`'s description matches **exactly** finding H2d in
`docs/architecture/readability-and-naming-audit.md:59,147-149`: `skyc`'s
`ALL_CODES` (a hand-maintained mirror in `crates/skyc/src/lib.rs`) had drifted
from the real taxonomy in `crates/sky_diagnostics/src/code.rs`, omitting 8
codes that had both a `title()` arm and an `include_str!` explain page:
`SKY-P0016, SKY-P0017, SKY-T0014, SKY-L0114, SKY-L0115, SKY-L0116, SKY-L0117,
SKY-L0119`. The audit's own reproduction: `skyc explain SKY-L0117` returned
`UnknownCode` at the time it was written.

This was fixed by **AUD-09's sibling AUD-15** (`docs/architecture/backlog.md`
line 66: *"AUD-15 🟠 completeness ✅ LANDED (`9b55853`, orchestrate lane)
`crates/sky_diagnostics/src/code.rs` — `ALL_CODES` promoted `pub`, skyc's
drifted hand-mirror deleted."*), which is precisely H2d's prescribed fix
("Promote `pub const ALL: &[Code]` from diagnostics; skyc consumes it; delete
local copy").

Verified on the current tree:

- `crates/skyc/src/lib.rs:29-30` imports `ALL_CODES` directly from
  `sky_diagnostics` — no local mirror exists anymore.
- `crates/sky_diagnostics/src/code.rs:460-473` `ALL_CODES` is `pub` and lists
  all 86 codes (`taxonomy_has_eighty_six_codes`, `:481`).
- `crates/sky_diagnostics/src/code.rs:364-454` `explain_page` returns `Some`
  for every one of the 86 `ALL_CODES` entries — including all 8 previously
  missing codes — enforced by `every_code_has_a_conforming_explain_page`
  (`:508-532`).
- `crates/skyc/src/lib.rs:1156-1167` `all_taxonomy_codes_resolve_via_explain_lookup`
  iterates all 86 `ALL_CODES` entries and asserts `explain_lookup(c.as_str())`
  is `Ok` for every one — a superset proof covering the specific 8.
- `crates/skyc/src/lib.rs:1186-1197` `explain_resolves_sky_t0014` is the
  named regression for one of the 8 codes, with a comment documenting the
  exact pre-fix/post-fix history.

**Empirically re-confirmed during this research pass** (read-only — ran the
existing test, did not modify source):

```
$ timeout 300 cargo test -p skyc all_taxonomy_codes_resolve_via_explain_lookup
running 1 test
test tests::all_taxonomy_codes_resolve_via_explain_lookup ... ok
```

### Action required

No source change. Update `docs/architecture/backlog.md` line 36 to strike
`#71` with a note pointing at AUD-15 (`9b55853`) as the closing commit and
`all_taxonomy_codes_resolve_via_explain_lookup` /
`explain_resolves_sky_t0014` / `code_index_lists_every_code` as the
regression evidence. No new test needed — the existing suite already proves
the property for all 86 codes, a strictly stronger guarantee than the
original 8-code finding.

### Verification command

```
timeout 300 cargo test -p skyc all_taxonomy_codes_resolve_via_explain_lookup explain_resolves_sky_t0014 code_index_lists_every_code
```

---

## Item 2 — #45: constrain kernel-scheme table exhaustive over canon lists

### Finding: mostly already landed; two sub-tasks remain, both already fully designed

`#45` is the tail of a pre-existing, fully-written plan:
`docs/superpowers/plans/2026-07-03-registry-phase-E.md` ("Registry migration
Phase E — the exit-0 SEAL"). That plan's Task 0, Task 1a/1b/1c, and Task 4 are
**already committed**:

| Task | Commit | What it did |
|---|---|---|
| Task 0 (totality pre-flight) | `288ab1a` | Added `stdlib_scheme_total_over_reachable` |
| Task 1a (wildcard-free `stdlib_scheme`) | `3bb21d0` | Narrowed the catch-all to explicit `KNOWN_UNBACKED` |
| Task 1b (`legacy_kernel_ty` → `None`) | `41596b2` | Made the legacy arm dormant |
| Task 1c (delete `kernel_ty` sentinel) | `2681973` | Deleted `Ty::Var(u32::MAX)`; added `no_ty_var_max_sentinel` |
| Task 4 (`#45` reserved-builtin-type gate) | `ef10dab` | `RESERVED_BUILTIN_TYPES` / SKY-N0026 |

Confirmed still green:

```
$ timeout 300 cargo test -p sky_types stdlib_scheme_total_over_reachable
test constrain::registry_phase_c_tests::stdlib_scheme_total_over_reachable ... ok
```

**The types-side exit-0-then-cargo-fail hole is therefore already closed**:
`crates/sky_types/src/constrain.rs:2398-2411` `constrain_var_kernel`'s tail is

```rust
let _ = (module, name); // retained for diagnostics; Task 2 removes them from the node
let registry = id.and_then(|k| self.stdlib_scheme(k));
let ty = Self::kernel_scheme_or_unsupported(registry, None, span)?;
self.instantiate(&ty)
```

Any `id: None` VarKernel now fails **at type-check** with `SKY-L0108`
(`kernel_scheme_or_unsupported`, `:2420-2429`) — it can never reach `cargo`
untyped.

**What remains** is exactly Task 2 + Task 3 of the same plan — and the phrase
"exhaustive over **canon lists**" in `#45`'s own title is Task 3's literal
description. Both are still open, confirmed by direct inspection:

- `crates/sky_canon/src/ast.rs:137-141` — `VarKernel` still carries
  `id: Option<StdlibKernel>` plus redundant `module: Symbol, name: Symbol`.
- `crates/sky_canon/src/env.rs:115-129` — `VarHome::Kernel(Option<StdlibKernel>,
  Symbol, Symbol)`, same redundancy.
- `crates/sky_types/src/constrain.rs:2408` literally says
  `// retained for diagnostics; Task 2 removes them from the node` — a live
  TODO marker still in the tree.
- `crates/sky_canon/src/lib.rs:1630-1772` `canon_equals_registry`'s reverse
  check (Task 3's target) only validates entries that **already** carry
  `Some(id)` (the "G1 reverse" loop at `:1698-1771` matches
  `VarHome::Kernel(Some(actual_sk), m, f)`, `:1748`) — it does **not** assert
  that every non-excluded `qual_vars` member has `Some(id)` in the first
  place. That "no reachable member sits on `None`" property is exactly what
  Task 3 adds.

### Task 3 (do this first — required, low risk, closes "exhaustive over canon lists")

**File:** `crates/sky_canon/src/lib.rs`, inside `canon_equals_registry`
(`:1643-1772`).

1. Extend the existing "G1 reverse check" loop (`:1742-1771`) with a second
   assertion pass. Currently:

   ```rust
   for (qual_sym, members) in &env.qual_vars {
       let qual_str = interner.resolve(*qual_sym).unwrap_or("<unknown>");
       if excluded_quals.contains(qual_str) {
           continue;
       }
       for (name_sym, home) in members {
           if let VarHome::Kernel(Some(actual_sk), m, f) = home {
               // existing G1 propagation check
           }
       }
   }
   ```

   Add a sibling arm that fires when the entry is `VarHome::Kernel(None, ..)`
   — i.e. every member of every non-excluded qualifier must carry `Some`:

   ```rust
   for (name_sym, home) in members {
       match home {
           VarHome::Kernel(Some(actual_sk), m, f) => {
               // existing G1 propagation check, unchanged
           }
           VarHome::Kernel(None, m, f) => {
               let name_str = interner.resolve(*name_sym).unwrap_or("<unknown>");
               let m_str = interner.resolve(*m).unwrap_or("<unknown>");
               let f_str = interner.resolve(*f).unwrap_or("<unknown>");
               panic!(
                   "Task 3 subset gate: qual_vars[{qual_str:?}][{name_str:?}] \
                    is VarHome::Kernel(None, {m_str:?}, {f_str:?}) — every \
                    non-excluded qual_vars member must resolve to a concrete \
                    StdlibKernel id (a `None` here means QUALIFIERS in env.rs \
                    names a member with NO matching StdlibKernel::ALL entry; \
                    either add the missing StdlibKernel variant + scheme, or \
                    add this qualifier to the excluded_quals set with a \
                    comment explaining why it is deliberately unbacked, \
                    mirroring KNOWN_UNBACKED in constrain.rs).",
               );
           }
           _ => {}
       }
   }
   ```

   Use `assert!`/explicit `panic!` (matching the file's existing style for
   this test, which already uses bare `assert!`/`assert_eq!` with rich
   messages — not `Result`), so the test fails loudly rather than silently.

2. Name the new coverage in the test's doc comment (currently
   `:1630-1641` documents only the one-directional forward check — update it
   to note the reverse direction is now a full subset gate, matching the
   plan's Task 3 language).

3. **Do not** touch `excluded_quals` (`:1723-1740`) or `KNOWN_UNBACKED`
   handling — those stay exactly as-is; `KNOWN_UNBACKED` qualifiers (PubSub)
   are structurally absent from `env.qual_vars` (proven by
   `known_unbacked_disjoint_from_qual_vars`, which Task 0 of the plan already
   added in `sky_canon`'s test module — confirm it still exists;
   `rg -n known_unbacked_disjoint_from_qual_vars crates/sky_canon/src/lib.rs`
   before writing the new assertion, to avoid duplicating it), so the new
   loop never iterates them.

**Verify:**

```
timeout 300 cargo test -p sky_canon canon_equals_registry
timeout 300 cargo test -p sky_canon known_unbacked_disjoint_from_qual_vars
```

If the new assertion fires, it prints the exact `(qualifier, name)` pair that
is un-backed — that is a genuine drift finding, not a test bug. Triage per the
panic message: either register the missing `StdlibKernel` variant (+
`stdlib_scheme` arm, following the pattern `#160`/`#85` used for `Error`), or
add the qualifier to `excluded_quals` with a comment citing why (mirroring
`KNOWN_UNBACKED`'s PubSub precedent in `constrain.rs:6387-6390`). Given
`LiveAppRouted`'s qualifier (`Live`, via `env.rs:1388`) already resolves to
`Some(StdlibKernel::LiveAppRouted)` (it is `REACHABLE_BUT_UNLOWERED`, not
`KNOWN_UNBACKED` — only its **scheme** is deliberately `None`, its **id** is
always `Some`), it will **not** trip this gate; expect this test to pass
clean on the first run, but do not skip actually running it.

### Task 2 (structural cleanup — land only after Task 3 is green; wide-touching, do the full-repo grep first)

This is **not required** to close the soundness hole (that's already closed
by Tasks 0/1a/1b/1c) — it is the redundancy cleanup the plan scoped as Task 2:
drop `Option<StdlibKernel>` + the now-redundant `module`/`name` fields from
the canon `VarKernel` node, and delete the ~1,000-line dead legacy string-match
table in `lower.rs::lower_callee` that Task 3 proves is unreachable (every
`id` a real user program can produce is `Some`).

**Pre-flight — run this exact grep before starting, and update every hit, not
just the files listed below** (this list was compiled by grepping the whole
workspace on 2026-07-09; re-run it to catch anything landed since):

```
rg -n "VarKernel|VarHome::Kernel" crates/ -g '!target'
```

Confirmed hit set at time of writing:

1. `crates/sky_canon/src/ast.rs:137-141` — the `VarKernel` node definition.
2. `crates/sky_canon/src/env.rs:115-129` — `VarHome::Kernel` enum variant.
3. `crates/sky_canon/src/env.rs:404-439` `install_builtin_vars` — constructs
   `VarHome::Kernel(id, module, func_sym)` at `:436` from
   `self.stdlib_index.get(&(module, func_sym)).copied()` (`:435`).
4. `crates/sky_canon/src/env.rs:445+` `install_prelude_qualifiers` —
   constructs `VarHome::Kernel(id, mod_sym, name_sym)` at `:1564` (QUALIFIERS
   loop) and `:1577` (FUNC_ALIASES loop).
5. `crates/sky_canon/src/resolve.rs:2317-2330` `var_home_to_expr` — maps
   `VarHome::Kernel(id, m, f)` to `canon::Expr_::VarKernel { id, module, name }`.
6. `crates/sky_canon/src/resolve.rs:2399-2429` `resolve_qual_var` — same
   mapping inline in its `match members.get(&name)` arm.
7. `crates/sky_canon/src/lib.rs` test module — several `#[test]` fns
   destructure `Expr_::VarKernel { id: _, module, name }` and assert against
   resolved interned strings (`:341-353`, `:365-374`, and the wildcard test
   around `:2464`). These need rewriting to assert against the `id`
   `StdlibKernel` variant directly (arguably a nicer test post-change — no
   interner round-trip needed), e.g.
   `assert_eq!(*id, StdlibKernel::LogPrintln)`.
8. `crates/sky_types/src/exhaust.rs:372` — matches `canon::Expr_::VarKernel { .. }`
   with the fully-open `..` pattern. **No change needed** — `{ .. }` does not
   name fields, so it is insensitive to the field-set change.
9. `crates/sky_types/src/constrain.rs:2214-2412` `constrain_var_kernel` — full
   rewrite (see below).
10. `crates/sky_types/src/constrain.rs:2466` — the `VarKernel` match arm in
    `constrain_expr` (or wherever it dispatches) that currently destructures
    `canon::Expr_::VarKernel { id, module, name }` and calls
    `constrain_var_kernel(*id, *module, *name, span)`. Confirm the exact call
    shape by reading `:2460-2470` at implementation time (line numbers may
    have shifted slightly since this research pass) and update to
    `constrain_var_kernel(*id, span)`.
11. `crates/sky_lower/src/lower.rs:8145-9153` `lower_callee`'s
    `canon::Expr_::VarKernel { id, module, name }` arm — full rewrite (see
    below). Deletes the entire `match (self.resolve(*module)?, self.resolve(*name)?) { ... }`
    body (roughly 1,000 lines) down to:

    ```rust
    canon::Expr_::VarKernel { id } => Ok(Callee::Kernel(*id)),
    ```

12. `crates/sky_lower/src/lower.rs:10070-10304` (test module) —
    `REGISTRY_ONLY_ALLOWLIST` const and `decl_equiv_legacy_match` test exist
    **solely** to validate the legacy string table being deleted in step 11.
    Delete both; their premise (a legacy arm to validate) is gone. If any
    part of their bookkeeping (e.g. "every KernelFn is accounted for") is
    still valuable, fold it into a simpler total-coverage assertion over
    `KernelFn::ALL`, but do not keep dead infrastructure referencing a
    deleted code path.
13. `crates/sky_lower/tests/unsupported.rs:445-450, ~502, ~688, ~1345, ~1400`
    — five `canon::Expr_::VarKernel { id: None, module, name }` literal
    constructions in test fixtures. In particular, `fn unknown_kernel_call`
    (`:678-714`) specifically exercises the legacy table's final `SKY-L0108`
    fallthrough that step 11 deletes — its premise ("a `VarKernel` whose id
    is unresolved and whose (module,name) has no legacy arm") becomes
    **unconstructible** once `id` is mandatory (canon itself can only
    produce a `VarKernel` with a concrete `id`; an unrecognized qualified
    name is rejected earlier, at name resolution, with `NameError` — already
    covered by `sky_canon`'s own resolve-error tests). Either delete
    `unknown_kernel_call` with a comment citing why the scenario no longer
    exists post-Task-2, or repoint it at a still-reachable lower-time
    failure (e.g. an unresolved `VarTopLevel` func id) if the test's
    original intent — proving `lower_callee`'s failure path is wired and
    produces `SKY-L0108`/`Feature::Kernels` — needs to stay covered some
    other way (check whether `both_miss_is_fail_closed` in `constrain.rs`,
    which already covers the type-check-side fail-closed path, makes this
    redundant before spending effort re-homing it). The other four sites
    just need their `VarKernel` literal updated from
    `{ id: None, module, name }` to `{ id: <ConcreteStdlibKernel variant for
    that test's fixture> }` — read each site's surrounding context to pick
    the right concrete variant (they were choosing a **valid** kernel name
    to exercise a *different* code path, e.g. arity mismatch, not exercising
    "unknown kernel" itself).
14. `crates/skyc/tests/golden_list_cps.rs:24` and
    `crates/skyc/tests/golden_list_ops_wiring.rs:5,17` — doc comments only,
    describing `VarHome::Kernel` "with NO `KernelFn` variant" as a real
    scenario. Post-Task-2 this is impossible by construction (Task 3
    proves it structurally). Update the prose (not test logic) to say so, or
    confirm the tests' actual assertions don't rely on constructing that
    state (read the full files before touching — they are `#[test]` files
    that build real `.sky` fixtures through the full pipeline, they do not
    hand-construct `VarHome` values, so likely only the comments need a
    wording pass).

**Exact rewrite for `crates/sky_canon/src/ast.rs`:**

```rust
VarKernel {
    id: StdlibKernel,
},
```

(drop `module`/`name` entirely; update the doc comment above it — the
current text at `:131-136` describes the `Option`/legacy-fallback rationale
that no longer applies.)

**Exact rewrite for `crates/sky_canon/src/env.rs`:**

```rust
/// A stdlib kernel function, resolved to its concrete registry id at parse
/// time. Every reachable qualifier's members are proven (by
/// `canon_equals_registry`'s Task-3 subset gate, `sky_canon::lib::tests`) to
/// carry a concrete id — there is no "unresolved kernel" state to represent.
Kernel(StdlibKernel),
```

Update every construction site (3 and 4 above) to drop the now-unused
`module`/`name` locals: e.g. `:436` becomes

```rust
if let Some(id) = self.stdlib_index.get(&(module, func_sym)).copied() {
    self.vars.insert(key, VarHome::Kernel(id));
} else {
    // This branch is now a hard invariant violation, not a legitimate
    // "unregistered kernel" state — install_builtin_vars only ever lists
    // names that are supposed to resolve. Fail loudly (`unreachable!` or a
    // `CompilerBug` diagnostic, matching the crate's existing fail-closed
    // convention) rather than silently dropping the binding.
}
```

Apply the analogous pattern at `:1564` (QUALIFIERS loop) and `:1577`
(FUNC_ALIASES loop) — both already compute `id` via
`self.stdlib_index.get(&(qual_sym, ...)).copied()`; after Task 3 proves this
is always `Some` for every table entry actually iterated here, the `None`
branch becomes provably dead and should fail loud (`unreachable!` with a
message citing the Task-3 gate), not silently construct a
`VarHome::Kernel(None, ...)` that can no longer even type-check.

**Exact rewrite for `crates/sky_types/src/constrain.rs::constrain_var_kernel`:**

```rust
fn constrain_var_kernel(&mut self, id: StdlibKernel, span: Span) -> DResult<VarId> {
    // Obligation pre-checks unchanged in content, only the `if let Some(k) = id`
    // guard becomes `let k = id;` — the eleven `matches!(k, StdlibKernel::...)`
    // blocks at `:2226-2397` are otherwise byte-identical.
    let k = id;
    if matches!(k, StdlibKernel::MathMin | StdlibKernel::MathMax) {
        // ...unchanged...
    }
    // ...unchanged through the Dict/Set/Log*With/LiveApp/LiveRoute blocks...

    // Parse-once registry lookup (Task 2 — module/name fully removed).
    let ty = self.stdlib_scheme(id).ok_or(Diagnostic::Lower {
        span,
        msg: LowerError::Unsupported(Feature::Kernels),
    })?;
    self.instantiate(&ty)
}
```

`kernel_scheme_or_unsupported` (`:2420-2429`) becomes dead once its only
caller passes a single `Option<Ty>` rather than combining registry+legacy —
either delete it and inline the `.ok_or(...)` above, or keep it as a
single-argument helper if `both_miss_is_fail_closed` (`:6587+`) still wants a
unit under test; re-read that test before deciding (it may itself become
obsolete once there is no "legacy" argument to combine — folding its
assertion into a direct `stdlib_scheme(id).is_none()` check for an
intentionally-unbacked kernel is the simplest path forward).

**Exact rewrite for `crates/sky_lower/src/lower.rs::lower_callee`:**

```rust
fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
    match &callee.value {
        canon::Expr_::VarKernel { id } => Ok(Callee::Kernel(*id)),
        canon::Expr_::VarTopLevel { module, name } => {
            // unchanged
        }
        _ => Err(bug(/* unchanged */)),
    }
}
```

This deletes the ~1,000-line string-match body (`:8155-9152` in the current
tree) in full.

**Verify (whole sequence, after all file edits land together in one commit —
this is a coupled cross-crate change and cannot be split without an
intermediate broken build):**

```
timeout 900 cargo build --workspace
timeout 1800 cargo test -p sky_canon -p sky_types -p sky_lower -p sky_backend_rust -p skyc
timeout 1800 cargo clippy --workspace --all-targets -- -D warnings
```

Then the example sweep (per CLAUDE.md — never `sky build`/`cargo build` from
repo root; use the `sky-rust-backend:examples-sweep` skill or
`scripts/example-sweep.sh`) to confirm no example that used to build now
regresses.

### New regression tests needed for #45

1. Task 3's extended `canon_equals_registry` reverse-subset assertion itself
   IS the regression test (see above) — it fails loudly on any future drift
   between `QUALIFIERS` (or `install_builtin_vars`) and `StdlibKernel::ALL`.
2. If Task 2 lands: a compile-time-only "test" in the sense that `VarKernel`
   no longer being `Option`-shaped means any future accidental reintroduction
   of an optional/legacy path is a type error, not a runtime gap — no
   additional runtime test is needed for that half, but DO keep (or migrate)
   the per-variant coverage the deleted `decl_equiv_legacy_match` provided,
   in a lighter form: a `KernelFn::ALL`-total sanity check that
   `lower_callee` succeeds for a synthetic `VarKernel { id: sk }` node, for
   every `sk` — this is now trivial (`Ok(Callee::Kernel(*id))` can't fail),
   so it may not be worth keeping at all; use judgement, but do not leave a
   coverage gap silently.

---

## Item 3 — #70: kernel arity-table drift (`decl().arity` vs `callee_arity`)

### Finding: two independently hand-maintained arity sources, no cross-check test exists yet

`StdlibDecl.arity` (`crates/sky_kernels/src/lib.rs:91`, "Sky-level arity:
number of arguments before the result") is set per-variant inside
`StdlibKernel::decl()` (`crates/sky_kernels/src/lib.rs:1183-3900+`, via the
`d(qualifier, name, arity, class, emit)` shorthand at `:1185-1199`).

Completely separately, `crates/sky_lower/src/lower.rs:6864-8116`
`fn callee_arity(&self, callee: &Callee) -> DResult<usize>` hand-lists every
`KernelFn` variant grouped into arity buckets:

```rust
Callee::Kernel(
    KernelFn::MathPi | KernelFn::MathE | ... ,
) => Ok(0),
Callee::Kernel(
    KernelFn::StringFromInt | ... ,
) => Ok(1),
// ... many more buckets ...
Callee::Kernel(KernelFn::DecFormatWith) => Ok(4),
Callee::Func(id) => { /* derives arity from patterns.len() — unaffected */ }
```

Since `KernelFn` is a type alias for `sky_kernels::StdlibKernel`
(`crates/sky_ir/src/ir.rs:1236`, `pub type KernelFn = sky_kernels::StdlibKernel;`),
`decl().arity` is **directly available** on the same value `callee_arity`
matches on — there is no representational reason for two tables to exist.
Because `match callee { Callee::Kernel(...) => ..., Callee::Func(...) => ... }`
must be exhaustive over every `KernelFn`/`StdlibKernel` variant, this compiles
today for every variant (Rust's exhaustiveness check catches a *missing*
variant), but nothing catches a **wrong** arity value in one of the buckets —
that is the drift `#70` names, and it is exactly the "exit-0-then-cargo-fail"
shape: `constrain_var_kernel`'s scheme governs how many args type-check
successfully, while `callee_arity` governs how many args the emitted call
site actually threads through at the IR level (`callee_arity` call sites:
`:5756, 5814-5839, 6328, 6769`, e.g. deciding eta-expansion / saturation / TEA
default-arg elision) — if the two disagree, a program can pass type-checking
with `stdlib_scheme`'s arity but get saturated/under/over-applied against
`callee_arity`'s different count, producing a Rust call with the wrong
argument count only `cargo` catches.

**No existing test cross-checks `decl().arity` against `callee_arity`'s
buckets.** (Confirmed: `rg -n "callee_arity|decl\(\)\.arity" crates/sky_lower/src/lower.rs`
shows only call sites and one comment cross-reference, no assertion.) This is
in contrast to the *qualifier/name* dimension, which **is** already
machine-checked by `decl_equiv_legacy_match` (soon to be deleted per Item 2's
Task 2, step 12 — all the more reason arity needs its own independent gate
before that safety net goes away).

### Fix — test-first, then collapse the hand table to the single source of truth

**Step 1 — add the parity test** (`crates/sky_lower/src/lower.rs`, test
module, alongside `decl_equiv_legacy_match`):

```rust
/// #70 — `callee_arity`'s hand-written per-variant arity buckets must agree
/// with `StdlibKernel::decl().arity` (the same enum, aliased as `KernelFn`).
/// A drift here is the exit-0-then-cargo-fail class: `constrain_var_kernel`
/// types a call against `stdlib_scheme`'s arrow count while `callee_arity`
/// governs how many args the IR actually saturates/eta-expands against —
/// disagreement produces a well-typed Sky program whose emitted Rust call
/// has the wrong argument count, caught only by `cargo`, never by `skyc`.
#[test]
fn callee_arity_matches_decl_arity() {
    let interner = Interner::new();
    let module = canon::Module { name: vec![], unions: vec![], defs: vec![] };
    let types = SolvedTypes {
        env: BTreeMap::new(),
        regions: BTreeMap::new(),
        bounds: BTreeMap::new(),
        warnings: Vec::new(),
        poly_var_map: BTreeMap::new(),
    };
    // `callee_arity` never reads `self.builtins`/`self.m` for a `Callee::Kernel`
    // arm (only the `Callee::Func` arm does), so a minimal/placeholder
    // `BuiltinCtors` is safe here — mirror `decl_equiv_legacy_match`'s
    // construction recipe (`:10123-10242`) rather than hand-rolling a new one.
    let builtins = /* same BuiltinCtors literal decl_equiv_legacy_match builds */;
    let lowerer = Lowerer::new(
        &module,
        &types,
        &interner,
        SymbolPools {
            eta_params: vec![],
            cap_params: vec![],
            param_binders: vec![],
            any_param_binders: vec![],
        },
        &builtins,
    );

    let mut mismatches = Vec::new();
    for &sk in KernelFn::ALL {
        let decl_arity = usize::from(sk.decl().arity);
        match lowerer.callee_arity(&Callee::Kernel(sk)) {
            Ok(computed) if computed == decl_arity => {}
            Ok(computed) => mismatches.push(format!(
                "{sk:?}: decl().arity={decl_arity} but callee_arity={computed}"
            )),
            Err(e) => mismatches.push(format!("{sk:?}: callee_arity() errored: {e:?}")),
        }
    }
    assert!(
        mismatches.is_empty(),
        "decl().arity / callee_arity drift found ({} entries):\n{}",
        mismatches.len(),
        mismatches.join("\n"),
    );
}
```

Reuse the exact `BuiltinCtors` construction from `decl_equiv_legacy_match`
(`crates/sky_lower/src/lower.rs:10123-10242`) verbatim — do not hand-roll a
second copy; if that test is deleted per Item 2 step 12 in the same overall
change, extract the `BuiltinCtors` builder into a small shared test helper
first so this test and any Task-2 replacement both use it.

**Step 2 — run it and triage.** Any mismatch printed is a genuine bug: cross-
check the actual Rust runtime function's signature in
`runtime/src/sky_runtime/*.rs` (via `decl().emit`, the runtime symbol name)
to determine which side is wrong, then fix `decl()`'s `arity` field (the
`StdlibDecl` is the more central, semantically-meaningful source — "Sky-level
arity" — so prefer fixing it over adjusting `callee_arity`'s bucket, unless
investigation shows the runtime genuinely needs a different physical arg
count than the Sky-level signature implies, which would itself be a separate,
deeper bug to file). Do **not** proceed to Step 3 until this test is green.

**Step 3 — collapse the hand table** (only after Step 2 is green). Replace
`crates/sky_lower/src/lower.rs:6871-8115`'s entire body with:

```rust
fn callee_arity(&self, callee: &Callee) -> DResult<usize> {
    match callee {
        Callee::Kernel(k) => Ok(usize::from(k.decl().arity)),
        Callee::Func(id) => {
            let idx = usize::try_from(id.as_raw()).unwrap_or(usize::MAX);
            let def = self.m.defs.get(idx).ok_or_else(|| {
                bug("sky_lower::callee_arity", "func id has no matching definition")
            })?;
            Ok(match def {
                canon::Def::Typed { patterns, .. } | canon::Def::Untyped { patterns, .. } => {
                    patterns.len()
                }
            })
        }
    }
}
```

This deletes ~1,200 lines and makes `#70`'s drift class **structurally
unrepresentable** going forward: there is only one place arity is declared.
Update `Step 1`'s test's doc comment to note it is now a tautology
(`decl().arity == decl().arity`) and either delete it as redundant or repoint
it to instead assert `callee_arity` is *implemented* as the direct
`decl().arity` delegation (a source-scan test in the same style as
`no_ty_var_max_sentinel`, asserting the string `"usize::from(k.decl().arity)"`
appears in the function and the old per-arity-bucket pattern does not) so a
future regression (someone re-introducing a hand-written bucket) is still
caught.

### Recommended stretch (optional, same drift class, cheap): #70b — `stdlib_scheme` arrow-arity parity

Independent of `callee_arity`, `crates/sky_kernels/src/lib.rs:1571`'s comment
names a THIRD, currently-unenforced invariant: *"Arity is 1 ... so the
FIRST_SCHEMED `arrow-count == decl().arity` invariant holds against the
scheme."* Several comments in `constrain.rs` (`:4239, 4290, 4316, 4896, 5736-5738`)
reference this same "arrow-count == decl().arity" invariant being maintained
by hand during scheme authoring, with no test enforcing it. Add (same
`crates/sky_types/src/constrain.rs` test module as `stdlib_scheme_total_over_reachable`):

```rust
fn arrow_arity(ty: &Ty) -> usize {
    let mut n = 0;
    let mut cur = ty;
    while let Ty::Fun(_, ret) = cur {
        n += 1;
        cur = ret;
    }
    n
}

#[test]
fn stdlib_scheme_arrow_arity_matches_decl() {
    let mut interner = Interner::new();
    let builtins = make_builder(&mut interner);
    let mut uf = UnionFind::<Content>::new();
    let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);

    let mut mismatches = Vec::new();
    for &k in StdlibKernel::ALL {
        let Some(ty) = builder.stdlib_scheme(k) else { continue }; // KNOWN_UNBACKED / REACHABLE_BUT_UNLOWERED
        let got = arrow_arity(&ty);
        let want = usize::from(k.decl().arity);
        if got != want {
            mismatches.push(format!("{k:?}: decl().arity={want} but scheme arrow-arity={got}"));
        }
    }
    assert!(mismatches.is_empty(), "scheme/decl arity drift:\n{}", mismatches.join("\n"));
}
```

This is genuinely optional (not literally what `#70` names) — include it only
if time allows; if it fails, expect the "direct-build" kernels
(`Math.min/max`, `Basics.clamp/negate/abs/min/max/compare/toString`,
Dict/Set key-tie, `Log.*With`, `Live.app`, `Live.route` —
`constrain.rs:2226-2397`) to need per-kernel judgement, since some of those
intentionally instantiate the *base* scheme and then re-tie variables rather
than using the scheme's raw arrow shape 1:1; read each one's comment before
concluding a mismatch is a bug rather than an intentional shape (e.g., the
base scheme underlying a direct-build kernel might have a different arrow
count than the direct-build result if the direct-build path adds/removes an
argument position — verify against the specific kernel's documented type
before filing).

### Verification commands for #70

```
timeout 600 cargo test -p sky_lower callee_arity_matches_decl_arity
timeout 900 cargo build --workspace
timeout 1800 cargo test -p sky_lower -p sky_types -p sky_backend_rust -p skyc
timeout 1800 cargo clippy --workspace --all-targets -- -D warnings
```

Then the example sweep, same as Item 2.

---

## Item 4 — AUD-09: `Match::from_parts_unchecked` is `pub`

### Finding

`docs/architecture/principles-audit-2026-07-09.md:84-89` (⚪ invalid-states):

> `crates/sky_ir/src/ir.rs:1626-1628` — Doc claims Match's only constructor
> validates arm exhaustiveness, but this `pub` escape hatch builds a Match
> from arbitrary arms (empty vec → `match x {}` → E0004, no Sky diagnostic).
> **Fix:** replace with a shape-preserving `map_bodies` combinator that
> cannot change patterns; or `pub(crate)`-seal + debug-assert arm shapes.

Current location (line numbers have shifted since the audit; re-verified):
`crates/sky_ir/src/ir.rs:1664-1673`:

```rust
/// Rebuild a `Match` from raw parts without re-running structural
/// validation. Only safe when the patterns are unmodified — the structural
/// invariants checked by [`Self::new`] / [`Self::new_flat`] are over the
/// arm pattern shapes, not over the arm bodies. Body rewrites (e.g.
/// variable-to-apply substitution) never change pattern shapes, so this
/// stays sound.
#[must_use]
pub const fn from_parts_unchecked(scrutinee: Box<Expr>, arms: Vec<Arm>) -> Self {
    Self { scrutinee, arms }
}
```

**`pub(crate)`-sealing is not viable**: `from_parts_unchecked` is called from
**outside** `sky_ir` — `crates/sky_lower/src/lower.rs:860, 1439, 2440` and
`crates/sky_backend_rust/src/emit_expr.rs:304, 622` — three body-rewrite
passes (`rewrite_captured_clones`, `rewrite_multiuse_clones`,
`rewrite_var_to_apply`) plus two more (`clone_free_target`, `substitute_var`).
`pub(crate)` would break all five call sites' crates. The audit's own
alternative — "a shape-preserving `map_bodies` combinator that cannot change
patterns" — is therefore the correct fix, and it is a clean fit: **all five
call sites already follow the identical shape**, confirmed by reading each
one:

```rust
let (scrutinee, arms) = m.into_parts();
let new_scrutinee = Box::new(<recurse>(*scrutinee));   // sometimes fallible (`?`)
let new_arms = arms.into_iter().map(|arm| {
    let new_body = <recurse-or-keep>(arm.body);        // sometimes fallible (`?`)
    Arm { pat: arm.pat, body: new_body }               // pat is ALWAYS carried through untouched
}).collect();
Match::from_parts_unchecked(new_scrutinee, new_arms)
```

`into_parts` (`crates/sky_ir/src/ir.rs:1659-1662`) has the same five callers
and no others — it stays sound to keep (decomposing a validated `Match`
cannot forge an invalid one; only *reconstructing* one without validation
can), but after this fix it is only used internally by the two new
combinator methods, so it should be narrowed from `pub` to `pub(crate)` as a
tidy-up (optional, not required for the soundness fix).

Of the five callers, four are **infallible** (`rewrite_multiuse_clones`
`:1342`, `rewrite_var_to_apply` `:2369`, `clone_free_target`
(`emit_expr.rs:239`), `substitute_var` (`emit_expr.rs:557`) — all
`fn(...) -> Expr`), and one is **fallible**
(`rewrite_captured_clones` `:631`, `fn(...) -> DResult<Expr>`). This drives
two combinator variants.

### Fix

**File:** `crates/sky_ir/src/ir.rs`. Replace `from_parts_unchecked`
(`:1664-1673`) with:

```rust
/// Rebuild a `Match` by transforming its scrutinee and every arm's body,
/// leaving every arm's PATTERN — and the arm count/order — completely
/// untouched. [`Self::new`]/[`Self::new_flat`]'s exhaustiveness invariant is
/// a property of the pattern SHAPES alone (see their doc comments), so a
/// transformation that only ever touches `scrutinee` and [`Arm::body`] can
/// never invalidate it. This is the sound, sealed replacement for the former
/// `pub fn from_parts_unchecked`, which took a raw `Vec<Arm>` and could
/// rebuild a `Match` with an empty arm list (`match x {}` — rustc E0004, no
/// Sky diagnostic) or a reordered/dropped-arm list; `pub(crate)`-sealing was
/// not viable instead, because every current caller of the old function
/// lives in a different crate (`sky_lower`, `sky_backend_rust`).
#[must_use]
pub fn map_bodies(
    self,
    scrutinee_map: impl FnOnce(Expr) -> Expr,
    mut body_map: impl FnMut(&Pat, Expr) -> Expr,
) -> Self {
    let (scrutinee, arms) = self.into_parts();
    let new_scrutinee = Box::new(scrutinee_map(*scrutinee));
    let new_arms = arms
        .into_iter()
        .map(|arm| {
            let new_body = body_map(&arm.pat, arm.body);
            Arm { pat: arm.pat, body: new_body }
        })
        .collect();
    Self { scrutinee: new_scrutinee, arms: new_arms }
}

/// Fallible sibling of [`Self::map_bodies`] for passes that can fail (e.g.
/// depth-limited clone-capture rewriting). Same shape invariant: only
/// `scrutinee` and [`Arm::body`] are transformed; each arm's `pat` is carried
/// through untouched, and a failure short-circuits before any arm is lost or
/// reordered.
pub fn try_map_bodies<E>(
    self,
    scrutinee_map: impl FnOnce(Expr) -> Result<Expr, E>,
    mut body_map: impl FnMut(&Pat, Expr) -> Result<Expr, E>,
) -> Result<Self, E> {
    let (scrutinee, arms) = self.into_parts();
    let new_scrutinee = Box::new(scrutinee_map(*scrutinee)?);
    let new_arms = arms
        .into_iter()
        .map(|arm| {
            let new_body = body_map(&arm.pat, arm.body)?;
            Ok(Arm { pat: arm.pat, body: new_body })
        })
        .collect::<Result<Vec<_>, E>>()?;
    Ok(Self { scrutinee: new_scrutinee, arms: new_arms })
}
```

Narrow `into_parts` (`:1653-1662`) from `pub` to `pub(crate)` (optional
tidy-up, not required — it does not itself carry a soundness risk).

**Update the five call sites** (both `Arm`/`Pat` are already imported in both
files, no new `use` needed):

1. `crates/sky_lower/src/lower.rs:815-860` (`rewrite_captured_clones`,
   fallible — use `try_map_bodies`):

   ```rust
   Expr::Match(m) => Ok(Expr::Match(m.try_map_bodies(
       |scrutinee| rewrite_captured_clones(clone_set, noncl_set, lambda_span, scrutinee, depth),
       |pat, body| {
           if pat_binds_any_in(pat, clone_set) || pat_binds_any_in(pat, noncl_set) {
               let inner_clone: BTreeSet<Symbol> = clone_set
                   .iter()
                   .copied()
                   .filter(|&s| !pat_binds_symbol(pat, s))
                   .collect();
               let inner_noncl: BTreeSet<Symbol> = noncl_set
                   .iter()
                   .copied()
                   .filter(|&s| !pat_binds_symbol(pat, s))
                   .collect();
               rewrite_captured_clones(&inner_clone, &inner_noncl, lambda_span, body, depth)
           } else {
               rewrite_captured_clones(clone_set, noncl_set, lambda_span, body, depth)
           }
       },
   )?)),
   ```

2. `crates/sky_lower/src/lower.rs:1424-1439` (`rewrite_multiuse_clones`,
   infallible — use `map_bodies`):

   ```rust
   Expr::Match(m) => Expr::Match(m.map_bodies(
       |scrutinee| rewrite_multiuse_clones(sym, remaining, scrutinee),
       |pat, body| {
           if pat_binds_symbol(pat, sym) {
               body
           } else {
               rewrite_multiuse_clones(sym, remaining, body)
           }
       },
   )),
   ```

3. `crates/sky_lower/src/lower.rs:2423-2440` (`rewrite_var_to_apply`,
   infallible):

   ```rust
   Expr::Match(m) => Expr::Match(m.map_bodies(
       |scrutinee| rewrite_var_to_apply(target, scrutinee),
       |pat, body| {
           if pat_binds_symbol(pat, target) {
               body
           } else {
               rewrite_var_to_apply(target, body)
           }
       },
   )),
   ```

4. `crates/sky_backend_rust/src/emit_expr.rs:287-304` (`clone_free_target`,
   infallible):

   ```rust
   Expr::Match(m) => Expr::Match(m.map_bodies(
       |scrutinee| clone_free_target(scrutinee, target),
       |pat, body| {
           if pat_binds_target(pat, target) {
               body
           } else {
               clone_free_target(body, target)
           }
       },
   )),
   ```

5. `crates/sky_backend_rust/src/emit_expr.rs:605-622` (`substitute_var`,
   infallible):

   ```rust
   Expr::Match(m) => Expr::Match(m.map_bodies(
       |scrutinee| substitute_var(scrutinee, target, replacement),
       |pat, body| {
           if pat_binds_target(pat, target) {
               body
           } else {
               substitute_var(body, target, replacement)
           }
       },
   )),
   ```

Each replacement is a pure refactor — behavior is byte-identical (same
recursion, same short-circuiting on the fallible one), so the existing test
suites for these four passes / one pass are the primary regression coverage;
they must stay green unchanged.

### New regression tests needed for AUD-09

1. **Unit tests in `crates/sky_ir/src/ir.rs`'s existing `#[cfg(test)] mod tests`**
   (alongside `match_new_accepts_exhaustive_and_round_trips_debug` etc.,
   `:1676+`):

   - `map_bodies_preserves_arm_patterns_and_count`: build a real 2-arm
     `Match` via `Match::new` (reuse the `msg_enum` helper at `:1682-1687`),
     call `.map_bodies(|s| s, |_, b| b)` (identity transform), and assert the
     result has the same arm count, the same patterns (compare `Pat`, which
     implements `PartialEq`), and structurally-equal bodies. Then call it
     again with a body transform that actually rewrites (e.g. wraps each arm
     body in a marker `Expr`) and assert only the bodies changed.
   - `try_map_bodies_short_circuits_on_err`: same setup, but `body_map`
     returns `Err(...)` for the second arm; assert the whole call returns
     `Err` and that no partial/corrupted `Match` is observable (the method
     consumes `self` and only returns `Result<Self, E>`, so this is really
     asserting the error variant and message, but it's worth pinning as a
     named regression since it's the property the old `from_parts_unchecked`
     callers relied on via `?` short-circuiting inside `.map().collect::<Result<...>>()`).
   - `try_map_bodies_scrutinee_error_short_circuits_before_any_arm_runs`: pass
     a `scrutinee_map` that returns `Err` and a `body_map` that would panic if
     called; assert the overall call is `Err` and (implicitly, since it
     didn't panic) `body_map` was never invoked.

2. **Source-level regression test**, mirroring `no_ty_var_max_sentinel`
   (`crates/sky_types/src/constrain.rs:6559-6576`) exactly, in
   `crates/sky_ir/src/ir.rs`'s test module:

   ```rust
   /// AUD-09 seal: `from_parts_unchecked` must never be reintroduced. Any
   /// caller that needs to rebuild a `Match` after a body-only rewrite must
   /// use `map_bodies`/`try_map_bodies`, which cannot change arm patterns or
   /// arm count/order.
   #[test]
   fn no_from_parts_unchecked_reintroduced() {
       let src = include_str!("ir.rs");
       for (idx, line) in src.lines().enumerate() {
           let code = line.split("//").next().unwrap_or(line);
           assert!(
               !code.contains("from_parts_unchecked"),
               "AUD-09 seal reintroduced at ir.rs:{} — rebuild via \
                Match::map_bodies/try_map_bodies instead: {line:?}",
               idx + 1,
           );
       }
   }
   ```

   (Build the "banned token" via a literal here, unlike
   `no_ty_var_max_sentinel`'s `concat!`-split trick — that trick exists there
   only because the banned token is short and generic enough
   (`Ty::Var(u32::MAX)`) to plausibly self-match inside the test's own
   source; `"from_parts_unchecked"` is long/specific enough that this test's
   own line containing it in a string literal would itself trip the `code`
   filter's `//`-split... actually check this: the test's own source line
   `!code.contains("from_parts_unchecked")` contains the banned token as a
   plain string literal, which the `code.split("//")` strip does NOT remove
   — so this test **would self-match** exactly like `no_ty_var_max_sentinel`
   worried about. Follow the same defusing pattern: build the needle via
   `concat!("from_parts_un", "checked")` so the test file's own occurrence of
   the split needle does not contain the contiguous banned string.)

3. **No new E2E/golden fixture is strictly required** — the five refactored
   call sites are pure internal rewrites with identical behavior, covered by
   whatever existing suites exercise `rewrite_captured_clones` /
   `rewrite_multiuse_clones` / `rewrite_var_to_apply` / `clone_free_target` /
   `substitute_var` today. **However**, note while researching this item:
   none of the existing AUD-04 golden fixtures
   (`crates/skyc/tests/golden_aud04_emit_expr_ir_capture.rs`, covering
   `TaskSeq`/`TaskSeqSync`/record/string-literal clone-capture) actually
   exercise a `case`/`Match` expression inside a closure that captures a
   clone-tracked variable — i.e. the `Expr::Match` arm in
   `clone_free_target`/`rewrite_captured_clones` has no dedicated golden
   coverage today, independent of this refactor. Recommended (not strictly
   required) addition: a `golden_match_arm_clone_capture` fixture — a
   `Task.andThen`/closure that captures a non-`Copy` value used both before
   and inside a `case` expression's arm bodies — asserting (a) `skyc build`
   succeeds and (b) (`SKY_E2E=1`) the emitted binary's stdout matches the
   uncorrupted value, following the exact `assert_skyc_ok`/`assert_e2e_output`
   pattern already in that file. This would have caught a hypothetical
   regression in the `map_bodies` refactor (e.g. an accidentally-dropped
   arm) that the pure `sky_ir` unit tests in (1) cannot, since those never
   invoke the real `sky_lower`/`sky_backend_rust` passes end-to-end.

### Verification commands for AUD-09

```
timeout 300 cargo test -p sky_ir
timeout 900 cargo build --workspace
timeout 1800 cargo test -p sky_lower -p sky_backend_rust -p skyc
timeout 1800 cargo clippy --workspace --all-targets -- -D warnings
```

If the recommended golden fixture is added, also run it under `SKY_E2E=1`
per its own doc header convention.

---

## Summary — commit sequencing recommendation

1. **#71** — doc-only backlog update (no code). Land any time, independently.
2. **#45 Task 3** — `sky_canon` only, additive test, low risk. Land first
   among the code items.
3. **AUD-09** — `sky_ir` + 5 call sites in `sky_lower`/`sky_backend_rust`.
   Self-contained, no interaction with #45/#70's files beyond the shared
   `Match` type. Can land in parallel with #45 Task 3, or right after.
4. **#70** — `sky_kernels` (no change) + `sky_lower::callee_arity` (test then
   collapse). Independent of #45/AUD-09; land any time after Class 1 (shared
   `sky_types`/`sky_lower` churn risk noted in the classification doc).
5. **#45 Task 2** — widest-touching, do last among these four so the
   `VarKernel`/`VarHome::Kernel` shape is stable while #70's `callee_arity`
   collapse and AUD-09's `Match` combinator work land on the pre-Task-2 node
   shape (both are compatible with either shape, but landing Task 2 last
   minimizes simultaneous churn in `sky_lower::lower.rs`, which all of #45
   Task 2, #70, and AUD-09 touch).

Every step above ends with, at minimum:

```
timeout 1800 cargo build --workspace
timeout 1800 cargo test --workspace
timeout 1800 cargo clippy --workspace --all-targets -- -D warnings
```

plus an example-sweep pass before considering the Class-3 campaign closed,
per CLAUDE.md's release-gate discipline (never skip the runtime-verify step
because the type-checker/unit-tests are green — this class's entire premise
is "green skyc, red cargo").
