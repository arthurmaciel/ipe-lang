# Windows runtime run-jail: axis → primitive mapping

Status: plan (tbd). Design-only; no code has changed. Companion to the shipped
Linux and macOS arms of the runtime run jail (`ipe_sandbox::run_jail`).

Fenced blocks are illustrative unless the prose says otherwise.

---

## 0. Problem and status

The runtime jail confines the *emitted app* when a user runs `ipe run`: the
binary executes contained to exactly its declared-plus-inferred capability set —
declared effects work, undeclared ones are impossible. `exec_in_run_jail` has a
Linux arm (bubblewrap + a seccomp filter) and a macOS arm (a `sandbox-exec`
Seatbelt SBPL profile plus a launcher-side env scrub). **Windows is a stub.**

Two facts follow from the stub, and both are unsafe-by-omission:

1. `platform_supports_jail()` is `false` on Windows, so `ipe run` runs the app
   **unconfined** — a Tier-2 native wrapper's undeclared syscall is uncontained.
2. The FFI admit path keys off that same predicate (`jail_for_host` →
   `JailForTarget::RefuseGap` on Windows), so a real-capability wrapper is
   **refused** at `ipe add` on Windows rather than admitted-and-isolated. Windows
   users cannot install any network/filesystem/subprocess wrapper at all.

Windows has no single `sandbox-exec` equivalent. Confinement is *assembled* from
several primitives. Every profile axis maps to a Windows primitive that confines
it: `subprocess` and `env` map as cleanly as the Linux/macOS arms; `filesystem`
and `network` are confined by AppContainer, whose one dependency — that the
filesystem carries persistent ACLs — is closed at launch by a runtime probe that
**fails closed**. This document maps each `SandboxProfile` axis to a concrete
Windows enforcement mechanism, specifies the launcher flow, and states how each
axis is confined.

The governing principle, inherited from the macOS arm's review: **`Holds` must
mean every axis the admit path trusts is actually confined by the emitted jail.**
The macOS review caught an over-claim on the `subprocess` and `env` axes (Seatbelt
does not scrub env; the launcher must). Windows honours the same principle by
confining every axis with a primitive and, where a primitive has a precondition
that admit-time cannot verify (the volume must persist ACLs for the AppContainer
filesystem boundary to hold), closing that precondition with a runtime probe that
refuses to launch when it is unmet — see §4.

---

## 1. The axes to map

`SandboxProfile` (in `run_jail.rs`) is a platform-independent value carrying four
confinement axes plus resource limits. The four axes, and the capability each
lowers from:

| Axis | Source capability | Linux mechanism | macOS mechanism |
|---|---|---|---|
| `network` (bool) | `Network`, `Database`(tcp) | fresh empty net namespace | SBPL `(deny network*)` |
| `filesystem` (scope) | `Filesystem`, `Database`(file) | ro-bind `/` + tmpfs masks | SBPL `(deny file-write*)` + scratch allow |
| `env_allowlist` | `Env` | `--clearenv` + re-export | launcher `env_clear` + re-export |
| `subprocess` (bool) | `Subprocess` | seccomp deny of the clone/fork family | SBPL `(deny process-fork)` / `process-exec*` |

`native-ffi` is **not** a fifth axis: it opens no OS control of its own (see
`profile_from_capabilities`, the `NativeFfi` no-op arm). Its effects are reached
through the other axes — a native wrapper makes syscalls, spawns, opens files, all
of which are the axes above — so it is contained exactly when those axes are.
This is stated explicitly in §2.5.

---

## 2. Axis → Windows primitive

### 2.1 subprocess → Job Objects

A **Job Object** is the Windows kernel's process-group container. The launcher
creates a job, assigns the app's process to it before the app's first thread
runs, and sets limits on the job:

- `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — every process in the job dies when the
  launcher's job handle closes. No process outlives the jail; there are no
  escapees.
- `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — cap the process count. When `subprocess` is
  **withheld**, the cap is `1`: the app itself, and no child may be created
  (`CreateProcess` in the job fails once the active count is at the limit). When
  `subprocess` is **granted**, the cap is the profile's `proc_cap`.
