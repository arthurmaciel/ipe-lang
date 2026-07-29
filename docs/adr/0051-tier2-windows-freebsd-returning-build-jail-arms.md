Status: Accepted
Date: 2026-07-28

# 51. Tier-2's Windows and FreeBSD returning build-jail arms

## Context

Tier-2 package admission observes a native package's real capability demand by
*differential confinement* (ADR 0046): it runs the untrusted build inside a jail
scoped to the *declared* capability set and reads a typed outcome. The vehicle is
the **returning** build jail — `build_in_jail(profile, …) -> JailOutcome` — whose
outcome is one of `Clean` (the only admit-eligible value), `Denied { axis }`
(a withheld axis was demanded), `BuildFailed` (an ordinary compile/link error or
any ambiguous exit), or `Unavailable` (no jail could be established). It is the
returning counterpart to the run jail, which `exec`s a confined process and never
returns.

Two platform arms of the returning build jail exist and are wired: `Linux/x86_64`
(a bwrap argv plus a seccomp subprocess-deny program) and macOS (a `sandbox-exec`
Seatbelt SBPL profile). Both lower the *same* `SandboxProfile` the run jail runs
under — the confinement is not forked — and both decode the *same* wrapper-owned
per-axis exit-code contract. Off those two targets `build_in_jail` is only a
refuse-stub that returns `Unavailable`, so Tier-2 cannot certify a native package
on any other platform: it refuses to certify, never admits unconfined (ADR 0046).

Windows and FreeBSD are the two platforms named for promotion (the enforcement
design's rollout §6). Both are today refuse-to-certify. Promoting them — and with
them unblocking a native package's admission on those hosts — requires a real
*returning* arm on each: an arm that actually confines the build subprocess along
every axis it claims and returns a structured `JailOutcome`, never a fail-open
stub. This ADR fixes the contract those two arms must satisfy and the per-axis
lowering each uses.

Two forces from the prior arms constrain any new arm:

- **The returning shape differs from the run jail's.** The run jail `exec`s and
  never returns, so it may leak a fd or hold a kernel object for the process
  lifetime. The build jail RETURNS and is called once per removable axis in the
  tightening loop, inside a long-lived audit/CI process. Every kernel object,
  handle, token, SID, or attribute list an arm allocates must be released on
  *every* path, or the audit leaks one per call.

- **No over-claim, per axis (ADR 0049).** An arm reports `Clean` only when the
  jail it established actually confined *every* axis the declared-scoped profile
  withheld. An axis the arm cannot confine is never silently downgraded to
  best-effort: the arm reports a refusal (`Unavailable`) rather than run the
  untrusted build under-confined. The differential signal is only sound if a
  withheld axis is genuinely denied — an unconfined axis would read as `Clean`
  and admit a package that demanded it.

## Decision

Both arms satisfy the same **returning-arm contract**, then lower each of the
four confinement axes onto the host's primitives, failing closed wherever a
primitive — or its precondition — is unavailable.

### The returning-arm contract (both platforms)

1. **Lower the shared profile, unforked.** The arm confines to exactly the
   `SandboxProfile` the run jail lowers on the same host; it does not derive an
   independent profile. What Tier-2 confines a build to and what the shipped
   artifact is confined to at run time cannot drift.
2. **Decode the wrapper-owned exit contract.** The arm waits for the probe and
   decodes its exit code through the *same* `JailOutcome::decode` the wired arms
   use: `0` ⇒ `Clean` (the only positive-proof branch); a recognised per-axis
   code ⇒ `Denied` naming the axis; a signal / no exit code / any unrecognised
   code ⇒ `BuildFailed`. The arm never inspects the untrusted payload's stdout to
   decide the outcome.
3. **Fail closed on every establishment failure.** A missing primitive, an
   unsatisfiable precondition, a profile that cannot be lowered, or a spawn
   failure yields `Unavailable` — the untrusted build is never run unconfined on
   any path. `Unavailable` is a refuse-to-certify, exactly as the refuse-stub was.
4. **Release every resource on every path.** RAII-own every kernel object so no
   error path leaks it and no `Clean` path leaves the child running outside its
   confinement.
5. **Claim only what is confined.** The arm's `Clean` is honest only if every
   withheld axis was denied. An axis the arm cannot confine on this host is a
   refuse-gap that yields `Unavailable`, never a silent `Clean`.

### The Windows arm

Windows lowers the four axes onto Win32 kernel objects, mirroring the run jail's
Windows launcher but *returning* the decoded outcome instead of propagating an
exit:

- **subprocess** — a **Job Object** owns the child, with
  `JOB_OBJECT_LIMIT_ACTIVE_PROCESS = 1` when the axis is withheld (the build is
  confined to a single process, so a subprocess-via-native escape is denied) and
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so closing the job on return kills the whole
  process tree — no orphan survives the audit call. This also caps a runaway
  `build.rs`.
- **filesystem + network** — the child runs under an **AppContainer lowbox
  token**. The container SID is derived once; network is granted by adding the
  internet-client capability SID to the token, withheld by omitting it (a lowbox
  token has no ambient network). Filesystem is confined by ACLing the scratch —
  and, only when the axis is granted, the working tree — to the container SID via
  a DACL, so an out-of-scratch write under a withholding profile is denied.
- **env** — scrubbed in the **launcher**, not by the token: the arm clears the
  inherited environment down to the profile's `env_allowlist` (plus the fixed
  minimal base) before `CreateProcessW`, exactly as the macOS arm does, because
  the token does not scrub env.

