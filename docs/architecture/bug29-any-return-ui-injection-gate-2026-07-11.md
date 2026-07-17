# Bug-29 — gate the `any`-return UI-msg injection on the Con name

> Status: SPEC (short). AUD-09 remaining sub-item; campaign Class 1
> (guardian-design). Source finding:
> `docs/architecture/principles-audit-2026-07-09.md` §"Bug-29 `any`-return UI
> injection matches ANY single-arg `Ty::Con`".

## Finding (re-verified against HEAD, 2026-07-11)

`any_ui_msg_injection` (`crates/sky_lower/src/lower.rs`, `lower_def`'s
`Def::Typed` arm) exists so `view : Model -> any` whose body region solves to
`Html<Ty::Var(uv)>` emits `Html<T1>` instead of `Html<()>` (E0271 against
`webview_app`'s `FView` bound). Its detection is:

- annotation return is `IrType::Generic(sym)` with `sym == "any"`, AND
- the body region type is a `Ty::Con { args, .. }` whose FIRST arg is a bare
  `Ty::Var`

— it never checks the Con **name**. So it also fires for `Maybe` / `List` /
`Set` / `Decoder` / any user generic type. Reproduced at HEAD:

```elm
w : Int -> any
w n = []
```

emits `pub fn main_w<T1: Clone>(n: i64) -> Vec<T1>` — generic ONLY in the
return position. A call site that pins the element type compiles; a call site
that never pins it (`let _ = w 1`) is rustc **E0282** ("type annotations
needed") — a skyc-exit-0 / cargo-fail SEAL breach. (Empirically: skyc accepts
the program above; whether cargo fails depends on the call sites, which is
exactly what makes it a silent trap.)

## Fix

In the `any_ui_msg_injection` detection, after matching
`Ty::Con { name, args, .. }` with `args.first() == Some(Ty::Var(_))`, ALSO
require the resolved Con name to be one of the UI msg-parametric constructors
the injection was built for:

```text
"Html" | "Element" | "Attribute"
```

(the same three names `ir_type_from_ty`'s UI arms special-case; `Attribute`
additionally disambiguates by `Ty::Con.module` there, but for the injection
the name check alone is sufficient — a user type named `Attribute` with a
free msg var still lowers through the same `Ui`-family IR and wants the
injection).

Non-UI Cons no longer inject. The downstream "wildcard-`any` return-type fix"
block then calls `ir_type_from_ty(body_ty)` WITHOUT the injected mapping; the
embedded free `Ty::Var` makes that fail with `Feature::Polymorphism`
(IPE-L0102) — i.e. the shape **fails closed with a Sky diagnostic** instead
of emitting a return-position-only generic that can E0282 at arbitrary call
sites.

### Divergence note

The Go backend accepts `w : Int -> any; w n = []` (its `any` erases to
`[]any`). Post-fix Ipê REJECTS it (IPE-L0102, actionable: annotate the
element type or drop the `any`). This is loud-not-silent-wrong, per the
sanctioned-divergence policy — record in `docs/divergences-from-sky.md` when
implemented. If a genuinely-polymorphic-value use case surfaces later, the
correct lift is a monomorphized concrete carrier (per the
prefer-concrete-over-generic codegen rule), not a resurrected name-blind
injection.

## Regression tests

1. UI positive (must stay green): existing Webview/Live `view : Model -> any`
   goldens (the shape the injection serves) — no change.
2. Non-UI negative: `w : Int -> any; w n = []` with an unpinning call site →
   skyc REJECTS with IPE-L0102 (never exit-0). Golden asserting the
   diagnostic.
3. Non-UI `Maybe`: `m : Int -> any; m n = Nothing` — same rejection class.

## Scope

One detection-site edit + tests. No IR change, no new diagnostic code
(reuses IPE-L0102).
