# Admission gate: per-platform sandbox matrix

Status: plan (tbd). Extends the shipped SP4 package gate — its decision is
recorded in `docs/adr/0044-package-coordination-manifest-index-gate.md` (the
where-it-runs, sandboxed-native-build, and fail-closed-isolation aspects) — and
complements ADR 0040 / ADR 0041.

Fenced blocks are illustrative unless the prose says otherwise.

---

## 0. Context — why per-platform, and why off-the-shelf

The SP4 gate answers one question: *is this package version safe and honest enough
for the index to serve it?* It runs the untrusted package's build + test + capability
probe inside the `ipe_sandbox` RCE jail. That jail is **Linux-only** (bubblewrap +
seccomp). Two blind spots follow:

1. **Per-OS behaviour is unseen.** A package's native Rust can behave differently per
   platform — a `cfg(windows)` `build.rs`, a platform-specific syscall, an
   `#[cfg(target_os)]` code path. A Linux-only sandboxed build and capability probe
   never exercises those paths. ADR 0040 named exactly this: a single-platform trace
   can miss `cfg`-gated native paths.
2. **We admit for platforms we never tested.** The project ships binaries for five
   targets — Linux x64, Linux arm64, macOS arm64, Windows x64, FreeBSD x64. A version
   admitted only against Linux may be broken, or malicious, on a platform a user
   actually runs.

The governing decision (see ADR 0041): **admission is our responsibility; execution
is the user's.** So the admission gate is where we spend effort without limit — and
the instruction is to run it **hard and redundant**: *better to be redundant than to
miss an OS-specific difference or escape.*

**Non-goal, stated up front: we do not build a sandbox.** This plan wires **existing,
off-the-shelf** jails per platform into the index CI. No in-house confinement library.

---

## 1. Decision

The **index-repo CI** — the authoritative gate — runs the SP4 audit as a **matrix over
every platform the project builds a binary for**. Each matrix job executes the
untrusted package's build + test + capability probe **inside an existing third-party
jail** for that OS. A version is admitted **iff the gate is green on every platform**.

Fail-closed, on two axes:

- **Any platform red ⇒ reject** (never "green on Linux, warn on the rest").
- **A jail that cannot establish on a platform ⇒ reject** that platform — never run
  the untrusted build unjailed and admit anyway. (This is the admission-side mirror of
  ADR 0041's "no TTY = cannot obtain consent = refuse".)

### 1.1 The matrix

| Platform | Off-the-shelf jail (primary) | Redundant 2nd layer | Hosting |
|---|---|---|---|
| Linux x64 / arm64 | bubblewrap **or** nsjail | container job `--network none` + seccomp profile | GitHub-hosted |
| macOS arm64 | `sandbox-exec` (Seatbelt SBPL profile) | runner-level network deny | GitHub-hosted |
| Windows x64 | Docker Windows container (process isolation) + restricted token / job object | AppContainer launcher | GitHub-hosted |
| FreeBSD x64 | `jail(8)` + Capsicum | — | Cirrus CI or `vmactions/freebsd-vm` |

Every jail, whatever the platform, enforces the same contract: no network, a read-only
filesystem except one scratch dir, resource + wall-clock caps, a non-root user, and a
scrubbed environment. The untrusted build/test runs inside; a syscall/effect tracer
records what it *attempts* — that record is the SP4 §1b/§2 capability diagnostic
(declared-vs-demanded).

**Redundant layers are AND-composed.** On Linux, "no network" is enforced by *both*
the container's `--network none` *and* the inner jail's seccomp socket-deny; a bypass
of one is still caught by the other. Redundancy is the point, not an accident.

### 1.2 GitHub Actions constraints the design must respect

1. **No netns loopback on hosted runners.** GitHub runners deny loopback configuration
   (`RTM_NEWADDR`) inside a `--unshare-net` network namespace — the same wall that
   reddened `static.yml`. So Linux jobs must deny network via a **container
   `--network none`** job or **seccomp socket-deny**, *not* by unsharing a net
   namespace. Jail-tool selection is dictated by what actually works on the hosted
   runner, and each choice is verified before it is committed.
2. **No GitHub-hosted BSD.** Linux/macOS/Windows are hosted; FreeBSD is not. The
   FreeBSD admission job runs on **Cirrus CI** (native FreeBSD images) or a
   `vmactions/freebsd-vm` VM on an ubuntu host — a second CI-provider / infra decision,
   not just another matrix row. (The release workflow already builds FreeBSD in a
   `vmactions` VM; that infra can be reused.)

### 1.3 What this closes

- **ADR 0040's gap** — `cfg(os)`-gated native paths a single-platform trace misses are
  exercised on each platform's own runner.
- **SP4's implicit single-platform blind spot** — the sandboxed native build (§2) and
  the fail-closed isolation (§3) now run per platform, not only on Linux.

---

## 2. Relationship to SP4

This plan does not replace SP4; it changes *where SP4 §2/§3 run* — from one Linux jail
to a per-platform matrix — and keeps everything else:

- SP4 §1 (universal Tier-1: provenance panic-scan, capability consistency, enforced
  semver, supply chain) is platform-independent and runs once; only the native
  build/test/probe fans out across the matrix.
- SP4's `ipe package audit` local pre-flight stays single-platform (the author's own
  OS) as a convenience; the index CI matrix is the authoritative multi-platform gate.
  Author pre-flight and index CI still call the same audit library, so they cannot
  diverge on the checks themselves — they diverge only in platform coverage, which is
  the intended asymmetry.

---

## 3. Phasing

Each platform is independent and lands on its own; the gate treats a not-yet-wired
platform as a **documented gap**, never a silent skip.

1. **Linux job** — container `--network none` outer + bubblewrap/nsjail inner. Where CI
   already lives; validates the layered pattern; wired into the SP4 index CI first and
   made blocking.
2. **macOS job** — `sandbox-exec` with an SBPL profile.
3. **Windows job** — Docker Windows container (verify process-isolation availability on
   the hosted windows runner) or a restricted-token / job-object wrapper.
4. **FreeBSD job** — Cirrus CI or `vmactions/freebsd-vm`, reusing the release FreeBSD
   infra; `jail(8)` + Capsicum.

Promotion rule: a platform is **advisory** until its job is wired and proven, then
**promoted to blocking**. Linux is blocking from day one; each later platform blocks as
it lands. The set of blocking platforms is recorded, so "admitted" always names exactly
which platforms vouched for the version.

---

## 4. Non-goals

- **Building an in-house sandbox.** Off-the-shelf jails only.
- **Runtime confinement of end-user `ipe run`.** That is ADR 0041 — user-consented,
  opt-in, and deliberately *not* mandatory. Admission (this plan) and execution (0041)
  are the two halves of the trust model and must not be conflated.
- **Per-capability seccomp syscall maps.** Coarse isolation (namespaces/containers +
  resource caps) is v1; a fine-grained capability→syscall filter is the SP4 §3 v2
  follow-up, not required for this matrix to ship.

---

## 5. Open questions

- **FreeBSD CI provider** — Cirrus CI (native images, another provider to wire) vs a
  `vmactions` VM on ubuntu (reuses release infra, slower). Decide on cost/latency.
- **Windows containment** — confirm Docker Windows process-isolation containers run on
  the hosted `windows-latest` runner; if not, fall back to a restricted-token /
  job-object launcher.
- **Blocking from day one vs incremental** — require all five platforms green before
  any admission, or Linux-blocking first with the rest promoted as they land?
  Recommendation: **incremental** (Linux blocking now, others promoted per §3), so the
  gate ships without waiting on the BSD/Windows infra, while never silently under-
  covering.