**Windows fail-closed points:**

- **The `FILE_PERSISTENT_ACLS` volume probe.** The AppContainer filesystem
  boundary is a *no-op* on a volume that does not persist and enforce DACLs
  (FAT/exFAT, some redirected `%TEMP%`, network shares): `SetNamedSecurityInfoW`
  returns success while enforcing nothing. Before launch the arm probes the
  scratch volume's filesystem flags (`GetVolumeInformationW`); if the volume lacks
  `FILE_PERSISTENT_ACLS`, the arm **refuses** (`Unavailable`) rather than run a
  build whose filesystem axis would be silently unconfined. The pure decision
  (`volume_flags_confine_filesystem`) is already shared with the run jail and
  unit-tested on any host.
- **Any failed construction step** — Job Object, token, capability SID, ACL,
  attribute list, or `CreateProcessW` — is `Unavailable`, never a fallback to an
  unconfined spawn.

### The FreeBSD arm

FreeBSD has no jail arm today in either the run jail or the build jail; this ADR
introduces its returning build-jail arm. It lowers the four axes onto FreeBSD's
capability-mode and jail primitives:

- **filesystem + network + subprocess** — the arm launches a small **entry
  wrapper** that acquires the confinement, then `exec`s the probe. The wrapper
  pre-opens exactly the scratch (and, when the filesystem axis is granted, the
  working tree) as directory descriptors, then enters **capability mode**
  (`cap_enter`): after entry the process has no global namespace, so a filesystem
  path outside the pre-opened descriptors, an outbound socket, and — with the
  descriptor rights restricted — a new process are all denied at the kernel
  boundary. Where a build genuinely needs a broader but still-bounded view than
  capability mode allows, the arm may instead confine the build subprocess in a
  **`jail(2)`** with a network-less `vnet`/`ip4=disable` posture and a
  scratch-rooted filesystem; the choice is an implementation detail of *how* the
  axis is lowered, not of *which* axes are claimed.
- **env** — scrubbed in the **launcher** (capability mode and jails do not scrub
  the inherited environment), exactly as the macOS and Windows arms do.

**FreeBSD fail-closed points:**

- **Capability mode is one-way and total.** Once `cap_enter` succeeds every
  ungranted namespace is denied, so a withheld axis cannot leak. If entering
  capability mode (or establishing the jail) *fails*, the arm refuses
  (`Unavailable`) — it never runs the probe with the confinement not established.
- **A descriptor not pre-opened is denied.** The filesystem axis is confined by
  which descriptors the wrapper pre-opens before entry; the arm pre-opens the
  scratch always and the working tree only when the axis is granted, so an
  out-of-scratch write under a withholding profile is denied — the same
  observable the other arms produce.
- **The subprocess axis is enforced by a kernel denial, not by omission.** A
  withheld subprocess axis must be a genuine kernel denial of process creation.
  Under Capsicum, plain `fork` still returns in capability mode, so the denial
  comes from `exec`/`fexecve` being unreachable and `pdfork` ungranted, or from a
  `jail(2)` posture — not from "we did not grant a way to spawn". Subprocess and
  env are *confined but not differentially probed*: only `Network` and
  `Filesystem` are `Denied { axis }`-nameable, so a denied withheld-subprocess
  surfaces as the killed child's `BuildFailed` (still a reject, still
  fail-closed), not a `Clean`. Omission is the real hazard — it denies nothing,
  running the build unconfined; if the platform cannot deny process creation for
  the withheld case, that is a refuse-gap (`Unavailable`), never a `Clean`.

### Alternatives rejected

- **A single cross-platform confinement abstraction.** No OS primitive maps
  cleanly onto the four axes; forcing one would under-confine an axis on some host
  or over-claim `Clean`. Per-axis lowering keeps each arm honest, exactly as ADR
  0049 decided for the run jail.
