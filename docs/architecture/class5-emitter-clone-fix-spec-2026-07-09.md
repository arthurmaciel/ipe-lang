# Class 5 mechanical items — emitter clone/borrow discipline fix spec (2026-07-09)

> Scope: per `docs/architecture/campaign-classification-2026-07-09.md` Class 5,
> the four MECHANICAL items only — **#99**, **#125**, **#142**, and AUD-09's
> unconditional field-access `.clone()` (O(n²) efficiency finding). **#53**
> (typed-token-AST rewrite) is Class 5's GUARDIAN-DESIGN-REQUIRED tail,
> explicitly out of scope here and scheduled last per the campaign's
> recommended processing order.
>
> All four fixes touch `crates/sky_backend_rust/src/emit_expr.rs`. Class 10's
> **#156** (`Ui.onSubmit` / `HtmlEventShape::Raw` `dyn Any` erasure,
> `emit_expr.rs:4116-4131`) ALSO touches this file. **Sequencing: land Class
> 5's four items first, then #156**, or rebase #156 after — the two land
> orders are file-disjoint in practice (#156's touch point is the
> `Ui.onSubmit`/raw-event-shape call emitter; the four items here touch
> the `Expr::Access` arm, `render_pat`/`emit_ctor_arm_pat`/`emit_whole_arm_head`,
> and `lower_let`/`lower_case`'s destructure dispatch) but both are large
> diffs against the same ~6800-line file, so whichever lands second should
> rebase, not merge blind.
>
> Per the AUD-04 lesson already applied in this file (commit `7cfc1e5`) and
> reinforced by `docs/architecture/prior-art-runtime-rust-2026-07-09.md` Part 2
> §2 (the ancestor Haskell backend never emits Rust text and then rewrites it
> — clone/Copy status is decided in an `EmitCtx` BEFORE printing): every fix
> below operates on the **IR tree**, never on rendered Rust source text. None
> of the four introduces a new textual-surgery pass.

---

## 0. Reading list already done (do not re-read before implementing)

- `crates/sky_backend_rust/src/emit_expr.rs` lines 1-250 (AUD-04's
  `free_vars` / `collect_free_vars` / `clone_free_target` / `substitute_var`
  / `pat_bound_symbols` / `pat_binds_target`) — the precedent every new
  helper below mirrors.
- `crates/sky_backend_rust/src/emit_expr.rs` lines 5473-5700 (`emit_match`,
  `emit_match_scrutinee`, `emit_arm_head`, `emit_whole_arm_head`,
  `emit_tuple_arm_head`, `emit_ctor_arm_pat`) — the match-arm rendering
  pipeline #99 patches.
- `crates/sky_backend_rust/src/emit_expr.rs` lines 5936-6138 (`render_pat`,
  `pat_contains_alias`, `emit_binding_stmts`/`push_binding_stmts`) — the #96
  irrefutable-alias fix #99 reuses.
- `crates/sky_backend_rust/src/emit_expr.rs` lines 5227-5245 (`Expr::Access`)
  and lines 6653-6754 (`emit_func`'s generic-clause / `Clone`-bound
  injection) — the #142/AUD-09 site.
- `crates/sky_lower/src/lower.rs` lines 9218-9274 (`lower_payload_pat`),
  9526-9652 (`lower_let`, incl. the #89 Decoder-thunk `PVar` arm),
  9654-9883 (`lower_case`, `lower_arm_pat`) — the #99/#125 lowering sites.
- `crates/sky_lower/src/lib.rs` (the `lower()` entry point — where
  `SymbolPools` is built) — the fresh-symbol-pool wiring point #125 extends.
- `docs/architecture/seal-jsondecp-design.md` §5.C — the #89 Decoder-thunk
  design #125 generalizes.
- `crates/skyc/tests/golden_aud04_emit_expr_ir_capture.rs` +
  `tests/golden/aud04_string_literal/Main.sky` — the golden-test harness
  shape every new regression test below follows.

---

## 1. #99 — refutable match-arm alias over non-Copy payload double-moves

### 1.1 Root cause

`case m of Just ((a, b) as w) -> <uses a, b, AND w>` emits (today,
unchanged since #96) via `render_pat`'s `Pat::Alias` arm
(`emit_expr.rs:5970-5974`):

```rust
Pat::Alias(inner, name) => {
    let name = ctx.emit_ident(*name)?;
    let inner = render_pat(ctx, inner)?;
    Ok(format!("{name} @ {inner}"))
}
```

producing the Rust arm head `Just(w @ (a, b)) => { ... }`. The doc comment
directly above this arm (`emit_expr.rs:5958-5969`) claims this spelling is
"correct ONLY in a by-REF / refutable MATCH-ARM position, where default
binding modes make the sub-bindings borrows so no move occurs" — but this
claim is **only true when the scrutinee itself is matched by reference**
(STR mode: `(scrut).as_str()`, or LIST mode: `(scrut).as_slice()` —
`emit_match_scrutinee`, `emit_expr.rs:5610-5620`). For the default WHOLE
mode (a plain `Ctor`/`Tuple` scrutinee — `Option`/`Result`/user-enum
matches), `emit_match_scrutinee` emits the scrutinee **by value**
(`scrut_expr` verbatim, no `&`), so Rust's default binding mode there is
MOVE, not ref. `w @ (a, b)` over a non-`Copy` `(String, String)` binds `w`
to the WHOLE tuple by move AND binds `a`/`b` by moving them out of the SAME
storage — a genuine partial-move conflict. The pattern itself compiles;
the break surfaces as E0382 ("use of moved value: `w`" / "borrow of
partially moved value") the moment the arm body reads `w` after having read
`a`/`b` — matching the backlog's exact repro (`use a, b, w`).

This is NOT what #96 fixed. #96 (commit `4986069`) fixed the exact same
`name @ inner` double-move for **by-value irrefutable** binder positions
(`let`, function/lambda params, single-arm tuple `case` — all lowered to
`Expr::Destructure`) by intercepting every alias in `emit_binding_stmts` /
`push_binding_stmts` (`emit_expr.rs:6076-6138`) BEFORE it reaches
`render_pat`. #96's own commit message says "render_pat's match-arm Alias
arm untouched (by-ref, byte-identical)" and files the refutable-match-arm
case as #99 — i.e. the by-ref assumption for match arms was never actually
verified against the WHOLE-mode (non-str/non-list) scrutinee case, and it
is false there.

STR-mode and LIST-mode arms are **unaffected** — their scrutinee really is
a reference (`&str` / `&[T]`), so `name @ inner` is genuinely a by-ref
binding there (no move). Do not touch `list_binder_rebinds` /
`str_binder_rebinds` / their `Pat::Alias` handling (`emit_expr.rs:5778,
5823-5826`) — those paths are correct as-is.

### 1.2 Scope boundary (what this fix handles vs. fails closed)

`Pat::Alias(inner, name)`'s `inner` can itself be an arbitrary refutable
pattern per the grammar (`ir.rs:1327-1331`), including a NESTED constructor
(`Just ((Ok x) as w)`) that genuinely needs Rust-level dispatch (which
variant matched) to select the arm. Reconstructing `name`'s whole value
from parts bound by `inner`'s own pattern is unsound in general (a
`Pat::Wildcard` sub-position discards data `name` needs to keep), so the
sound general fix is: **whenever `inner` requires NO runtime dispatch at
all** (i.e. `inner` is a `Var`/`Wildcard` leaf, or any nesting of
`Tuple`/`Record`/`Alias` over such leaves — no `Ctor`/literal/`Slice`
anywhere in it), rewrite the pattern the way #96 already proved sound for
irrefutable binders. Whenever `inner` DOES need dispatch (contains a
`Ctor`/literal/`Slice` anywhere), reject at LOWERING time with a clean
diagnostic rather than attempt a fundamentally different (and much larger)
by-reference-matching redesign — that redesign is exactly the shape of
work `docs/architecture/prior-art-runtime-rust-2026-07-09.md` Part 2 §2
warns against solving with another one-off patch; it is also not needed for
the concrete #99 repro (`(a, b) as w`, a `Tuple` of `Var`s — dispatch-free).

This mirrors the existing precedent at `lower_payload_pat`
(`lower.rs:9264-9267`) and `lower_destructure_pat`
(`lower.rs:9309-9313`), both of which already fail closed
(`Feature::NestedPayloadPatterns`, SKY-L0112) on a DIFFERENT nested-shape
gap (record-under-payload) rather than silently mis-lowering it.

### 1.3 New shared predicate — `sky_ir::is_dispatch_free`

Add to `crates/sky_ir/src/ir.rs`, in the same "Pat predicate" family as the
existing `is_irrefutable` / `is_list_shaped` / `is_ctor_headed` /
`is_product_shaped` (`ir.rs:1384-1470`) — but note **do not reuse
`is_irrefutable`**: that predicate treats `Pat::Tuple(_)` / `Pat::Record(_)`
as unconditionally `false` (refutable) by design, because it answers a
DIFFERENT question ("is this arm head a genuine catch-all for
`Match::new_flat`'s backstop" — a top-level Tuple/Record head at that point
would already have been routed to the `Destructure` path in `lower_case`,
so it never legitimately reaches that check as a real catch-all). #99 needs
a distinct question: "does ANY node in this pattern require Rust to perform
a runtime discriminant check to decide whether the pattern matches" — a
`Tuple`/`Record` of vars needs NO such check (Rust's tuple/struct patterns
never fail structurally; only enum-variant and literal patterns can).

```rust
/// Whether this pattern needs NO Rust-level runtime dispatch (discriminant
/// check) anywhere in its shape — i.e. it contains no [`Pat::Ctor`],
/// literal leaf, or [`Pat::Slice`] at any depth. A `Var` / `Wildcard` leaf,
/// or any nesting of `Tuple` / `Record` / `Alias` over such leaves, is
/// dispatch-free: Rust's tuple/struct/binding patterns always succeed
/// structurally, so matching them costs no discriminant check and (unlike
/// [`is_irrefutable`], which answers a different question about catch-all
/// arms) is safe to evaluate at ANY nesting depth, including inside another
/// constructor's payload.
///
/// Used by the Rust backend (`sky_backend_rust::render_arm_pat_alias_safe`)
/// to decide whether an `Alias` node's inner shape can be safely rebuilt
/// from a CLONE of the alias binder (the #96/#99 by-value alias-split
/// fix) — safe exactly when reconstructing every leaf `inner` binds is
/// possible without having discarded any data, which holds only when
/// `inner` never needed a runtime check to get there. The [`sky_lower`]
/// lowerer calls this too, to reject an alias over a dispatch-needing
/// inner pattern in a REFUTABLE match-arm position (SKY-L0127) rather than
/// let it reach the backend, where honoring it soundly would require
/// matching the scrutinee by reference throughout (a materially larger
/// redesign, not attempted here).
#[must_use]
pub fn is_dispatch_free(pat: &Pat) -> bool {
    match pat {
        Pat::Wildcard | Pat::Var(_) => true,
        Pat::Alias(inner, _) => is_dispatch_free(inner),
        Pat::Tuple(elems) => elems.iter().all(is_dispatch_free),
        Pat::Record(fields) => fields.iter().all(|(_, p)| is_dispatch_free(p)),
        Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) | Pat::Ctor { .. } | Pat::Slice { .. } => false,
    }
}
```

Re-export it from `crates/sky_ir/src/lib.rs`'s `pub use ir::{...}` list
(alongside `ir_type_is_derivable` / `ir_type_is_serde`, the existing
precedent for a cross-crate-shared `Pat`/`IrType` predicate — both
`sky_lower` and `sky_backend_rust` already import that pair the same way):

```rust
pub use ir::{
    ir_type_is_derivable, ir_type_is_serde, is_dispatch_free, Arm, BinOp, BoundSet, Callee,
    EnumDef, Expr, Func, FuncId, HtmlEventShape, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef, UiCtor, UiPlain, Variant,
};
```

Add a unit test in `sky_ir::ir`'s existing test module (next to
`is_irrefutable`'s own tests, `ir.rs:2035-2039`):

```rust
#[test]
fn dispatch_free_over_tuple_of_vars_and_wildcards() {
    let x = /* fresh Symbol */;
    assert!(is_dispatch_free(&Pat::Tuple(vec![Pat::Var(x), Pat::Wildcard])));
}

#[test]
fn dispatch_free_false_over_nested_ctor_or_literal() {
    let x = /* fresh Symbol */;
    assert!(!is_dispatch_free(&Pat::Tuple(vec![Pat::Int(0), Pat::Var(x)])));
    // a Ctor nested inside a Tuple inside an Alias must still fail:
    assert!(!is_dispatch_free(&Pat::Alias(
        Box::new(Pat::Tuple(vec![Pat::Ctor { home: ModPath(vec![]), ty: x, variant: x, args: vec![] }])),
        x,
    )));
}
```

### 1.4 Lowering-time fail-closed gate (new SKY-L0127)

Two lowering sites currently lower `PAlias` unconditionally, both need the
SAME gate added (they are independent code paths, not delegating to each
other):

- `Lowerer::lower_arm_pat`'s `PAlias` arm (`lower.rs:9839-9842`) — the
  TOP-level arm-head alias (`case m of (x as w) -> …` shapes that don't
  themselves head a tuple/record — those route elsewhere).
- `Lowerer::lower_payload_pat`'s `PAlias` arm (`lower.rs:9228-9231`) — a
  NESTED alias inside a constructor payload or tuple element (the #99
  repro's actual path: `Just ((a, b) as w)`'s alias is a CTOR PAYLOAD
  element, lowered via `lower_payload_pat`, not `lower_arm_pat`).

`lower_destructure_pat`'s own `PAlias` arm (`lower.rs:9305-9308`) does
**not** need a new gate — it already transitively fails closed: its
sibling arms reject `PCtor`/literal leaves BEFORE the alias case would ever
recurse into them (`lower.rs:9298-9302`, `Feature::TuplePatternMatch`,
pre-existing), so an alias-over-refutable-inner there is already unreachable.

Add a shared helper (private to `sky_lower::lower`, next to `unsupported`):

```rust
/// Reject an `as`-alias whose inner pattern needs Rust-level dispatch
/// (a nested constructor / literal / slice anywhere) in a REFUTABLE
/// match-arm position. Honoring it soundly would require matching the
/// scrutinee by reference throughout the arm (STR/LIST mode's existing
/// approach) rather than by value — out of scope for #99's fix; see
/// `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` §1.2.
/// The common irrefutable-inner shape (`(a, b) as w`, a `Var`/`Wildcard`
/// leaf, or any `Tuple`/`Record` nesting of those) passes through
/// unchanged — the backend's #99 fix handles it.
fn gate_alias_inner_dispatch_free(inner: &Pat, span: Span) -> DResult<()> {
    if sky_ir::is_dispatch_free(inner) {
        Ok(())
    } else {
        Err(unsupported(span, Feature::AliasOverRefutablePayload))
    }
}
```

Wire it in at both sites — after lowering `inner`, before constructing the
`Pat::Alias`:

```rust
// lower_arm_pat, replacing lines 9839-9842:
canon::Pattern_::PAlias(inner, name) => {
    let lowered_inner = Self::lower_arm_pat(inner)?;
    Self::gate_alias_inner_dispatch_free(&lowered_inner, p.span)?;
    Ok(Pat::Alias(Box::new(lowered_inner), name.value))
}
```

```rust
// lower_payload_pat, replacing lines 9228-9231:
canon::Pattern_::PAlias(inner, name) => {
    let lowered_inner = Self::lower_payload_pat(inner)?;
    Self::gate_alias_inner_dispatch_free(&lowered_inner, p.span)?;
    Ok(Pat::Alias(Box::new(lowered_inner), name.value))
}
```

(`gate_alias_inner_dispatch_free` takes a shared, non-`&self` fn since
neither call site needs `&self` state for it; make it a free fn or an
associated fn on `Lowerer` matching the style already used for
`is_destructure_head` — an associated fn with no `&self` receiver,
`lower.rs:9359-9365`.)

### 1.5 New diagnostic — SKY-L0127

`crates/sky_diagnostics/src/diagnostic.rs`: add a `Feature` variant next to
`NestedPayloadPatterns` (`diagnostic.rs:525-533`):

```rust
/// An `as`-alias in a REFUTABLE match-arm position whose inner pattern
/// itself needs Rust-level runtime dispatch (a nested constructor,
/// literal, or list/cons pattern anywhere) — `Just ((Ok x) as w)`. The
/// common alias shape (`(a, b) as w`, dispatch-free) is fully supported;
/// only a dispatch-NEEDING inner is gated here, because honoring it
/// soundly by value would double-move a non-`Copy` payload (the exact
/// #99 bug) and honoring it by reference would require matching the
/// whole arm by reference — a materially larger redesign. [SKY-L0127]
AliasOverRefutablePayload,
```

`crates/sky_diagnostics/src/code.rs`:

```rust
pub const SKY_L0127: Code = Code("SKY-L0127");
// … message table:
SKY_L0127 => "alias over a dispatch-needing nested pattern not supported yet",
// … explain-doc table:
SKY_L0127 => Some(include_str!("../explain/SKY-L0127.md")),
// … ALL_CODES list: append SKY_L0127.
```

`crates/sky_diagnostics/src/diagnostic.rs`'s `feature_code`-style match
(the one mapping `Feature::NestedPayloadPatterns => SKY_L0112` etc.,
around `diagnostic.rs:998`): add `Feature::AliasOverRefutablePayload =>
SKY_L0127`.

`crates/sky_diagnostics/explain/SKY-L0127.md` (new file, mirror
`SKY-L0112.md`'s shape): explain the rule in one paragraph + a "supported
vs not" code pair (`(a, b) as w` — supported; `(Ok x) as w` nested inside
another constructor arm needing its own dispatch — not yet), and point at
this spec doc for the architecture rationale.

### 1.6 Backend fix — alias-safe by-value arm-pattern rendering

New functions in `emit_expr.rs`, placed near `render_pat` /
`emit_binding_stmts` (after `render_pat`, before `pat_contains_alias` at
`emit_expr.rs:6020`):

```rust
/// Does this pattern contain a [`Pat::Alias`] ANYWHERE in its shape —
/// unlike [`pat_contains_alias`] (which only recurses into `Tuple`,
/// because it exists solely for the by-VALUE Destructure grammar where
/// `Ctor`/`Record`/`Slice` never legitimately appear), this ALSO recurses
/// into `Ctor` args, `Record` fields, and `Slice` prefix/rest — all of
/// which DO appear in a refutable match-arm pattern.
fn pat_contains_alias_in_arm(pat: &Pat) -> bool {
    match pat {
        Pat::Alias(..) => true,
        Pat::Tuple(elems) => elems.iter().any(pat_contains_alias_in_arm),
        Pat::Ctor { args, .. } => args.iter().any(pat_contains_alias_in_arm),
        Pat::Record(fields) => fields.iter().any(|(_, p)| pat_contains_alias_in_arm(p)),
        Pat::Slice { prefix, rest } => {
            prefix.iter().any(pat_contains_alias_in_arm)
                || rest.as_deref().is_some_and(pat_contains_alias_in_arm)
        }
        Pat::Var(_) | Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => false,
    }
}

/// Render a BY-VALUE (whole-scrutinee, non-str, non-list) match-arm
/// sub-pattern, routing any [`Pat::Alias`] through the SAME "bind the
/// whole, destructure the inner shape from a CLONE" strategy #96's
/// [`emit_binding_stmts`] already proved sound for irrefutable Destructure
/// positions — because in THIS context the scrutinee is matched BY VALUE
/// (never `&str`/`&[T]`), so `render_pat`'s `name @ inner` spelling
/// (sound only under a by-REF default binding mode) would double-move
/// `name` and `inner`'s own bindings for any non-`Copy` payload (#99).
///
/// A subtree with no alias anywhere renders through the existing,
/// byte-identical [`render_pat`] (fast path — zero behavior change for
/// the overwhelmingly common alias-free case). `prelude` accumulates the
/// `let` statements that re-derive every aliased binder; the caller
/// splices it into the SAME prelude slot `emit_ctor_arm_pat`'s cyclic-
/// self-edge unboxing already uses (`unbox_lines`) or
/// `emit_whole_arm_head`'s `prelude` return.
fn render_arm_pat_alias_safe(
    ctx: &EmitCtx,
    pat: &Pat,
    counter: &mut usize,
    prelude: &mut String,
) -> DResult<String> {
    if !pat_contains_alias_in_arm(pat) {
        return render_pat(ctx, pat);
    }
    match pat {
        Pat::Alias(inner, _name) => {
            // SKY-L0127 (§1.4/1.5) guarantees `inner` is dispatch-free
            // by the time lowering succeeds; fail closed rather than
            // silently mis-emit if that invariant is ever violated —
            // never trust a backend-side "this can't happen" silently.
            if !sky_ir::is_dispatch_free(inner) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::render_arm_pat_alias_safe",
                    detail: "alias over a dispatch-needing inner pattern \
                             reached the backend; SKY-L0127 should have \
                             rejected this at lowering".to_owned(),
                });
            }
            let temp = format!("__sky_arm_alias_{}", *counter);
            *counter += 1;
            // `emit_binding_stmts` (the #96 machinery, `emit_expr.rs:6076`)
            // already handles `Pat::Alias` exactly this way: `let <name> =
            // <src>; let <inner-pattern> = <name>.clone();` — reuse it
            // verbatim, passing the WHOLE alias node and the fresh temp
            // as `src`.
            for stmt in emit_binding_stmts(ctx, pat, &temp)? {
                prelude.push_str(&stmt);
                prelude.push(' ');
            }
            Ok(temp)
        }
        Pat::Tuple(elems) => {
            let mut subs = Vec::with_capacity(elems.len());
            for e in elems {
                subs.push(render_arm_pat_alias_safe(ctx, e, counter, prelude)?);
            }
            Ok(format!("({})", subs.join(", ")))
        }
        Pat::Ctor { home, ty, variant, args } => {
            let path = match ctx.builtin_runtime_enum(home, *ty) {
                Some(runtime) => format!("{runtime}::{}", ctx.emit_ident(*variant)?),
                None => format!("{}::{}", ctx.enum_name(home, *ty)?, ctx.emit_ident(*variant)?),
            };
            if args.is_empty() {
                Ok(path)
            } else {
                let mut subs = Vec::with_capacity(args.len());
                for a in args {
                    subs.push(render_arm_pat_alias_safe(ctx, a, counter, prelude)?);
                }
                Ok(format!("{path}({})", subs.join(", ")))
            }
        }
        Pat::Record(fields) => {
            // Mirror `render_record_pat`'s struct-name resolution
            // (`emit_expr.rs:6168-6199`) but recurse sub-patterns through
            // this alias-safe renderer instead of the plain one.
            let mut key = Vec::with_capacity(fields.len());
            for (sym, _) in fields {
                key.push(ctx.resolve_ident(*sym)?.to_owned());
            }
            let struct_name = ctx.record_name_for_literal(&key)?.to_owned();
            let mut parts = Vec::with_capacity(fields.len());
            for (sym, sub) in fields {
                let field_ident = ctx.emit_ident(*sym)?;
                if let Pat::Var(var) = sub && ctx.emit_ident(*var)? == field_ident {
                    parts.push(field_ident);
                } else {
                    let rendered = render_arm_pat_alias_safe(ctx, sub, counter, prelude)?;
                    parts.push(format!("{field_ident}: {rendered}"));
                }
            }
            if parts.is_empty() {
                Ok(format!("{struct_name} {{ .. }}"))
            } else {
                Ok(format!("{struct_name} {{ {}, .. }}", parts.join(", ")))
            }
        }
        // A `Slice` carrying a nested alias reaches LIST mode, which
        // matches by reference and is unaffected by #99 (§1.1) — but this
        // by-VALUE renderer is never invoked from that path (see the
        // call-site wiring below), so reaching here is an internal
        // invariant violation, not a real user program.
        Pat::Slice { .. } => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::render_arm_pat_alias_safe",
            detail: "Pat::Slice reached the by-value alias-safe renderer; \
                     list-mode arms must route through render_pat directly"
                .to_owned(),
        }),
        Pat::Var(_) | Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {
            render_pat(ctx, pat)
        }
    }
}
```

Wire it into the TWO by-value call sites (leave STR-mode / LIST-mode
untouched — they still call `render_pat` directly, correctly):

1. `emit_ctor_arm_pat` (`emit_expr.rs:5696-5753`) — replace the per-arg
   `sub_pats.push(render_pat(ctx, sub)?);` (line 5737) with a call through
   `render_arm_pat_alias_safe`, threading a `counter`/`prelude` pair that
   feeds into the SAME `unbox_lines` string the function already returns:

   ```rust
   let mut alias_counter: usize = 0;
   let mut alias_prelude = String::new();
   // … in the per-arg loop, replace `render_pat(ctx, sub)?` with:
   sub_pats.push(render_arm_pat_alias_safe(ctx, sub, &mut alias_counter, &mut alias_prelude)?);
   // … after the loop, before `Ok((format!("{path}({})", …), unbox_lines))`:
   unbox_lines.push_str(&alias_prelude);
   ```

2. `emit_whole_arm_head`'s non-Ctor branch (`emit_expr.rs:5640-5658`) —
   replace `Ok((render_pat(ctx, pat)?, prelude))` with the alias-safe
   render, folding its own generated prelude into the existing `prelude`
   string (which already carries the str/list rebind text):

   ```rust
   } else {
       let mut prelude = if str_mode {
           str_binder_rebinds(ctx, pat)?
       } else if list_mode {
           list_binder_rebinds(ctx, pat)?
       } else {
           String::new()
       };
       let rendered = if str_mode || list_mode {
           // STR/LIST mode: scrutinee IS a reference; `render_pat`'s
           // `name @ inner` is sound here (§1.1) — unchanged.
           render_pat(ctx, pat)?
       } else {
           let mut alias_counter: usize = 0;
           let rendered = render_arm_pat_alias_safe(ctx, pat, &mut alias_counter, &mut prelude)?;
           rendered
       };
       Ok((rendered, prelude))
   }
   ```

`emit_tuple_arm_head` delegates per-column to `emit_whole_arm_head`
(`emit_expr.rs:5679`), so fixing (2) covers tuple-arm columns for free — no
separate change needed there.

### 1.7 Regression tests

New fixture `tests/golden/i99_alias_tuple_match_arm/Main.sky`:

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Sky.Core.String as String
import Std.Log exposing (println)

describe : Maybe (String, String) -> String
describe m =
    case m of
        Just ((a, b) as w) ->
            a ++ "|" ++ b ++ "|" ++ Debug.tuplePair w
        Nothing ->
            "none"

main =
    println (describe (Just ("hello", "world")))
```

