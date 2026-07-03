# Implementation Plan — Graceful diagnostic for a let-bound app-entry cfg (task #48)

No prior design doc exists for this item; this plan does the design-then-plan in
one pass, then turns it into a mechanical, TDD, task-by-task sequence. Every
anchor below was re-verified against HEAD (`691e275`).

Reference capability: **Sky** (`../sky`) accepts a let-bound app cfg because its
Go backend synthesises struct fields from the record's *type*, not from the
literal at the call site. ipê's Rust backend currently requires the cfg (and, for
`Webview.app`, its nested `window`/`size`) to be an **inline literal**, because
the emit stage reads the literal's field expressions directly. That difference is
a milestone boundary of the Rust backend — not a defect in the reference. Until
non-literal cfg lowering lands, the correct behaviour is a **clear, spanned,
fail-closed diagnostic**, not an internal-compiler-error.

---

## Goal

Replace an internal-compiler-error (`Diagnostic::CompilerBug`, rendered `SKY-I0001`
"the compiler is broken", no source span) with a clear user-facing
`SKY-L0119` "not-yet-supported" diagnostic — carrying the offending source span —
when an app-entry cfg is written as a **let-bound variable** (or any non-record
expression) instead of an inline record literal. Three surfaces:

1. **Top-level cfg** of `Live.app` / `Tui.app` / `Tui.program` / `Webview.app`
   — e.g. `let cfg = { … } in Live.app cfg`.
2. **`Webview.app`'s nested `window` field** — e.g.
   `Webview.app { …, window = win }` where `win` is let-bound.
3. **`Webview.app`'s nested `window.size`** — e.g.
   `window = { title = "X", size = dims }` where `dims` is let-bound.

`Live.appRouted` is already gated cleanly at lower (`SKY-L0118`,
`Feature::RoutedLiveApp`, `lower.rs` ≈ 2591) — it is **out of scope** here and
needs no change; the task title lists it only as a sibling app-entry.

### Current behaviour (verified against HEAD)

- **Surface 1 (top-level let-bound cfg).** `lower_call`'s app-entry intercept
  (`lower.rs:2543-2598`) matches the kernel, then for a non-`Record` arg falls
  into `_ => self.lower_expr(arg0)` (lines 2558 and 2583). `lower_expr` runs
  `reject_function_through_type_var` (`lower.rs:2202`): the cfg var's region type
  is a record embedding the `init`/`update`/`view`/`subscriptions` functions, so
  it returns `SKY-L0107` ("function value in a record field not supported yet") —
  a **misleading** message (the user's problem is "inline your cfg", not
  first-class functions). If the solver left no region entry for that var span,
  no gate fires, an `Expr::Var` reaches emit, and `emit_{live,tui,webview}_call`'s
  `let Expr::Record(fields) = cfg_e else { CompilerBug }`
  (`emit_live.rs:66-73`, `emit_tui.rs:66-73` & `88-95`, `emit_webview.rs:67-74`)
  fires an **ICE**.
- **Surface 2/3 (Webview nested).** The cfg IS an inline literal, so the
  `canon::Expr_::Record` arm calls `lower_app_cfg_record` (`lower.rs:2510-2522`),
  which lowers each field via `lower_expr`. A let-bound `window`/`size` value has
  a plain-record / plain-tuple region type (no embedded function), so no gate
  fires; it lowers to `Expr::Var`. Emit then hits the G4 gates
  `let Expr::Record(win_fields) = window_e else { CompilerBug }`
  (`emit_webview.rs:118-126`) or
  `let Expr::Tuple(size_elems) = size_e else { CompilerBug }`
  (`emit_webview.rs:132-139`) — a hard **ICE** for well-typed Sky.

### Target behaviour

All three surfaces produce `SKY-L0119` at the offending span **during lowering**.
The three emit-stage `else { CompilerBug }` guards are retained verbatim as
**defensive invariants** (defence-in-depth: they become unreachable-by-
construction, exactly like `emit_live.rs`'s `LiveAppRouted` arm which documents
"gated at lower (SKY-L0118)"). Their doc comments are updated to cite the new
`SKY-L0119` lower gate.

## Architecture

