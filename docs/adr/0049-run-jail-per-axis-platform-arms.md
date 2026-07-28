Status: Accepted
Date: 2026-07-28

# 49. Runtime run-jail confines per axis, with a fail-closed volume probe

## Context

The runtime run-jail (ADR 0038, scoped to native-bearing programs by ADR 0040)
confines the emitted binary to exactly its `inferred ∪ declared` capability set.
`SandboxProfile` carries four platform-independent confinement axes — `network`,
`filesystem`, `env_allowlist`, and `subprocess` — plus resource limits. Each
host OS exposes a different confinement toolkit, so every platform arm must lower
those four axes onto whatever primitives the host provides.

Two forces shape how an arm is allowed to claim a profile is enforced:

- **No over-claim.** The macOS arm review surfaced the governing rule: when the
  jail reports it `Holds` a profile, *every* axis the FFI admit path trusts must
  actually be confined by the emitted jail. Seatbelt, for instance, does not
  scrub environment variables — so the macOS arm must confine the `env` axis at
  the launcher, not silently trust the OS profile for it.
- **Some primitives have a precondition admit-time cannot verify.** On Windows
  the `filesystem` and `network` axes are confined by an AppContainer lowbox
  token, and the AppContainer filesystem boundary only holds on a volume that
  persists and enforces DACLs. Whether the target volume does so is a runtime
  fact, unknown when a package is admitted.

No single primitive confines all axes on any platform, and no primitive exists on
every platform. A portable design must therefore decide (a) how a profile lowers
per axis, and (b) what happens when a required primitive — or its precondition —
is unavailable.

## Decision

The run-jail confines **per axis**, and every platform arm confines every axis it
claims, assembling primitives rather than trusting one blanket mechanism:

- **Linux** — a network namespace (`network`), a read-only root bind plus tmpfs
  masks (`filesystem`), `--clearenv` + re-export (`env`), and a seccomp deny of
  the clone/fork family (`subprocess`).
- **macOS** — a `sandbox-exec` Seatbelt SBPL profile for `network`, `filesystem`,
  and `subprocess`; the launcher performs the `env` scrub, because Seatbelt does
  not.
- **Windows** — a Job Object (subprocess axis) wrapping an AppContainer
  lowbox-tokened child (`filesystem` + `network`), with the launcher scrubbing
  `env`.

Where a primitive carries a precondition admit-time cannot check, the arm closes
it with a **runtime probe that fails closed**. On Windows the AppContainer
filesystem boundary is a no-op on a volume without `FILE_PERSISTENT_ACLS`, so the
arm probes `GetVolumeInformationW`'s filesystem flags before launch: if the
volume does not persist and enforce DACLs, the arm **refuses to launch** rather
than run a target whose filesystem axis would be unconfined.

Confinement composes only downward: a profile may be established by an arm that
is **at least as isolated per axis** as the profile demands, checked axis by
axis, never as an aggregate. An arm that cannot confine an axis the profile
requires does not weaken to "best effort" — it reports a refuse-gap.

Alternatives rejected:

- **A single cross-platform confinement abstraction.** No OS primitive maps
  cleanly onto the same four axes; forcing one would either under-confine an axis
  on some host or over-claim `Holds`. Per-axis lowering keeps each arm honest.
- **Trusting AppContainer unconditionally on Windows.** That silently
  under-confines the filesystem axis on non-ACL volumes — the exact over-claim
  the no-over-claim rule forbids. The fail-closed volume probe is the price of
  claiming the axis.
- **Degrading to unconfined when a primitive is missing.** A security control
  that quietly turns off is worse than one that refuses; an absent primitive
  becomes a refuse-gap, never a silent bypass.

## Consequences

- The admit path may trust a `Holds` profile because every axis it reports is
  actually enforced by the arm — the property the FFI isolation hand-off depends
  on.
- On a Windows host without an ACL-persisting volume, a native wrapper needing
  the filesystem axis refuses rather than runs uncontained — surfaced as a
  refuse-gap the user resolves, not a silent hole.
- New platform arms (BSD Capsicum/pledge, others) inherit one obligation: confine
  each of the four axes, or declare a refuse-gap for the axes they cannot. They
  may never claim `Holds` for an axis they do not enforce.
- The standing invariant: for any target the jail reports it `Holds`, each of the
  four axes is confined by a concrete primitive whose preconditions are satisfied
  at launch — verified per axis, fail-closed on any gap.

## Conventions

ADRs describe Ipê on its own terms. This decision is stated as a standalone Ipê
decision, without reference to any prior or external implementation.
