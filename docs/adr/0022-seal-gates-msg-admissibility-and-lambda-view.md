Status: Accepted

# 0022. Msg-admissibility uses derivability (not serde); a lambda-aware extractor closes the view/update gate bypass

## Context

The exit-0 seal requires `ipe accept ⇒ cargo build`. ADR 0002/0007's move seal
and the Model-admissibility gate close part of it, but a well-typed Ipê program
could still put a non-derivable value (Html, Cmd, Task, function) into its Model
or Msg — `ipe` accepts, `cargo` fails on the missing trait bound. Two gaps
remained: the Msg slot had no admissibility gate, and a lambda-bound
`view`/`update` field bypassed the existing Model gate, because gate recovery
only handled named `FuncValue`s.

## Decision

- **The Msg predicate is `ir_type_is_derivable` for Web, Tui, AND Webview — not
  serde.** Only the Web *Model* is persisted to the session store; Msg is
  transient (dispatched over channels, never serialised). This asymmetry is
  correct and must be preserved: Html is admissible in a Web *Msg* (it is
  `Clone`) but inadmissible in a Web *Model* (no `Serialize`/`DeserializeOwned`).
- **Recover Msg from `update`'s first parameter**, where it appears directly —
  not from `view`'s return, where it is nested inside `Html<Msg>`.
- **Introduce one lambda-aware extractor `fn_param_ty(e, idx)`** used by both the
  Model gate and the new Msg gate. It recovers the concrete parameter `IrType`
  whether the field is an `Expr::FuncValue` or an `Expr::Lambda` (lambda params
  carry concrete `IrType`s materialized from the solved region type; curried
  lambdas are flattened so `params[0]` is always the first user parameter),
  simultaneously closing the lambda-view bypass.

Rejected alternatives: gating only at `cargo` level (leaves the seal hole open);
using `ir_type_is_serde` for Msg (false-rejects admissible Web Msgs carrying
Html/Element/Color); recovering Msg from `view`'s return (Msg is nested inside
`Html<Msg>` — `update`'s first param is the direct path).

## Consequences

- Both Model and Msg are provably admissible for their app shape before `ipe`
  exits-0; a lambda-bound `view`/`update` no longer bypasses the gate. The
  diagnostic is split by slot (`IPE_L0121` Msg, `IPE_L0120` Model).
- **Invariant that must keep holding:** the Model/Msg derivability asymmetry
  (Model needs serde, Msg needs only derivability) is load-bearing — collapsing
  them re-introduces false rejects or reopens the seal. A documented, *narrower*
  residual remains for cfg fields bound to let-bound locals or partial
  applications (still fail-open) — tracked for follow-up (gate at the solver-type
  site, or fail-closed reject), never silently widened.
