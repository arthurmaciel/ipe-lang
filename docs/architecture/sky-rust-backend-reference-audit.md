# ipê Rust backend — reference audit against `../sky` (feat/runtime-rust)

## Framing

`../sky` on branch `feat/runtime-rust` is the **reference**: it is a freely
available implementation of the Sky→Rust path that already passes the entire
non-FFI example set, so its behaviour is the parity oracle for ipê. The two
trees share a **vendored runtime fork** — the 48 `src/sky_runtime/*.rs` modules
carry identical names on both sides, so runtime divergence is *within-module*
(behavioural / parity / kernel-coverage drift), not structural. The real
divergence is in the **backend**: the reference emits Rust from a Haskell
compiler (`src/Sky/Generate/Rust/*.hs` + `src/Sky/Build/Rust/*.hs`), while ipê
is Rust-all-the-way (`skyc`) with the work split across `crates/sky_lower`,
`crates/sky_types`, and `crates/sky_backend_rust`. Because the host language
differs, we port **strategies and invariants, not literal code**. Every finding
below is judged strictly against the PRINCIPLES order — **security > correctness
> soundness > efficiency > completeness > readability** — plus the two
foundational rules **parse, don't validate** and **make invalid states
unrepresentable**. "More principled" is decided by that order alone, never by
familiarity or line count. Where ipê is already equal or ahead, that is stated
plainly so the reference is not cargo-culted.

---

## Comparison table

Verdict legend: **T+** = theirs-more-principled (adopt); **O+** =
ours-equal-or-better (keep, do not cargo-cult); **~** = orthogonal (subsystem
not yet built on one side, or tooling ergonomics).