- `JOB_OBJECT_LIMIT_BREAKAWAY_OK` and `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` are
  **denied** (never set). Breakaway is the documented escape hatch by which a
  child leaves the job; withholding it means a child cannot escape the container
  even if it asks.

This is a clean map: the "may I spawn?" question is exactly the job's active-process
limit, and containment (kill-on-close, no-breakaway) is structural. Job Objects
are the subprocess axis's honest enforcement. **Holds.**

Nesting note: modern Windows permits nested jobs, so the app being inside a job
does not prevent the launcher assigning it — but the launcher must assign the
process *before* resuming its main thread (create suspended, assign, resume), or a
race lets the first instructions run un-jobbed. See §3.

### 2.2 filesystem + network → AppContainer vs restricted token

Two lower-level primitives can carry the filesystem and network axes. Both are
token-based (the app runs under a modified access token); they differ in *how* they
deny.

**Restricted / lowbox token** (`CreateRestrictedToken`): removes or disables SIDs
and privileges from the launcher's token, and can add *restricting* SIDs so that
every access check is intersected against a deny-heavy set. This is a coarse,
subtractive tool: it denies broadly by stripping the token's authority, but it does
not give a *positive per-resource* model. Network is not a first-class token
concept, so a restricted token cannot cleanly deny outbound sockets — it can only
deny by stripping capability the socket path does not actually consult. Filesystem
denial works, but by ACL/SID intersection, which depends on the on-disk ACLs being
correct — a world-writable path is still writable.

**AppContainer** (a lowbox token with **capability SIDs**): the app runs at the
AppContainer integrity level with an explicit, *additive* capability set. By
default an AppContainer can touch **nothing** outside its own per-container profile
directory. Filesystem access is granted per-path by ACLing the target with the
container's package SID; network is a first-class capability — `internetClient`,
`internetClientServer`, `privateNetworkClientServer` — so **outbound network is
denied unless the capability SID is present**. This is the deny-by-default,
grant-per-axis model the profile wants, and it mirrors the Linux/macOS arms:

- `network` withheld → do **not** grant `internetClient`/`internetClientServer` →
  outbound connect is denied by the AppContainer network isolation. Granted → add
  the capability SID.
- `filesystem` `Isolated` → the app sees only its container profile dir plus the
  one scoped-writable scratch (ACLed to the container SID). `WorkingTreeReadWrite`
  → additionally ACL the working tree for the container SID.

**Choice: AppContainer.** It is the only Windows primitive that models network as a
positive per-capability grant (the restricted token cannot deny sockets cleanly)
and gives per-path filesystem ACLs deny-by-default, so both the `filesystem` and
`network` axes are confined by it. The restricted token is not used for these axes:
it cannot enforce network, so a network wrapper under it would run uncontained.
AppContainer confines both axes, and both are therefore in the confined set (§4).

The one dependency to close: AppContainer filesystem isolation is *ACL-mediated*,
not a namespace remap. It denies-by-default correctly, but the container SID ACL is
a no-op on a filesystem that does not persist ACLs — a FAT/exFAT volume, a
redirected `TEMP`, or some network shares — where the write boundary would not
hold. The Linux `--ro-bind /` + tmpfs mask has no such dependency. The launcher
closes this at run time with a **`FILE_PERSISTENT_ACLS` probe that fails closed**:
before launch it calls `GetVolumeInformationW` on every path it is about to ACL for
the container SID — always the scoped-writable scratch, and the working tree when
that is bound read-write — and if any of those volumes does not report
`FILE_PERSISTENT_ACLS`, the launcher **refuses to run** rather than launch the app
with an unenforced filesystem boundary. The filesystem axis is thus confined on
every volume the app is allowed to run on, because a volume that cannot enforce the
boundary is refused before the app starts.

### 2.3 env → launcher-side scrub before `CreateProcess`