(Swap `Debug.tuplePair` for a plain concatenation of `Tuple.first w ++
Tuple.second w` if `Debug.tuplePair` doesn't exist in the stdlib surface —
the fixture only needs to read `w` AFTER `a`/`b`, over a non-`Copy`
payload; any expression doing that is sufficient. Expected stdout:
`hello|world|(hello, world)` or equivalent.)

New test file `crates/skyc/tests/golden_i99_alias_match_arm.rs`, following
`golden_aud04_emit_expr_ir_capture.rs`'s exact shape (`assert_skyc_ok` +
`assert_e2e_output` under `SKY_E2E=1`):

- `i99_alias_tuple_match_arm_compiles_and_runs` — `assert_skyc_ok` (proves
  the pre-fix E0382 is gone) + `assert_e2e_output` on the expected string
  (proves `w`, `a`, `b` all read correctly, not just "compiles").

New fixture `tests/golden/i99_alias_ctor_rejected/Main.sky` (RED-side
control — proves the fail-closed gate fires, not silently over-broad):

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)

type Wrap = Wrap (Maybe Int)

describe : Wrap -> String
describe w =
    case w of
        Wrap ((Just x) as inner) -> "got"
        Wrap Nothing -> "none"

main =
    println (describe (Wrap (Just 1)))
