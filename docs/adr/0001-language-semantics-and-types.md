Status: Living (consolidated)
Date: 2026-09-18
Archive: misc/docs/archive/ADR/

# 0001. Language semantics & types

How Ipê's surface names, records, patterns, effects, and cross-module
inference are defined and enforced — the type-level rules that decide what a
well-formed program means and which programs are turned away before lowering.
The unifying discipline is fail-closed structure: every acceptance path is
made sound by construction, so an ill-typed shape has no representation rather
than a deferred blowup.

## Decisions

### Record-alias auto-constructor is a synthesized typed function
A record type alias — `type alias T p₀..pₖ = { f₀ : A₀, …, fₙ : Aₙ }` —
introduces an in-scope value `T : A₀ -> … -> Aₙ -> { f₀:A₀, …, fₙ:Aₙ }`,
positional in *declared* field order. It is synthesized once, at the canon site
where the alias's source-order fields are known, as an ordinary typed top-level
`Def` registered in the value namespace. From that point every stage — HM,
lowering, backend — sees a plain typed function: no new IR node, no walker arm,
no special case. A stage that "forgot record-ctor handling" is therefore
unrepresentable. Declared order is captured from the one source `TRecord` vec
and projected together into the def's parameter patterns, its body record
literal, and its arrow argument types, so argument `i` provably binds field
`fᵢ`; a constructor whose argument order disagrees with its field order is not
constructible. A non-record alias gets no value binding (using it as a value
stays an ordinary name error); a head alias to another record alias gets no
ctor (the gate is strict on a *literal* `TRecord` body); a name collision with a
user value is a hard `DuplicateValue` diagnostic; a function-headed field alias
gets no ctor (its body would be a rejected first-class-function record literal).

### Untyped top-level bindings generalize only at the module boundary
An untyped polymorphic top-level binding stays **monomorphic within its home
module** — one shared inference variable for all same-module references, so
reusing such a helper at two different types in its own module is rejected — and
generalizes only when the module finishes. At boundary completion, a binding's
residual plain-`Flex` variables — excluding `Super`-bounded, rigid-contaminated,
and obligation-carrying roots — are quantified into a scheme; cross-module
references instantiate that scheme fresh per use site, exactly as annotated
bindings do. Ipê is pure, so no value restriction applies. The permitted drift
direction is always toward *under*-acceptance: any membership test deciding
"is this var quantifiable?" may only err conservative, never accept a program
past what monomorphic-within-module semantics allow.

### Every record reaching the backend has a fully-pinned concrete field set
The backend resolves records by exact sorted-field-set match and fails loud on a
miss — sound only because every record reaching it has a concrete field set.
Row-polymorphic functions either resolve to a concrete shape before lowering or
are rejected at type-check. Four mechanisms hold the invariant: open-record
unification; deferred field access (subset access legal by construction);
open-record kernel schemes mirrored from stdlib; and monomorphic env pinning
(an unannotated binding pins on its first concrete use). This couples to
boundary generalization above: promotion must keep record-row tails monomorphic
at the boundary, or add per-record-shape callee monomorphisation to the backend
first. The rejection fixtures (two-superset, closed-superset) must keep
*rejecting* — flipping them to accept without that machinery reintroduces the
panic-on-unknown-shape. Row-var annotation syntax `{ r | f : T }` does not parse
(a recorded completeness gap, not a runtime hazard).

### The andMap function payload's arity-1 restriction is a type obligation
`Maybe.andMap` / `Result.andMap` require their function payload to be arity-1
(`a -> b`), never a curried arity-≥2 arrow that would fail at runtime. The check
is a property of the payload's **solved type**, not the syntax at any reference
point: it is attached as a structural obligation to the payload-result slot at
constrain time (the same `Content::Super` bound mechanism that carries `Set` /
`Dict` / `Cache` key obligations), checked post-solve against a fully concrete
type, and survives arbitrary aliasing including cross-module generalization
(obligation-carrying roots are excluded from boundary quantification). A lowering
backstop in the single callee funnel re-checks it, so any obligation miss is a
build-time failure, never a silent acceptance. Syntactic call-site matching was
tried and reverted: a curried `andMap` reference passes through `let`-bindings,
point-free aliases, higher-order arguments, and record fields — an open-ended
enumeration a solved-type obligation closes at the source. The precision
trade-off is conservative: an unannotated cross-module wrapper reused at two
different but individually-safe arity-1 payload types is rejected; annotating the
wrapper routes it through the re-propagated path.

