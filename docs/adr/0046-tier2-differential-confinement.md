Status: Accepted
Date: 2026-07-27

# 46. Tier-2 native-code capability enforcement is differential confinement, not tracing

## Context

The Tier-1 package gate (`ipe package audit`, ADR 0044) proves a package's
declared capability set equals the set the compiler infers over the whole
shipped module tree. That proof is exact only for pure Ipê code. When a package
crosses into native `Rust.` code it carries the `native-ffi` axis, and inference
cannot see past that marker: Tier-1 admits the declared set on the author's
word. A package could declare `[]` and open a socket from native code, and
Tier-1 would not catch it.

Closing that hole requires observing what the native code actually demands and
reconciling it against the declaration. The obvious mechanism is a syscall
tracer that enumerates every demanded effect and classifies it into a capability
axis. No such tracer exists, and a robust cross-platform one is a large,
security-sensitive subsystem in its own right. What does exist is a jail that
*denies* a withheld capability at the OS boundary, scoped from the same
capability-lowering the runtime jail uses.

## Decision

Tier-2 observes by **differential confinement**, not by tracing. It builds and
exercises the package's native code inside a jail scoped to *exactly* the
declared capability set, then reads the outcome:

- **used-but-undeclared** — a probe action is denied under the declared-scoped
  jail. The native code demanded an axis the declaration withheld. Reject,
  naming the axis.
- **declared-but-unused** — a per-axis tightening pass removes one declared axis
  and the build+probe still passes. The axis is over-broad. Reject — but only
  when the static wrapper capability scan *also* agrees the axis is unreached
  (below).
- **sandbox-unavailable** — the jail cannot be established on a platform that
  should have one. Reject that platform; never run the untrusted build
  unconfined and admit.
- **build-fails-in-jail** — an ordinary compile/link/test error, distinct from a
  capability denial. Reject, reported as a build failure.
- **clean on every wired platform** — the only admit path.

Two structural properties make this sound:

**The denial signal is wrapper-owned.** The untrusted build runs as a *child* of
a probe wrapper we author. The wrapper owns the per-axis exit-code contract; a
denied syscall in the child surfaces as the wrapper's exit code, not as the
child's own `exit(0)`. The untrusted build can therefore never forge a clean
result, because it does not own the exit the decoder reads.

**The confinement is not forked.** The declared-scoped profile is lowered by the
same `profile_from_capabilities` the runtime jail runs under, so what Tier-2
confines a build to and what the shipped artifact is confined to at run time
cannot drift.

Differential confinement is strictly weaker than a tracer: it observes
reachability-under-denial, not intent, so a capability compiled in but not
exercised by the probe reads as unused. This is sound in the fail-closed
direction — it can only over-reject a package a tracer would admit, never admit
one a tracer would reject. Over-rejection is the correct bias for a
supply-chain gate.

The declared-but-unused reject is cross-checked against the static wrapper scan
(`capability_scan.rs`) to blunt a laundering path: were the tighten pass alone to
force an author to drop a declaration for a capability that is compiled in but
merely un-exercised, the shipped artifact would then carry that capability
*undeclared*. Requiring both the tighten and the static scan to agree the axis
is unreached means Tier-2 never pushes an author to under-declare a
genuinely-present capability.

Alternatives rejected: a syscall tracer (does not exist; a large subsystem;
deferred as a strictly-tightening future replacement). Scraping the untrusted
build's stderr for denial text (the package would control the signal — forgeable).
Admitting an un-run platform on the author's word (fail-open; contradicts the
runtime jail's per-target posture).

## Consequences

- Only platforms whose jail is wired and proven can certify a native package.
  The wired platforms are **linux-x64** (bwrap + a seccomp socket-deny filter)
  and **macos-arm64** (`sandbox-exec` under a Seatbelt SBPL profile). The
  confinement is not forked per platform: both lower the SAME capability profile —
  Linux to a bwrap argv + seccomp program, macOS to an SBPL profile — and both
  decode the SAME wrapper-owned per-axis exit-code contract, so the reconciler and
  the admit predicate are identical across platforms. Other platforms are a
  refuse-to-certify: the version is not marked admitted for them, and the audit
  surface says so — it never claims Tier-2 for a platform it did not run on, and a
  certify names exactly the platform whose jail ran. As each remaining platform's
  jail lands it promotes to blocking, exactly as the runtime jail's per-target
  posture does.
- The audit spends bounded extra effort: one jailed build+probe for the
  declared-scoped run, plus one per removable axis for the tightening pass. The
  axis set is tiny and the jail caps wall-clock and resources, so an untrusted
  `build.rs` that spins is killed by the cap, not waited out.
- The invariant that must hold: the admit path is a single conjunction over the
  typed jail outcome (`Clean` on the declared-scoped run, no axis removable), and
  every other branch is a typed reject or a recorded platform-skip. If a future
  change lets any non-clean outcome reach admit, the gate is broken. A standing
  red-canary fixture (a native package that opens a socket while declaring `[]`)
  guards this: it must always reject, naming the axis.
- A native package that exposes no probeable entrypoint is under-observed. It is
  never silently admitted as clean; Tier-2 certifies only what it exercised.

## Conventions

ADRs describe Ipê on its own terms. Do not reference any prior or external
implementation, parity with another system, or project ancestry — state each
decision as a standalone Ipê decision.