```

Test asserting `skyc::build` on this fixture returns
`Err(Diagnostic::Lower { msg: LowerError::Unsupported(Feature::AliasOverRefutablePayload), .. })`
(or the `skyc` CLI-level equivalent — check the exact `DResult` surface
`skyc::build` exposes; if it's already collapsed to a rendered diagnostic
string by the time it reaches the test harness, assert the string contains
`SKY-L0127`). This is a compile-only unit/integration test, no `SKY_E2E`
needed (there is nothing to run).

Unit-level regression (fast, in `sky_ir::ir`'s own test module, §1.3): the
two `is_dispatch_free` tests above.

---

## 2. #125 — Decoder thunk coverage: tuple-destructure + record-field binders

### 2.1 Root cause

`#89`'s Fix C (commit `fe7561f`, `docs/architecture/seal-jsondecp-design.md`
§5.C) closed the `let d = <decoder> in … decodeString d j1 … decodeString d
j2 …` double-move (`Decoder` is `!Clone`, `decode_from_json_string` consumes
it by value) by wrapping the RHS in a zero-arg thunk lambda and rewriting
every `Var(name)` read to `Apply(Var(name), [])`. The gate lives in
`Lowerer::lower_let` (`lower.rs:9526-9652`) and matches **only**
`canon::Pattern_::PVar(name)` (`lower.rs:9531`):

