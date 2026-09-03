Status: Accepted
Date: 2026-09-03

# 0068. One foreign-interface boundary, parameterized by target

## Context

Ipê reaches outside its pure core in several directions: in-tree kernels, native
Rust crates, JS ports, browser custom elements, and — eventually — other
languages and wasm components. The tempting shape is one subsystem per direction,
each with its own namespace, its own trust story, and its own idea of what a
"foreign value" is. That shape multiplies the security surface: every new target
reinvents the boundary, and a reviewer must re-verify each one from scratch.

The observation that avoids this: these directions differ only in *who is on the
far side*, not in *what the boundary must guarantee*. That is the same structure
as `Ipe.Tea.*` — one update/view/init spine instantiated per shape — where the
spine is the invariant and the shape is a parameter.

## Decision

Treat the foreign interface as **one boundary, parameterized by target**. Every
target instantiates the same spine; none forks it.

**Five invariants hold across all targets:**

1. **One closed capability vocabulary.** The compiler owns the set of disclosure
   axes; a package declares against it and cannot mint axes. Every target extends
   the same enum, never a parallel one.
2. **One boundary predicate — the SEAL — with two halves.** An *ingress decode
   gate* (untrusted bytes entering must parse into a concrete, declared,
   monomorphic ADT — fail-closed, bounded), and an *egress restriction* (no
   function, no open row, no `Secret` leaves the pure core). Ingress-gating is
   `sealed`-only; egress is per-level.
3. **One trust level** — a closed, compiler-owned enum keyed on the far side:
   `proven` / `contained` / `sealed`.
4. **One coverage surface** — `ForeignSurface` checks every foreign symbol carries
   the discipline its level requires, uniformly, regardless of target.
5. **One statically-resolvable boundary** — the foreign target is named by a
   compile-time string literal (`Kernel.kernel "…"`, `Rust.fn "…"`,
   `Js.CustomElement.fromFile "…"`), never a runtime-computed value. No dynamic
   target ⇒ no runtime-loaded foreign code ⇒ the reachable foreign surface is
   statically enumerable. Invariants 1 and 4 depend on this.

**Trust is a level, not a binary**, because "trusted" splits into two disciplines:

| Level | Far side | Guarantee | Mechanism |
|---|---|---|---|
| `proven` | in-tree kernel, type-checked | compiler owns it end to end | none |
| `contained` | native Rust, opaque | declared-and-contained, not proven | capability jail on effects |
| `sealed` | untrusted JS | nothing until parsed | SEAL ingress decode gate |

`contained` jails *effects, not data*: a returned value is ordinary untrusted
content once in the pure core, handled by the same downstream injection-safety as
a value from a `proven` kernel. `Secret` egress is per-level: into `proven`
freely (crypto kernels take keys), **never** into `sealed`, into `contained` only
behind an explicitly declared capability (fail-closed default: forbid).

`contained` is a binding obligation, not a label:
`contained ⟺ inspector-verified ∧ caps-declared ∧ glue-compiles ∧ jailed`; if any
conjunct fails the binding is a coverage hole (a compile error), never blind
trust. It is enforced across independent stages — declaration → constrain → lower
→ emit (a total match over the closed level enum, no wildcard, so no arm can emit
`contained` without the jail or `sealed` without the decode gate) → coverage
re-check → runtime.

**The build-time inspector (Rust) and the runtime decode gate (JS) are the same
job at different times**, and which time is forced by whether the toolchain can
*see* and *trust* the far side at build. Rust compiles the far side in, so its
shape is guaranteed at build (rustc backstops a mis-shaped inspection: `ipe`
exit-0 ⇒ the emitted Rust builds); a runtime gate would be redundant. JS is a
runtime host the toolchain can neither see nor trust, so the shape can only be
guaranteed at the moment the value exists — at runtime. Two axes decide the
mechanism: producer trust (untrusted ⇒ runtime parse) and build-visibility (which
trust lets you rely on). This *derives* the mechanism for future targets rather
than guessing per-target (a wasm-component: build-visible + trusted ⇒
inspector + sandbox = `contained`).

**Foreign code calling back into Ipê uses a typed correlation-token channel**, not
a function pointer: the callback crosses as an opaque id, the runtime holds the
real closure keyed by that id, invocation is a checked lookup, and an unknown or
expired id is dropped fail-closed. This keeps invariant 2 intact ("foreign code
holding an Ipê callable" is unrepresentable) and avoids the use-after-free class a
real trampoline would reintroduce. Hot inner loops stay entirely native (bind the
whole native operation via `Rust.fn`), so the boundary is never weakened for
speed.

## Consequences

- One namespace: `Ipe.Ffi.<Target>.<verb> "<literal>"`, a plain value binding with
  a reserved literal-only constructor and no bespoke keyword. Adding a target adds
  a level arm and a namespace, not a subsystem.
- The security review of any target reduces to "which conjuncts of its level does
  it satisfy," checked mechanically by `ForeignSurface` — including a shared
  `refusal-tested` aspect that asserts a standing test drives each fail-closed
  path, so a refusal cannot silently vanish.
- Efficiency (a redundant runtime gate on Rust, a trampoline for callbacks) is
  deliberately declined where it sits below Soundness/Security in the precedence
  order; the trust-level model absorbs the performance-sensitive cases instead.
- The spine is validated by walking a not-yet-built target (a wasm component)
  through it: it lands as `contained` with no new machinery, which is the evidence
  the parameterization is real rather than a post-hoc grouping of four subsystems.
