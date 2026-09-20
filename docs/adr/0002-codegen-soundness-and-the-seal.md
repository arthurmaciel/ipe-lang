Status: Living (consolidated)
Date: 2026-09-18
Archive: misc/docs/archive/ADR/

# 0002. Codegen soundness & THE SEAL

THE SEAL is the pipeline's load-bearing invariant: if `ipe` accepts a program
(exit 0), the emitted Rust MUST `cargo build`. This ADR records how that
guarantee is made structural — a totality obligation the type system enforces at
compile-accept time — rather than a set of symptom-keyed patches, together with
the emitter's move/clone/borrow discipline and the single-source kernel registry
those guarantees rest on.

## Decisions

### THE SEAL is structural totality, not per-case patching
Each SEAL-violation class is closed at its source with a totality obligation the
type system enforces, so a new acceptance path fails closed at compile-accept
time instead of open at cargo time. The universe a traversal ranges over is a
closed typed enum; the traversal itself is a single total recursion with no
wildcard arm, so a new variant is a compile error at the walk, not a
silently-missed descent. The rejected strategy is discover-then-patch keyed to
sweep failures: the trigger set is unbounded and the detector is incidental, so
coverage is only whatever the sweep happens to exercise. The bias is always
toward selecting/lowering: a too-conservative misclassification is merely
wasteful, while a too-permissive one is a SEAL breach.

The concrete instance is runtime-feature selection. `RuntimeFeatureId` is a
closed enum whose variants are exactly the runtime crate's declared features, and
`ir_type_feature_requirement` (`src/compiler/ir/src/ir.rs`) maps each `IrType`
leaf to its required feature with no catch-all arm — so a new carrier type forces
the author to extend the walk before the compiler will build. The same shape
governs lowering and pattern machinery: an un-lowered construct or an uncovered
pattern is a compile error in the compiler, never an emit of Rust that cannot
type-check.

### Move/clone/borrow discipline: exactly-one-bare-occurrence
Every non-`Copy` value stays `Clone` (the derive-seal), so a clone is always
available as the escape hatch. For each local binding of non-`Copy` type, among
its owned-consume reads in evaluation order the **last** is emitted bare (a move)
and every earlier one is emitted `x.clone()`; borrow-position reads (comparison /
`++` / interpolation operands) never consume and impose no clone obligation. At
most one occurrence is emitted in value-moving form and nothing runs after it, so
the value is consumed at most once with no later use — provably total, and
minimal (a once-read binding is byte-identical to a bare move). Runtime value
kernels take arguments by value, so this analysis, not a borrow rewrite, is what
keeps `String.startsWith "#" s` from moving `s` out from under a later read.

Across mutually-exclusive `match`/`if` arms the clone-emitting counter is
snapshot/restored per arm, seeded from that arm's own use count — never seeded
with a shared `MAX(arm uses)`. A shared spent counter is order-dependent and
unsound: spending in one arm robs another, reopening the double-move. Set
membership ("is this var multi-use anywhere?") may over-approximate; the
sequential counter that is *spent* during the rewrite may not. The rewrite lives
in `rewrite_multiuse_clones` (`src/compiler/lower`).

### Emitter borrow discipline at the edges
Three emitter sites sit downstream of the move seal and decide where a clone is
required versus forbidden:

- **Refutable as-pattern alias.** A by-move arm binding both the whole and its
  parts (`Just ((a,b) as w)`) would make `w` and `a`/`b` fight over the tuple
  (E0382). `is_dispatch_free` (`src/compiler/ir`) is true for
  `Var`/`Wildcard`/`Tuple`/`Record` nesting and false for `Ctor`/literal/`Slice`;
  a dispatch-free inner is rewritten to bind-then-destructure-from-a-clone, and a
  dispatch-needing inner is rejected upstream with `IPE-L0127` (the backend-side
  check is defensive, not the primary gate).
- **Decoder-thunk destructures.** Decoders are `!Clone`, so a destructure binding
  Decoder-typed components mints one zero-arg thunk over the whole value and
  re-projects a fresh call per read; the gate fires whenever the aggregate type
  is-or-contains `IrType::Decoder`.
- **Field-access copy elision.** `Expr::Access` carries a solved `field_ty`;
  provably-`Copy` field types emit a bare access, everything else `.clone()`. A
  `Generic(_)` or unsolved field type conservatively clones — never a regression
  versus unconditional cloning, and it avoids an O(n²) deep copy of `String`/`Vec`
  fields in render loops.