```rust
canon::Pattern_::PVar(name) => {
    if let Some(dec_ty) = self.decoder_ir_type(b.body.span) {
        // … thunk-wrap + rewrite_var_to_apply …
    } else {
        // … T5 multi-use-clone for CloneOk types …
    }
}
```

Every OTHER binding-pattern shape (`PTuple`, `PRecord`, `PAlias` wrapping
either) falls through to the catch-all `_` arm (`lower.rs:9644-9648`):

```rust
_ => Expr::Destructure {
    binder: self.lower_binder_pat(&b.pat, &b.body)?,
    value: Box::new(value),
    body: Box::new(acc),
},
```

which has NO Decoder-awareness whatsoever. So `let (d1, d2) = buildBoth ()
in JsonDec.decodeString d1 j1 |> … ; JsonDec.decodeString d1 j2 |> …`
(`d1` reused, bound via tuple destructure) or `let { userDecoder } =
decoders in decodeString userDecoder j1 … decodeString userDecoder j2`
(bound via record-field destructure) emit a plain `let (d1, d2) = value;`
/ `let Decoders_R { user_decoder, .. } = value;` — ordinary Rust move
semantics — and any REUSE of the Decoder-typed component double-moves at
`cargo build`: a loud, real E0382, exactly matching the backlog's wording.

`lower_case`'s single-arm product `case` (`lower.rs:9667-9675`) has the
IDENTICAL gap — it builds a plain `Expr::Destructure` with no
Decoder-awareness either:

```rust
if branches.len() == 1 {
    let binder = self.lower_binder_pat(&first.pat, scrut)?;
    return Ok(Expr::Destructure {
        binder,
        value: Box::new(scrutinee),
        body: Box::new(self.lower_expr(&first.body)?),
    });
}
```

`case (d1, d2) of (a, b) -> …` where `a`/`b` are Decoder-typed and reused
hits the same E0382. This shares 100% of the fix machinery below (same
call shape: a binder pattern, a value, a body) — fix both in the same
change; do not leave the `case` sibling for a follow-up (a genuine
duplicate of the SAME already-diagnosed gap left unfixed would violate the
no-deferral rule).

### 2.2 Fix design — generalize the thunk to any destructure binder

The `PVar` thunk rewrites EVERY free `Var(name)` read to `Apply(Var(name),
[])`, because there is exactly one bound name and it directly names the
whole (re-buildable) Decoder value. A `Tuple`/`Record` binder introduces
MULTIPLE names from ONE value in ONE Rust `let`/`match` binding, so the
same "rewrite each read to a thunk call" trick needs each read to also
RE-PROJECT the right component out of a fresh call — not just re-call a
zero-arg thunk. Do this WITHOUT adding a new `Expr` node (no tuple-index
accessor expression exists in the IR — only `Expr::Access` for NAMED record
fields) by reusing `Expr::Destructure` itself as the projector: each read
site re-runs a FRESH, masked copy of the ORIGINAL pattern (every OTHER
bound name replaced by `Pat::Wildcard`) against a fresh thunk call, keeping
only the one name being read.

```text
let __sky_destr_thunk = move || -> <value's IrType> { <value> };
-- every free read of `d1` becomes:
{ let (d1, _) = (__sky_destr_thunk)(); d1 }
-- every free read of `d2` becomes:
{ let (_, d2) = (__sky_destr_thunk)(); d2 }
```

This is sound for the SAME reason #89 Fix C is sound (§5.C's own
justification, reused verbatim): Decoders are pure values — building one
runs no effects — so re-evaluating the whole tuple/record construction per
read is semantics-neutral, construction-cost-only. It generalizes uniformly
over `Tuple`/`Record`/`Alias`-of-either, and it never needs a NEW `IrType`
projection because `Expr::Destructure` already exists precisely to bind a
pattern from a value.

**Gate: apply this whenever the binder's aggregate value type is-or-contains
`IrType::Decoder` ANYWHERE** (not just when the specific read name is
Decoder-typed) — i.e. thunk the WHOLE destructure uniformly (all bound
names in that one pattern, decoder-typed or not, get the masked-redestructure
treatment), mirroring #89 Fix C's own "unconditional, no use-count gate"
decision (`seal-jsondecp-design.md` §5.C bullet 1) for the SAME reason:
mixing "some names bound directly, others via thunk" in ONE Rust binding
statement is not representable without either two different `let` forms or
literal tuple/field projection (which the IR can't express) — uniform
treatment is the simpler, still-sound choice, at the (accepted, matching
precedent) cost of a byte-diff on any co-bound non-Decoder sibling name.
When NO name's type is/contains `IrType::Decoder`, the whole pattern falls
through UNCHANGED to the existing plain `Expr::Destructure` path — byte-identical
for the overwhelmingly common non-Decoder case.

### 2.3 New predicate — `ir_type_contains_decoder`

Add to `sky_lower::lower` (private, next to `decoder_ir_type`,
`lower.rs:6162-6180`ish):

```rust
/// Does `ty` structurally contain `IrType::Decoder` anywhere (itself, or
/// nested inside a `Tuple`/`Record`/`Maybe`/`Result`/`List`)? Gates #125's
/// destructure-thunk rewrite: a `Tuple`/`Record` binder whose aggregate
/// type contains a Decoder anywhere needs the WHOLE destructure thunked
/// (§2.2) — a Decoder nested inside e.g. `Maybe (Decoder a)` is out of
/// today's realistic reach (Decoders aren't optional in practice) but the
/// predicate stays structurally total rather than special-cased to Tuple/
/// Record only, matching `ir_type_contains_task`'s existing shape in the
/// Rust backend (`emit_expr.rs`, AUD-04).
fn ir_type_contains_decoder(ty: &IrType) -> bool {
    match ty {
        IrType::Decoder(_) => true,
        IrType::Tuple(elems) => elems.iter().any(ir_type_contains_decoder),
        IrType::Record(fields) => fields.values().any(ir_type_contains_decoder),
        IrType::Maybe(inner) | IrType::List(inner) => ir_type_contains_decoder(inner),
        IrType::Result(e, a) => ir_type_contains_decoder(e) || ir_type_contains_decoder(a),
        _ => false,
    }
}
```

### 2.4 New predicate — `pat_bound_symbols` (lower-side) + `mask_pattern_except`

`sky_lower::lower` needs its OWN copy of "collect every symbol a pattern
binds" (the backend already has one, `emit_expr.rs:68-105`
`pat_bound_symbols` — per the established crate-boundary convention
documented right there, `sky_lower` and `sky_backend_rust` each keep their
own copy rather than share one, since IR flows one-way lower → backend):