Two-crate change, one direction of data flow (diagnostics → lower → emit-doc):

1. **Diagnostics** (`crates/sky_diagnostics`) — add one closed `Feature` variant
   `LetBoundAppCfg` and its `SKY-L0119` code across the four exhaustive tables
   (`feature_code`, `feature_label`, `title`, `explain_page`) + the `ALL` test
   corpus + the `lib.rs` re-export + a new `explain/SKY-L0119.md` page. The enum
   arms are exhaustive (no `_` wildcard), so a missing arm is a **compile error**
   — the fail-closed guarantee for adding a feature.
2. **Lower** (`crates/sky_lower/src/lower.rs`) — a single private helper
   `lower_app_entry_cfg(peek, arg0)` that both intercept arms call. It:
   - rejects a non-`Record` arg with `SKY-L0119` at `arg0.span` (Surface 1);
   - for a `WebviewApp` kernel, validates the nested `window`/`size` shape on the
     **canon** fields (which carry spans) before delegating to
     `lower_app_cfg_record` (Surfaces 2/3).
   The emit-stage detection is not moved — it stays as a defensive `CompilerBug`.
3. **Emit-doc** (`crates/sky_backend_rust/src/emit_{live,tui,webview}.rs`) —
   documentation-only: update the guard comments to name the `SKY-L0119` gate and
   an end-to-end regression test in `crates/skyc/tests/` proving the ICE is gone.

## Tech Stack

Rust (workspace crates `sky_diagnostics`, `sky_lower`, `skyc`). `cargo test -p
<crate>` per crate. Lower-stage unit tests build canon ASTs directly via the
existing harness in `crates/sky_lower/tests/unsupported.rs`
(`run` / `run_with_regions` / `assert_unsupported`). End-to-end negative test via
`skyc::build` (`crates/skyc/src/lib.rs:182`) which returns
`CliError::Pipeline { diag: Diagnostic, .. }` (`lib.rs:128-131`) — assert
`diag.code() == SKY_L0119` with **no cargo build** of the emitted project.

## Global Constraints

**PRINCIPLES order — apply in this priority when any step forces a trade-off:**
1. **Security** — no new attack surface. (This item narrows behaviour; it opens
   no untrusted-input path. Obligation: introduce none.)
2. **Correctness** — no well-typed program that compiled before now fails, and no
   program that should fail now compiles. The only behaviour change is:
   *ICE/`SKY-L0107` on a non-literal app cfg* → *`SKY-L0119` on the same input*.
   Every inline-literal cfg (Live/Tui/Webview happy path) stays byte-identical —
   guarded by re-running the existing `webview_e2e` / `tui_e2e` suites.
3. **Soundness** — the diagnostic path never panics and never reaches the emit
   `CompilerBug` from well-typed source. The retained emit guards stay
   fail-closed (`CompilerBug`, never `unwrap`/`_ =>` silent-accept).
4. **Efficiency** — the helper is one extra canon-field walk on the app-entry
   call only (bounded by the cfg field count). No hot-path cost.
5. **Completeness** — cover all three surfaces + all four app kernels in one pass.
6. **Readability** — one shared helper over two duplicated fallbacks.

**Two fundamental rules (non-negotiable):**
- **PARSE, DON'T VALIDATE.** The helper consumes the canon `Expr` and returns
  either a lowered IR `Expr` *or* a spanned `SKY-L0119` — it does not lower first
  and re-inspect. The Webview shape is checked once on canon (where spans live),
  before the IR (which drops spans) exists.
- **MAKE INVALID STATES UNREPRESENTABLE.** `Feature` is a closed enum with
  exhaustive matches (no `_` arm) — adding `LetBoundAppCfg` forces every table to
  handle it or fail to compile. The emit stage's "cfg must be a record" invariant
  is preserved by construction: once lower rejects every non-literal cfg, the
  emit `Expr::Record` destructure cannot fail on well-typed input.

## Parallel-safety / file-overlap