### The kernel registry is one source of truth
A kernel's identity is resolved from its `(qualifier, name)` string pair to a
typed `KernelId` **once at canonicalisation** (parse, don't validate); every
downstream stage holds the typed id and never re-matches a string.
`KernelId = Stdlib(StdlibKernel) | Ffi(FfiKernelId)` is a two-tier sum — a closed
`StdlibKernel` enum whose exhaustive `match` and `ALL` slice make an un-schemed
kernel a compile error, and an opaque data-driven FFI index
(`src/compiler/kernels/src/lib.rs`). The `Ty::Var(u32::MAX)` fail-open fallback is
deleted: a canon-listed-but-unschemed kernel is a non-exhaustive-match compile
error or an explicit `IPE-L0108`, never a silent flexible variable deferring
failure to cargo.

Every fact the compiler needs about a kernel — qualifier, name, arity, class,
runtime symbol, capability, runtime module, type scheme — is assembled in one
`KernelDef` descriptor built by `def()`, and every consumer derives from it: the
backend's kernel name and the IR pretty-printer's display name each delegate to
`def().runtime_fn`, with a test pinning the delegation so no second name table
can reappear. No consumer may reintroduce a parallel table of a fact `KernelDef`
already owns. `Match` construction is likewise sealed: the raw
`from_parts_unchecked` escape hatch is replaced by `map_bodies`/`try_map_bodies`,
which transform scrutinee and arm bodies while closing over the original
patterns, so a body-only rewrite can never lose or reorder arms.

Type schemes are only partly folded into `KernelDef`: `shape: Option<&TyShape>`
carries a kernel's scheme when it is structurally expressible, and the remaining
polymorphic schemes still resolve through the `stdlib_scheme` table keyed by
`SchemeKey`. Completing that migration is the outstanding follow-on this
structure enables but does not finish.

### Kernel schemes enter only on three-source arity agreement
A kernel joins the schemed set (`FIRST_SCHEMED`) only when its arity triple-agrees
across the registry `def().arity`, the lowerer `callee_arity`, and the runtime
function's parameter count. `arrow-count == decl().arity == callee_arity` is the
fail-closed tripwire: a runtime signature change not mirrored in the registry, or
a dropped scheme arm, surfaces as a drift-test failure, never as a silent
`Ty::Var(MAX)` hole. Enrollment is atomic — three-source agreement is the proof
the batch is trustworthy by construction. This is why every list operation the
prelude exposes is wired as a kernel with a fail-closed scheme (a `List.x` name
resolves to `VarHome::Kernel` unconditionally and never reaches compiled
`Ipe.List` source, so a kernel is the only wiring that makes the name callable
and schemed); a missing `stdlib_scheme` arm returns `None` → an IPE error, never
exit-0-then-cargo-fail.

### Fail-closed admissibility gates for function payloads, msg, and any-return
Where a well-typed program could otherwise place a value the emitted Rust cannot
support, the gate lives at ipe time and fails closed:

- **Function values in constructor / `Maybe` / `Result` payloads.** Construction
  is admissible — runtime enums carry bounded generic derives, concrete user
  enums drop auto-derives via the derive-demotion fixpoint, and the upstream
  `==`/`toString`/serde/Model obligations are already guarded before lowering. So
  the blanket construction ban is lifted; two narrow gates remain: a reuse gate
  (`IPE-L0127`, since `Box<dyn Fn>` is not `Clone`) and an `andMap` call-site
  arity gate. `Box<dyn Fn>` with captures is a sanctioned divergence from a bare
  `fn`-pointer approach — strictly more general and required by Ipê semantics.
  The construction lift stays sound only while those three upstream guards hold.
- **Model and Msg admissibility.** Msg admissibility is `ir_type_is_derivable`
  (Web, Tui, Webview); Model admissibility additionally requires serde, because
  the Web Model is persisted to the session store while Msg is transient. This
  asymmetry is load-bearing — Html is admissible in a Web Msg but not a Web Model
  — and collapsing it re-introduces false rejects or reopens the seal. One
  lambda-aware extractor `fn_param_ty(e, idx)` recovers the concrete parameter
  `IrType` whether a `view`/`update` field is a `FuncValue` or a `Lambda`,
  closing the lambda-view bypass; Msg is recovered from `update`'s first
  parameter, where it appears directly rather than nested in `Html<Msg>`.
- **Wildcard `any` return.** A `view : Model -> any` whose body solves to
  `Html<Ty::Var>` must reach the use sites as the body's real type, not a
  return-position-only generic (which is a call-site-dependent E0282 SEAL
  breach). The constraint layer records each wildcard-`any`-return binding and
  ties every use's instantiated arrow result to the binding's solved body
  (`tie_wildcard_any_uses_to_bodies`, `src/compiler/types/src/constrain`); a
  genuinely-free residual then fails closed with `Feature::Polymorphism`
  (`IPE-L0102`), actionable rather than a silent generic.

  > **NEEDS UPDATE AFTER IMPLEMENTATION** — the archived 0025 described a
  > lower-level `any_ui_msg_injection` in `lower_def` gated on the resolved Con
  > name (`Html`/`Element`/`Attribute`); the current code carries the `view`-return
  > case at the constraint layer instead (`wildcard_any_return_bindings` +
  > `tie_wildcard_any_uses_to_bodies` in `constrain_ast.rs`), with `IPE-L0102` as
  > the fail-closed backstop; untracked. Once the Con-name-gated injection's
  > removal is confirmed complete, drop the `any_ui_msg_injection` framing from
  > this section entirely and describe only the constraint-tie mechanism.

### Pattern & lowering completeness
Recognition and region-threading that worked for one shape are applied uniformly
to its siblings, so no malformed or unhandled shape slips through:

- **Interpolation literals.** The `{{…}}` resolver recognises string/bool/char
  literals (alongside numerics) *before* the ambiguous `.`-split, so a literal is
  never interned as a variable name and left an unresolved `VarLocal` past
  canonicalisation — the invariant that canon leaves no unresolved locals.
- **Nested sub-patterns.** The constrainer inserts regions for every constructor
  sub-pattern (mirroring lambda params); nested records reuse the record-pattern
  lowerer, and a nested list pattern — which Rust cannot slice-pattern inline on a
  `Vec` field — lowers to a `Pat::Var` binder plus an arm-level length/shape
  guard, with elements recovered by an IR-level `Let` prelude (never rendered
  text). Such a pattern is refutable, so a non-matching length falls through the
  guard rather than panicking.
- **Local type shadowing a dep-imported type.** A pre-pass local-vs-dep check
  emits the same `IPE-N0012` as the dep-vs-dep clash, run before `type_home_map`
  is mutated, so a ctor can never point at a local type while the home map points
  at the dep. The check order (dep pre-pass → local-vs-dep → per-module loops) is
  strict; two modules independently declaring the same name with no import
  between them stay legal.
