# Sky (Haskell) Upstream Learnings — Adopt / Adapt / Reject

> **Scope.** Synthesis of five facet audits comparing the upstream Haskell Sky
> compiler against the Rust Ipê backend port. Every learning is critiqued
> against our six principles **in strict order** — (1) security (2) correctness
> (3) soundness (4) efficiency (5) **COMPLETENESS** (6) readability — plus the
> two verbatim rules **"PARSE, DON'T VALIDATE"** and **"MAKE INVALID STATES
> UNREPRESENTABLE."** A learning earns import only if it makes Ipê *more secure,
> sound, correct, or COMPLETE* without violating a higher principle.

## 1. Honest framing

Upstream's real edge is **completeness and traceability of the canon/import
layer**, not its type theory. Two things it does structurally better and we are
demonstrably missing: (a) it keeps a first-class `_importAliases` map so any
canonical reference can be traced back to the import that introduced it, and
(b) it filters every dependency through its own `exposing` clause
(`filterDepByExports`) *before* the importer sees a single name — a fail-closed
export firewall. Its module-dependency threading also resolves forward
declarations (union constructors) at import-injection time with the type-var
quantification intact, which is exactly the COMPLETENESS axis we undervalue.
What we must **not** copy: upstream's `SuperType` constraint-*merging* algebra
(`Number ∪ Comparable → CompAppend`) — our independent-bit `TyBounds` model is
sounder and simpler for our module scope; its **untyped** `errorToString`/
`toString` coercion (we add a typed `SHOW` obligation, a genuine
soundness+security win over upstream); and any Haskell-ism that treats the
kernel-vs-source distinction implicitly rather than as an enforced,
compile-checked registry status. In short: adopt upstream's completeness
discipline, reject its constraint-merging and its untyped stringify.

## 2. ADOPT / ADAPT / REJECT table