```rust
/// Collect every symbol `pat` binds (recursively) into `out`. Local twin
/// of `sky_backend_rust::pat_bound_symbols` (same shape, same crate-
/// boundary rationale — see that fn's doc comment).
fn pat_bound_symbols(pat: &Pat, out: &mut BTreeSet<Symbol>) {
    match pat {
        Pat::Var(s) => { out.insert(*s); }
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {}
        Pat::Alias(inner, s) => { out.insert(*s); pat_bound_symbols(inner, out); }
        Pat::Ctor { args, .. } | Pat::Tuple(args) => {
            for p in args { pat_bound_symbols(p, out); }
        }
        Pat::Record(fields) => {
            for (_, p) in fields { pat_bound_symbols(p, out); }
        }
        Pat::Slice { prefix, rest } => {
            for p in prefix { pat_bound_symbols(p, out); }
            if let Some(p) = rest { pat_bound_symbols(p, out); }
        }
    }
}

/// Rebuild `pat` with every bound name EXCEPT `keep` erased to
/// `Pat::Wildcard` — an `Alias`'s own name collapses to a bare
/// `Pat::Var(keep)` at that position when `keep` is the alias name itself
/// (dropping the alias wrapper entirely; a single flat name needs no `as`),
/// otherwise the alias erases and recurses into `inner` (its own name is
/// irrelevant to this masked, single-name extraction). Used to build the
/// per-read-site re-destructure pattern for #125 (§2.2) — reusing the
/// ORIGINAL pattern's shape (masked) sidesteps needing any tuple-index /
/// record-field EXPRESSION accessor in the IR, since `Expr::Destructure`
/// already exists to bind a pattern from a value.
fn mask_pattern_except(pat: &Pat, keep: Symbol) -> Pat {
    match pat {
        Pat::Var(s) => if *s == keep { Pat::Var(*s) } else { Pat::Wildcard },
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => pat.clone(),
        Pat::Alias(inner, s) => {
            if *s == keep {
                Pat::Var(*s)
            } else {
                mask_pattern_except(inner, keep)
            }
        }
        Pat::Tuple(elems) => Pat::Tuple(elems.iter().map(|p| mask_pattern_except(p, keep)).collect()),
        Pat::Record(fields) => Pat::Record(
            fields.iter().map(|(n, p)| (*n, mask_pattern_except(p, keep))).collect(),
        ),
        // `Ctor` / `Slice` never appear in a #125-eligible binder (irrefutable
        // destructure grammar forbids them — `lower_destructure_pat`'s own
        // fail-closed arms, `lower.rs:9298-9302,9314-9318`); kept total via
        // an unreachable-in-practice clone rather than a partial match.
        Pat::Ctor { .. } | Pat::Slice { .. } => pat.clone(),
    }
}
```

### 2.5 New rewrite — `rewrite_destructure_read`

```rust
/// Shadow-aware rewrite: replace every FREE `Expr::Var(target)` in `expr`
/// with a fresh, masked re-destructure of `thunk_name`'s call result — the
/// #125 generalization of `rewrite_var_to_apply` (which only ever needs a
/// bare `Apply`, since a `PVar` binder has exactly one name that directly
/// names the re-buildable value). Stops recursing into any scope that
/// rebinds `target` — identical shadow rules to `rewrite_var_to_apply`
/// (`Let`/`Destructure`/`Lambda` params/`Match` arm patterns).
fn rewrite_destructure_read(target: Symbol, root_pat: &Pat, thunk_name: Symbol, expr: Expr) -> Expr {
    match expr {
        Expr::Var(s) if s == target => Expr::Destructure {
            binder: mask_pattern_except(root_pat, target),
            value: Box::new(Expr::Apply {
                func: Box::new(Expr::Var(thunk_name)),
                args: vec![],
            }),
            body: Box::new(Expr::Var(target)),
        },
        // … every other Expr arm recurses exactly like `rewrite_var_to_apply`
        // (`lower.rs`, the #89 F2 rewrite) — same shadow-stop rules at
        // `Let`/`Destructure`/`Lambda`/`Match`. Do not hand-duplicate that
        // 50+-arm match from scratch: factor `rewrite_var_to_apply`'s body
        // into a shape parameterized over "what does a free Var(target)
        // become" (an `impl Fn(Symbol) -> Expr` closure argument), and call
        // it from both `rewrite_var_to_apply` (closure = `|s| Apply { func:
        // Var(s), args: vec![] }`) and this fn (closure = the Destructure
        // builder above) — avoids a second full-tree-walk copy-paste, and
        // keeps the two rewrites' shadow-handling provably identical instead
        // of two chances to drift.
    }
}
```

Concretely: rename the existing `rewrite_var_to_apply(target: Symbol, expr:
Expr) -> Expr` to a private `rewrite_var_free_occurrences(target: Symbol,
expr: Expr, on_hit: &impl Fn(Symbol) -> Expr) -> Expr`, replacing its
`Expr::Var(s) if s == target => Expr::Apply { func: Box::new(Expr::Var(s)),
args: vec![] }` leaf with `Expr::Var(s) if s == target => on_hit(s)`, and
add two thin wrappers:

```rust
fn rewrite_var_to_apply(target: Symbol, expr: Expr) -> Expr {
    rewrite_var_free_occurrences(target, expr, &|s| Expr::Apply {
        func: Box::new(Expr::Var(s)),
        args: vec![],
    })
}

fn rewrite_destructure_read(target: Symbol, root_pat: Pat, thunk_name: Symbol, expr: Expr) -> Expr {
    rewrite_var_free_occurrences(target, expr, &|s| Expr::Destructure {
        binder: mask_pattern_except(&root_pat, s),
        value: Box::new(Expr::Apply { func: Box::new(Expr::Var(thunk_name)), args: vec![] }),
        body: Box::new(Expr::Var(s)),
    })
}
```

This refactor is itself a pure, behavior-preserving extraction (existing
`#89` goldens must stay byte-identical) — land it as its own small
sub-commit / first step, verified against the existing `m4h_json_dec_*`
goldens before adding the new destructure path on top.

### 2.6 Wiring into `lower_let` and `lower_case`

Factor a shared helper on `Lowerer` (used by both call sites):

```rust
/// Build the `Expr` for a destructure-binder `let`/single-arm-`case`
/// binding, applying the #125 Decoder-thunk generalization when `value`'s
/// aggregate type contains `IrType::Decoder` anywhere. Falls through to a
/// plain `Expr::Destructure` (byte-identical to pre-#125 emission) when it
/// does not.
fn build_destructure_or_decoder_thunk(
    &self,
    binder: Pat,
    value: Expr,
    value_span: Span,
    body: Expr,
) -> DResult<Expr> {
    let value_ir_ty = self
        .region_ty(value_span)
        .and_then(|ty| self.ir_type_from_ty(ty, value_span).ok());
    let Some(ir_ty) = value_ir_ty.filter(ir_type_contains_decoder) else {
        return Ok(Expr::Destructure { binder, value: Box::new(value), body: Box::new(body) });
    };

    // T3 (#121)-style capture-clone rewrite on the thunk body, mirroring
    // the PVar arm exactly (lower.rs:9544-9560): the thunk has zero
    // params, so every free VarLocal in `value` is an outer capture.
    let thunk_body = {
        let captures = self.captured_locals(&[], /* the ORIGINAL un-lowered canon value expr — thread it through, see note below */);
        let mut clone_set: BTreeSet<Symbol> = BTreeSet::new();
        let mut noncl_set: BTreeSet<Symbol> = BTreeSet::new();
        for (sym, ir_ty) in captures {
            match ir_ty.as_ref().map(clone_class) {
                Some(CloneClass::CloneOk) => { clone_set.insert(sym); }
                Some(CloneClass::NonClone) => { noncl_set.insert(sym); }
                Some(CloneClass::CopyLeaf) | None => {}
            }
        }
        rewrite_captured_clones(&clone_set, &noncl_set, value_span, value, 0)?
    };
    let thunk_name = self.fresh_destructure_thunk_symbol()?; // §2.7 pool
    let thunk = Expr::Lambda { params: vec![], ret: ir_ty, body: Box::new(thunk_body) };

    let mut bound: BTreeSet<Symbol> = BTreeSet::new();
    pat_bound_symbols(&binder, &mut bound);
    let mut new_body = body;
    for name in &bound {
        new_body = rewrite_destructure_read(*name, binder.clone(), thunk_name, new_body);
    }
    Ok(Expr::Let { name: thunk_name, value: Box::new(thunk), body: Box::new(new_body) })
}
```

