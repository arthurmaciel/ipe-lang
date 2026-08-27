# Unsafe-import acknowledgment — warn, don't patronize

> Status: design proposal, no implementation. Every fenced flag/signature below
> is **illustrative of the proposed surface**, not shipped API. This note builds
> directly on the disclosed `.Unsafe` convention and the inferred `unsafe`
> capability (`docs/internals/design/tbd/unsafe-escape-convention-design.md`); read
> that first — this adds one thing on top of it: a build-time acknowledgment
> when *user* code reaches for a disclosed unsafe escape.

## The philosophy: make the safe way easiest, warn only on real exposure

Ipê is already ahead of most languages (Rust apart) at making risk **explicit**
rather than implicit:

- The **type** is a trust boundary: reserved, opaque, validating-parse-only
  security types (`Path`, `SqlFragment`, `Secret`, `Url`, `Regex`, …) can only
  be built by a constructor that parses. A value is a proof a check ran.
- The **escape hatch** is a trust boundary too: every parse-bypassing sink lives
  in a disclosed `Ipe.<M>.Unsafe` submodule, named `unsafe*`, and importing one
  infers the `unsafe` capability — visible in `ipe capabilities`, gated at
  `ipe add`.

What is missing is the **moment of consent for the person building their own
program**. Today the `unsafe` capability is inferred and *disclosed*, but a
developer who writes `import Ipe.Html.Unsafe` gets no build-time signal that they
have just opened an XSS sink. Disclosure serves the *auditor of a dependency*;
this feature serves the *author exposing themselves*.

The stance is deliberately non-patronizing: **make the safest way the easiest,
and warn only when the user genuinely exposes themselves.** The safe path
(`Html.text`, `Sql.column`, `Secret.use`) is silent and terse. The warning fires
only on a real, disclosed exposure — an imported `.Unsafe` submodule — never on
ordinary code. There is no nagging, no ceremony on the safe road.

## The feature

When compiling **user code** (an `ipe build` / `ipe run` of the user's own
project, not the stdlib) that imports any `Ipe.<M>.Unsafe` module:

1. The compiler **warns clearly**: which module, and what the risk is (XSS for
   `Html.Unsafe`, SQL injection for `Db.Unsafe`, secret leakage for
   `Secret.Unsafe`, …), keyed off the already-computed `unsafe` capability and
   its per-domain `via …` breakdown.
2. The build **requires acknowledgment**. Interactively, it prompts. A flag —
   `--accept-risks` — lets the user take responsibility, proceed, and suppress
   the prompt in non-interactive / CI builds.
3. The safe default path is completely untouched — no import of a `.Unsafe`
   submodule means no warning, no prompt, no flag needed.

Illustrative surface (not shipped):

```
ipe build --accept-risks      # acknowledge every disclosed unsafe import, proceed
ipe run   --accept-risks
```

```
warning[IPE-…]: this program imports an unsafe escape hatch
  --> src/Page/Product.ipe
   |
   |  import Ipe.Html.Unsafe as H
   |         ^^^^^^^^^^^^^^^^ imports a parse-bypassing sink
   |
   = Ipe.Html.Unsafe.unsafeRaw builds HTML from an unchecked String,
    bypassing the XSS escaper. Untrusted input reaching it is a
    cross-site-scripting vulnerability.
   = the safe path is Ipe.Html.text (escaped) — reach for Unsafe only
    for genuinely trusted, verbatim HTML.
   = re-run with --accept-risks to take responsibility and proceed
    (or add `unsafe` to [capabilities] accept in ipe.toml).
```

## Where the check lives

The fact this feature needs is **already computed**. `program_capabilities_scan`
(`src/compiler/lower/src/lower.rs`) sets `Capability::Unsafe` from
`program.imports_unsafe_submodule`, and the `Unsafe` capability arm already
exists in the closed vocabulary (`src/compiler/kernels/src/capability.rs`). The
CLI already resolves a program's inferred capabilities at build/run time
(`infer_package_capabilities` / `program_capabilities` /
`ResolvedCapabilities`, `src/ipe-cli/src/run_sandbox.rs`).

