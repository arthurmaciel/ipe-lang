# 40. Jail scoped to native code; admission jail as capability diagnostic

Date: 2026-07-24

## Status

Accepted. Amends and partly supersedes ADR 0038 (runtime capability sandbox):
0038's fail-closed-or-refuse posture is retained **only** for native-code-bearing
programs, and only where a platform jail exists. This ADR narrows *when* the
runtime jail runs and reframes *why* the admission jail exists.

## Context

Two sandboxes exist, for two different purposes, and both had been generalized
past the point where they earn their keep.

The **runtime jail** (ADR 0038) wraps the emitted binary on every `ipe run` and
`ipe exec`, confined to `inferred ∪ declared`, fail-closed: if no platform jail
primitive is available, the program refuses to run. Applied to *every* program
this has two costs. First, it contradicts the capability model's own guarantee:
for pure Ipê an unreachable capability is **absent from the binary**, so the
guarantee is structural and there is nothing to enforce at run time — a jail
around pure code can only forbid what the binary already cannot do. Second, it
makes an ordinary Ipê program unrunnable on any host without a jail primitive
(containers, CI, macOS, BSD, Windows), turning a security control that only
matters for native code into a portability tax on all code. Mandating a jail on
every platform also implies building and maintaining a jail backend for every
platform before pure programs can run anywhere — a large, open-ended obligation
gating unrelated work.

The **admission jail** confines the build of an untrusted candidate crate when a
package is proposed for the index. Its stated job had been to *prove the crate
can be contained*. But package admission already runs in an ephemeral, isolated
CI environment, so "protect the runner" is largely covered; and proving
sandboxability on one platform says nothing about another. The durable value the
jail offers at admission is different from containment: run the native code under
a jail that **denies** capabilities and **observes** what it attempts, then diff
the observed demand against the declared manifest.

## Decision

**Scope the runtime jail to native-code-bearing programs; let pure Ipê run free.**

- A program whose capability union reaches no opaque native (`Rust.`) code runs
  **directly — no jail, no warning**. Its capability set is a compile-time
  structural fact; there is nothing to enforce at run time.
- A program that reaches native code is jailed by default **where a platform jail
  is available**, exactly as ADR 0038 specifies (union profile, deny-by-default
  lowering, baseline denials, artifact-carried floor).
- Where the program reaches native code and **no platform jail is available**,
  the run is **best-effort with loud consent**, not a hard refusal: emit an
  unmissable red warning naming the program as containing native Rust whose
  effects are not proven safe, recommend a platform jail, record the consent, and
  proceed. This is a deliberate move from fail-closed to fail-open-with-consent,
  for native code only.

*Rejected — keep the ADR 0038 fail-closed-everywhere posture.* It jails pure code
that has nothing to enforce, blocks ordinary programs on jail-less hosts, and
predicates running any program on porting a jail to every platform. The security
it buys over "pure code is structural, native code is loudly flagged" is
approximately none for pure code and, for native code, the ecosystem baseline is
weaker still: comparable toolchains run untrusted native build and install code
with no sandbox and no warning at all. Native effects here are opt-in and already
capability-consented; a recorded, unmissable warning is a defensible boundary for
a developer tool and does not hold the whole language hostage to a jail backend.

*Rejected — warn for pure code too.* There is nothing to warn about; pure Ipê
cannot exceed its inferred, consented set. A warning there is noise that trains
users to ignore the native warning that matters.

**Reframe the admission jail from containment proof to capability diagnostic.**

- At admission the jail runs the candidate crate with capabilities denied and
  observes what it demands (socket, filesystem write, subprocess, environment,
  clock), producing an observed-demand profile.
- Admission gates on **declared-vs-demanded**: the declared manifest is
  authoritative; the jail trace corroborates it and rejects under-declaration.
  Containment is not the gate — measurement is.
- **A single canonical platform (Linux) suffices.** Capability demands are
  portable at the granularity that gates admission — a crate that opens sockets
  opens them everywhere — so one tracer produces the profile that admits the
  package. The manifest, not the trace, remains authoritative; the trace need not
  be exhaustive.

*Rejected — per-platform admission jails.* Running the diagnostic on every
platform proves containment we do not gate on and would block admission on the
absence of a backend. The one documented limitation — `cfg`-gated native paths a
Linux trace never exercises — is mitigated by the manifest being authoritative
and the trace being corroboration, not proof.

## Consequences

- Pure Ipê programs run on any host with no jail primitive: containers, CI,
  macOS, BSD, Windows. The static-binary and jail end-to-end paths stop requiring
  a jail for pure hello-world, which was the standing cause of their CI red.
- The security control now lives exactly where effects are opaque. The guarantee
  stays honestly tiered — pure Ipê structural, native contained-or-loudly-flagged
  — and the README and capability docs must state it that way.
- Cross-platform jail backends (Seatbelt, Capsicum, Windows AppContainer/job
  objects) become optional hardening that upgrades native programs from
  warn-and-run to jailed on that platform — no longer a precondition for running
  any program.
- The runtime override narrows: the escape is now the documented platform-absence
  path for native code (warn + recorded consent), not a general unsandbox flag.
  The fail-closed, deny-by-default lowering of ADR 0038 still governs the case
  where a jail *is* present.
- The invariant that must hold: **the native-vs-pure split is drawn from the same
  capability inference that the manifest gate uses**, so a program is classified
  "native-bearing" if and only if its union reaches declared native code. If that
  classification could be under-approximated, a native program could be treated as
  pure and skip the jail — so the split is a compile-time fact derived from the
  inference pass, never a source heuristic.
- Admission stays sound without a jail on the admitting host: gate on the manifest
  first; the Linux trace is corroboration that can run wherever the tracer runs.
