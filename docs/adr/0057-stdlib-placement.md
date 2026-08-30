Status: Proposed
Date: 2026-08-05

# 0057. Stdlib placement — capability, security-defense, perf, or computation

## Context

Much of the standard library is wired as native kernels by default. Every native
function enlarges the audited/attack surface and the recompile cost: adding a
currency to `Money` today rebuilds the compiler, because the currency table is a
Rust `match`. Two independent analyses converged on the same dividing line — the
`Money` currency-table case (a per-module placement study) and the UI-emit-split
design (§3) from the `Ui`/`Html` builders, whose
"does the kernel buy a speedup? no" measurement showed the builders are cold
allocation, not a throughput primitive. This ADR makes that line a decision and
adds two refinements surfaced in review.

## Decision

Classify every stdlib function into one of four destinations. A function is
**native** if *any* of three tests holds; otherwise it is an Ipê package.

1. **Core intrinsic** — what the compiler/runtime itself depends on (`Basics`,
   the TEA reactor engine). Rebuilds the compiler, by nature rarely.

2. **Native** — stays out of the `.ipe` layer if it is:
   - **a capability** — touches the outside world (OS, network, filesystem,
     clock, entropy) or needs a vetted native crate. This is exactly the surface
     the capability/trust model gates.
   - **a security defense** — its *correctness is itself a security property*:
     escapers/sanitizers (HTML, SQL, shell), injection/traversal validators
     (`CssSafety`, `valid_sql_ident`, URL-host parsing), and crypto. A defense is
     **not** attack surface to be shrunk by moving it up — it is a trusted
     component to keep native, small, directly auditable, fuzzable, and
     **non-overridable**. The idiomatic shape is parse-don't-validate: the native
     validator is the *only* constructor of an opaque safe type, and that type
     carries the safety outward, so downstream `.ipe` code that consumes a safe
     value is safe by construction without itself being security code.
   - **a measured performance primitive** — a hot throughput core (`String`,
     `List`, `Dict`, `Set`, `Math`), or a combinator that runs on the **happy
     path** (`Result`/`Maybe`/`Tuple` combinators thread through every success).
     Kept native until a benchmark says otherwise.

3. **Ipê package** — everything else: pure, *cold* computation and data
   expressible in Ipê. Value/data builders (`Ui`/`Html`), data tables (`Money`
   currencies, `Locale`, `Palette`), decoder/encoder *combinators* over a native
   parse core, and cold formatting/display helpers. These are editable,
   overridable, and shrink the compiler + kernel registry.

The security-defense test **overrides** the "computation → package" pull: even
though a validator is pure computation, its failure mode is a vulnerability, not
a wrong value, and Security is the first principle — so it stays native.

**Worked example — `Html` splits within one module.** The `Html`/
`Html.Attributes`/`Html.Events` *constructors* only assemble `Element`/
`Attribute` values → package. The *serialiser* (`Html.render` + the
`escapeText`/`escapeAttr` escapers) is the XSS injection barrier → native. The
surface-reduction argument thus inverts inside a single module: moving the
constructors up shrinks the native surface; moving the escaper up would enlarge
the trusted computing base for the escaper and make it harder to audit and fuzz.
Escaping is concentrated in the one native serialiser, so the assembled tree is
just data and only rendering is a defense.

## Consequences

- **Security first.** The non-defense computation leaves the native surface,
  shrinking the audited/attack surface; the defenses stay concentrated, native,
  and vetted rather than diffused into compiler-generated code.
- **Readability + no-recompile + a smaller kernel registry** — each moved
  function deletes ~6 anti-drift registry rows and becomes editable Ipê.
- **Not a big-bang, and gated.** The "no recompile" payoff needs three
  mechanisms — materialise the stdlib source (embed→local), auto-import with DCE
  that is free-for-unused, and packages. Until DCE is free-for-unused, each
  `.ipe` move pays emitted-binary size, so gate each move on a size check or
  sequence it after DCE.
- **Priority.** The clear wins are data and cold builders: the `Money` currency
  table, the `Json`/`Db` decoder combinators, and the `Ui`/`Html` builders. The
  ADT combinators (`Maybe`/`Result`/`Tuple`, `Error` construction) are hot or
  ubiquitous and stay native until benchmarked; only their cold formatting/
  display helpers are candidates. The types themselves stay core.

This decision supersedes the earlier per-module placement study; it promotes that study's conclusions here, reclassifies `CssSafety` from package to native per the security-defense test, and extends the audit to `Markdown`→HTML and `Path`.

## Conventions

ADRs describe Ipê on its own terms. The placement line partitions the stdlib by
what a function *is* — capability, defense, perf primitive, or cold computation —
with Security taking precedence over the surface-reduction it otherwise enables.