- **Registry migration (Phase B/E — `#45`/`50109f3`/`691e275`)** edits
  `lower.rs`'s `lower_callee` (line 3538) and the kernel-scheme table, plus
  `constrain.rs` and `sky_kernels`. **This plan** edits `lower.rs` only in
  `lower_call`'s app-entry intercept (≈ 2543-2598) and adds a private helper +
  `lower_app_cfg_record` (≈ 2510). **Different functions, same file** → low but
  non-zero conflict risk. Sequence after the registry migration lands, or rebase
  the ≈40-line intercept diff. This plan does **not** touch `lower_callee`, the
  scheme table, `constrain.rs`, or `sky_kernels`.
- **`#49` TCO** adds two `sky_ir` variants + edits `lower.rs` (`lower_def`,
  new `analyze_tail_recursion`/`rewrite_tail_calls`) and `emit_expr.rs`. **No
  overlap with `sky_ir` or `emit_expr.rs` here**; the only shared file is
  `lower.rs`, and TCO's hooks are in `lower_def` + new fns, disjoint from the
  app-entry intercept. Independent.
- **Diagnostics crate** — this plan appends one `Feature` variant + one `SKY-L0119`
  code. If `#49`/registry also add a code concurrently, the `ALL` array and the
  `taxonomy_has_*_codes` count assertion are the merge points; bump the count to
  match the union. Each new code owns disjoint match arms otherwise.

---

## Task 1 — Add `Feature::LetBoundAppCfg` + `SKY-L0119` to the diagnostics crate

**Files (all under `crates/sky_diagnostics/`):**
- `src/diagnostic.rs` — `Feature` enum (add variant) + `feature_code` map arm
- `src/code.rs` — `SKY_L0119` const, `title` arm, `explain_page` arm, `ALL` corpus, count assertion
- `src/render.rs` — `feature_label` arm
- `src/lib.rs` — re-export `SKY_L0119`
- `explain/SKY-L0119.md` — new explain page (NEW FILE)

**Interfaces**

Consumes: nothing new (extends closed enums + `const fn` tables).

Produces:
```rust
// diagnostic.rs — new closed-enum variant (append after RoutedLiveApp, ≈ line 544)
pub enum Feature {
    // …
    RoutedLiveApp,
    /// The cfg record for an app entry point (`Live.app` / `Tui.app` /
    /// `Tui.program` / `Webview.app`) — or, for `Webview.app`, its nested
    /// `window` record and `window.size` tuple — was written as a let-bound
    /// variable (or any non-record expression) rather than an inline record
    /// literal. The Rust backend reads the cfg's field expressions directly at
    /// the call site to emit the runtime entry call, so a non-literal cfg has no
    /// fields to read. Inline the record until non-literal cfg lowering lands.
    /// [SKY-L0119]
    LetBoundAppCfg,
}

// diagnostic.rs — feature_code (≈ line 843), add arm before the closing `}`
Feature::LetBoundAppCfg => SKY_L0119,

// code.rs — const (after SKY_L0118, ≈ line 190)
/// an app-entry cfg must be an inline record literal, not a let-bound variable
pub const SKY_L0119: Code = Code("SKY-L0119");

// code.rs — title (after the SKY_L0118 arm, ≈ line 297)
SKY_L0119 => "app entry cfg must be an inline record literal",

// code.rs — explain_page (after the SKY_L0118 arm, ≈ line 419)
SKY_L0119 => Some(include_str!("../explain/SKY-L0119.md")),

// render.rs — feature_label (after the RoutedLiveApp arm, ≈ line 607)
Feature::LetBoundAppCfg => {
    "the cfg for an app entry point (`Live.app` / `Tui.app` / `Tui.program` / \
     `Webview.app`), and for `Webview.app` its nested `window` record and \
     `window.size` tuple, must be written inline as a record/tuple literal, \
     not a let-bound variable [feature: let-bound-app-cfg]"
}

// lib.rs — re-export (append to the L-code line, ≈ line 15)
//   …, SKY_L0117, SKY_L0118, SKY_L0119, SKY_L0200, …
```

**Steps**