(Note on `captured_locals`'s second argument: the existing `PVar` arm calls
it as `self.captured_locals(&[], &b.body)` where `b.body` is the CANON
(pre-lowering) expression — `build_destructure_or_decoder_thunk` must
receive that same canon expression, not the already-lowered `value: Expr`,
so its signature should take an extra `canon_value: &canon::Expr` parameter
purely for this capture-analysis call; thread it from both call sites,
which both already have the canon expr in scope — `b.body` in `lower_let`,
`scrut` in `lower_case`.)

`lower_let`'s catch-all (`lower.rs:9644-9648`) becomes:

```rust
_ => {
    let binder = self.lower_binder_pat(&b.pat, &b.body)?;
    self.build_destructure_or_decoder_thunk(binder, value, b.body.span, acc, &b.body)?
}
```

`lower_case`'s single-arm branch (`lower.rs:9667-9675`) becomes:

```rust
if branches.len() == 1 {
    let binder = self.lower_binder_pat(&first.pat, scrut)?;
    let body = self.lower_expr(&first.body)?;
    return self.build_destructure_or_decoder_thunk(binder, scrutinee, scrut.span, body, scrut);
}
```

### 2.7 Fresh-symbol pool wiring

Mirror the EXISTING `param_binders`/`any_param_binders` pool idiom exactly
(`lower.rs:2577-2598, 2720-2832`; wired in `crates/sky_lower/src/lib.rs:70-92,
144-156`) — `Lowerer` cannot mint a fresh `Symbol` mid-lowering (it only
holds `&'a Interner`, immutable), so the pool is pre-sized by a SYNTACTIC
counting pass and consumed via a `Cell<usize>` cursor.

New counting fn in `lower.rs` (public, next to `count_destructure_param_sites`,
`lower.rs:2709-2779`):

```rust
/// Count every destructure-headed `let` binding AND single-arm product
/// `case` in the module — one pre-minted symbol needed per site for #125's
/// Decoder-thunk generalization (§2.6), REGARDLESS of whether that binding
/// ultimately turns out to be Decoder-typed (the type-dependent gate runs
/// later, once solving has completed; this pass is purely syntactic, like
/// its `count_destructure_param_sites` sibling). Over-counting is
/// harmless; under-counting fails closed as a [`bug`], never an index
/// panic.
pub fn count_destructure_thunk_sites(m: &canon::Module) -> usize {
    fn is_destructure_headed(pat: &canon::Pattern_) -> bool {
        match pat {
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_) => true,
            canon::Pattern_::PAlias(inner, _) => is_destructure_headed(&inner.value),
            _ => false,
        }
    }
    fn walk_expr(e: &canon::Expr) -> usize {
        match &e.value {
            canon::Expr_::Let(bindings, body) => {
                bindings
                    .iter()
                    .map(|b| {
                        usize::from(is_destructure_headed(&b.pat.value)) + walk_expr(&b.body)
                    })
                    .sum::<usize>()
                    + walk_expr(body)
            }
            canon::Expr_::Case(scrut, branches) => {
                let head = branches.len() == 1
                    && branches.first().is_some_and(|b| is_destructure_headed(&b.pat.value));
                usize::from(head)
                    + walk_expr(scrut)
                    + branches.iter().map(|b| walk_expr(&b.body)).sum::<usize>()
            }
            // … every other recursive arm identical to
            // `count_destructure_param_sites`'s `walk_expr` (`lower.rs:2726-2768`,
            // Lambda/Call/Binop/If/Tuple/List/Cons/Record/Access/Update, leaves 0)
            // — reuse that shape verbatim, do not hand-diverge it.
        }
    }
    m.defs
        .iter()
        .map(|d| match d {
            canon::Def::Typed { body, .. } | canon::Def::Untyped { body, .. } => walk_expr(body),
        })
        .sum()
}
```

`SymbolPools` (`lower.rs:2827-2832`) gains a field:

```rust
pub struct SymbolPools {
    pub eta_params: Vec<Symbol>,
    pub cap_params: Vec<Symbol>,
    pub param_binders: Vec<Symbol>,
    pub any_param_binders: Vec<Symbol>,
    pub destructure_thunk_binders: Vec<Symbol>, // NEW
}
```

`Lowerer` gains the matching field + cursor + accessor (mirroring
`fresh_param_binder`, `lower.rs:3010-3020`):

```rust
destructure_thunk_binders: Vec<Symbol>,
destructure_thunk_cursor: Cell<usize>,
// …
fn fresh_destructure_thunk_symbol(&self) -> DResult<Symbol> {
    let i = self.destructure_thunk_cursor.get();
    let sym = *self.destructure_thunk_binders.get(i).ok_or_else(|| {
        bug("sky_lower::fresh_destructure_thunk_symbol", "destructure-thunk-binder pool exhausted")
    })?;
    self.destructure_thunk_cursor.set(i + 1);
    Ok(sym)
}
```

`crates/sky_lower/src/lib.rs`'s `lower()` entry point gains, alongside the
existing pool mints (after `let any_param_binders = …;`, before
`let builtins = …;`):

```rust
let destructure_thunk_binders =
    interner.fresh_symbols("destr_thunk_", lower::count_destructure_thunk_sites(m))?;
```

and thread it into every `SymbolPools` construction/destructure site — all
THREE need the new field added or the struct literal/pattern fails to
compile:

1. `lib.rs`'s real `lower::SymbolPools { eta_params, cap_params,
   param_binders, any_param_binders }` literal (`lib.rs:148-153`) — add
   `destructure_thunk_binders`.
2. `Lowerer::new`'s own destructure of its `pools: SymbolPools` parameter
   (`lower.rs:2852-2857`, `let SymbolPools { eta_params, cap_params,
   param_binders, any_param_binders } = pools;`) — add
   `destructure_thunk_binders` to the pattern, then store it (alongside the
   new `destructure_thunk_cursor: Cell::new(0)`) in the constructed
   `Lowerer { … }` struct literal.
3. The empty test-only pool in `lower.rs`'s own test module
   (`lower.rs:10235-10240`, `SymbolPools { eta_params: vec![], cap_params:
   vec![], param_binders: vec![], any_param_binders: vec![] }`) — add
   `destructure_thunk_binders: vec![]`.

### 2.8 Regression tests

New fixtures (mirroring §7's R4/`m4h_json_dec_pipeline_reuse` shape from
`seal-jsondecp-design.md`, but with a tuple/record binder instead of a bare
`PVar`):

`tests/golden/i125_decoder_tuple_destructure/Main.sky` — build TWO
decoders as one tuple value, destructure, use ONE of them twice:

```elm
module Main exposing (main)

import Sky.Core.Json.Decode as JsonDec
import Sky.Core.Json.Decode.Pipeline as JsonDecP
import Std.Log exposing (println)

buildPair : () -> (JsonDec.Decoder String, JsonDec.Decoder Int)
buildPair _ =
    ( JsonDecP.required "name" JsonDec.string (JsonDec.succeed identity)
    , JsonDec.int
    )

main =
    let
        (nameDecoder, ageDecoder) = buildPair ()
        r1 = JsonDec.decodeString nameDecoder "{\"name\":\"Alice\"}"
        r2 = JsonDec.decodeString nameDecoder "{\"name\":\"Bob\"}"
    in
    case (r1, r2) of
        (Ok a, Ok b) -> println (a ++ "|" ++ b)
        _ -> println "err"
