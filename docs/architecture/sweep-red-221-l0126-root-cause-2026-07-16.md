# Sweep red #221 — `36-composite-server` IPE-L0126 root cause

Read-only diagnosis. No code changed; all repro work under `/tmp` copies with
an instrumented THROWAWAY compiler build (`/tmp/ipe-instr`,
`CARGO_TARGET_DIR=~/.cache/ipe/fable-221-instr-target`).

## TL;DR

The reported location is FALSE. `src/Main.sky:73` (`\db ->` in `runMigrate`)
is a misattribution artifact. The real reject is in **`src/Server.sky:45-46`**:

```elm
guarded h =
    wrap (rateLimit "api" cfg.rateLimitCapacity cfg.rateLimitRefill h)
```

Two distinct defects compose:

1. **The reject itself** — a completeness gap in our function-value
   representation: partial application of a sibling *let-bound* function value
   (`wrap`, carrier `Box<dyn Fn>`, non-`Clone`) synthesizes a residual closure
   that reads `Var(wrap)` at closure depth 1, where the lowerer's depth-0
   callee-position exemption does not apply → fail-closed IPE-L0126. Legal Sky;
   Go reference runs it; the reference Rust backend handles this exact shape
   (`rateLimit … h` is named verbatim in its comments) via a **clonable
   `Arc<dyn Fn>` carrier + pre-cloned captures**.

2. **Diagnostic misattribution** — lowering diagnostics carry only a
   file-local byte `Span`, no module home. `skyc`'s `source_for_span`
   heuristic maps the Server.sky span onto whatever merged def numerically
   contains those bytes with the closest `lo` — in the pristine tree that is
   `Main.sky`'s `runMigrate` → the phantom `Main.sky:73:2`.

## (a) What IPE-L0126 is + the reported construct

`IPE-L0126` ("non-Clone capture in a closure is not yet supported",
`Feature::NonCloneCapture`): the capture-clone rewrite that keeps emitted
closures `Fn` (re-callable) fail-closes when a closure captures a binding
whose `CloneClass` is `NonClone` (functions, tasks, decoders, `Cmd`/`Sub`,
generics) and reads it anywhere except as the *direct callee at depth 0*.
Under the current `Box<dyn Fn>` carrier, forwarding such a capture into an
inner `move` closure would move it out of the outer environment on first call
→ the outer closure degrades to `FnOnce` → rustc E0525 at cargo time; the
lowerer rejects instead (THE SEAL: fail closed at skyc time).

The caret at `Main.sky:73:2` covers `(\db ->` — but byte-mapping proves the
span belongs to Server.sky (see (c2)). `runMigrate` compiles clean when
isolated with `State.sky` + `Migrations.sky`.

## (b) Minimal trigger

9 lines, single module, no stdlib effects (`/tmp/b221z`):

```elm
inc : Int -> Int -> Int
inc a b =
    a + b

main =
    let
        wrap f x = f x + 1        -- wrap : (Int -> Int) -> Int -> Int
        guarded f = wrap (inc f)  -- PARTIAL app of sibling let-bound fn value
        r = guarded 1 2
    in
        println (String.fromInt r)
```

→ `IPE-L0126`, instrumentation: `site=rewrite_captured_clones sym=wrap
depth=1`, `lambda_span` = the `guarded` binding.

Contrast pair: `guarded f = wrap (inc f) 2` (FULL application — `wrap` stays a
depth-0 direct callee) compiles, exit 0. Likewise hoisting `wrap` to a
top-level def compiles (a top-level ref is not a local capture).

Trigger shape, in words: **a let-bound closure partially applies a sibling
let-bound function value it captures.**

## (c) Root cause

### (c1) The reject — chain through `crates/sky_lower/src/lower.rs`

1. `lower_lambda` (lower.rs:8076) lowers `guarded`'s lambda; `captured_locals`
   (lower.rs:8040) classifies the capture `wrap` by `clone_class` of its
   region type `Fun([Fun([ServerRequest], Task(ServerResponse)),
   ServerRequest], Task(ServerResponse))` — the curried `Handler -> Handler`
   arrow, FLATTENED to arity 2. `clone_class(Fun(_, _)) = NonClone`
   (lower.rs:991) → `noncl_set = {wrap}`.
2. `guarded`'s body `wrap (rateLimit … h)` supplies **1 of `wrap`'s 2
   flattened args** → `eta_expand_value_partial` (lower.rs:10898) synthesizes
   the residual `Expr::Lambda { eta_0 } { Apply(Var(wrap), [<eta'd rateLimit
   partial>, eta_0]) }`. `Var(wrap)` now sits INSIDE a synthesized lambda.
3. `rewrite_captured_clones` (lower.rs:1261) walks `guarded`'s body at depth
   0; the `Expr::Lambda` arm recurses at depth+1; the `Apply` func-position
   exemption for `noncl_set` symbols is gated `depth == 0` (lower.rs:1313) —
   correctly, because at depth > 0 the inner `move` closure steals the value
   from the outer env (E0525). `Var(wrap)` at depth 1 falls through to
   lower.rs:1276 → `Err(unsupported(lambda_span, Feature::NonCloneCapture))`.