1. Write the failing test — bump the corpus + count in `code.rs`'s `tests`
   module. Edit the `ALL` array (≈ line 414) to append `SKY_L0119` after
   `SKY_L0118`, and change both count assertions from `75` to `76`
   (`taxonomy_has_seventy_five_codes` body `assert_eq!(ALL.len(), 75)` → `76`, and
   `codes_are_distinct_and_well_formed`'s `assert_eq!(seen.len(), 75)` → `76`).
   Rename the test fn `taxonomy_has_seventy_five_codes` → `taxonomy_has_seventy_six_codes`.
2. Run it — fails to compile (const `SKY_L0119` undefined, `title`/`explain_page`
   non-exhaustive is not the failure yet — the undefined const is):
   ```bash
   cargo test -p sky_diagnostics 2>&1 | tail -20
   # expected: error[E0425]: cannot find value `SKY_L0119` in this scope
   ```
3. Minimal impl — add the const, `title` arm, `explain_page` arm in `code.rs`;
   the `Feature` variant + `feature_code` arm in `diagnostic.rs`; the
   `feature_label` arm in `render.rs`; the `lib.rs` re-export; and create
   `explain/SKY-L0119.md`. The page MUST satisfy
   `every_code_has_a_conforming_explain_page`: line 1 exactly
   `# SKY-L0119: app entry cfg must be an inline record literal`, and ≥ 3
   ```` ```sky ```` fences. Draft:
   ````markdown
   # SKY-L0119: app entry cfg must be an inline record literal

   You gave an app entry point (`Live.app`, `Tui.app`, `Tui.program`, or
   `Webview.app`) a cfg that is not an inline record literal — a let-bound
   variable, a function result, or a piped value. The Rust backend reads the
   cfg's fields (`init`, `update`, `view`, `subscriptions`, …) directly from the
   record you write at the call site, so it needs the literal there, not a name
   that stands for it.

   `[feature: let-bound-app-cfg]`

   This trips the gate:

   ```sky
   main =
       let cfg =
               { init = init
               , update = update
               , view = view
               , subscriptions = subscriptions
               }
       in
       Live.app cfg
   ```

   Write the record inline instead:

   ```sky
   main =
       Live.app
           { init = init
           , update = update
           , view = view
           , subscriptions = subscriptions
           }
   ```

   The same applies to `Webview.app`'s nested `window` record and its `size`
   tuple — both must be inline literals:

   ```sky
   main =
       Webview.app
           { init = init
           , update = update
           , view = view
           , subscriptions = subscriptions
           , window = { title = "My App", size = ( 800, 600 ) }
           }
           |> Task.run
   ```

   Non-literal cfg lowering is tracked and will land in a future milestone.
   ````
4. Run it — passes:
   ```bash
   cargo test -p sky_diagnostics 2>&1 | tail -20
   # expected: test result: ok. N passed; 0 failed
   ```
5. Commit: `diagnostics: add SKY-L0119 for a let-bound app-entry cfg`.

---

## Task 2 — Reject a non-literal app cfg at lower with `SKY-L0119`

**Files:**
- `crates/sky_lower/src/lower.rs` — new `lower_app_entry_cfg` helper + wire both
  intercept arms + Webview `window`/`size` canon validation
- `crates/sky_lower/tests/unsupported.rs` — two new regression tests

**Interfaces**

Consumes:
```rust
// existing, verified at HEAD:
fn lower_app_cfg_record(&self, fields: &[(Symbol, canon::Expr)]) -> DResult<Expr>   // lower.rs:2510
const fn unsupported(span: Span, feature: Feature) -> Diagnostic                    // lower.rs:661
fn resolve(&self, sym: Symbol) -> DResult<&str>                                     // used throughout lower.rs
// canon shapes (sky_canon::ast): Expr_::Record(Vec<(Symbol, canon::Expr)>),
//                                Expr_::Tuple(Vec<canon::Expr>), Located<Expr_> with `.span`
// sky_ir::{Callee, KernelFn::{LiveApp, TuiApp, TuiProgram, WebviewApp}}
```

Produces:
```rust
/// Lower the single cfg argument of an app-entry kernel, fail-closed on any
/// non-literal shape.
///
/// The Rust backend emits the runtime entry call by reading the cfg record's
/// field expressions directly (see `emit_{live,tui,webview}_call`), so the cfg
/// MUST be an inline `canon::Expr_::Record`. A let-bound / piped / call-result
/// cfg has no literal fields to read and is rejected here with `SKY-L0119`
/// (`Feature::LetBoundAppCfg`) at the argument's span — never allowed to reach
/// emit, where it would fire a spanless `CompilerBug`.
///
/// For `Webview.app`, the nested `window` field must itself be an inline record
/// literal and its `size` field an inline 2-tuple literal (the G4 emit gates).
/// Those are validated here on the canon fields (which carry spans) so a
/// let-bound `window`/`size` gets `SKY-L0119` at the offending span, not an ICE.
fn lower_app_entry_cfg(&self, peek: &Callee, arg0: &canon::Expr) -> DResult<Expr> {
    let canon::Expr_::Record(fields) = &arg0.value else {
        return Err(unsupported(arg0.span, Feature::LetBoundAppCfg));
    };
    if matches!(peek, Callee::Kernel(KernelFn::WebviewApp)) {
        self.reject_non_literal_webview_window(fields)?;
    }
    self.lower_app_cfg_record(fields)
}

/// Webview `window` must be an inline record and `window.size` an inline tuple.
/// Checked on canon (spanned) fields; a present-but-non-literal shape is
/// `SKY-L0119` at that value's span. A MISSING window/size is left untouched —
/// the constrain scheme enforces the 5-field shape, so absence is a genuine
/// compiler bug handled fail-closed by emit's `lookup_field`.
fn reject_non_literal_webview_window(
    &self,
    fields: &[(Symbol, canon::Expr)],
) -> DResult<()> {
    for (name, value) in fields {
        if self.resolve(*name)? == "window" {
            let canon::Expr_::Record(win_fields) = &value.value else {
                return Err(unsupported(value.span, Feature::LetBoundAppCfg));
            };
            for (wname, wvalue) in win_fields {
                if self.resolve(*wname)? == "size"
                    && !matches!(&wvalue.value, canon::Expr_::Tuple(_))
                {
                    return Err(unsupported(wvalue.span, Feature::LetBoundAppCfg));
                }
            }
        }
    }
    Ok(())
}
```

**Wiring** — replace the two duplicated `match &arg0.value { Record => …, _ =>
lower_expr }` blocks in `lower_call`'s intercept with a single call each:

```rust
// LiveApp arm (lower.rs:2547-2565): replace lines 2552-2559 (the `let lowered_cfg
// = match … {}`) with:
let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;

// Tui/Webview arm (lower.rs:2577-2590): replace lines 2581-2584 likewise with:
let lowered_cfg = self.lower_app_entry_cfg(&peek, arg0)?;
```

Both arms keep their surrounding `if let Some(arg0) = args.first() { … return
Ok(Expr::Call { callee: peek, args: vec![lowered_cfg] }); }` frame. Note
`lower_app_entry_cfg` borrows `peek` (`&Callee`); the subsequent
`Expr::Call { callee: peek, … }` moves it — order the helper call **before** the
move (as written).

**Steps**

1. Write the failing tests in `crates/sky_lower/tests/unsupported.rs`. Add
   `SKY_L0119` to the top-of-file `use sky_diagnostics::{…}` list. Model on the
   existing kernel-call tests (`unsupported.rs:360-410`, which build a
   `canon::Expr_::VarKernel { id: None, module, name }` callee — `module =
   intern("Live")`, `name = intern("app")` resolves to `KernelFn::LiveApp` via
   `lower_callee`, `lower.rs:3538`).

   Test A — top-level let-bound cfg:
   ```rust
   #[test]
   fn let_bound_live_app_cfg_is_unsupported() -> DResult<()> {
       // `Live.app cfg` where `cfg` is a plain local var (not a record literal)
       // must lower to SKY-L0119 at the argument span — never an ICE, never the
       // misleading SKY-L0107 first-class-function message.
       let mut i = Interner::new();
       let main = i.intern("main")?;
       let live = i.intern("Live")?;
       let app = i.intern("app")?;
       let cfg = i.intern("cfg")?;
       let callee = Located::new(
           Span::new(10, 18),
           canon::Expr_::VarKernel { id: None, module: live, name: app },
       );
       let arg_span = Span::new(19, 22);
       let arg = Located::new(arg_span, canon::Expr_::VarLocal(cfg));
       let body = Located::new(
           Span::new(10, 22),
           canon::Expr_::Call(Box::new(callee), vec![arg]),
       );
       let def = canon::Def::Typed {
           home: vec![],
           name: Located::new(Span::new(0, 4), main),
           free_vars: Vec::new(),
           patterns: Vec::new(),
           body,
           ty: con_int(&mut i)?, // body type is irrelevant to the intercept
       };
       assert_unsupported(
           run(Vec::new(), vec![def], BTreeMap::new(), &mut i),
           Feature::LetBoundAppCfg,
           SKY_L0119,
           arg_span,
       );
       Ok(())
   }
   ```
   Test B — Webview let-bound `window`:
   ```rust
   #[test]
   fn let_bound_webview_window_is_unsupported() -> DResult<()> {
       // `Webview.app { …, window = win }` where `win` is a local var must lower
       // to SKY-L0119 at the window value span, not an emit-stage CompilerBug.
       let mut i = Interner::new();
       let main = i.intern("main")?;
       let webview = i.intern("Webview")?;
       let app = i.intern("app")?;
       let init = i.intern("init")?;
       let update = i.intern("update")?;
       let view = i.intern("view")?;
       let subs = i.intern("subscriptions")?;
       let window = i.intern("window")?;
       let win = i.intern("win")?;
       let placeholder = |span| Located::new(span, canon::Expr_::VarLocal(init));
       let win_span = Span::new(90, 93);
       let fields = vec![
           (init, placeholder(Span::new(30, 34))),
           (update, placeholder(Span::new(40, 46))),
           (view, placeholder(Span::new(50, 54))),
           (subs, placeholder(Span::new(60, 73))),
           (window, Located::new(win_span, canon::Expr_::VarLocal(win))),
       ];
       let cfg = Located::new(Span::new(25, 95), canon::Expr_::Record(fields));
       let callee = Located::new(
           Span::new(10, 21),
           canon::Expr_::VarKernel { id: None, module: webview, name: app },
       );
       let body = Located::new(
           Span::new(10, 95),
           canon::Expr_::Call(Box::new(callee), vec![cfg]),
       );
       let def = canon::Def::Typed {
           home: vec![],
           name: Located::new(Span::new(0, 4), main),
           free_vars: Vec::new(),
           patterns: Vec::new(),
           body,
           ty: con_int(&mut i)?,
       };
       assert_unsupported(
           run(Vec::new(), vec![def], BTreeMap::new(), &mut i),
           Feature::LetBoundAppCfg,
           SKY_L0119,
           win_span,
       );
       Ok(())
   }
   ```
   > Design note resolved: the function-typed cfg fields are stubbed as
   > `VarLocal(init)` with **no region entries** (the `run` harness passes an
   > empty `regions` map, `unsupported.rs:29-37`). `lower_app_cfg_record` calls
   > `lower_expr` per field, which for a bare `VarLocal` with no region type does
   > NOT trip `reject_function_through_type_var` — but Test B's `window` check
   > fires **before** any field is lowered, so the field stubs never matter. For
   > Test A the arg is rejected before `lower_app_cfg_record` is reached at all.
   > If a future harness change makes the stubs get lowered, replace them with
   > `canon::Expr_::Unit`.

2. Run — fails: Test A currently returns `SKY-L0107` (or Ok), Test B currently
   Ok/ICE-at-emit (lower alone succeeds, so `assert_unsupported`'s `res.is_err()`
   fails):
   ```bash
   cargo test -p sky_lower --test unsupported let_bound 2>&1 | tail -25
   # expected: 2 failed (code mismatch / expected err got ok)
   ```
3. Minimal impl — add `lower_app_entry_cfg` + `reject_non_literal_webview_window`
   as private methods on `Lowerer` (next to `lower_app_cfg_record`, ≈ line 2522),
   and rewire the two intercept arms to call `self.lower_app_entry_cfg(&peek,
   arg0)?`. Add `Feature`/`KernelFn` to imports if not already in scope (they are:
   `lower.rs` already uses both).
4. Run — passes:
   ```bash
   cargo test -p sky_lower 2>&1 | tail -25
   # expected: test result: ok. (incl. the two new + all existing unsupported-gate tests)
   ```
5. Commit: `lower: gate a non-literal app-entry cfg with SKY-L0119`.

---

## Task 3 — Retain emit guards as defensive invariants + end-to-end regression

**Files:**
- `crates/sky_backend_rust/src/emit_live.rs` — doc comment update (guard kept)
- `crates/sky_backend_rust/src/emit_tui.rs` — doc comment update (guards kept)
- `crates/sky_backend_rust/src/emit_webview.rs` — doc comment update (guards kept)
- `crates/skyc/tests/webview_e2e.rs` — one end-to-end negative test (NEW test fn)

**Interfaces**

Consumes:
```rust
skyc::build(entry: &Path, out_dir: &Path, runtime_dir: &Path) -> Result<(), CliError>  // skyc/src/lib.rs:182
// CliError::Pipeline { file: PathBuf, src: String, diag: Diagnostic }                 // lib.rs:128-131
sky_diagnostics::SKY_L0119   // from Task 1
skyc::resolve_runtime() -> Result<PathBuf, _>   // used by compile_and_build, webview_e2e.rs:138
```

Produces: no code-behaviour change in the emit crate — the three
`else { CompilerBug }` guards (`emit_live.rs:66-73`, `emit_tui.rs:66-73` &
`88-95`, `emit_webview.rs:67-74`, `118-126`, `132-139`) stay **verbatim** as
unreachable-by-construction defensive checks. Only their doc comments change, e.g.
in `emit_webview.rs`'s `emit_webview_app_inner` header (≈ line 87) and at each
guard: append "— unreachable for well-typed source: a non-literal `window`/`size`
is rejected at lower with `SKY-L0119` (`Feature::LetBoundAppCfg`); this guard is a
defensive invariant, mirroring the `LiveAppRouted` precedent."

**Steps**

1. Write the failing end-to-end test in `crates/skyc/tests/webview_e2e.rs`. It
   compiles a Sky.Webview source whose `window` is let-bound and asserts
   `skyc::build` returns `CliError::Pipeline` whose `diag.code() == SKY_L0119` —
   **no cargo build** (so it runs fast and needs no wry/tao toolchain):
   ```rust
   #[test]
   fn let_bound_webview_window_is_sky_l0119_not_ice() {
       let src = r#"module Main exposing (main)

   import Sky.Core.Prelude exposing (..)
   import Std.Webview as Webview
   import Std.Ui as Ui

   type Msg = Noop

   init _ = ( 0, Cmd.none )
   update _ m = ( m, Cmd.none )
   view _ = Ui.layout [] (Ui.text "hi")
   subscriptions _ = Sub.none

   main =
       let win = { title = "X", size = ( 800, 600 ) } in
       Webview.app
           { init = init
           , update = update
           , view = view
           , subscriptions = subscriptions
           , window = win
           }
           |> Task.run
   "#;
       let dir = std::env::temp_dir().join("l0119_webview_window_sky");
       let _ = std::fs::remove_dir_all(&dir);
       std::fs::create_dir_all(&dir).unwrap();
       let entry = dir.join("Main.sky");
       std::fs::write(&entry, src).unwrap();
       let out = std::env::temp_dir().join("l0119_webview_window_out");
       let _ = std::fs::remove_dir_all(&out);
       let runtime = skyc::resolve_runtime().expect("runtime available");
       let err = skyc::build(&entry, &out, &runtime)
           .expect_err("a let-bound window must be rejected, not compiled");
       match err {
           skyc::CliError::Pipeline { diag, .. } => {
               assert_eq!(
                   diag.code(),
                   sky_diagnostics::SKY_L0119,
                   "expected SKY-L0119, got {diag:?}"
               );
           }
           other => panic!("expected a Pipeline diagnostic, got {other:?}"),
       }
   }
   ```
   > Verify before running: (a) `skyc::CliError` and its `Pipeline` variant are
   > `pub` (they are — `lib.rs:116,128`); (b) `Diagnostic::code()` is `pub`
   > (`diagnostic.rs:694`); (c) `resolve_runtime` is `pub` (used at
   > `webview_e2e.rs:138`). If the exact source above trips an *earlier*
   > diagnostic (e.g. a stdlib import the M-subset lacks), reduce it to the
   > minimal shape that reaches lower — the load-bearing part is `window = win`
   > (let-bound) inside an otherwise-inline `Webview.app` cfg. The parallel
   > Task-2 unit test (Test B) already covers the lower gate in isolation, so this
   > e2e test is the ICE-is-gone belt-and-braces check, not the primary proof.
2. Run — fails today: the let-bound `window` reaches emit and returns
   `CliError::Pipeline { diag: CompilerBug }`, so `diag.code()` is `SKY-I0001`,
   not `SKY-L0119` (with Task 2 already merged this passes — run Task 3 test
   against a pre-Task-2 checkout to see the red, or assert the message text):
   ```bash
   cargo test -p skyc --test webview_e2e let_bound_webview_window 2>&1 | tail -20
   # pre-Task-2: assertion failed: left SKY-I0001, right SKY-L0119
   ```
3. Minimal impl — update the three emit files' guard doc comments (no logic
   change). The behaviour fix already landed in Task 2; this task documents the
   invariant and locks it with the e2e regression.
4. Run — passes, and confirm no happy-path regressions:
   ```bash
   cargo test -p skyc --test webview_e2e 2>&1 | tail -20   # incl. webview_counter_build_only
   cargo test -p skyc --test tui_e2e 2>&1 | tail -20
   # expected: ok — inline-literal Live/Tui/Webview cfgs still compile + build
   ```
5. Commit: `emit: document SKY-L0119 lower gate; regress let-bound webview window`.

---

## Verification (whole-item, before declaring done)

```bash
cargo test -p sky_diagnostics 2>&1 | tail -5   # SKY-L0119 corpus + conforming explain page
cargo test -p sky_lower       2>&1 | tail -5   # two new gates + all existing unsupported tests green
cargo test -p skyc            2>&1 | tail -5   # e2e negative + webview/tui happy-path unchanged
cargo clippy --workspace --all-targets 2>&1 | tail -5   # no new lints (exhaustive matches, no `_`)
```

Expected: all green; no `CompilerBug`/`SKY-I0001` reachable from a well-typed
let-bound app cfg; every inline-literal app cfg byte-identical to HEAD.

## Spec ambiguities resolved (to make this mechanical)

1. **Reject at lower, not emit.** The emit stage operates on `sky_ir::Expr`, which
   carries no source span — a rejection there is necessarily a spanless
   `CompilerBug`. The only place with the offending span is lower (canon `Expr`
   has `.span`). So the gate lives in `lower_call`'s intercept; the emit guards
   stay as defensive invariants.
2. **Current top-level behaviour is `SKY-L0107` (mislabelled), not always an ICE.**
   A let-bound top-level cfg usually hits `reject_function_through_type_var` →
   `SKY-L0107` (a misleading "first-class functions" message); it ICEs only when
   the solver left no region entry. Either way the new gate fires first and
   produces the correct `SKY-L0119`.
3. **Keep vs delete the emit `CompilerBug` guards.** Kept as defence-in-depth,
   matching the established `LiveAppRouted` precedent (`emit_live.rs:83-88`, an
   arm documented "gated at lower (SKY-L0118)"). Deleting them would trade a
   fail-closed invariant for a silent-accept risk — forbidden by MAKE INVALID
   STATES UNREPRESENTABLE.
4. **`Live.appRouted` out of scope.** Already cleanly gated (`SKY-L0118`); the
   task lists it only as a sibling app-entry. No change.
5. **One shared helper over two duplicated fallbacks.** Both intercept arms
   (LiveApp; Tui/Webview) had an identical `_ => lower_expr(arg0)` fallback;
   folding them into `lower_app_entry_cfg` centralises the gate and shrinks the
   diff that overlaps the registry migration.
6. **Webview `window`/`size` validated on canon, single walk, present-only.** The
   check runs on the spanned canon fields before lowering; a *missing* window/size
   is left to the constrain-scheme + emit `lookup_field` invariant (genuine
   `CompilerBug` if it ever fires), since presence is structurally guaranteed.