| # | Concept / pass / capability | Their approach | Our approach | Verdict | Deciding principle | Adoption + roadmap slot |
|---|---|---|---|---|---|---|
| 1 | Expression lowering shape | `Can.Expr_` → Rust string in one walker (`ExprEmitter.hs`) | `canon → typed sky_ir::Expr → Rust` (two-stage) | **O+** | soundness / invalid-states | keep typed IR checkpoint |
| 2 | Kernel dispatch | `(mod,name)` string `case`; **fail-open** snake_case default | closed 406-variant `KernelFn`; **fail-closed** SKY-L0108 | **O+** | security → correctness | keep; add typed FFI arm at FFI-phase |
| 3 | Case-arm pattern emission (top level) | raw `match`, slice patterns, Maybe/Result bridge | same + explicit ctor field-count `CompilerBug` guard | **O+** | soundness | keep |
| 4 | Nested list/cons/record inside ctor payload | recurses, compiles | fail-closed `Err(NestedCtorDiscrimination/NestedPayloadPatterns)` | **~** (ours safer, less complete) | soundness > completeness | close completeness gap — **pre-sweep** |
| 5 | Refutable function-arg patterns (`f (Just x) =`) | synthesize `let…else { panic! }` (reachable panic) | reject at lower (SKY-L0115/0116) | **O+** | soundness ("no panic from well-typed Sky") | close via front-end desugar, NOT their panic — **pre-sweep/before-push** |
| 6 | Bare `.field` accessor-as-function | `Can.Accessor` → closure | not representable (no AST variant) | **~** (front-end gap) | completeness | front-end AST work — **pre-sweep** |
| 7 | Tail-call optimization | `TailCallOpt.hs` analysis; **Go-only** emission; Rust backend has NO TCO | absent (task #49) | **T+** (vs Go); tie (vs their Rust) | soundness (constant stack vs uncatchable overflow trap) | port analysis, author Rust `loop` emission ourselves — **before-push (#49)** |
| 8 | Kernel type source of truth (#45 root) | `.sky` annotation is the ONE typed source; unknown kernel = unbound-name error | duplicate hand-kept `kernel_ty` table + `Ty::Var(u32::MAX)` fallback | **T+** | parse-don't-validate / invalid-states | single fail-closed source + parity tripwire — **pre-sweep (#45)** |
| 9 | Type-render fallback | `TypeRenderer.hs` unmatched → `"String"` default (2nd exit-0-cargo-fail) | `render_type : IrType → DResult`, closed enum, no default | **O+** | soundness floor | keep (contingent on #45 fix) |
| 10 | Opaque-type rendering architecture | stringly-keyed `Map (String,String) String` + `{M}` placeholder subst | first-class closed `IrType` variants, structural `Box<IrType>` msg | **O+** | invalid-states | keep |
| 11 | Record-alias resolution | best-superset row-poly widen; miss → `"String"` | exact sorted-key; miss → `CompilerBug` | **O+** (soundness) / theirs more complete | soundness > completeness | add superset fallback ONLY if a sweep example trips CompilerBug — **conditional pre-sweep** |
| 12 | Generic record structs + effectful-callback rendering | 3-way: stored `Arc<dyn Fn>+Send+Sync` / passed `impl Fn` / ADT-embedded bare `fn` | uniform `Box<dyn Fn>` (+ handler special-case) | **T+** (one axis) | correctness | port 3-way classification when derive/Clone callback lands — **before-push, per-subsystem** |
| 13 | Numeric coercion at FFI boundary (`NumCoerce.hs`) | one saturating helper; `try_from` for `usize/isize`; total, panic-free | absent (FFI parked) | **~** (FFI-phase) | soundness | port invariants verbatim — **FFI-phase (#42, blocks #41)** |
| 14 | Phantom-error turbofish pinning | central `kernelsNeedingErrorPin` / `topLevelErrorPin` / `kernelsZeroArg` registries | scattered `::<SkyError>` arms + some structural bottom-up inference | **~** (theirs central; ours structural where it applies) | readability vs invalid-states | fold pin flags into #45 single source — **pre-sweep, bundled with #45** |
| 15 | Crate-version single source + drift guard | `crate-specs.toml` embedded SSOT + `crate_specs_sync.rs` drift test | typed `crate_specs.rs` `const CrateSpec` table (name+version SSOT) read by every `*_cargo_toml` surgery fn; co-located `crate_specs_match_manifests` drift test asserts SSOT ≡ `runtime/Cargo.toml` (all 11) **and** ≡ golden base manifest (tokio) | **O+ (closed #50)** | invalid-states + correctness | done — typed const table (deliberately Rust consts, not re-parsed TOML: compiler-checked structured data over string re-parse); drift test additionally covers the golden base manifest the reference lacks |
| 16 | Feature-toggle manifest surgery | table-driven from `CrateSpec` renders | anchored `replacen`, fail-loud `CompilerBug` on anchor miss | **O+** | soundness | keep surgery; only sink versions into SSOT |
| 17 | FFI crate binding (FfiCall/FfiInstance) | typed `Call` ADT + `FromJSON`-time `validateCall`; per-instantiation bindability gate + `E4400`; `cargoProfilePanicIsUnwind` guard | absent | **T+** (ours absent) | security → correctness | port invariants verbatim, no string-hole template — **FFI-phase (#42)** |
| 18 | FFI inspector runner / sandbox | `quoteShell` arg-quoting only; **no build sandbox** (RCE-on-`sky add` exposure) | absent | **~** (arg-quote worth porting; sandbox is a gap BOTH miss) | security (top) | port arg-quoting #40; sandbox is ipê net-new hardening — **#41 (exceeds reference)** |
| 19 | Console mini-app pre-build | atomic tmp+rename publish, crash-safe fingerprint-last ordering | absent | **~** | build ergonomics | reuse patterns when built — **post-DONE** |
| 20 | Go-parity oracle harness (rust-equivalence/equivalence-corpus/equivalence-render) | 3-layer differential vs Go reference; render normalizers drive strict UI diff | normalizers vendored byte-identical but **undriven**; sweep defaults `NO_EQUIV=1` | **T+** | correctness (green build ≠ correct) | stand up Go oracle, port corpus then render — **pre-DONE (literal endgame gate)** |
| 21 | Deterministic parity fixtures (`tests/sky/`, 140) | end-to-end Sky projects pinning kernel/codegen behaviour on both backends | none (only 8 vendored soundness + 3 numeric-parity tests) | **T+** | correctness | port non-FFI subset, prioritize silent-divergence classes — **pre-sweep** |
| 22 | Security render fixtures (`69-html-render-parity`, `70-style-injection`) | executable `</style>`-breakout + script-verbatim assertions | prose in `css-attr-injection-safe-emit.md`; styleNode hole open | **T+** | security | port as stored-HTML snapshots (no oracle needed) — **pre-sweep, gates #47** |
| 23 | Emitted-code soundness harvester (`quality-audit.sh`) | enumerates panic/unsafe/`dyn Any`/lossy-cast over generated code | clippy hard-deny on vendored runtime only; not over emitted projects | **T+** (partial) | soundness | port pointed at `sky-out/rust/` — **before-push** |
| 24 | Harness self-tests (`examples_test.sh`, `keep_go_parity_test.sh`) | test the sweep classifier itself | absent | **T+** | correctness | port alongside equivalence harness — **with equivalence port** |
| 25 | Numeric-parity unit tests (decimal/money/regex) | none | `runtime/tests/{decimal,money,regex}_parity.rs` (Go values inline, no oracle needed) | **O+** | correctness-per-cost | keep + extend to json-escape / float-threshold / Dict-Set |
| 26 | Runtime within-module behaviour (env, decimal, json, http, auth, jwt, cache, regex, telemetry) | reference baseline | fork uniformly hardened ahead (see §Runtime) | **O+** | security/correctness/soundness | keep; do NOT cargo-cult reference runtime back |
| 27 | `stringify.rs` float sci-notation threshold | `!(-4..21)` (switch at exp≥21) | `!(-4..6)` (switch at exp≥6) + pinning test | **O+** | correctness | **resolved** — Go 1.26.2 oracle confirms flat exp≥6 (ours); reference's 21 diverges. Pinned in stringify.rs::float_go_v_parity + string.rs::ff_go_g_threshold_is_six_not_twentyone |

---

## Runtime within-module verdict (Slice 4 detail)

The runtime is a vendored fork and ipê is **uniformly at-or-ahead** across all
48 modules; there is **no** runtime module where the reference is more
principled. 12 modules are byte-identical. Every behavioural divergence is
ours-ahead:

- **Env access** — ours routes through a process-global lock
  (`system::read_env_var` / `locked_set_var_if_absent`); theirs uses raw
  `std::env` and relies on call-ordering. *Soundness* (env data-race is UB).
- **decimal/money** — ours `MidpointAwayFromZero` (Go `StringFixed` parity),
  16dp div cap, `MAX_RATES`-honoured auto-inverse, `saturating_abs` in
  `allocate`. Theirs banker's-rounds `toStringFixed`, uncapped div, wraps at
  `i64::MIN`. *Correctness + soundness*.
- **json** — ours clamps indent (`MAX_JSON_INDENT=16`) and `optional`
  propagates decode errors on present-but-malformed fields; theirs is
  unbounded and silently swallows malformed input. *Security*.
- **http_client / ws_client / http_stream** — ours redacts URL userinfo/query
  in errors, defaults SSRF-deny ON in production, returns `Err` on invalid HTTP
  method; theirs echoes the URL, is SSRF-opt-in-only, and `unwrap_or(GET)`
  silently downgrades. *Security + correctness*.
- **auth** — ours fail-closes on id-column decode error; theirs
  `unwrap_or(0)` authenticates as user 0. *Security*.
- **jwt** — ours rejects `now==exp` (Go parity), makes exp/nbf optional, adds
  boundary + RS256 tests; theirs accepts one instant past expiry and rejects
  legitimately exp-less tokens. *Correctness/security*.
- **cache** — ours saturating counters; theirs raw `+=`/`-=` (debug overflow
  panic). *Soundness*.
- **regex split** — ours drops the trailing zero-width empty (Go
  `regexp.Split` parity); theirs keeps boundary empties. *Correctness*.
- **telemetry / trace** — ours strips CRLF from CSP frame-ancestors and scrubs
  log controls + U+2028/9; theirs uses them verbatim. *Security*.

**One cross-fork disagreement (RESOLVED):** `stringify.rs` float
scientific-notation threshold — theirs switches at exp≥21, ours at exp≥6. Ours
is correct. A Go 1.26.2 oracle re-probe confirms `fmt %v` ≡
`strconv.FormatFloat(f,'g',-1,64)` for every input, with a **flat** cut to
scientific notation at decimal exponent ≥ 6 (and < −4) and **no** exponent-21
behaviour anywhere: `1000000 → "1e+06"`, `1e15 → "1e+15"`, `1e20 → "1e+20"`,
`999999 → "999999"`, `0.0001 → "0.0001"`, true `-0.0 → "-0"`. The reference's 21
is the diverging value — it would emit `1000000`, sixteen zeros for `1e15`, etc.
Pinned by two discriminating regression tests
(`stringify.rs::float_go_v_parity` + `string.rs::ff_go_g_threshold_is_six_not_twentyone`),
each proven to fail if the threshold drifts to 21. Reproduce the oracle in-place:

```go
// probe.go — run: go run probe.go   (Go 1.26.2)
package main

import ( "fmt"; "math"; "strconv" )

func main() {
    negZero := math.Copysign(0, -1) // TRUE IEEE-754 negative zero
    show := func(f float64) {
        fmt.Printf("%%v=%-22q g=%-22q\n",
            fmt.Sprintf("%v", f), strconv.FormatFloat(f, 'g', -1, 64))
    }
    for _, f := range []float64{
        99999, 100000, 999999, 1000000, 1000001, 1234567,
        1e15, 1e20, 1e21, 123456.789, 0.0001, 0.00001, 1.5, -1.5,
    } { show(f) }
    show(negZero); show(math.Inf(1)); show(math.Inf(-1)); show(math.NaN())
}
```

---

## Adopt list (theirs-more-principled only)

Each item names the concrete invariant to bring over and the task it attaches
to.

1. **Kernel type from the single declaration (#45).** Make the embedded
   `Ffi.kernel` binding's own `.sky` HM annotation the source of a kernel's
   call-site type (or, per the banked Phase-C bridge, move the scheme INTO
   `StdlibDecl` so the registry is the single source). Delete the duplicate
   `kernel_ty` arm set and the `_ => Ty::Var(u32::MAX)` fallback; replace the
   fallback with a fail-closed `Err(SKY-L0108)` for any kernel lacking a
   scheme, plus a `stdlib_scheme ≡ kernel_ty` parity tripwire. **Bundle the
   phantom-error turbofish + zero-arg pins (item 14)** into that same source as
   optional `turbofish: Option<&'static str>` / `zero_arg: bool` fields so a
   new phantom-error kernel can't be forgotten. *Task #45. Roadmap: pre-sweep.*

2. **Tail-call optimization (#49).** Port the reference's backend-agnostic
   `TailCallOpt.hs` *analysis* verbatim — `isTailRecursive = countTailSelfCalls
   > 0 && countNonTailSelfCalls == 0`, tail-position propagators over
   Case/If/Let/LetRec/LetDestruct, call-args non-tail. Then author the **Rust
   emission ourselves** (no reference exists — their Rust backend has no TCO):
   a detected tail-recursive top-level fn emits `loop { … }` with simultaneous
   tuple-temp param rebind + `continue` per tail self-call and `break <value>`
   at every other tail position. Keep their scope exclusions (non-tail, mutual,
   let-rec out of scope for v1). *Task #49. Roadmap: before-push. Soundness
   hole — ipê stack-overflow-traps where Go runs in constant stack.*

3. **Crate-version single source + drift guard (NEW task).** Introduce an
   embedded `crate-specs.toml` (or one `const` table) as the authoritative
   version+features source that `project.rs`'s five manifest emitters read, and
   a Rust drift test mirroring `crate_specs_sync.rs` asserting emitted versions
   ≡ `runtime/Cargo.toml`. Closes a live three-copy silent-skew class in the
   manifests already emitted. *NEW: "crate-version SSOT + drift test". Roadmap:
   before-push.*

4. **Go-parity oracle harness + parity fixtures (the literal DONE gate).**
   Stand up a Go reference build path (invoke `../sky --backend go` cross-repo,
   or cache Go outputs as fixtures); port `equivalence-corpus.sh` first (pure-stdlib
   stdout byte-diff — cheapest, highest signal), then `equivalence-render.sh` (drives
   the already-vendored `equivalence_normalize_html.py` / `equivalence_tui_grid.py`
   normalizers); flip `examples-sweep.sh` off `SKY_SWEEP_NO_EQUIV`; port the
   harness self-tests. Author the non-FFI subset of `tests/sky/` as ipê
   fixtures now (they double as skyc goldens), prioritizing the
   silent-divergence classes: json HTML-escape, float display threshold,
   Dict/Set determinism, money rounding, the 6 `kernel-parity-probe*`. *NEW:
   "Go oracle + equivalence harness". Roadmap: fixtures pre-sweep; oracle + render
   flip pre-DONE.*

5. **Security render fixtures + FFI binding invariants.**
   - Port `70-style-injection` and `69-html-render-parity` as stored-HTML
     snapshot fixtures NOW (no Go oracle needed): they assert the `</style>`
     breakout-strip and script/style-verbatim rules, and become the acceptance
     test for the open `Std.Html.styleNode` verbatim XSS hole. *Task #47 gate.
     Roadmap: pre-sweep.*
   - When the FFI consumer lands, mirror the reference's `Call` typed-AST with
     `FromJSON`-time `validateCall` (hole-misplacement unrepresentable), the
     per-instantiation bindability gate with static trait table + `E4400`-class
     typed diagnostic, the `cargoProfilePanicIsUnwind` guard before any
     catch_unwind closure boundary, and `NumCoerce`'s one-saturating-helper +
     `try_from`-for-platform-widths invariants verbatim. Port `quoteShell`
     arg-quoting at inspector wiring. The build **sandbox is ipê net-new
     hardening** — the reference runs `cargo build` on untrusted crates
     un-sandboxed (RCE-on-`sky add`), so here ipê must exceed, not mirror.
     *Tasks #40 (arg-quote), #41 (sandbox, exceeds ref), #42 (consumer +
     NumCoerce). Roadmap: FFI-phase.*

Also adopt when their triggering subsystem lands: the **3-way callback
classification** (item 12) — stored `Arc<dyn Fn>+Send+Sync` / passed `impl Fn`
/ ADT-embedded bare `fn` — needed the moment a `derive(PartialEq)` ADT stores a
function (e.g. `ShouldRetry e = RetryWhen (e -> Bool)`) or a `Clone` stored
callback appears; and the **`quality-audit.sh`** harvester pointed at emitted
`sky-out/rust/` projects (before-push).

---

## Capability-gap list (sweep target — what they pass that ipê doesn't yet)

Front-end / lowering (block affected sweep fixtures):
- Bare `.field` accessor-as-function (no canon AST variant).
- Nested list/cons/record inside a ctor payload (`Just (h :: t)`, `Ok {name}`).
- Refutable function-argument patterns (`f (Just x) = …`) — close via
  front-end desugar to `case`, never a panic.
- No TCO — tail-recursive Sky stack-overflow-traps on long lists.

Behavioural parity (dormant until Go oracle):
- No executable Go≡Rust behavioural guarantee at all — only emission goldens.
- Not yet build+run+parity-checked: `config` (TOML/YAML/JSON), `email`,
  `csv-builder`, `cache-cli`, `char`, `retry`/`retry-transient`,
  `sqlvalue-params` / `db-migrate-cli` / `db-postgres-compile`,
  `live-routing`/`live-form`/`live-sessions`/`live-pubsub`/`live-db-startup`,
  `tui-input`, `ws-client-onmessage`/`ws-server-capturing`, `server-413`.
- No executable proof the styleNode verbatim path can't break out of `<style>`.

Unbuilt subsystems (arrive with their kernels, not principle gaps):
- Opaque `IrType` coverage for Csv, Cache, Email, WebSocket (client+server),
  Http client + Http.Stream, Server.Stream writer.
- FFI scalar coercion entirely (no Sky↔foreign numeric width binding possible).
- Function-typed ADT/record fields under `derive`/`Clone`.
- Whole FFI inspect→emit path (`43-114 ffi-*` corpus), console pre-build cache.

---

## Roadmap timing

Consistent with the existing roadmap (exit-0 registry B–E + TailCallOpt + #46
pre-sweep; #47 + F7 before push; sweep; parity; push; FFI post-DONE):

**Pre-sweep**
- #45 kernel-type single source + turbofish/zero-arg pins folded in (item 1/14).
- Close lowering completeness gaps: nested payload patterns (item 4), `.field`
  accessor (item 6), refutable arg-pattern desugar (item 5).
- Author parity fixtures incl. the 6 `kernel-parity-probe*` (item 21).
- Port `70-style-injection` + `69-html-render-parity` snapshots as #47's gate
  (item 22).
- Runtime int-div (`sky_int_div` wiring, F6-adjacent) — soundness hole.

**Before push**
- #49 TCO analysis port + Rust `loop` emission (item 2).
- Crate-version SSOT + `crate_specs_sync` drift test (item 3 / NEW).
- `quality-audit.sh` over emitted `sky-out/rust/` (item 23).
- ~~Re-probe the `stringify.rs` float threshold vs Go oracle (item 27).~~ Done —
  Go 1.26.2 oracle confirms ours (flat exp≥6); pinned by two discriminating tests.
- 3-way callback classification IF a derive/Clone callback subsystem lands
  before push (item 12).

**Pre-DONE (literal endgame gate)**
- Stand up Go oracle → port `equivalence-corpus.sh` → `equivalence-render.sh` → flip off
  `SKY_SWEEP_NO_EQUIV` → port harness self-tests (items 20/24).

**FFI-phase (post-DONE)**
- #40 inspector wiring + `quoteShell` arg-quoting.
- #41 build sandbox (ipê net-new hardening beyond the reference).
- #42 FFI consumer: `Call` typed-AST + `validateCall`, bindability gate +
  `E4400`, `cargoProfilePanicIsUnwind`, `NumCoerce` invariants (items 13/17/18).
- Console pre-build with atomic-publish + crash-safe fingerprint ordering
  (item 19).
- `keep-go-parity.sh` sync planner when upstream sync resumes.

---

## Where ipê is already equal-or-better (do not cargo-cult)

- **Typed IR checkpoint** (`canon → sky_ir → Rust`) vs their AST→string single
  pass — malformed shapes are unrepresentable, not caught only at rustc.
- **Closed 406-variant `KernelFn` + fail-closed SKY-L0108 dispatch** vs their
  stringly-typed fail-open snake_case default — their default is the exact
  exit-0-then-cargo-fail class; the enum *is* the registry.
- **`render_type : IrType → DResult` closed enum, no `"String"` default** vs
  their `TypeRenderer` `_ -> "String"` — the renderer is total by construction
  (contingent on #45 keeping loose types out of lowering).
- **First-class `IrType` opaque variants + structural `Box<IrType>` msg** vs
  their `{M}`-placeholder string substitution + `collectRenderedTVars`
  re-derivation.
- **Exact-key fail-loud record resolution** (`CompilerBug` on miss) vs their
  best-superset-widen-or-`"String"`.
- **Explicit ctor field-count `CompilerBug` guard** in case emission, which the
  reference lacks.
- **Fail-closed refusal of refutable arg patterns** vs their contained runtime
  `panic!` — soundness ("no panic from well-typed Sky") outranks the
  completeness they gain.
- **Anchored manifest surgery with fail-loud `CompilerBug`** on every anchor
  miss — matches their CrateSpecs `error`-on-missing posture; only the version
  literals need to move to an SSOT.
- **Entire runtime fork is a strict superset** — every module equal-or-larger
  with the reference's logic plus security/correctness/soundness hardening
  (env-lock, decimal/money rounding + caps, json indent-clamp + error-propagate,
  URL redaction + SSRF-default-on, fail-closed auth, jwt boundary + optional
  claims, saturating cache counters, Go-parity regex split, CSP/log-control
  scrub). Do NOT cargo-cult reference runtime code back in.
- **Numeric-parity unit tests** (`decimal_parity.rs` / `money_parity.rs` /
  `regex_parity.rs`) with Go expected values inline — the reference has none;
  these lock the silent-divergence classes and run without a Go oracle.
- **Sweep runs in CI by default** (night-gate OFF by default) — better
  CI-correctness posture than their night-gated default.

---

## Open decisions

- **RESOLVED (item 27):** `stringify.rs` float sci-notation threshold — the two
  forks disagreed (exp≥21 vs exp≥6). Ours is correct: a Go 1.26.2 oracle re-probe
  (`fmt %v` ≡ `strconv.FormatFloat(f,'g',-1,64)`) shows a flat exp≥6 cut with no
  exponent-21 behaviour (`1000000 → "1e+06"`, `1e15 → "1e+15"`, `1e20 → "1e+20"`,
  `999999 → "999999"`). The reference's 21 is the diverging value. Pinned by
  `stringify.rs::float_go_v_parity` + `string.rs::ff_go_g_threshold_is_six_not_twentyone`,
  each proven RED under a temporary 21-flip.
- **OPEN (item 11):** whether to add superset-widening as a fallback to
  `record_struct_by_key`. Only warranted if a row-narrowed subset shape is
  observed reaching `render_record_use`; ipê's type-directed lowering may
  already deliver full shapes, making it moot. Do not add speculatively — verify
  with a failing example first, and never fall back to `"String"`.