### Error-constructor payloads have nominal type identity
The `FfiPanic` / `TypeMismatch` error constructors and the `Error` payload carry
nominal `Ty::Con` identities (`PanicInfo`, `TypeInfo`, `ErrorInfo`) matching the
runtime's nominal structs — not anonymous structural records. One Ipê type, one
Rust lowering: a raw record-literal construction is a clean `ipe`-time mismatch
rather than an exit-0-then-cargo-fail, and an unannotated helper over a
pattern-bound payload lowers its parameter to the nominal type, agreeing with the
call site. Construction goes through the smart constructors (`Error.io`, …,
`Error.withDetails`); field access (`p.message`, `p.stack`) works via fixed field
tables; record *update* (`{ p | … }`) is rejected with the dedicated IPE-T0017
("built-in type — fields readable but cannot be rebuilt with record-update
syntax"). Backend coercion was rejected — it fixes construction but not the
escape direction where a nominal payload flows into a structurally-typed position.

### Or-pattern alternatives bind the same names at the same types
Every alternative of an or-pattern (`p₁ | p₂ | …`) must bind the identical set of
variable names, and each shared name must have one type across all alternatives.
Because only one alternative matches at run time and which one is unknown when the
body is type-checked, the body must see a consistent binding environment
regardless — so `Just x | Nothing` (differing name sets) and `Error s | Success n`
(differing names) are rejected, while nullary variants, wildcard payloads, a
shared name at a shared type (`Circle r | Square r`), and literals are accepted.
Enforced in two stages: name-set equality proven fail-fast in canon, and per-name
type equality checked after the solve. Exhaustiveness is orthogonal — the
usefulness algorithm expands an or-pattern into the row-union of its alternatives.
Permitting differing bindings (a name sometimes unbound) and unioning differing
payloads into an ad-hoc sum were both rejected as unsound or intent-obscuring.

### `Ipe.Basics` and the three-tier auto-import model
The dividing line for what is ambient is an axis, not a list: **core-language
*types* are vocabulary (implicit); library types and all *functions* are imports
(explicit).**
- **Tier A — `Ipe.Basics`.** Auto-imported unqualified into every module, scoped
  to exactly Elm's `Basics`: the arithmetic/comparison/function operators, `Bool`
  with its operators, the base types `Int`/`Float`/`Char`/`String`,
  `identity`/`always`/`++`, `Order` with `LT`/`EQ`/`GT`, `Never`/`never`, and the
  numeric/math functions (`min`/`max`/`abs`/`clamp`/`negate`/`compare`). Nothing
  library-flavoured lives here.
- **Tier B — core type vocabulary.** In scope with no import: the type names
  `List`, `Maybe`, `Result` and their constructors, plus `True`/`False` and
  `LT`/`EQ`/`GT`. Local definitions shadow these names normally.
- **Tier C — everything else.** Every other module function requires an explicit
  `import` and is used qualified (`List.map`, `String.toUpper`, `Dict`, `Http`); a
  Tier-C name used without its import fails to resolve (`StdlibImportRequired`,
  IPE-N0034). This is stricter than Elm deliberately, so the import list is a
  complete inventory of a file's capabilities; an LSP "add import" code action is
  the ergonomic counterpart. Nothing library-flavoured may ever migrate into
  Tier A or B, and adding a Tier-B type is a deliberate change to a small fixed
  list.

### `do`-notation is Task-sequencing sugar
`do` is a keyword-introduced, layout-delimited, **`Task`-only** block that
desugars in the parser to existing nodes — no new IR, no runtime, no Monad
typeclass. Three line forms distinguished by operator alone: `p <- e` runs the
`Task` `e` and binds its result (`e |> Task.andThen (\p -> rest)`); `p = e` is a
pure bind (`let p = e in rest`); a bare line runs a `Task` for effect and
discards it (lowered as `let _ = e in rest`, keeping a long block a shallow chain
rather than a deep lambda nest). A required trailing expression is the block's
result. Synthetic nodes are stamped with the block's own span so diagnostics
never point at code the user did not write. The sugar is confined to `Task`
because `Task` is the one type with no visible constructors — the only type where
run-and-bind adds a genuinely new capability rather than saving nesting; `do` over
a non-`Task` is a compile error, and extending it to `Result`/`Maybe` (which
already have `andThen`) was rejected.

A **stepless `do`** — a block whose every statement is a `=` pure-let bind, with
no `<-` bind and no bare-run line anywhere — is a compile error (`SteplessDo`,
IPE-P0065), directing the author to `let … in` for pure bindings. The check is
purely structural (the parser cannot see types), run in the `desugar_do` fold
before any other stage. This makes `do` and `let … in` **disjoint by
construction**: `let … in` is the sole form for pure sequential binding, `do` is
Task sequencing with at least one real step, and an author cannot pick the wrong
one and have it silently accepted. Concurrent fan-out has no keyword form — it is
the plain function call `results <- Task.parallel [a, b, c]` inside a `do`, which
is already unambiguous, discoverable, and consistent with every other `Task`
combinator; `doParallel` is not a reserved word and lexes as a plain identifier.

### Per-module typecheck behind closed typed interfaces
The per-module types the language server reads for hover, completion, and
expected-type are served from a genuinely-per-module solve, gated fail-closed by
**closed typed interfaces**. A module's `TypedInterface` is its typed export
surface — each exported binding's generalized scheme plus its union definitions —
with span-free schemes and canonicalized variable ids, so a body-only edit that
preserves the exported schemes yields a byte-equal interface. `infer_module`
solves one module's constraints, instantiating every cross-module reference
against the dep's interface scheme fresh per use site, sharing one inference core
with the whole-program solve so the two paths cannot drift. The scoped result is
served only when the module's own solve is green, every exported scheme is
**closed** (no reachable residual non-quantified variable, checked *before*
defaulting), and every dependency interface is closed — precisely the condition
under which the joint solve decomposes; every other case falls back to the
whole-program projection verbatim. Closedness is necessary because Ipê inference
carries information *against* the import direction (boundary-promoted schemes,
deferred obligations, and program-wide defaulting can be pinned by an importer),
which no dependency-first solve can otherwise reproduce. The build path is
untouched — emitted bytes cannot change — and wherever the scoped path engages
its result must equal the normalized whole-program projection. Bounded-scheme
generalization of obligation-carrying exports was rejected: it would redefine the
language (the joint solve gives every importer the one pinned type), and a scoped
tier must not redefine the language.
