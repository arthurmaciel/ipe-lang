# Principled-Decisions Audit — elm/compiler + Sky-Haskell vs ipê

Merged ledger of four principled-decision research sweeps across the ipê Rust
compiler: the elm/compiler frontend, the Sky-Haskell frontend, the Sky-Haskell
backend, and the diagnostics/DX surface. Each celebrated decision from the
reference compilers was tested against the adoption gate and recorded here with
a verdict — even the ones we reject, so the "do not change this" rationale is
durable.

## The adoption gate

`PRINCIPLES.md` ordering is a strict tie-breaker:

1. Security
2. Correctness
3. Soundness
4. Efficiency
5. Completeness
6. Readability

plus the two fundamental rules: **parse-don't-validate** and
**make-invalid-states-unrepresentable**.

A candidate is adopted **only if it strictly improves adherence to one
principle/rule without harming a higher one** — the same test as
sanctioned-divergence. Many famous decisions are already implemented in ipê, or
ipê's current approach is strictly better; those are recorded as REJECT with the
principle rationale, not silently dropped.

Counts: **2 adopt · 9 adapt · 12 reject** (24 raw candidates, 1 cross-area
duplicate merged → 23 distinct).

---

## 1. Headline table (adopt-high first)

| # | Candidate | Source | Principle improved | ipê current state | Verdict | Value |
|---|-----------|--------|--------------------|-------------------|---------|-------|
| 1 | Port TailCallOpt — rewrite user tail-recursive fns to `loop{…; continue}` | Sky-Haskell | P3 Soundness + P4 Efficiency | Worse/absent — no loop/continue emission; user tail-recursion → stack-overflow abort | **adopt** | high |
| 2 | JSON / machine-readable diagnostic report mode (`--report=json`) | elm/compiler | P5 Completeness (tooling) | Fully structured Diagnostic, but only a human renderer | **adopt** | medium |
| 3 | Populate field/arg/tuple **path breadcrumb** on type-mismatch errors | both | P6 Readability + diagnosis correctness | Worse-but-pre-wired — `path` field + renderer exist; `unify.rs` hard-codes empty | **adapt** | medium |
| 4 | Defer **ambiguous-import** rejection to the use-site | Sky-Haskell | P5 Completeness (stops rejecting valid programs) | Worse (over-strict) — rejects at import registration | **adapt** | medium |
| 5 | Reject **tabs** in layout-significant whitespace | elm/compiler | make-invalid-states + P2 Correctness | Worse (silent guess) — tab counted as 1 column | **adapt** | medium |
| 6 | Report **multiple independent errors** per phase (isolated, no cascade) | elm/compiler | P5 Completeness + P6 Readability | Worse — single Diagnostic, fail-fast at first error | **adapt** | medium |
| 7 | Render **multi-line source spans** (per-line gutter + markers, capped) | elm/compiler | P6 Readability | Worse — span truncated to first line | **adapt** | medium |
| 8 | FFI **type-boundary rejection** (`isSimpleTypedType`, reject Go-residue) | Sky-Haskell | parse-don't-validate + P1 Security | N/A yet — FFI consumer not ported (design-only) | **adapt** | medium |
| 9 | Enclosing-**construct START** as secondary caret in parse errors | elm/compiler | P6 Readability | Construct tracked, SecondarySpan renderer exists; threading unconfirmed | **adapt** | low |
| 10 | **Structural (size-scaled)** solver-budget mode | Sky-Haskell | P5 Completeness + P4 guard-efficiency | Worse-but-adequate — fixed 5M / absolute only | **adapt** | low |
| 11 | Whole-program **DCE** with typed `Ref` ADT before emission | Sky-Haskell | P4 Efficiency (build speed) only | Absent — emit all defs, rely on rustc/LLVM | **adapt** | low |
| 12 | Canonical name = `(home module, name)` + interned symbols | elm/compiler | P2 Correctness + make-invalid-states | **Better** — already keyed `(home, name)`; forged-symbol guard | reject | low |
| 13 | Friendly errors-as-data: typed Report ADT, Doc/Render split | elm/compiler | P6 Readability + parse-don't-validate | **At-or-beyond** — typed Diagnostic + `explain` + `fix` | reject | low |
| 14 | Exhaustiveness with concrete missing-pattern witnesses (Maranget) | elm/compiler | P2 + P6 | **Better** — witnesses + no-wildcard walker discipline | reject | low |
| 15 | Hand-written parser with contextual errors + precise spans | elm/compiler | P6 + P2 + P4 | **Already adopted** — recursive-descent + Construct + defect enums | reject | low |
| 16 | Elm-style Category/Expected constraint provenance strings | elm/compiler | P6 Readability only | Already blames the correct per-subexpr span; only text context missing | reject | low |
| 17 | Adopt Haskell **shallow head-only** exhaustiveness checker | Sky-Haskell | — (would regress P3/P2) | **Better** — full Maranget usefulness/matrix on nested patterns | reject | low |
| 18 | Explicit immutable `LowerCtx` reader vs global IORef state | Sky-Haskell | P3/P4 | **Better** — `Lowerer` struct, zero global mut state (structural) | reject | low |
| 19 | Reserved-ident mangling: raw-identifier `r#self` | Sky-Haskell | P2 Correctness | **Better** — trailing-underscore (r#self/r#Self are invalid Rust) | reject | low |
| 20 | Integer `//`-by-zero → return 0 (Elm semantics) | Sky-Haskell | would help P3, harms P2 Go-parity | Closed — classify-and-abort matches Go oracle `rt.IntDiv` | reject | low |
| 21 | IR-level exhaustiveness smart constructor (`Match::new`) | both | make-invalid-states + P3 | **Better** — fallible IR constructor; Haskell lacks the backstop | reject | low |
| 22 | Kernel registry as single source of truth for docs/LSP/explain | Sky-Haskell | P2 + P5 + parse-don't-validate | **Better by design** — reject Sky's regex-scrape of compiler source | reject | medium |
| 23 | Structural type-diff: elide agreeing structure, highlight divergence | elm/compiler | P6 Readability | Partial/adequate — `path` localizer already covers most value | reject | low |

---

## 2. REJECT — what NOT to change, and why

Recording these is as load-bearing as recording the adopts: it documents that a
famous decision was evaluated and that ipê already meets or beats it, and it
installs guard rails against a future "simplification" that would regress toward
the reference shape.

### Already implemented, at-or-beyond the reference

- **Canonical name = `(home, name)` + interned symbols** (#12). Elm's single
  most load-bearing data-structure decision. ipê already keys constrain
  top-level/untyped tables by `(Vec<Symbol> home, Symbol name)`; references are
  `VarTopLevel { module, name }`; `Type::Con` carries a `home` field; the
  multi-module overwrite bug (`Lib.helper` vs `Main.helper`) is closed. The
  interner's `from_raw` resolves a forged/cross-interner symbol to `None`
  instead of a silent empty string — a soundness bonus Elm's plain interner
  lacks. **Guard rail:** any new binding-reference node MUST carry its home
  module and any new lookup table MUST key by `(home, name)`; a bare-`Symbol`
  table would silently reintroduce cross-module aliasing.

- **Friendly errors-as-data** (#13). ipê errors are typed `Diagnostic` enums
  (the only free-form `String` is `CompilerBug.detail`), each mapped to a
  forge-proof stable code, rendered in a deterministic panic-free 4-band layout.
  Beyond Elm, ipê ships `ipe explain <CODE>` pages and machine-applicable
  `ipe fix` — neither exists in Elm.

- **Exhaustiveness with witnesses** (#14). `NonExhaustiveCase { missing }`
  (IPE-T0010) lists uncovered constructors, and the no-`_ ->`-catchall walker
  discipline makes a newly-added variant a compile error rather than a silently
  swallowed case — stricter than Elm's wildcard-tolerant matches.

- **Hand-written contextual parser** (#15). `sky_parse` is recursive-descent
  carrying the enclosing `Construct`, an `ExpectedSet`, and structured
  `HeaderDefect`/`ExposingDefect`/`CaseDefect`/`LetDefect`/`IfDefect` payloads
  with precise spans. The only incremental gain is the secondary-caret
  refinement, surfaced separately as adapt #9.

- **Full Maranget exhaustiveness** (#17). `exhaust.rs` ports the complete
  usefulness/matrix algorithm and analyses nested patterns
  (`Just (Just a)` missing `Just Nothing`), list/cons signatures, and tuples,
  returning precise witnesses. The Haskell frontend does only a shallow
  top-level head check and requires wildcards for literals — adopting it would
  regress P3/P2 and let a nested non-exhaustive case reach an
  exit-0-then-cargo-fail. **Guard rail:** keep the Maranget algorithm; it is the
  soundness floor for nested patterns against the Rust backend's native `match`
  lowering.

- **Immutable `Lowerer` reader** (#18). `lower.rs` threads a `Lowerer<'a>`
  struct carrying the per-region HM type map with zero global mutable state
  (no `thread_local`/`static mut`/`RefCell`/`lazy_static`/`once_cell`). The
  Haskell backend spent a documented 6-PR migration moving away from 7 racy
  `NOINLINE` IORefs that caused a v0.15.3 editor panic; Rust's borrow checker
  makes the racy shape unrepresentable, so ipê starts where they finished.
  **Guard rail:** keep region-type lookups fail-closed — a missing region must
  surface as a `CompilerBug` diagnostic, never default to `any` (the
  wildcard-`any` soundness posture).

- **Reserved-identifier mangling** (#19). `naming.rs` mangles reserved Rust
  keywords with a trailing underscore (`match` → `match_`), valid for every
  keyword including the four (`self`, `Self`, `crate`, `super`) the grammar
  forbids as raw identifiers. The Haskell Rust-emitter emits `r#self`/`r#Self`,
  which the Rust grammar rejects — strictly less correct (would emit
  non-compiling Rust). Optional courtesy: file a bug against the experimental
  `../sky` Rust-emitter.

- **IR-level exhaustiveness smart constructor** (#21). `sky_ir::Match::new` is
  a fallible constructor that rejects any arm whose head is not a constructor,
  any arm naming a variant outside the scrutinee's enum, and any variant absent
  from the top constructors — a construction-time backstop the Haskell backend
  lacks (it emits the switch and trusts the earlier phase). **Guard rail:**
  extend this smart-constructor pattern to other IR nodes (task #31).

### Would harm a higher principle

- **Integer `//`-by-zero → 0** (#20). Under the strict tie-breaker P2 > P3,
  adopting Elm-0 semantics would diverge from the Go oracle (`rt.IntDiv` panics)
  to gain totality. ipê's classify-and-abort (`sky_int_div` panics →
  DivisionByZero → exit, pinned by a golden) is the best reconciliation: no UB,
  no silent wrong answer, clear operator signal, Go-parity. This gate is
  **closed**, correcting a stale memory that claimed it open. **Residual note:**
  `sky_int_div` relies on the synchronous panic classifier — verify
  `Cmd.perform`/`Task` equivalents install the classifier before shipping
  concurrent division paths, or a ÷0 on a spawned stack becomes a raw abort.

- **Ipê's kernel-registry regex-scrape** (#22). Ipê's `Ipê.Doc.KernelRegistry`
  regex-scrapes `lookupKernelType` (a 317-arm match) at Template-Haskell
  compile time — a validate-don't-parse drift hazard. ipê's designed `KernelId`
  registry (one resolved handle → {signature, per-backend emission}) already
  avoids it. **Affirmative lesson:** when `ipe doc`/LSP/`explain` are built they
  MUST read the authoritative `KernelId` table, never re-derive kernel facts by
  scanning compiler source.

### Not worth the churn (readability-only, below the justification bar)

- **Elm Category/Expected provenance strings** (#16). ipê already blames the
  correct per-subexpression span (list element uses its own span; binop operands
  use lhs/rhs spans; call args use their own span), so the offending
  sub-expression is already underlined. The residual gain is a text adornment
  ("as the 2nd argument to `f`") requiring a `Category` enum threaded through
  the whole ~195 KB `constrain.rs` — a broad refactor for a P6-only delta.
  Revisit only under a dedicated error-message-quality milestone.

- **Structural type-diff elision** (#23). `TypeMismatch` already carries a
  `path` localizer (rendered `(at user.age)`) plus full expected/found docs.
  Full structural elision risks hiding the very sub-type the reader needs
  (a communication-correctness regression) and complicates the
  byte-identical-under-`NO_COLOR` invariant. If ever pursued, the principled
  minimal form renders along the existing `path` and elides only proven-equal
  siblings.

---

## 3. Top adopt/adapt candidates — detail

### #1 · Port TailCallOpt — user tail-recursion → constant stack (ADOPT, high)

**Source:** `src/Ipê/Build/TailCallOpt.hs`.
**Improves:** P3 Soundness (primary) + P4 Efficiency.

Sky-Haskell has a dedicated pass rewriting tail-recursive functions into a
`loop { … continue }` with param reassignment (constant stack). ipê's Rust
backend emits **no** loop/continue for self-recursion (verified: zero `loop {` /
`continue` in the backend). Stdlib `foldl`/`find`/`any`/`all`/`member`/`drop`
are safe only because they are native Rust kernels; any **user-authored**
tail-recursive function (e.g. `loop n acc = if n==0 then acc else loop (n-1) (acc+1)`)
lowers to a real recursive Rust `fn` call → deep input → stack-overflow SIGABRT.
That abort is a **reachable, non-recoverable trap from well-typed Ipê code** that
the synchronous panic classifier cannot catch (guard-page abort, not a panic),
**plus** a documented-behaviour divergence (Elm/Ipê promise tail recursion =
constant stack), **plus** an O(N)→O(1) stack regression. It is the single largest
real principled gap between the two backends.

**Port shape.** Mirror `isTailRecursive`/`rewriteTailCalls`: detection = body is
`Case`/`If`/`Let` (or direct self-call) and every self-reference is in tail
position with matching arity (mutual/indirect recursion explicitly out of
scope). Emission maps cleanly to Rust: wrap the body in `loop { … }`; each tail
self-call becomes `(p0, p1, …) = (new0, new1, …); continue;`; every other tail
position becomes `break <expr>`/`return`. Rust's labeled loop + tuple
reassignment is cleaner than the Go version. **Fail-closed:** if the detector is
unsure, fall back to plain recursion — never mis-rewrite. Interim mitigation
(NOT a substitute): a recursion-depth guard or the `stacker` crate converts the
abort into a catchable error; the principled fix is the loop rewrite. Flag the
interim in `ipe-wasm-target-gate` (already notes "stack-overflow = reachable
trap from non-TCO list ops").

### #2 · JSON / machine-readable diagnostic report mode (ADOPT, medium)

**Source:** elm/compiler `--report=json`.
**Improves:** P5 Completeness (tooling); realizes Elm's Doc-vs-Render split as a
second renderer over already-structured data.

ipê's `Diagnostic` is fully structured typed enums
(`crates/sky_diagnostics/src/diagnostic.rs`) but only a single human-console
renderer exists (`render.rs`); `ipe` (`crates/ipe/src/lib.rs run_build`) has no
`--report`/`--json` flag. Add a `render_json(&Diagnostic) -> serde_json::Value`
alongside `render()` plus a `ipe build --report=json` flag emitting code,
severity, primary span (byte + line/col), secondary spans, help lines, and the
explain-page pointer. Because the Diagnostic is already owned, zonked structured
data (no interner needed at report time), this is purely additive and cannot
regress the human path — and it strengthens parse-don't-validate by proving the
diagnostic model is renderer-agnostic. Value is medium (not high) because the
in-repo LSP can call the diagnostic producers directly.

### #3 · Populate field/arg/tuple path breadcrumb on type-mismatch (ADAPT, medium)

**Source:** both (Sky-Haskell `Solve.hs renderPathln`/`recordFieldln`).
**Improves:** P6 Readability + diagnosis correctness; no harm to any higher
principle (pure enrichment).

`TypeError::TypeMismatch` already has `path: Box<[Box<str>]>` and `render.rs`
already prints `(at user.age)` when non-empty — but `unify.rs mismatch()`
hard-codes `path: Box::new([])`. When a constraint's two sides are deep
structures (annotation vs inferred record/fn), the leaf mismatch is shown with
**no** indication of where it diverged; the Haskell computes exactly this.
Thread an accumulating breadcrumb (record field name / tuple index / type-arg
position / `->` arg-vs-result) through `unify_flat`'s recursive calls and set it
on the `TypeMismatch` built at the deepest failing leaf. The renderer is already
done — scoped, no new diagnostic variant, no soundness surface. Keep
whole-structure mismatches (field-set mismatch, arity mismatch) rendering as-is;
only field/element **value** mismatches need the path.

### #4 · Defer ambiguous-import rejection to the use-site (ADAPT, medium)

**Source:** Sky-Haskell `detectExposingCollisions` + `checkAmbiguousUses`.
**Improves:** P5 Completeness (stops rejecting valid programs); does **not** harm
P2 Correctness because a wrong resolution can only occur at a bare use, which is
exactly where the check still fires.

`resolve.rs check_and_inject_value`/`inject_ctors_for_type` raise
`NameError::AmbiguousImport` (IPE-N0024) at import registration whenever two dep
modules expose the same unqualified name — regardless of whether it is ever used
bare. Two `exposing (..)` imports that share names (Set+Dict both expose
`empty`/`insert`/`member`/`map`/`filter`/`union`/`foldl`) are rejected even when
the program qualifies every reference. Track `unqual_origins` as
`name → set-of-source-modules` without erroring at registration; register the
name as ambiguous rather than binding it; raise IPE-N0024 only when a bare
identifier resolves onto an ambiguous name. Eager rejection has a
simplicity/invalid-states merit, but the shared-unused-name state is genuinely
valid, so completeness wins under the gate. Preserves the existing message +
two-module witness list.

### #5 · Reject tabs in layout-significant whitespace (ADAPT, medium)

**Source:** elm/compiler (`Tab` syntax error).
**Improves:** make-invalid-states-unrepresentable + P2 Correctness.

`sky_parse/src/lexer.rs advance()` counts every non-newline char as +1 column,
so a leading `\t` = 1 column; Sky-Haskell's `Space.hs` arbitrarily uses 4. Both
are arbitrary, and a tab/space-mixed file can parse into a block structure that
differs from its visual indentation — and the two compilers disagree. Elm makes
this a hard syntax error. Emit a new `IPE-P####` ("tabs are not allowed in
indentation; use spaces") when a tab appears in leading whitespace, eliminating
the layout ambiguity by construction rather than picking a magic width. Low
real-world frequency (most code uses spaces) but closes a genuine "code means
something other than it looks" hazard. **Reject** copying Haskell's tab=4; adopt
Elm's outright reject.

### #6 · Report multiple independent errors per phase (ADAPT, medium)

**Source:** elm/compiler (also present in Sky-Haskell `renderCliMany` +
`sortDiagnostics`). *(Merges the elm-area "multiple errors per pass" and the
diagnostics-area "multiple diagnostics per phase" candidates.)*
**Improves:** P5 Completeness + P6 Readability.

Every ipê stage returns a single `Diagnostic` and fails fast at the first `?`
(`ipe::build`/`build_project` → `CliError::Pipeline { diag }`; no accumulation).
Adopt the technique **only at layers where declarations are genuinely
independent** — name-resolution and type-checking: constrain/solve each
top-level definition against the shared env, collect a `Vec<Diagnostic>`, sort
(port Ipê's `sortDiagnostics`: by file, region, severity), and render all. Keep
the parser and the soundness / exit-0-then-cargo-fail gates **fail-fast** —
partial parse trees are not safe to keep constraining. The load-bearing guard is
**cascade suppression**: after the first error a poisoned value must not spawn
spurious follow-on diagnostics (Elm substitutes an error-type sentinel). This is
Correctness-neutral **iff** per-declaration isolation is preserved; the change is
gated on implementing that sentinel — which is why it is ADAPT, not ADOPT, and
why M0 originally chose single-diagnostic.

### #7 · Render multi-line source spans (ADAPT, medium)

**Source:** elm/compiler `Reporting/Render/Code.hs`.
**Improves:** P6 Readability.

`render.rs push_span_block` slices only the span's first line and clamps
`hi_byte = min(span.hi, loc.line_end)`, silently truncating a multi-line span
(multi-line record literal, case expression, type signature) to line one.
Render `startLine..endLine` with a per-line gutter + column carets (Elm's red
`>` marker). Preserve ipê's invariants: checked/clamped byte→line/col (no raw
slicing), panic-free on DUMMY/OOB, deterministic producer-order walk,
byte-identical plain output under `NO_COLOR`. **Hardening addition over Elm:**
**cap** very long regions so adversarial/generated source cannot make one
diagnostic dump hundreds of lines — matches ipê's fail-fast-on-pathological-input
stance.

### #8 · FFI type-boundary rejection (ADAPT, medium)

**Source:** Sky-Haskell `FfiGen.hs` (`isSimpleTypedType`, Go-residue check).
**Improves:** parse-don't-validate + P1 Security.

ipê's FFI consumer/generator is not yet ported (tasks #40-42, design-only).
`FfiGen.hs` rejects Ipê-type strings carrying Go-side residue and anything shaped
like a function/channel/map/ellipsis, refusing to emit a broken wrapper — the
correct boundary discipline for the security-critical `ipe add` path. When
porting the consumer (task #42), reproduce rejection-at-boundary: an untrusted
introspected type either parses into a typed, representable `FfiType` (smart
constructor) or is rejected with a typed `IPE-F####` diagnostic — never emitted
speculatively. Pair with the filed sandbox gate (task #41) and the
shell-injection surface at `FfiGen.hs:280` (single-quote-wrapped `pkgPaths`
concatenated into a shell string — use argv arrays, no shell string). Design
guidance, not a live port divergence.

### #9 · Enclosing-construct START as secondary caret (ADAPT, low)

**Source:** elm/compiler two-caret "I was partway through this X, which started
here … but got stuck here".
**Improves:** P6 Readability.

The parser already tracks the enclosing `Construct` (`parser.rs bump(Construct::…)`)
and the renderer already supports `SecondarySpan` with a distinct underline.
What is unconfirmed is whether the construct-START offset is threaded into the
emitted Diagnostic. Thread the byte offset where the current Construct opened and
attach it as `HelpLine::SecondarySpan { role: ConstructStart }` on malformed-*
diagnostics (header/exposing/case/let/if). If already attached this is a no-op
verification; otherwise a small additive fix reusing existing render machinery.
Value low — ipê already names the construct in prose via the defect enums; the
incremental gain is the second caret, not new information.

### #10 · Structural (size-scaled) solver-budget mode (ADAPT, low)

**Source:** Sky-Haskell `readBudgetMode`.
**Improves:** P5 Completeness + P4 guard-efficiency.

`solve.rs Budget::from_env` supports only unset→fixed 5M, N→absolute,
0→disabled. Haskell adds a **structural** default:
`max(DEFAULT_SOLVER_BUDGET, constraint_count * 200)` — catching an
N-constraints → ≫N·factor-steps blow-up while scaling the guard with program
size. When `IPE_SOLVER_BUDGET` is unset, compute
`max(DEFAULT_SOLVER_BUDGET, constraints.len() * 200)`, honour a
`IPE_SOLVER_BUDGET_FACTOR` override, keep 0=disabled / N=absolute. Low value —
5M is already generous; pure guard-rail tuning, no soundness change. Worth doing
when large multi-module programs land.

### #11 · Whole-program DCE with typed Ref ADT (ADAPT, low)

**Source:** Sky-Haskell `Dce.hs` (`TopRef`/`FfiRef`/`CtorRef`, `expandCtorClosure`).
**Improves:** P4 Efficiency (build speed) only.

ipê emits all defs (`for func in &module.funcs`) and relies on rustc/LLVM DCE —
sound, and actually **safer** on one axis: keeping all constructors means ipê
never needs the Haskell's `expandCtorClosure` fixup that keeps sister ctors of a
matched ADT alive (a subtlety the Haskell backend must carry precisely because
it prunes). The only cost is larger generated source + longer cargo compile
before LLVM strips dead code. If adopted, port only the reachability walk from
the entry root(s), keeping the typed `Ref` distinction so pruning FFI sigs and
user defs can't be confused; **do not** port the ctor-closure fixup (unnecessary
for us). Prioritise only if sweep/CI cargo-build time becomes a bottleneck.

---

## 4. Roadmap-timing buckets

Default home for adopted work is **roadmap C.5** (adopt principled strategies,
post-DONE). Items that help the current critical path (exit-0 pass / examples
sweep / diagnostics quality) and are cheap or already half-wired are flagged
**pull-early**. Rejects are inert but carry guard rails.

### Pull-early (on or feeding the current critical path)

- **#1 TailCallOpt (high).** On the critical path: a reachable non-recoverable
  trap from well-typed code that surfaces in the examples-sweep the moment a user
  writes a deep tail-recursive function. Fixing it is a soundness/Go-parity gate,
  not a nicety. Pull before declaring the sweep green on recursion-heavy examples.
- **#3 Type-mismatch path breadcrumb (medium).** Cheap — renderer already done,
  only the `unify.rs` breadcrumb threading is missing. High leverage for the
  agent-driven workflow (fewer ipe round-trips per fix). Pull early.
- **#5 Reject tabs in layout whitespace (medium).** Small, closes a
  parse-correctness / make-invalid-states ambiguity that can make sweep
  determinism depend on tab width. Cheap to pull early.
- **#4 Defer ambiguous-import rejection (medium)** — *conditional.* Pull early
  **iff** any example uses two `exposing (..)` imports with a shared name (which
  would currently block the sweep as a false rejection); otherwise C.5.

### C.5 — post-DONE (adopt principled strategies)

- **#2 JSON diagnostic report mode** — feeds the designed LSP, which is not yet
  built.
- **#6 Multiple independent errors per phase** — requires cascade-suppression +
  error-type sentinel machinery; DX win but gated on that infra.
- **#7 Multi-line source spans** — pure readability; a rendering change with a
  capping requirement.
- **#9 Construct-START secondary caret** — low, verification-or-small.
- **#10 Structural solver-budget mode** — guard tuning; do when large programs
  land.
- **#11 Whole-program DCE** — build-speed optimisation; do once the backend is
  feature-complete and only if cargo-build time bottlenecks the sweep.
- **#8 FFI type-boundary rejection** — folds into the FFI-consumer port design
  (tasks #40-42); land with that phase, not before.

### Rename / de-abbreviation pass

- None. No adopted candidate is naming-related; none belong to the
  Ipê→ipê rename or source-name de-abbreviation pass.

### Reject (no timing — inert, guard rails recorded in §2)

#12–#23 (12 candidates). ipê already meets or beats each, or adopting would harm
a higher principle. Guard rails installed in §2 to prevent regression toward the
reference shape.