The generative reason is **not** the depth gate (which is sound under the
carrier) but the carrier itself: `sky_backend_rust/src/emit_types.rs:320`
renders every general first-class function as
`Box<dyn Fn(…) -> R + Send + Sync + 'static>` — non-`Clone` — so a captured
function value structurally cannot be forwarded, and `clone_class` must floor
`Fun` to `NonClone`. The L0126/L0125 family and `reject_fn_value_reuse`
(lower.rs:4026, multi-use of a fn-carrying binding) all exist only to
fail-close over this representation choice.

Already-inconsistent edge inside our own pipeline: the exact Handler shape
`Fun([ServerRequest], Task(ServerResponse))` renders as
`ServerHandler<SkyError>` — an **`Arc<dyn Fn>` alias, which IS `Clone`**
(emit_types.rs:254) — yet `clone_class` still calls it `NonClone`. The
classifier and the emitter's carrier choice are two hand-maintained tables
with no single source of truth; that drift is the same defect class one shape
over.

### (c2) The misattribution — `crates/skyc/src/lib.rs:657-698`

Lowering diagnostics carry a bare `Span` (file-local byte offsets,
`sky_diagnostics/src/span.rs` — no file id). After `link::link` merges all
modules, `source_for_span` guesses the owning file: among all defs whose
`body_span` numerically contains the span, pick closest-`lo` (narrowest as
tiebreak). The failing span is Server.sky bytes `{2036, 2118}` (`guarded h =
… wrap (rateLimit …)`, line 45). `Server.run`'s def starts ~1000 bytes before
that offset, while Main.sky's `runMigrate` body *coincidentally* starts at
byte ~1990 → `lo_dist` 46 beats ~1000 → the renderer blames
`Main.sky:73:2`, which is exactly byte 2036 of Main.sky (`(\db ->`).
Verified byte-for-byte on the pristine example.

This is why bisection was treacherous: editing Main.sky moved the *blamed*
location (`Main.sky:73:2` → `Main.sky:72:24` → `Routes/Health.sky:54:19`)
while the single underlying error (Server.sky `wrap`) never changed.
Instrumented runs show exactly ONE L0126 fire in every layout.