```

`tests/golden/i125_decoder_record_destructure/Main.sky` — same shape via a
record-field binder (`let { nameDecoder } = someRecordOfDecoders in …`
reused twice).

New test file `crates/skyc/tests/golden_i125_decoder_destructure_thunk.rs`,
same `assert_skyc_ok` / `assert_e2e_output` shape as
`golden_aud04_emit_expr_ir_capture.rs` / the R4 test from #89:

- `i125_decoder_tuple_destructure_reuse_compiles_and_runs` —
  `assert_skyc_ok` (proves the E0382 the pre-fix RED run recorded is gone)
  + `assert_e2e_output` on `"Alice|Bob"`.
- `i125_decoder_record_destructure_reuse_compiles_and_runs` — analogous.
- A THIRD fixture/test covering the `lower_case` single-arm sibling (§2.1):
  `case buildPair () of (nameDecoder, ageDecoder) -> …` reusing
  `nameDecoder` twice inside the arm body — proves §2.6's `lower_case`
  wiring, not just `lower_let`'s.

Non-regression guard (unit-level, no `SKY_E2E`): a fixture where NEITHER
tuple element is Decoder-typed (`let (a, b) = (1, "x") in a + a` reused)
must emit BYTE-IDENTICAL Rust to pre-#125 — add this as an existing-golden
byte-diff assertion (reuse an existing `m3b1_tuple`-family golden's `main.rs`
snapshot check) to prove the fast path (§2.2's "falls through unchanged"
claim) actually holds.

RED-first protocol (per the project's testing rules): add each fixture,
confirm the pre-fix build fails at `cargo build` with E0382 (record the
exact rustc message in the test's doc comment, mirroring
`golden_aud04_emit_expr_ir_capture.rs`'s own witness doc comments), THEN
land the fix and flip green.

---

## 3. #142 + AUD-09 unconditional field-access `.clone()` — combined fix

These are the SAME site and the SAME fix; #142 is the "restore the fast
path" follow-up filed right after #139 landed the unconditional clone,
AUD-09 independently re-flagged it as an O(n²) efficiency finding
(`emit_expr.rs:4794` in the audit's line numbering — **stale**, per the
campaign brief; current location is `emit_expr.rs:5227-5245`, confirmed by
fresh read). Land as ONE change, close both backlog lines together.

### 3.1 Root cause

`#139` (commit `829f3f6`) made `Expr::Access` emit an UNCONDITIONAL
`.clone()` (`emit_expr.rs:5227-5245`):

```rust
Expr::Access { record, field } => {
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    let field = ctx.emit_ident(*field)?;
    Ok(format!("({base}).{field}.clone()"))
}
```