Neither the Job Object nor the AppContainer token scrubs environment variables —
exactly as neither bubblewrap-in-profile nor Seatbelt scrubs env. So the `env` axis
is enforced **launcher-side**, identically to the macOS arm's `macos_scrubbed_env`:

1. Build the child's environment block from scratch (do **not** inherit the
   launcher's), starting from the fixed minimal base the other arms use.
2. Re-export only the profile's `env_allowlist` names, read from the launcher's own
   environment. A named var absent from the host is simply not re-exported.
3. Pass that block explicitly to `CreateProcess` (the `lpEnvironment` parameter);
   never pass `NULL`, which would inherit the launcher's full environment.

The scrub is a pure function of the profile and the host env — the same shape the
macOS launcher already proves — so it is directly portable and **Holds**.

### 2.4 native-ffi → covered by subprocess + token

`native-ffi` widens no axis on its own. The app *is* the emitted binary; native
Rust in a wrapper runs inside that process and reaches the OS only through the same
syscalls the other axes govern. Because the process runs inside the Job Object and
under the AppContainer token, native code:

- cannot spawn past the job's active-process cap or break away (§2.1),
- cannot open a socket without the network capability SID (§2.2),
- cannot write outside the ACLed scratch/working-tree (§2.2).

So `native-ffi` is contained exactly when subprocess + filesystem + network are.
There is nothing extra to build for it; it is covered by the primitives above,
stated here so the admit path's treatment of `NativeFfi` as
admissible-with-consent on a `Holds` target is honest on Windows too.

### 2.5 The mapping, one line each

- **subprocess** → Job Object (`KILL_ON_JOB_CLOSE`, `ACTIVE_PROCESS` cap =
  1-when-withheld, `BREAKAWAY_OK` denied). **Holds.**
- **filesystem** → AppContainer per-path ACL, deny-by-default, scratch + optional
  working-tree ACLed to the container SID, guarded by a launch-time
  `FILE_PERSISTENT_ACLS` probe (`GetVolumeInformationW`) that refuses to run on a
  volume that cannot persist ACLs. **Holds — confined by AppContainer, fail-closed
  on non-ACL volumes.**
- **network** → AppContainer network capability SID (`internetClient*`), absent ⇒
  outbound denied. **Holds — confined by AppContainer.**
- **env** → launcher-side scrub + explicit `lpEnvironment` to `CreateProcess`
  (never inherit). **Holds.**
- **native-ffi** → no dedicated primitive; contained by the Job Object + token via
  subprocess/filesystem/network. **Holds iff those hold.**

---

## 3. The launcher flow

The Windows launcher replaces the Unix `exec`-into-`bwrap`/`sandbox-exec` shape
with a create-suspended → configure → resume sequence, because a token and a job
must be attached *before* the app runs its first instruction. Illustrative order:

```text
1. Parse the ipe.profile + capfloor from the artifact (shared, cross-platform:
   parse_profile / scan_capfloor / satisfies_capfloor already exist).
2. Build the AppContainer token:
   - derive a per-run container SID,
   - capability SID list = { internetClient* iff profile.network },
   - create the lowbox token from the launcher token + those capabilities.
3. ACL the resources the profile grants to the container SID:
   - always: the one scoped-writable scratch dir,
   - iff filesystem granted: the working tree (read-write).
   Before ACLing each, probe its volume with GetVolumeInformationW; if any does not
   report FILE_PERSISTENT_ACLS, refuse to run (the container SID ACL would be a
   no-op there, leaving the write boundary unenforced).
4. Create the Job Object:
   - KILL_ON_JOB_CLOSE, no BREAKAWAY_OK,
   - ACTIVE_PROCESS = 1 when subprocess withheld, else proc_cap.
5. Build the scrubbed environment block (§2.3) — never inherit.
6. CreateProcess the app:
   - CREATE_SUSPENDED | the AppContainer attribute (lpAttributeList carries the
     container SID + capabilities),
   - lpEnvironment = the scrubbed block.
7. AssignProcessToJobObject(job, child) — BEFORE the resume, so no instruction
   runs un-jobbed.
8. ResumeThread(child main thread). Wait; the job handle stays open for the app's
   lifetime so KILL_ON_JOB_CLOSE holds.
```