- **Trusting AppContainer unconditionally on Windows.** That silently
  under-confines the filesystem axis on a non-ACL volume — precisely the
  over-claim the no-over-claim rule forbids. The volume probe is the price of
  claiming the axis.
- **Degrading to an unconfined build when a primitive is missing.** A returning
  arm that quietly ran the untrusted build unconfined and returned `Clean` would
  admit a package that demanded a withheld axis — the exact hole the whole audit
  exists to close. An absent primitive is a refuse-gap (`Unavailable`), never a
  silent pass.
- **Reusing the run jail's non-returning launcher.** The run jail `exec`s and may
  hold resources for the process lifetime; the build jail is called in a loop in a
  long-lived process and must release every resource on return. The arms share the
  *lowering* but not the launch/return mechanics.

## Consequences

- Once each arm lands and is proven on a real runner, Tier-2 promotes that
  platform from refuse-to-certify to a certifying platform, exactly as the run
  jail's per-target posture promotes — the audit surface then names that platform
  as one whose jail actually ran. Until an arm lands, its platform stays
  refuse-to-certify: `build_in_jail` returns `Unavailable` there and Tier-2 never
  claims a certification it did not run.
- A landed arm is necessary but not sufficient for *audit-layer* promotion. The
  audit's Tier-2 probe drives `build_in_jail` with a POSIX-shell probe wrapper
  through a `/usr/bin/env … /bin/sh` invocation prefix (`audit_native`'s
  `JailProbeRunner`). **FreeBSD** has `/bin/sh`, so its landed arm promotes the
  audit layer mechanically (a `cfg`-gate + a `jail(8)` tool-confirm): FreeBSD
  certifies as `freebsd-x64`. **Windows** runs `payload[0]` directly through
  `CreateProcessW` (no shell), so the shell probe wrapper cannot drive its jail;
  Windows audit-layer certification additionally needs a Windows-native probe
  wrapper (a `.ps1` fixture implementing the same per-axis exit contract) and a
  Windows invocation prefix. Until that lands, Windows stays refuse-to-certify in
  the audit layer even though its `build_in_jail` arm is proven by the
  `windows-tier2` `build_jail_windows_e2e` red-canary.
- The admit predicate is unchanged: a single conjunction over `JailOutcome`
  (`Clean` on the declared-scoped run, no axis removable). Adding a platform arm
  adds a lowering, never a new admit branch — no non-`Clean` outcome may ever reach
  admit.
- On a Windows host whose scratch volume does not persist DACLs, a native package
  needing the filesystem axis is refused rather than certified against an
  unconfined build — surfaced as a refuse-gap, not a silent hole.
- The standing invariant: for any Tier-2 run that reports `Clean`, every axis the
  declared-scoped profile withheld was denied by a concrete primitive whose
  preconditions held at launch — verified per axis, fail-closed on any gap, and
  every allocated resource released on return.

### What the implementation lane's guardian must re-verify

- **No path returns `Clean` without positive proof.** Trace every arm exit; only
  a decoded exit `0` from a jail that was actually established may reach `Clean`.
  Every establishment or precondition failure must be `Unavailable`; every
  ambiguous exit `BuildFailed`.
- **The Windows volume probe gates the launch.** Confirm the arm probes the
  scratch volume and refuses on a cleared `FILE_PERSISTENT_ACLS` bit *before*
  spawning — not after — and that the ACL is applied to the container SID, not a
  no-op.
- **The FreeBSD subprocess axis is a genuine denial.** Confirm a withheld
  subprocess axis denies process creation at the kernel boundary — the chosen
  primitive must block the `fork` family *and* `pdfork`/exec (plain `fork`
  survives `cap_enter` alone), not merely omit a spawn primitive. Its observable
  is the killed child's `BuildFailed`, not a named `Denied { axis }`.
- **Every resource is released on every path**, including the error paths — no
  Job Object, token, SID, descriptor, or jail leaks across the tightening loop's
  per-axis calls.
- **The env scrub matches the shared allowlist** on both arms (launcher-side),
  with no second env list that can drift from the profile's `env_allowlist`.
- **A red-canary fixture rejects on each newly-promoted platform**: a native
  package that opens a socket while declaring `[]` must decode to
  `Denied { axis: Network }` and reject, on that platform's real runner, before the
  platform is advertised as certifying. The canary must demand a differentially
  probed axis (`Network` or `Filesystem`) — a subprocess- or env-based canary
  decodes to `BuildFailed`, not `Denied { axis }`, and would not exercise the
  axis-naming path the fixture guards.

## Conventions

ADRs describe Ipê on its own terms. Do not reference any prior or external
implementation, parity with another system, or project ancestry — state each
decision as a standalone Ipê decision.