justified by the doc comment as "prevents partial-move errors when the
same owner or field is accessed more than once… the Rust compiler will
elide redundant clones under standard optimisation passes." The audit
(`principles-audit-2026-07-09.md` §"efficiency — Unconditional `.clone()`
on every record-field access") correctly disputes the elision claim: rustc
does NOT elide a `.clone()` call on a heap type (`String`/`Vec`/a
synthesized struct) — that comment is aspirational, not a real guarantee.
Every heap-field read is therefore an unconditional O(field-size) deep
copy, compounding to O(n²) when a record with a heap-backed field renders
inside a per-element loop (the audit's literal example: "list-of-records
renders").

`#139`'s companion change (same commit) always injects a `Clone` bound on
every generic type parameter in `emit_func` (`emit_expr.rs:6707-6754`,
`bounds.with_clone()`), justified by the SAME "field reads emit `.clone()`"
argument.

### 3.2 Fix — type-directed Copy elision (bounded, safe scope)

The audit's suggested fix has two halves: "(a) no `.clone()` for `Copy`
fields; (b) clone heap fields only on non-last-use." **This spec implements
(a) only**, and explicitly scopes (b) OUT as a separate, larger follow-up —
see §3.5. (a) alone is a real, always-correct, immediately measurable win
(every `Int`/`Float`/`Bool`/`Char`/`Unit`/`Order`/`Decimal`/`ErrorKind`
field read in the codebase currently pays a needless `.clone()` call, which
for these types compiles to a bitwise copy anyway per the CURRENT doc
comment's own claim — so (a) removes a genuinely zero-benefit operation
with zero risk). (b) requires a real last-use / liveness analysis across
arbitrary expression trees (deciding, for a HEAP-backed field, whether THIS
particular read is provably the final read of that owner in the enclosing
scope) — a materially bigger, more failure-prone undertaking (getting
"last use" wrong in the UNSAFE direction — cloning when a move was actually
needed — is merely a missed optimization; getting it wrong in the OTHER
direction — eliding a clone that was actually needed — reopens the exact
E0382 class `#139` was fixing in the first place). Given the MECHANICAL
classification of this item, ship the safe half now; file the last-use half
as its own backlog item rather than attempt it under this same mechanical
lane's risk budget.

`Expr::Access` carries no type today (`ir.rs:1127-1131`: only `record:
Box<Self>`, `field: Symbol`) — the field's type must be threaded in at
lowering time to let the backend decide Copy-ness without guessing.

**Step 1 — `sky_ir::ir::Expr::Access` gains a `field_ty: IrType` field:**

```rust
/// A record field access `record.field`. `field_ty` is the field's own
/// solved type — carried so the Rust backend can decide, WITHOUT any
/// textual heuristic, whether the read needs a `.clone()` (a heap-backed
/// field) or can skip it (a Rust-`Copy` scalar) — see #142/AUD-09's
/// type-directed Copy-elision fix,
/// `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` §3.
Access {
    record: Box<Self>,
    field: Symbol,
    field_ty: IrType,
},
```

Update every existing construction/destructure site (all within `sky_ir`,
`sky_lower`, `sky_backend_rust` — enumerated below; none are outside these
three crates):

- `sky_ir/src/ir.rs:2089` (the crate's own unit test constructing an
  `Expr::Access`) — add a concrete `field_ty` (e.g. `IrType::Int`).
- `sky_ir/src/pretty.rs:1404` and `:1908` — the Format-module printer's
  `Expr::Access { record, field }` match arms; add `field_ty` to the
  destructure (`, field_ty` — unused in the printed output, so `_field_ty`
  is fine unless the printer should also show it for debugging; keep
  behavior byte-identical, so `_field_ty`).
- `sky_lower/src/lower.rs:928` (`rewrite_captured_clones`'s reconstruction
  arm) — add `field_ty` to both the destructure and the rebuilt literal
  (pass through unchanged: `field_ty` is not rewritten, only `record` is).
- `sky_lower/src/lower.rs:1111`, `:1316` (read-only `{ record, .. }`
  destructures in `lambda_body_refs_sym` / `count_var_uses`) — already use
  `..`, no change needed.
- `sky_lower/src/lower.rs:1485` (`rewrite_multiuse_clones`'s reconstruction
  arm) — same as the `:928` site: add `field_ty`, pass through.
- `sky_lower/src/lower.rs:5735` (`lower_expr`'s `canon::Expr_::Access` arm
  — **the one genuine "compute a fresh value" site**):

  ```rust
  canon::Expr_::Access(record, field) => {
      let field_ty = self
          .region_ty(e.span)
          .and_then(|ty| self.ir_type_from_ty(ty, e.span).ok())
          .unwrap_or(IrType::Generic(*field)); // see note below
      Ok(Expr::Access {
          record: Box::new(self.lower_expr(record)?),
          field: *field,
          field_ty,
      })
  }
  ```

  Note on the fallback: `ir_type_from_ty` can fail for a still-generic
  field type in a polymorphic function body (the same "may legitimately
  fail" situation `lower_case`'s T5 rewrite already tolerates,
  `lower.rs:9756-9758`). Do NOT fail closed here — a missing `field_ty` must
  not turn a working program into a lowering error, since the field's
  CONCRETENESS was never load-bearing before this change. Fall back to
  `IrType::Generic(*field)` (an arbitrary non-Copy-classified placeholder —
  `ir_type_is_definitely_copy`, §3.3, returns `false` for `Generic`, so the
  fallback conservatively KEEPS the `.clone()` — semantically identical to
  today's unconditional behavior for that one case, never a regression).

- `sky_backend_rust/src/emit_expr.rs:198, 336, 520` (AUD-04's `collect_free_vars`
  / `clone_free_target` / `scan_free_target_into` — the three read-only or
  reconstructing walkers) — `:198` and `:520` already destructure `{ record,
  .. }` (no change). `:336` (`clone_free_target`'s reconstruction arm)
  needs `field_ty` added to both sides, passed through unchanged (never
  rewritten by this pass — only `record` is).
- `sky_backend_rust/src/emit_expr.rs:5227` (the MAIN emission arm — see
  Step 3 below) and any other `Expr::Access` destructure inside
  `substitute_var` (grep for the 5th match `emit_expr.rs` reported earlier
  — confirm during implementation via `rg -n "Expr::Access" crates/sky_backend_rust/src/emit_expr.rs`
  and update every hit; the count at spec-writing time was 5 in this file).

**Step 2 — new predicate in `emit_expr.rs`** (backend-local; mirrors
`sky_lower::lower::clone_class`'s `CopyLeaf` classification exactly, kept
duplicated per the same crate-boundary convention as `pat_bound_symbols`):

```rust
/// Is `ty` a Rust type that is UNCONDITIONALLY `Copy` in every emission
/// this backend produces? Mirrors `sky_lower::lower::clone_class`'s
/// `CopyLeaf` classification (kept duplicated across the crate boundary
/// per this file's established convention — see `pat_bound_symbols`'s doc
/// comment, `emit_expr.rs:68-70`). Deliberately conservative: a
/// `Generic(_)` type parameter is bounded only by `Clone` (`emit_func`,
/// §3.4), NEVER `Copy`, so it must return `false` even though a caller
/// might monomorphize it to a Copy type at some call site — the backend
/// has no per-call-site visibility here. A user `Enum` also returns
/// `false`: synthesized enums derive `Clone`, not `Copy` (verify this
/// holds by grepping the enum-derive emission before shipping — if any
/// enum variant set IS given `Copy` in the future, that's an ADDITIVE
/// case to this match, not a reason to guess `true` today).
fn ir_type_is_definitely_copy(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::Int
            | IrType::Float
            | IrType::Bool
            | IrType::Char
            | IrType::Unit
            | IrType::Order
            | IrType::Decimal
            | IrType::ErrorKind
    )
}
```

Before landing, verify (via `rg` over `emit_types.rs` / the runtime crate)
that `Order`, `Decimal`, and `ErrorKind` really do `#[derive(Copy)]` in
their emitted/runtime form — `sky_lower::lower::clone_class`'s own comment
already asserts this ("`Decimal` is `#[derive(Copy)]`") for the SAME
classification; confirm the other two the same way rather than trust the
lowerer's comment blindly (cheap, one `rg` each).

**Step 3 — use it in the emission arm** (`emit_expr.rs:5227-5245`):

```rust
Expr::Access { record, field, field_ty } => {
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    let field_ident = ctx.emit_ident(*field)?;
    if ir_type_is_definitely_copy(field_ty) {
        Ok(format!("({base}).{field_ident}"))
    } else {
        Ok(format!("({base}).{field_ident}.clone()"))
    }
}
```

Update the doc comment above this arm (`emit_expr.rs:5228-5241`) to drop
the false "rustc will elide redundant clones" claim and instead describe
the type-directed elision precisely (point at this spec's §3 for the
rationale + the explicitly-deferred last-use half).

### 3.3 `emit_func`'s blanket `Clone` bound — leave unchanged (with rationale)

Do NOT narrow `emit_func`'s unconditional `Clone` bound injection
(`emit_expr.rs:6707-6754`) as part of this fix. It is NOT solely justified
by `Expr::Access` — it is ALSO relied on by: `#104`/T5's multi-use
`CloneVar` rewrites (match-arm and `let`-binding), list-mode's
`.clone()`/`.to_vec()` rebind prelude (`collect_elem_rebinds`,
`emit_expr.rs:5843-5877`), and `#96`'s `emit_binding_stmts` clone-split
(§1.6/§2 above, both of which clone a GENERIC-typed alias/tuple-element
routinely). Narrowing the bound would require auditing every one of those
call sites for whether they still need `Clone` on a given type parameter —
a materially larger, higher-risk change than "restore the borrow fast path
on Access", and NOT what #142's own wording asks for (the backlog line
targets `Expr::Access`'s "borrow fast-path", not the bound). Leave
`emit_func` untouched; note this explicitly as the scope boundary so a
future reader doesn't assume it was overlooked.

### 3.4 Regression tests

New fixture `tests/golden/i142_copy_field_no_clone/Main.sky`:

```elm
module Main exposing (main)

import Std.Log exposing (println)
import Sky.Core.String as String

type alias Counter = { count : Int, label : String }

render : Counter -> String
render c =
    -- `c.count` read twice (Copy — must NOT clone); `c.label` read twice
    -- (heap-backed String — MUST still clone, correctness unaffected).
    String.fromInt (c.count + c.count) ++ "|" ++ c.label ++ c.label

main =
    println (render { count = 3, label = "x" })
```

Expected stdout: `6|xx`.

New test file `crates/skyc/tests/golden_i142_access_copy_elision.rs`:

- `i142_copy_field_no_clone_compiles_and_runs` — `assert_skyc_ok` +
  `assert_e2e_output` on `"6|xx"` (proves correctness is unaffected by the
  elision — this is the load-bearing behavioral check).
- **Emission-level regression** (unit test, no `SKY_E2E`, following the
  §7 "Regression guards (unit-level, no E2E)" pattern from
  `seal-jsondecp-design.md`): compile the fixture through `skyc`'s
  lower+emit pipeline directly (not the CLI) and assert the emitted
  `main.rs` source contains `.count)` NOT immediately followed by
  `.clone()` (i.e. `(c).count` bare) while `.label` IS followed by
  `.clone()` — a substring/regex assertion on the generated Rust text,
  proving the Copy/non-Copy split actually took effect (not just that the
  program happens to still run correctly, which a no-op fix could also
  pass).

Non-regression guard: an existing record-heavy golden (e.g.
`tests/golden/m1_records` or `m2c_generic_records`, both touched by
`#139`'s own golden refresh) should have its `main.rs` snapshot re-diffed —
expect byte changes ONLY at `Int`/`Bool`/etc. field-access call sites
(the ones this fix un-clones), zero changes anywhere else. Record the
before/after diff in the commit message, same discipline `#139`'s own
commit used ("9 files byte-changed; 56 others byte-identical").

### 3.5 Explicitly deferred (do not attempt under this item)

- **Last-use analysis for heap-backed fields** (audit's fix-half (b)) — a
  genuine liveness pass over arbitrary expression trees to decide "is this
  the final read of this owner in this scope"; file as its own backlog
  item when picked up (not part of #142/AUD-09's mechanical scope).
- **`Arc`-backed persistent record containers** (the audit's alternative
  suggestion) — a runtime representation change (`Rc`/`Arc<RecordFields>`
  instead of an owned struct), which would need every record-construction
  and `Update` site re-examined; a much larger, cross-cutting change than
  this item's scope.

---

## 4. Cross-item interaction summary

| Item | File(s) touched | New IR/diagnostic surface | Depends on |
|---|---|---|---|
| #99 | `sky_ir::ir` (new predicate), `sky_lower::lower` (`lower_arm_pat`, `lower_payload_pat`), `sky_diagnostics` (SKY-L0127), `sky_backend_rust::emit_expr` (`render_arm_pat_alias_safe` + 2 call sites) | `sky_ir::is_dispatch_free`, `Feature::AliasOverRefutablePayload` / SKY-L0127 | #96 (`emit_binding_stmts`, reused verbatim) |
| #125 | `sky_lower::lower` (`lower_let`, `lower_case`, new `SymbolPools` field), `sky_lower::lib` (pool wiring) | none (no new diagnostic — always succeeds, generalizes an existing silent success path) | #89 (`rewrite_var_to_apply`, refactored to share its shadow-walk; `captured_locals`/`clone_class`/`rewrite_captured_clones`, reused verbatim) |
| #142 + AUD-09 | `sky_ir::ir` (`Expr::Access` gains `field_ty`), `sky_ir::pretty`, `sky_lower::lower` (5 pass-through sites), `sky_backend_rust::emit_expr` (new predicate + emission arm) | `Expr::Access::field_ty : IrType` (schema addition to an EXISTING variant — every construction/destructure site enumerated in §3.2 Step 1 must be updated; this is the CLAUDE.md §8 "New AST nodes require explicit walker arms" rule applied to a field addition, not a new node, but the same completeness discipline) | none |

None of the three items' backend changes touch the SAME lines of
`emit_expr.rs` as each other (§1 touches `render_pat`'s call sites +
`emit_ctor_arm_pat`/`emit_whole_arm_head`; §3 touches the `Expr::Access`
arm + its doc comment) — land in any order relative to each other, but
land ALL of Class 5's mechanical items (this doc) before Class 10's #156
per the campaign's stated sequencing (`campaign-classification-2026-07-09.md`
§"Recommended processing order" step 3), and rebase #156 on top if it lands
first instead.

## 5. Verification commands (run after EACH item, not just at the end)

```bash
# Workspace-wide gate — MUST stay green after every one of the 4 items:
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Per-item golden/E2E tests (compile-check, fast):
cargo test -p skyc --test golden_i99_alias_match_arm
cargo test -p skyc --test golden_i125_decoder_destructure_thunk
cargo test -p skyc --test golden_i142_access_copy_elision

# Full E2E (build + run the emitted binary, assert stdout) — bounded, per
# repo timeout rules:
SKY_E2E=1 cargo test -p skyc --test golden_i99_alias_match_arm
SKY_E2E=1 cargo test -p skyc --test golden_i125_decoder_destructure_thunk
SKY_E2E=1 cargo test -p skyc --test golden_i142_access_copy_elision

# Existing #89 goldens must stay green (proves #125's rewrite-fn refactor,
# §2.5, is behavior-preserving for the PVar case it didn't change):
SKY_E2E=1 cargo test -p skyc --test golden_m4h_json_dec

# Existing #96 goldens must stay green (proves #99 didn't touch the
# by-value/Destructure alias path it reuses, only the match-arm path):
SKY_E2E=1 cargo test -p skyc --test golden_l0105_alias_move_seal
SKY_E2E=1 cargo test -p skyc --test golden_m3b3_alias
SKY_E2E=1 cargo test -p skyc --test golden_m3b3_alias_tuple
```

Time-bound every `cargo`/emitted-binary invocation per the repo's
"test/build timeout gate" rule (CLAUDE.md §3) — none of the above should
run unbounded in CI or locally.