The typecheck path already solved this class: `sky_db::typecheck` /
`infer_attributed` returns `(diag, home)` and skyc resolves `home` exactly
(lib.rs:716-726, the #144 fix). Lowering never got the same treatment even
though `Lowerer::current_home` knows the answer at every `unsupported()` site.

## (d) What `../sky` does for this construct

- **Go backend** (the parity oracle): Go closures capture by reference,
  garbage-collected — no ownership/Clone dimension at all. Example 36 builds
  and runs (`../sky/examples/36-composite-server` exists as a reference
  example; the Server.sky comments describe the middleware composition as a
  deliberately exercised shape).
- **Reference Rust backend** (`../sky/src/Sky/Generate/Rust/Builder/`):
  - First-class Handler-shaped values render as **`Arc<dyn Fn>`**
    (ExprEmitter.hs ~1999-2012 — the comment names `rateLimit … h` and
    `handleRegister cfg db` verbatim as the partial-app residuals that "must
    be Arc-wrapped to match").
  - `arcWrapClosure` (ExprEmitter.hs ~2029) wraps the residual `move` closure
    in `Arc::new(…)` and **pre-clones every captured outer var**
    (`let v = v.clone(); …`) so the Arc owns `'static` captures and the outer
    closure remains re-callable.
  - The runtime's `IntoServerHandler` has an `Arc<F: Fn>` impl so the wrapped
    form registers and unsizes at Handler params.

So the reference compiles this construct by making the function-value carrier
clonable and cloning at capture; we reject it. **Verdict: OUR
lowering/representation gap, not a legitimate program error.** L0126 is an
honest fail-closed diagnostic, but for this program it is a completeness gap
vs the reference contract.

## (e) Structural fix + invariant

### Fix A — clonable function-value carrier (the real fix)

Adopt the reference's model: first-class function values get ONE carrier and
it is `Clone` — `Arc<dyn Fn(…) -> R + Send + Sync + 'static>` (or the
in-flight clone-relay equivalent). Then:

- `clone_class(Fun(_, _))` becomes `CloneOk`; captured fn values get
  `.clone()` per call like every other `CloneOk` capture (Arc clone = cheap
  refcount bump — also the Efficiency-correct choice).
- The depth-0 exemption, the L0125/L0126 family for fn captures, and
  `reject_fn_value_reuse`'s multi-use reject all dissolve — whole defect
  class unrepresentable, not patched per-site.
- Call sites go through `Fn::call` on `&self` exactly as today.

**Invariant:** *every value type a Sky closure can capture implements
`Clone`* — equivalently, acceptance of a well-typed capture never depends on
its syntactic position. This is the make-invalid-states-unrepresentable form;
PRINCIPLES.md already names the ad-hoc alternative as the #172 anti-pattern
("coerce every function-typed leaf, not just inline-lambda siblings").

Secondary invariant (kills the c1 drift): `clone_class` must be DERIVED from
the emitter's carrier table (single source of truth), not maintained in
parallel — a shape that renders `Arc` can never again classify `NonClone`.

Do NOT symptom-patch (e.g. special-case depth-1 callee position, or rewrite
example 36): instrumentation shows further latent fn-captures in the same
example (`kont` in the Routes auth continuation, exempt today only by being a
depth-0 callee) and `wrap`/`guarded` are each used 2-4×, which trips
`reject_fn_value_reuse` next. Only the carrier fix clears the family.

### Fix B — home-attributed lowering diagnostics (independent, small)

Thread `current_home` into lowering diagnostics the same way typecheck does:
lowering error channel becomes `(Diagnostic, ModPath)` (or `Diagnostic` gains
an optional `home`), and skyc's `span_attributed_err` (lib.rs:743) resolves
`home_to_source` exactly, falling back to the heuristic only for genuinely
homeless spans (`Span::DUMMY`). **Invariant:** *a diagnostic's span is only
ever resolved against the source text of the module that produced it.*
Ship B even if A lands first — every other lowering diagnostic class
(IPE-L01xx) misattributes the same way in multi-module projects, and it turns
any future multi-module red from a bisection swamp into a one-look fix.

## (f) Affected files + regression tests

Fix A (carrier):
- `crates/sky_lower/src/lower.rs` — `clone_class`, `rewrite_captured_clones`
  (+depth gate), `eta_expand_partial` / `eta_expand_value_partial` /
  `eta_expand_partial_ctor` / `eta_expand_over_partial`,
  `reject_fn_value_reuse`. **⚠ OVERLAP: this is the same file and the same
  machinery the concurrent clone-relay restructure is rewriting — this fix is
  most likely THAT work's end state. Parent must sequence: land clone-relay
  first, then re-test 36 before any further L0126 work.**
- `crates/sky_backend_rust/src/emit_types.rs` (`Box<dyn Fn>` → `Arc<dyn Fn>`
  render + collapse of the ServerHandler/Ws special cases into the general
  arm), `emit_expr.rs` (closure construction → `Arc::new`, capture
  pre-clones), the `FnOnceChain`/`Decoder` payload arms need an explicit
  keep-or-migrate decision.
- `runtime/src/sky_runtime/` — kernel params currently typed `Box<dyn Fn>`
  (if any); `ServerHandler` is already `Arc`.

Fix B (attribution):
- `crates/sky_lower` (attach home at `unsupported()`/`bug()` construction or
  at the `lower_def` boundary), `crates/sky_db` (lowering query error type),
  `crates/skyc/src/lib.rs` (`span_attributed_err`), `crates/sky_diagnostics`
  (only if `Diagnostic` itself carries the home).

Regression tests that should exist:
1. Golden + `IPE_E2E=1` for the (b) minimal trigger — sibling let-bound
   partial application; today expected-red, green after A. Companion greens:
   the full-application and top-level-`wrap` contrast variants (must stay
   green even before A).
2. Multi-module attribution test: dep-module lowering error whose span lands
   inside an unrelated entry-module def byte range; assert the rendered path
   is the dep file (extend the existing `CliError::Pipeline { file, .. }`
   assertions around `crates/skyc/src/lib.rs:2278/2382`).
3. `examples/36-composite-server` sweep row (auto-included by the disk-derived
   build_set) as the integration gate.

## Confidence + residuals

**Confidence: high** on both root causes — mechanism reproduced three ways
(pristine example, module-isolated copy, 9-line minimal), instrumented at the
exact emission site (`rewrite_captured_clones`, `sym=wrap`, `depth=1`,
`lambda_span={2036,2118}`), and byte-verified for the misattribution
(Server.sky:45 bytes ↔ Main.sky:73:2).

Residuals (honest):
- Post-fix pass of example 36 is NOT proven: at least two more latent members
  of the family are visible (`kont` capture in the Routes continuation;
  `wrap`/`guarded` multi-use vs `reject_fn_value_reuse`), plus whatever hides
  behind the current fail-fast. Expect 36 to need one re-diagnosis round after
  clone-relay lands.
- The `Routes/Health.sky:54:19` sighting was observed on an edited /tmp copy
  (different byte layout); on the pristine tree the single blamed location is
  `Main.sky:73:2`. Same one underlying error in all layouts.
- Go-reference build of 36 was not re-run this session (reference example
  present; brief asserts it runs — consistent with the reference backend's
  explicit handling of this exact shape).
- `FnOnceChain` (#164 curried one-shot chains) and `Decoder(Fun)` payloads
  interact with an Arc migration; unexamined here, flagged for the A design.
- Instrumented compiler was a /tmp source copy; repo working tree untouched by
  this diagnosis (pre-existing dirt: `scripts/progressive-development/backlog.jsonl`,
  `examples/manually-found-errors.md` — not mine).