So the acknowledgment is a thin gate at the **capability-resolution site in the
CLI**, right after the inferred set is known and before the build proceeds — the
same place the sandbox profile is built from `caps.inferred`. It is **not** a new
compiler pass; the compiler already discloses `unsafe`, and this reads that
disclosure. To render the good diagnostic (which module, which sink), the CLI
uses the per-domain `via Ipe.Html.Unsafe, …` breakdown the escape-convention
design already specifies for `ipe capabilities --verbose`; that breakdown is the
same import-set the scan keyed off, so no new bookkeeping is required beyond
threading the offending import spans to the diagnostic.

**Scope: user code only.** The warning is for a developer exposing *their own*
program. Stdlib compilation (the `.Unsafe` submodules define the hatches; they
are `EmbeddedStdlib` origin) is exempt — a `.Unsafe` module *is* the sink, it
does not "reach for" one. The gate fires on `ModuleOrigin::User` imports of a
`.Unsafe` submodule, mirroring the origin discipline the reserved-namespace gate
already uses.

## Interactive vs non-interactive

- **Interactive TTY** (`IsTerminal` is already used in
  `src/ipe-cli/src/style.rs`): print the warning and prompt for a yes/no
  acknowledgment. A "no" is a typed, fail-closed refusal — the build does not
  proceed.
- **Non-interactive / CI** (no TTY, or `--accept-risks` given): **never block on
  a prompt** — a build that hangs waiting for stdin in CI is a worse failure than
  the risk it guards. Without pre-acceptance, a non-interactive build **fails
  closed** with the warning as an error and the remedy (`--accept-risks` or the
  manifest token). With `--accept-risks`, or a manifest pre-acceptance, it
  proceeds silently. CI must be able to pre-accept once and stay quiet.
- **Manifest pre-acceptance.** Illustratively, an `[capabilities] accept`
  entry that lists `unsafe` (distinct from `[capabilities] declared`, which is
  about a package's *own* native effects per
  `docs/adr/0044-package-coordination-manifest-index-gate.md`) records durable,
  reviewable consent in-repo, so a repeatedly-built project does not re-prompt
  and CI needs no flag. The flag is the one-off; the manifest token is the
  durable form.

## How it composes

- **With the capability floor.** `unsafe` is a *disclosure* capability, not a
  jail axis (like `native-ffi`, it names a provenance fact, not an OS-isolable
  resource). The acknowledgment is a build-time consent step layered on that
  disclosure; it does not change the runtime sandbox, which continues to gate the
  resource axes (`network`, `filesystem`, …). A deployment that wants a
  hard-ban stance (no escape hatch permitted at all) still gets it by declaring
  zero `unsafe` and letting the fail-closed non-interactive path reject any
  inferred `unsafe`.
- **With `.Unsafe` packaging.** This is what makes a *packaged* `Html.Unsafe` or
  `Db.Unsafe` (see `docs/internals/design/tbd/stdlib-core-vs-package-policy.md`)
  safe-by-consent. Wherever the `.Unsafe` submodule lives — core or a package —
  the same import-derived `unsafe` capability fires and the same acknowledgment
  gate applies. Residency does not weaken or strengthen the boundary; consent
  does. A first-party `Html` package's `.Unsafe` sink is exactly as gated as a
  core one.
- **With the secure-before-mark precedent.** The acknowledgment only ever fires
  on *irreducible* hatches, because only irreducible hatches are allowed into a
  `.Unsafe` submodule in the first place (the escape-convention design's
  precedence gate: a fixable `*Raw` must be fixed, not labeled). The warning is
  never a substitute for structural safety — it is the last, consented step for
  the genuinely-irreducible escape.

## Not deferred

Unlike the stdlib package migration (which waits on the packaging / FFI tiers),
**this feature is buildable now.** It depends on nothing that is not already
shipped: the `unsafe` capability, its import-derived inference, the CLI
capability-resolution site, and interactive-terminal detection all exist today.
It is a self-contained CLI gate over an already-computed fact.

## Security review

Any implementation of this feature, and every `.Unsafe` module migration it
composes with, is a **language-boundary / soundness change** and requires
**security-soundness-guardian review before merge** — the acknowledgment gate is
a security control, and a bug that lets an `unsafe` import proceed silently (or
that blocks CI on a prompt) is a security regression.