| # | Learning | Facet | Principle advanced (in order) | Verdict | Concrete Ipê action / task |
|---|---|---|---|---|---|
| L1 | Dedicated `_importAliases : qualifier → canonical module` map, populated at import time | Import-alias (#78) | (5) COMPLETENESS, (2) correctness | **ADOPT** | Add `pub import_aliases: BTreeMap<Symbol, Vec<Symbol>>` to `crates/sky_canon/src/env.rs`; populate in `resolve.rs::inject_dep_exports` (~L576) with `(qualifier, dep_path)`. **#78** |
| L2 | Registry-vs-source is a load-bearing SOUNDNESS invariant: kernel modules' surface is registry-controlled, not parsed | Import-alias (#78) | (3) soundness, (5) completeness | **ADOPT** | In `canonicalise_module`, if a stdlib module is in the compiled index, use kernel registry not parsed `ModuleExports`. Add tripwire test `canon_stdlib_registry_parity_check`. **#78** |
| L3 | `dep_path.last()` qualifier heuristic is O(1) but incomplete for dotted qualifiers (`Db.Decode`) | Import-alias (#78) | (4) efficiency, (5) completeness | **ADAPT** | After L1 lands, resolve `Q.member` via `import_aliases.get(Q)` first; pre-populate for prelude qualifiers in `install_prelude_qualifiers`. **#78** |
| L4 | `filterDepByExports` enforces a dep's own `exposing` clause before contributing names | Import-alias (#78) / Canon | (5) completeness, (2) correctness, (1) security | **ADAPT** | Add `filter_dep_by_exports` to `resolve.rs`, call in `inject_dep_exports` **before** injecting; warn-only first, then error. Keep kernel registry ↔ export metadata in sync via CI check. **#78** |
| L5 | `Error = Error ErrorKind ErrorInfo` emitted via standard tuple-ADT lowering; 2-field layout must be preserved | Error module | (2) correctness, (3) soundness | **ADOPT** | Register `Error` in canon QUALIFIERS; wire `ErrorKind`/`ErrorInfo` ctors with correct arity; codegen the explicit `Error` constructor. Validate line-by-line vs `Error.ipe:33-34`. **#78** |
| L6 | `errorToString` is a **Basics** kernel of arity 1 constrained by Stringify — NOT an `Error` export | Error module | (2) correctness, (5) completeness | **ADOPT** | Add `BasicsErrorToString` to `sky_kernels::StdlibKernel` (`Basics`,`errorToString`,1,Pure,`basics_error_to_string`); wire in `lower.rs::lower_callee`. **#77** |
| L7 | `errorToString`'s generic is gated by the Stringify (`SHOW`) obligation at every call site; codegen synthesizes `impl SkyStringify` | Error module / Obligations | (1) security, (2) correctness, (3) soundness | **ADOPT** | `constrain_var_kernel("errorToString")` must enforce `SHOW`; add `Stringify` to `sky_types/src/constrain.rs` alongside Comparable/Number/Appendable. Regression: `toString (\x -> x)` fails `IPE-T0014`. **#77** |
| L8 | `SkyStringify` for `Error` must be **total** via autoref `Wrap(&field).dispatch()` (Debug fallback), never direct `field.sky_show()` | Error module | (3) soundness, (2) correctness | **ADOPT** | Emitter must use the `Wrap`-dispatch pattern for `Error`'s impl; verify all 5 `ErrorDetails` variants have impls/fallbacks. Regression: `errorToString(Error.ffi …)` no panic. **#77** |
| L9 | `ErrorDetails` is structurally present but semantically unused by `toString` (renders kind+message only) | Error module | (2) correctness, (5) completeness | **ADAPT** | Keep `details` in the generated struct (do NOT elide); `toString` renders kind+message only; add a separate `Debug.toDump`-style full-detail renderer later. **#77 (footnote)** |
| L10 | Error/errorToString are **zero-coverage** in Ipê today; gap is registration+lowering glue, not architecture | Error module | (5) completeness, (2) correctness | **ADAPT** | Land **#77 + #78** before any sweep that touches `Error`. Mirror upstream 100% or document intentional gap. **#77/#78** |
| L11 | `SuperType` ADT (4 ctors, with merging) vs `TyBounds` bitset (8 independent bits) | Obligations | (3) soundness | **ADAPT** | Keep the bitset. Document in `ty.rs` that bits are **not** merged (each checked independently). Regression that combining bits still behaves correctly. |
| L12 | Obligations generated but checked **post-solve** (not pre-bound to result vars) to allow polymorphic operator use | Obligations | (2) correctness | **ADOPT** | Verify `constrain.rs` does NOT pre-bind obligation bits; obligations run via post-solve gates. Document the deferral in module doc. |
| L13 | Number flexes default to `Int` at read-back | Obligations | (2) correctness | **ADOPT** | Already implemented (`lib.rs:127-147`). No action; keep regression. |
| L14 | Appendable/CompAppend absent because List is not in M0 | Obligations | (5) COMPLETENESS | **REJECT (now) / defer to M1** | Reserve a `TyBounds` bit + TODO near `SHOW`; add M1 regression stub (`[1] ++ [2]` fails in M0, passes M1). Coordinate with **#36**. |
| L15 | Typed **Stringify** obligation (`SHOW` bit) — upstream has none (untyped coercion) | Obligations | (3) soundness, (2) correctness, (1) **security WIN over upstream** | **ADOPT** | Wire `Log.*With` + `Debug.toString` + `toString` sigs to carry `SHOW`. Regression as in L7. **#77** |
| L16 | Rigid super-types carry rigidity + obligations simultaneously (`Super { rigid, bounds }`) | Obligations | (2) correctness, (3) soundness | **ADOPT** | Already correct (`unify.rs:143-163`). Document: a rigid Super carries the annotation's promise. |
| L17 | Two-level super check: head-only at unify time, deep (reject functions anywhere) post-solve | Obligations | (3) soundness, (2) correctness | **ADOPT** | Already correct. Document the head-only/deep split in `unify.rs`. |
| L18 | `kernel_swaps_first_two` reverses Maybe/Result args — **contradicts** upstream `SigRegistry` (fn-first everywhere) | Kernel wiring | (2) correctness | **REJECT** | Audit `kernel_swaps_first_two` call sites; run a fixture using `Maybe.map`/`Result.map` on Go oracle + Rust. If sweep passes it's luck — remove the swap to match SigRegistry fn-first order. **#70** |
| L19 | Closures pass as `impl Fn + Clone` by value; free vars captured by Rust ownership, no closure struct | Kernel wiring | (3) soundness | **ADOPT** | Confirm `emit_lambda` emits `move |params| { body }`; document the closure ABI in `naming.rs`. |
| L20 | No `SigRegistry` equivalent in Ipê — a MAJOR completeness gap blocking closure-type inference + turbofish pinning | Kernel wiring | (5) COMPLETENESS | **ADAPT** | Port `SigRegistry.hs` into Rust as `KernelFn → ([params], ret)`; start List/Maybe/Result. Unblocks empty-arg inference + generic pinning. **#70, #45** |
| L21 | `kernelSigPrefix` normalizes short module names (`List`→`Sky_Core_List`) only for registry lookup, keeping `(module,name)` intact | Kernel wiring | (2) correctness | **ADOPT** | Mirror the two-step: split on first `_`, normalize with `Sky_Core_` prefix before registry lookup. Document alongside the SigRegistry port. |
| L22 | Arc-stored sibling closures pre-clone each captured var (`let x = x.clone();`) to avoid E0382 | Kernel wiring | (3) soundness | **ADOPT** | Implement pre-clone in `emit_expr.rs` when a capturing lambda is stored into an Arc field (Ui handlers, middleware). **#43** |
| L23 | Single-module type-parameter anchoring keeps `Element msg`/`Attribute msg` from canonicaliser type-stripping | Std.Ui/Html (#76) | (3) soundness | **ADOPT** | Keep Ui `Element`/`Attribute` in one module; add wrapper fns in the parent so sub-modules call fns not ctors. Test: cross-module ctor import fails at compile time. **#76** |
| L24 | Phased kernel registration; ~160 members intentionally unbacked (`id=None`) as pure-Sky | Std.Ui/Html (#76) | (5) COMPLETENESS, (2) correctness | **ADAPT** | Build a Pure-vs-Kernel registry-status tracker; audit all 160, implement kernels for the high-value 30-40 blocking examples, mark the rest explicitly. **#76** |
| L25 | 8-category kernel taxonomy makes incompleteness visible | Std.Ui/Html (#76) | (5) COMPLETENESS, (4) efficiency | **ADOPT** | Reorganize `sky_kernels/src/lib.rs` into 8 semantic groups with per-group expected counts + count-assert tests. Turns #76 into a structured audit. **#76** |
| L26 | Dual-path attribute compilation: pure-Sky construction, kernel-backed CSS emission | Std.Ui/Html (#76) | (3) soundness, (4) efficiency, (2) correctness | **ADOPT** | Route `Attribute → kernel-call` for CSS emission during the Element tree walk; don't inline-string from ADT destructuring in Sky. **#76** |
| L27 | Unbacked members should be a **type-level** fact: registry status `Kernel \| PureSky \| Todo`, missing = compile error (no `id=None` silent fallback) | Std.Ui/Html (#76) | (3) soundness (**MAKE INVALID STATES UNREPRESENTABLE**) | **ADOPT** | Add mandatory `status` field to the Ui/Html kernel registry; referenced-but-unregistered member = hard canon error. **#76** |
| L28 | Forward decls (dep union ctors) resolved at import-injection time with type-vars quantified in the ctor annotation | Canon / DepInfo | (1) security, (2) correctness | **ADOPT** | Verify `build_module_exports` copies `ctor.annot` from the source union (quantified), not re-synthesized. Audit every `dep.ctors` annotation. |
| L29 | `import M exposing (..)` injects ALL ctors of a type, not a curated subset | Canon / DepInfo | (4) efficiency, (5) completeness, (6) readability | **ADOPT** | In `inject_dep_exports` `Exposing::All`, ensure `inject_ctors_for_type` iterates every ctor in `dep.types[t]`. Sanity test: injected count == declared count. |
| L30 | No implicit `ExportEverything` default — every module MUST declare `exposing (...)`; stdlib are kernel qualifiers, not source modules | Canon / DepInfo | (1) security, (5) completeness | **ADOPT** | Do NOT add an implicit default. Linter: reject modules without `exposing`. Every ported stdlib `.ipe` gets an explicit clause matching upstream. **#78** |
| L31 | Two-phase export verification (build_module_exports filter → inject-time check) is sound and **more testable** than upstream's one-shot | Canon / DepInfo | (2) correctness, (3) soundness, (5) completeness | **ADOPT** | Ensure `build_module_exports` runs on every `.ipe` before downstream import. Per-module test verifying ctors/values/types match intended public API; no internal leak. **#78** |

**ADOPT count: 22.** (ADAPT: 7 — L3, L4, L9, L10, L11, L20, L24. REJECT: 2 — L14 deferred, L18.)

## 3. Prioritized action list (mapped to backlog)

Ordered by blast radius on the example sweep. **#78 first — it is the dominant
sweep blocker** (canon can't even resolve stdlib imports/aliases until it
lands).

### P0 — #78 Import-alias + stdlib canon registration (dominant sweep blocker)
- **L1** Add `import_aliases` map to `Env` (`env.rs`); populate in `inject_dep_exports`.
- **L2** Registry-vs-source enforcement + `canon_stdlib_registry_parity_check` tripwire.
- **L30/L31** Enforce explicit `exposing` on every ported stdlib `.ipe`; run `build_module_exports` before downstream import; per-module public-API test.
- **L4** Add `filter_dep_by_exports` (warn-only → error) — the export firewall.
- **L28/L29** Forward-decl ctor quantification + `exposing (..)` injects all ctors.
- **L3** (follow-on) qualifier resolution via `import_aliases` incl. dotted `Db.Decode`.

### P1 — #77 Error module + errorToString + Stringify obligation
- **L5** Register `Error`/`ErrorKind`/`ErrorInfo` in canon with correct arity + layout.
- **L6** Add `BasicsErrorToString` kernel + `lower.rs` wiring.
- **L7 / L15** Add `Stringify` (`SHOW`) obligation to `constrain.rs`; gate `errorToString`, `toString`, `Debug.toString`, `Log.*With`. Regression `IPE-T0014`.
- **L8** Total `SkyStringify for Error` via `Wrap`-dispatch; all 5 `ErrorDetails` variants covered.
- **L9/L10** Keep `details` in the struct; land #77+#78 before any Error-touching sweep.

### P2 — #76 Std.Ui / Std.Html member breadth (160 unbacked canonicals)
- **L23** Single-module `Element`/`Attribute` anchoring + wrapper fns; compile-time ctor-leak test.
- **L27** Registry `status` field; referenced-but-unregistered = hard error (**invalid states unrepresentable**).
- **L25** 8-category taxonomy + per-group count-assert tests.
- **L24** Pure-vs-Kernel audit of all 160; implement the 30-40 example-blocking kernels.
- **L26** Dual-path: route `Attribute → kernel-call` for CSS emission.

### P3 — Obligations / bound-system hardening (#45, #70)
- **L20** Port `SigRegistry` (List/Maybe/Result) — unblocks turbofish + empty-arg inference.
- **L12/L16/L17** Verify/document post-solve deferral, rigid-Super carry, two-level check.
- **L11** Document non-merging bitset; regression.
- **L14** Reserve Appendable/CompAppend bit + M1 stub (deferred).

### P4 — Backend / runtime wiring (kernel HOF shapes, #43, #70)
- **L18** Audit + likely remove `kernel_swaps_first_two` (correctness bug latent behind fn-first SigRegistry).
- **L22** Arc-stored closure pre-clone (fixes E0382 in Ui/middleware chains).
- **L19/L21** Confirm/document closure ABI + `kernelSigPrefix` two-step name resolution.

## 4. Rejected & why (do not relitigate)

- **`SuperType` constraint-merging algebra (`Number ∪ Comparable → CompAppend`)** —
  *L11, kept as ADAPT-to-bitset.* Our independent-bit `TyBounds` is **sounder and
  simpler** for M0/module scope; merging adds an algebra we don't need and a
  class of merge-order bugs we avoid entirely. Each obligation is checked
  independently. Reject the merge; keep the bitset.
- **Untyped `errorToString`/`toString` coercion (upstream/Go behavior)** —
  superseded by our typed `SHOW` obligation (L7/L15). Upstream lets a function
  value reach `toString` and only fails (or misrenders) at runtime; Ipê rejects
  it at type-check. This is a **security + soundness win we keep**, not a gap to
  close by copying upstream.
- **`kernel_swaps_first_two` (Maybe/Result arg reversal)** — *L18.* Contradicts
  upstream `SigRegistry`, which is **fn-first for every HOF**. It is a latent
  correctness bug that only survives because tests exercise those kernels
  rarely. Remove it to match SigRegistry; do not treat it as a deliberate
  adaptation.
- **Appendable / CompAppend obligations now** — *L14.* Correctly out of scope in
  M0 (no `List` type yet). Not a defect — a **deferred completeness item** with a
  reserved bit + TODO + M1 regression stub. Revisit when #36/List lands, not
  before.
- **`ErrorDetails` inspection during stringification** — *L9.* Intentionally
  unused by `toString` upstream (renders kind+message only). We preserve the
  field structurally for a *separate* dump/log renderer, but must NOT wire it
  into `errorToString` — doing so would diverge from the Go oracle.
- **Re-deriving qualifier from `dep_path.last()` at later stages** — *L3
  rationale.* Efficient but lossy for dotted qualifiers; replaced by the
  `import_aliases` cache. Don't reintroduce last-segment re-derivation once L1
  lands.
