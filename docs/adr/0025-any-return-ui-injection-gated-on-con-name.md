Status: Accepted

# 0025. The `any`-return UI-msg injection fires only for named UI constructors

## Context

`any_ui_msg_injection` (in `lower_def`'s `Def::Typed` arm) exists so that
`view : Model -> any`, whose body region solves to `Html<Ty::Var(uv)>`, emits
`Html<T1>` instead of `Html<()>` (which would fail the `webview_app` `FView`
bound with E0271). Its original detection matched only *shape* — an annotation
return of `IrType::Generic("any")` plus a body region that is a `Ty::Con` whose
first arg is a bare `Ty::Var` — and never checked the Con *name*. So it also
fired for `Maybe`/`List`/`Set`/`Decoder`/any user generic, emitting a function
generic *only in return position* (`fn w<T1: Clone>(n: i64) -> Vec<T1>`). A call
site that never pins the element (`let _ = w 1`) is rustc E0282 — an
exit-0-then-cargo-fail seal breach that is silent because it depends on the call
sites.

## Decision

Require the resolved Con name to be one of the UI msg-parametric constructors the
injection was built for — `"Html" | "Element" | "Attribute"` (the same three
`ir_type_from_ty`'s UI arms special-case) — before injecting. Non-UI Cons no
longer inject; the downstream wildcard-`any` return-type handling then calls
`ir_type_from_ty(body_ty)` without the injected mapping, and the embedded free
`Ty::Var` makes it **fail closed** with `Feature::Polymorphism` (`IPE-L0102`)
instead of emitting a return-position-only generic.

Rejected alternative (for any future genuinely-polymorphic-value use case): a
resurrected name-blind injection — the correct lift is a monomorphized concrete
carrier, per the prefer-concrete-over-generic codegen rule (see ADR-adjacent
prior art), never a return-only generic.

## Consequences

- **Invariant that must keep holding:** the injection is a UI-specific
  accommodation, gated on the UI Con name; it must never widen a non-UI `any`
  return into a return-position-only generic (that shape is the seal breach). A
  non-UI `… -> any` body carrying a free type var fails closed with `IPE-L0102`,
  actionable ("annotate the element type or drop the `any`").
- **Sanctioned divergence:** a backend that erases `any` to a wildcard accepts
  `w : Int -> any; w n = []`; Ipê rejects it — loud-not-silently-wrong.