Fail-closed at every step: a token that cannot be built, a job that cannot be
created, an ACL that cannot be set, or a `CreateProcess` that fails ⇒ **refuse to
run**, never fall back to an unjailed `CreateProcess`. This mirrors the Linux arm's
"absent primitive ⇒ `RunJailDefect::PrimitiveUnavailable`, refuse" and the macOS
arm's "absent `sandbox-exec` ⇒ refuse".

Note the launcher does not `exec`-replace itself (Windows has no `exec`); it stays
alive as the job owner and propagates the child's exit code. That is a structural
difference from the two Unix arms, not a confinement gap.

---

## 4. Single-source predicate reuse and axis honesty

### 4.1 The `on_jailed_target!` predicate must include Windows only when it holds

The Linux/macOS arms are stamped by the single-source `on_jailed_target!` macro:
`platform_supports_jail()` returns `JAIL_COMPILED_IN`, and the real
`exec_in_run_jail` arm and the stub arm are gated on that same predicate and its
negation. The FFI admit path can therefore never claim a jail that is not compiled
in — fail-closed by construction. **The Windows arm MUST reuse this pattern:** if
Windows is added to the `yes:` set of `on_jailed_target!`, then the real Windows
`exec_in_run_jail` arm must compile, and `platform_supports_jail()` becomes `true`
on Windows — the two cannot drift, and the
`platform_supports_jail_matches_the_compiled_in_jail_arm` test keeps them in lock.

### 4.2 The admit-time verdict and the run-time boundary check

`JailForTarget` is an all-or-nothing boolean at admit time: `Holds` (every
runtime-enforced axis contained) or `RefuseGap` (none). That is right for Linux and
macOS, where a single primitive (`bwrap`+seccomp / `sandbox-exec`) either
establishes and confines *all* axes or is absent. Windows meets the same bar: the
Job Object confines `subprocess`, the launcher scrub confines `env`, and
AppContainer confines both `filesystem` and `network`, so all four axes are in the
confined set and the Windows verdict is `Holds`.

The one axis whose primitive carries a run-time precondition is `filesystem`: the
AppContainer ACL only binds on a volume that persists ACLs. That precondition is
not an admit-time property of the wrapper — the same wrapper is contained on an
NTFS working tree and would not be on a redirected FAT `TEMP` — so it is not
resolved by weakening the admit verdict, but by the launcher's `FILE_PERSISTENT_ACLS`
probe (§2.2, §3), which **fails closed**: if any volume it is about to ACL cannot
persist ACLs, the launcher refuses to run. The admit path trusts the filesystem
axis, and the run-time probe guarantees the boundary the admit path trusted is
actually enforced — the app never runs with an unenforced filesystem boundary.

### 4.3 How each axis is confined

- `subprocess` → Job Object (active-process cap, no breakaway). Confined.
- `env` → launcher-side scrub + explicit `lpEnvironment`. Confined.
- `filesystem` → AppContainer per-path ACL, deny-by-default, guarded at launch by
  the `FILE_PERSISTENT_ACLS` probe that refuses to run on a volume that cannot
  persist ACLs. Confined; fail-closed rather than run with an unenforced boundary.
- `network` → AppContainer network capability SID; absent ⇒ outbound denied.
  Confined.

No axis is ever silently run unconfined. Where a run-time condition would leave a
boundary unenforced (a non-ACL volume for the filesystem axis), the launcher
refuses before the app starts rather than over-claim — the honest per-target
posture, identical in spirit to the pre-jail refuse-until-jail rule.

---

## 5. CI provability: hosted vs self-hosted, and the duality per axis

The macOS run-jail proof is a dedicated CI job on a real `macos-latest` runner that
(1) asserts the primitive is present and refuses-to-certify if absent, and (2) runs
an **enforce-vs-control duality**: a forbidden action must SUCCEED unjailed
(positive control, proving the action is really possible) and be DENIED under the
jail (enforce, proving the denial came from the jail, not a missing capability).
Windows needs the same, per axis.

