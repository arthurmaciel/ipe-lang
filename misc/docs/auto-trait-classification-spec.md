# Spec — one typed, target-aware Send/Sync classification of `IrType`

Crates: `ipe_lower` (`lower.rs`), `ipe_ir`, `ipe_types` (builtin tag map),
`ipe_backend_rust` (`emit_expr/func.rs`). Principles: SEAL, SSOT, make invalid
states unrepresentable. Lane tier: high (emit).

## Class-closing property

Whether a type (or a generic reaching it) must be `Send`/`Sync`/`'static`
is answered by ONE function over `(IrType, Target)` with an exhaustive match,
consulted by every site that emits a bound. The obligation set for a generic is
derived from the closure's capture set (what the emitted closure actually
holds), not from hand-enumerated per-shape matchers. Kernel-scheme matching
compares constructor tags, not only arity.

## Why the instances exist (origin/main 983fdc404)

- `lower.rs:4824` `propagate_call_site_bounds` doc asserts Send/Sync/'static
  hold for every concrete Ipê type — false: `IpeTask`, `IpeCmd`, `IpeSub` are
  not `Sync`; `IpeSub<M>` is Send only if `M` is (#2961 R1).
- `lower.rs:3384` `ir_type_generic_reaches_bare` stops at `Sub` (#2961 R2).
- Send obligations collected by hand matchers (`lower.rs` ~4180–4309); an
  in-body local composite over `a` captured by a closure whose signature never
  names `a` gets no bound → E0277 (#2955). Fix stranded on
  `lane/2955-capture-send` / `integration/lsp-loose` (commit 9aa4de4c4,
  golden `capture_send_local_composite`), not on main
  (`closure_capture_obligations` absent).
- `render_bounds` (`emit_expr/func.rs`) and lower's carrier obligation assume
  native; on wasm32 `IpeTask`, `PerformThunk`, `SubSpawn` are not Send
  (#2869). Fix stranded on `lane/2869-wasm-send` (193 behind).
- `scheme_var_instance` matches `TyShape::Con` vs `Ty::Con` by arity only;
  the `BuiltinTag`→symbol map lives only in the `ipe_types` builder (#2867).
  Fix on `lane/2867-scheme-tags` — applies cleanly to main.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2961 typed auto-trait classification (R1, R2) | real | `lower.rs:3384`, `:4824` |
| #2955 capture-set obligations | real on main; fix stranded (9aa4de4c4) | `closure_capture_obligations` absent |
| #2869 wasm carriers not Send | real; fix stranded lane/2869-wasm-send (conflicts) | — |
| #2867 compare constructor tags | real; fix on lane/2867-scheme-tags applies clean | `git apply --check` |

## Implementation plan

1. Land lane/2867-scheme-tags (applies clean): exposes the builtin tag map once.
2. Land the #2955 commit (9aa4de4c4 + golden) rebased:
   `closure_capture_obligations` derives bounds from the capture set.
3. `ipe_ir::auto_traits(ty: &IrType, target: Target) -> AutoTraits { send:
   Bound, sync: Bound }` where `Bound = Always | Never | IfParams(Vec<Symbol>)`;
   exhaustive over `IrType`. Runtime truth per variant is pinned by a
   compile-time test in the runtime crate (`static_assertions`-style
   `fn _assert<T: Send>()` per carrier, native and wasm cfg).
4. `ir_type_generic_reaches_bare`, `propagate_call_site_bounds`,
   `render_bounds`, and the Cmd/Sub/Decoder carrier obligation all consult
   `auto_traits`; delete the prose invariant.
5. Re-apply lane/2869-wasm-send's target gate as the `Target` parameter.

## Prove the refusals

- `IPE_E2E=1`: in-body local composite captured (`lens` repro in #2955);
  `TaskSeq` continuation capturing `Sub msg` (R2) — either builds or refused at
  ipe time;
- wasm-client SEAL fixture: `Cmd (Cmd m)` instantiation — refused at ipe time
  or builds for `wasm32-unknown-unknown`;
- alias with reordered args of equal arity does not misalign a scheme var.

Lane tier: high. Guardian: required.