### 5.1 What a hosted `windows-2022` runner can prove

The GitHub-hosted `windows-2022` image can create Job Objects, restricted tokens,
and AppContainer profiles directly — **none of these need a Windows-container Docker
daemon.** This is the key distinction from the separate admission/build-jail Windows
proof (the per-platform admission gate design), which runs the untrusted *build*
inside a Docker Windows container and therefore needs the container runtime. The
*runtime* run jail here is a launcher API sequence, not a container, so it is
provable on a plain hosted runner.

Provable on hosted `windows-2022`, each as an enforce-vs-control pair:

- **subprocess** — control: a child `CreateProcess` succeeds outside the job.
  Enforce: inside a job with `ACTIVE_PROCESS = 1` and no breakaway, the child
  `CreateProcess` fails / the child is killed on job close.
- **network** — control: an outbound connect succeeds outside the AppContainer.
  Enforce: inside an AppContainer without `internetClient`, the same connect is
  denied. (Requires the runner to permit an outbound connect at all under control;
  if the hosted runner blocks egress network globally, the positive control is
  invalid and this axis moves to §5.2.)
- **filesystem** — control: a write outside the scratch succeeds under the launcher
  token. Enforce: under the AppContainer token, the out-of-scratch write is denied.
  Must run on an NTFS work dir so the container SID ACL is meaningful (the hosted
  image's default work volume is NTFS, so this holds).
- **env** — control: an inherited variable is visible to a child spawned with
  `lpEnvironment = NULL`. Enforce: with the scrubbed block, a non-allowlisted
  variable is absent from the child's environment. Pure user-mode; always provable
  hosted.

### 5.2 What might need a self-hosted runner

- **network under a genuinely-permissive egress control.** If the hosted runner's
  network policy blocks or intercepts outbound connections, the *positive control*
  (the connect must succeed unjailed) cannot be established, and a duality with a
  dead control proves nothing. That axis's proof then needs a runner with real
  outbound egress — a self-hosted Windows runner, or a hosted job explicitly
  confirmed to allow the control connection. The enforce half is always hosted-safe;
  only the control half is at risk.
- **non-ACL-volume fail-closed refusal (§2.2).** Proving that the launcher refuses
  to run when the scratch/working-tree volume cannot persist ACLs needs a
  non-ACL-persisting volume (FAT/exFAT) attached, which a hosted image does not
  provide by default. This is a *negative* proof (the launcher refuses there rather
  than run with an unenforced boundary), so it can be a unit/integration test with a
  mounted VHD image rather than a live volume — but if that cannot be arranged
  hosted, it is self-hosted.

Everything else — subprocess, env, and the enforce half of network and filesystem —
is provable on a plain hosted `windows-2022` runner with no Docker daemon. The job
follows the macOS pattern: assert the primitives are constructible and
**refuse-to-certify** (hard-fail, never skip-green) if a required primitive is
absent, then run the per-axis duality.

---

## 6. Boundaries / out of scope

- **This is the runtime run jail, not the admission/build jail.** The per-platform
  admission-gate design confines an untrusted package's *build + capability probe*
  inside a jail (on Windows, a Docker Windows container + restricted token / job
  object) as the index's admission gate. That is a *separate concern* with a
  *separate proof* (it needs the container runtime; this does not). This document is
  only about `exec_in_run_jail` — the jail around the *already-admitted, emitted
  app* at `ipe run`. The two share vocabulary (Job Objects, AppContainer) but not
  code paths or CI jobs.
- **Resource limits** (`RunResourceLimits`) are not a confinement axis and are not
  mapped here; the Job Object can additionally carry memory/CPU limits, which is a
  refinement, not part of the axis→primitive contract.
- **The async-ffi-bridge design** governs how a wrapper's async work is driven; it
  is orthogonal to confinement and unaffected by this arm.
- **No implementation.** This is a design; the arm, its tests, and the CI job are
  the next round's work.
