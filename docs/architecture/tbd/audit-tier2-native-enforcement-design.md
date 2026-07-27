# `ipe package audit` — Tier-2 native-code capability enforcement

Status: design (tbd). Specifies the second tier of the package gate: a
sandboxed, per-platform native build whose *observed* capability behaviour is
compared, fail-closed, against the package's *declared* capability set. Extends
the shipped Tier-1 gate (`src/ipe-cli/src/audit.rs`, recorded in
`docs/adr/0044-package-coordination-manifest-index-gate.md`) and the per-platform
admission substrate (`docs/architecture/tbd/admission-gate-per-platform-sandbox.md`,
ADRs 0040 / 0041).

Fenced blocks are illustrative unless the prose says a fixture ran.

---

## 0. The one question Tier-2 answers

Tier-1 proves capability honesty *only over the Ipê-inferable set*: for a pure
Ipê package the compiler's inferred union must equal the manifest's declared set
(`capability_consistency`, `audit.rs:428`). But a package that crosses into
`Rust.` code carries the `native-ffi` axis, and Tier-1 explicitly cannot see
past that marker — it prints "its native effects cannot be inferred from Ipê
alone" (`audit.rs:437`) and admits the declared set on the author's word. ADR
0044 §Consequences states this honestly: *native code's true set is declared and
enforced at runtime, not proven by this gate.*

Tier-2 is the check that turns "declared on the author's word" into "observed
under confinement and reconciled against the declaration". It answers:

> When this package's native code is actually built and exercised inside a jail
> scoped to *exactly its declared capabilities*, does it stay within that set —
> and is every declared capability actually needed?

A native package is admitted iff the observed behaviour equals the declared set.
Any divergence, any jail that cannot be established, and any build/test that
fails inside the jail are all hard rejects.

---

## 1. Scope + prerequisites verdict

### 1.1 What Tier-2 must do

1. For each platform the project ships a binary for, **build (and where a package
   defines them, run its tests / a capability-probe entrypoint over) the
   package's native/FFI code inside that platform's off-the-shelf jail**, reusing
   the admission substrate (`scripts/admission/jail-*`,
   `.github/workflows/admission-sandbox.yml`) — never a re-invented sandbox.
2. Scope that jail to the package's **declared** capability set (lowered through
   the same `SandboxProfile` path `ipe run` uses,
   `src/ipe-cli/src/run_sandbox.rs:101` → `run_jail::profile_from_capabilities`).
3. **Observe** what the native code demands relative to that scoped jail and
   **reconcile** observed-vs-declared, fail-closed on any mismatch.
4. Extend `ipe package audit` and the index admission CI to run this tier for
   native-bearing packages, and advertise it in help/output **only** once it
   genuinely runs.

### 1.2 Prerequisite verdict: **PARTIAL — the containment substrate exists; the
*observation* primitive does not.**

The three load-bearing pieces split cleanly:

- **EXISTS — static wrapper capability inference.** `capability_scan.rs`
  (`src/compiler/ffi/src/`, FFI Tier-2 §5, reviewed and approved under the FFI
  Tier-2 capability gate) lexes author wrapper Rust and *proposes* an
  over-approximate capability set, failing closed on any unenumerable construct
  (`extern`, `#[link]`, `libc::`, `include!`, `#[path]`, non-lexing source). This
  is an author-time *install* gate that refuses a wrapper reaching any
  runtime-unenforceable capability. It is a proposer, **not** a declared-vs-demanded
  reconciler, and it runs on source, not on a built+exercised artifact.

- **EXISTS — the containment jails.** The runtime jail (`run_jail.rs`, ADR 0040)
  confines the *run* of the emitted binary and lowers a capability set to a
  bwrap+seccomp profile; the admission substrate (`jail-linux.sh` et al.) jails
  an arbitrary *build* command per platform and proves the isolation contract
  (no net, ro-fs-except-scratch, caps, non-root, scrubbed env) with fixed
  positive/negative probes.

- **MISSING — the effect-observation primitive.** The admission-gate design (§1)
  *claims* the jail runs "a syscall/effect tracer [that] records what it attempts
  — that record is the SP4 §1b/§2 capability diagnostic (declared-vs-demanded)".
  **That tracer does not exist.** Evidence:
  - `scripts/admission/jail-linux.sh` + `tests/fixtures/admission/untrusted-build.sh`
    only *deny* and assert **fixed** net/fs-escape probes (exit codes 2–5). There
    is no enumeration of what the build demanded, and no declared-vs-demanded
    output.
  - The seccomp filter (`src/compiler/sandbox/src/seccomp.rs:84`) emits only
    `SECCOMP_RET_KILL_PROCESS`, `SECCOMP_RET_ERRNO(EPERM)`, and
    `SECCOMP_RET_ALLOW` — pure deny/allow. There is no `SECCOMP_RET_LOG`,
    `SECCOMP_RET_TRACE`, audit log, or ptrace-based observer anywhere in
    `src/compiler/sandbox/`.
  - `audit.rs` never references the jail; its native handling ends at the
    `native-ffi` note (`audit.rs:437`).

**Consequence for the design.** "Observe the capabilities the sandbox exposes"
cannot be read as *enumerate demanded syscalls* — no primitive produces that
enumeration, and building one (a robust cross-platform syscall tracer classified
into capability axes) is a large, security-sensitive subsystem in its own right.
So Tier-2 must be designed around the primitive we *do* have: a **deny-only jail
scoped to the declared set**. That yields a sound observation by *differential
confinement* (§2.2), not by tracing.

### 1.3 One PR or a campaign?

**A campaign.** This is not a single implementable task: it depends on a missing
observation primitive and on the still-maturing per-platform admission matrix
(only Linux is wired and blocking; macOS/Windows/FreeBSD are documented gaps —
admission-gate §3). Split as in §6. The first sub-PR (Linux, differential-deny)
is independently shippable and fail-closed; each later platform promotes as its
jail lands, exactly mirroring the admission-gate promotion rule.

---

## 2. The enforcement design

### 2.1 Where it runs, and on what

Tier-2 is a **new fifth check** appended to `run_audit` after the four Tier-1
checks, gated on native-bearing packages. The schematic order (illustrative):

```
provenance → capability(Tier-1) → semver → supply_chain → native_tier2(NEW)
```

`native_tier2` is a **no-op skip with a printed note** unless
`declared.contains(NativeFfi)` — reusing `is_native_bearing`
(`run_sandbox.rs:52`). A pure Ipê package is structurally bounded by inference
(ADR 0040) and Tier-1 already proved it exactly; it needs no jailed build.

Authoritative execution is the **index CI matrix** (admission-gate §1): the same
`run_audit` library runs per platform. The author's local `ipe package audit`
runs Tier-2 only for the author's own OS, as a pre-flight convenience, and says
so — the local run can never *admit*; only the CI matrix admits.

### 2.2 Observation by differential confinement (the crux)

Because no tracer exists, "used-but-undeclared" is detected by **building and
exercising the native code inside a jail scoped to exactly the DECLARED set** and
observing the *outcome*:

- **Build under the declared-scoped jail.** Lower `declared` → `SandboxProfile`
  (the `run_run` path, `run_sandbox.rs:101`), establish the platform jail with
  that profile, and run the package's `cargo build` + any package-declared test /
  capability-probe entrypoint inside it.
  - If it **succeeds**, the native code did not need any capability the declared
    set withholds → no used-but-undeclared axis. (A denied syscall inside the
    jail surfaces as a build/test failure or a probe non-zero exit.)
  - If it **fails specifically because a withheld capability was denied**, that is
    a **used-but-undeclared** reject. Distinguishing a capability-denial failure
    from an ordinary compile error is the delicate part (§2.4).

- **Detect declared-but-unused by tightening.** For each declared axis, re-run the
  build+probe under a jail with *that one axis removed*. If the run still
  succeeds with the axis withheld, that axis was **declared-but-unused** → reject
  (an over-broad claim, per the Tier-1 §1b posture). Clock and random carry no OS
  control and are exempt (they are the only admissible-without-jail axes, matching
  `resolve_refusal`'s clock/random exemption, `run_sandbox.rs:144`). This
  tightening loop is O(declared axes) extra jailed builds; acceptable because
  admission spends effort without limit (admission-gate §0) and the axis set is
  tiny (≤ 6).

This is strictly weaker than a true tracer (it observes *reachability under
denial*, not *intent*), but it is **sound in the fail-closed direction**: it can
only *reject* a package a tracer would admit (a capability compiled-but-never-hit
at probe time reads as unused), never *admit* one a tracer would reject. Over-
rejection is the correct bias (PRINCIPLES §Security: absent proof, take the
conservative branch). The honest-surface note (§3) states this limitation.

### 2.3 The fail-closed matrix

| Situation | Verdict | Rationale |
|---|---|---|
| **declared-but-unused** (axis removable, build+probe still passes) | **REJECT** | Over-broad claim; the declared set must be exactly the consent surface (Tier-1 §1b). Clock/random exempt. |
| **used-but-undeclared** (build/probe fails *by capability denial* under the declared-scoped jail) | **REJECT** | Hidden effect the consumer never consented to. |
| **sandbox-unavailable** (no jail primitive on a platform that *should* have one) | **REJECT that platform** | Admission-side mirror of ADR 0041 "no consent ⇒ refuse". Never run the untrusted build unjailed and admit. |
| **build-fails-in-jail** (non-capability compile/link/test error) | **REJECT** | A package whose native code cannot build+pass its own probe under confinement is not admissible; the diagnostic distinguishes this from a capability denial (§2.4). |
| **platform not yet wired** (jail backend genuinely absent, e.g. Windows on hosted CI) | **DOCUMENTED SKIP, never silent** | §3.3 — refuse-to-certify that platform, record it, do not mark the version admitted for it. |
| declared == observed on every wired platform | **ACCEPT** | The only admit path. |

There is **no** warn-and-pass row. Every non-accept outcome is a typed
`Rejection` (extend the `Check` enum with `Check::NativeTier2`) or a recorded
platform-skip; the CLI boundary prints and exits non-zero.

### 2.4 Distinguishing capability-denial from an ordinary build failure

This is the single most security-load-bearing mechanic, because misclassifying a
capability denial as "ordinary build error" (or vice-versa) breaks the matrix.
Design it as **parse, don't validate** over the jail outcome. The typed outcome
shape (illustrative, not a committed signature):

```rust
// illustrative — the jail run yields a typed outcome, never a bare exit code
struct JailOutcome {
    established: bool,
    exit: ExitStatus,
    denials: Vec<DeniedAxis>, // populated from the wrapper's exit contract
}
```

- `denials` is populated from the **enforcement primitive's own signal**, not by
  scraping stderr text:
  - Linux: run the probe's forbidden actions as the admission fixture already
    does (explicit net-connect / fs-escape attempts return distinct exit codes
    2–5); extend the probe to a per-axis exit code so a denial names *which* axis
    tripped. A general syscall EPERM inside the build is treated as
    build-fails-in-jail unless it maps to a known probe axis — fail-closed.
  - The probe is authored by *us*, not the package, and lives in the jail wrapper
    — the untrusted package cannot forge a "no denial" signal because the signal
    is the wrapper's exit-code contract, not the package's stdout.
- A denial on a withheld axis ⇒ used-but-undeclared. A non-zero exit with **no**
  recognised denial ⇒ build-fails-in-jail. Both reject; the distinction only
  shapes the diagnostic. Ambiguity (unrecognised failure) ⇒ reject (fail-closed).

`JailOutcome` makes the illegal state — "admitted despite a denial" — unrepresentable:
`native_tier2` returns `Ok(())` only for `established && exit.success() &&
denials.is_empty()` across the declared-scoped run, plus the tightening loop
proving no axis is removable.

---

## 3. THE SEAL / honest surface

### 3.1 The seal

**Tier-2 must never admit a native package whose code, when built and exercised
under a jail scoped to its declared set, escapes that set.** Concretely, the
admit path is a single conjunction (§2.4) and every other branch is a typed
reject. The seal is enforced structurally, not by a comment:

- The admit predicate lives in one place and is total over `JailOutcome`.
- `Check::NativeTier2` reject is a `Rejection` value, inspectable by tests, like
  every Tier-1 reject.
- The declared-scoped profile is lowered by the **same** `profile_from_capabilities`
  the runtime jail uses — a drift between "what Tier-2 confines" and "what the
  shipped artifact is confined to at `ipe run`" is impossible by construction
  (single source of the lowering).

### 3.2 Honest help / output

`ipe package audit` today advertises only Tier-1 (`audit.rs:1` module doc, the
"all Tier-1 checks passed" line at `audit.rs:145`, and the explicit "Deferred …
blocked on FFI Tier 2" note at `audit.rs:35`). Per the honest-surface rule
(never advertise unimplemented), the help/output MUST NOT mention Tier-2 until it
genuinely runs. The rollout is strictly monotone:

- Before the first Tier-2 sub-PR merges: no change to the surface.
- When Linux Tier-2 lands: the passing line names the platforms Tier-2 actually
  ran on ("Tier-2 native check passed on: linux-x64") and the deferral note is
  narrowed to the platforms not yet wired — never dropped wholesale.
- The `native-ffi` note (`audit.rs:437`) is *replaced* by the real Tier-2 verdict
  only on platforms where the check ran; on unwired platforms the honest
  "cannot certify" note remains.

### 3.3 Unverified / absent platforms

- **Windows on hosted CI:** the admission substrate already treats this as a
  **documented UNVERIFIED skip** (no Windows-container Docker daemon on hosted
  runners). Tier-2 inherits exactly that posture: the version is **not** marked
  admitted-for-windows; the job summary carries the loud "UNVERIFIED" note; the
  set of platforms a version is admitted for **names windows only if a
  Windows-container-capable runner actually ran the jail**. This is
  refuse-to-certify, not fail-open: an un-run platform is never counted as vouched.
- **Any wired platform whose jail fails to establish at run time:** hard reject
  that platform (sandbox-unavailable row, §2.3) — distinct from "not wired".
- **The differential-confinement limitation** (§2.2: a capability compiled but
  not exercised by the probe reads as unused) is stated in the doc and in the
  reject diagnostic for declared-but-unused, so an author is never misled about
  what the check proves.

---

## 4. PRINCIPLES analysis (precedence order)

1. **Security (highest).** The whole tier is a supply-chain / RCE-adjacent gate.
   Every uncertain branch rejects: sandbox-unavailable, ambiguous jail failure,
   unrecognised denial, un-wired platform → all conservative. The untrusted native
   build **never** runs outside a jail on an admitting path (the ADR 0041 mirror).
   The observation is deliberately *over-rejecting* (§2.2) so no undeclared
   capability can slip through as "not observed". The probe signal is
   wrapper-owned, so the untrusted package cannot forge a clean result.
2. **Correctness.** The declared-scoped profile is lowered by the *same*
   `profile_from_capabilities` as the runtime jail, so Tier-2's confinement and
   the shipped artifact's confinement cannot diverge. The admit predicate is a
   single total function over `JailOutcome`; the tightening loop is deterministic
   over the (sorted) declared axis set.
3. **Soundness.** No new panic surface: the jail run returns typed `JailOutcome`
   / `RunJailDefect`, propagated with `?`; no `unwrap`/index/cast on the outcome
   path (matching the existing `run_sandbox.rs` discipline, which is
   `#![forbid(unsafe_code)]` and unwrap-free outside `#[cfg(test)]`). `JailOutcome`
   makes "admitted despite denial" unrepresentable.
4. **Efficiency.** Yielded to Security: the O(axes) tightening builds cost extra
   jailed compiles. Bounded (≤ 6 axes, ≤ tiny), and admission "spends effort
   without limit" by charter (admission-gate §0). Optional optimisation: skip the
   tightening pass for an axis the *static* wrapper scan (`capability_scan.rs`)
   already proves reached — but only as a *skip of a redundant tighten*, never as
   grounds to admit.
5. **Completeness.** Yielded: only wired platforms certify; the rest are
   documented gaps, never silent. The differential-confinement observation is
   less complete than a true tracer, and that gap is stated (§3.3), tracked, and
   safe (it only over-rejects).
6. **Readability.** The fifth check mirrors the shape of the existing four
   (typed `Check` variant, `Rejection`, one focused function), so the gate stays
   uniform.

---

## 5. Implementation + verification plan

### 5.1 Files to change / add

- `src/ipe-cli/src/audit.rs` — add `Check::NativeTier2`; append `native_tier2`
  after `supply_chain` in `run_audit`; gate on `is_native_bearing(declared)`;
  emit the platform-scoped passing line; narrow (never drop) the deferral note.
- `src/ipe-cli/src/run_sandbox.rs` (or a new `audit_native.rs`) — a
  `build_in_jail(profile, emitted_dir, probe) -> Result<JailOutcome, CliError>`
  wrapping the admission jail for the *build* (not the run) path, and the
  differential-confinement reconciler.
- `src/compiler/sandbox/` — a `JailOutcome` type and, if needed, a build-jail
  entry distinct from `exec_in_run_jail` (the run jail replaces the process;
  the audit build jail must *return* an outcome).
- `scripts/admission/jail-*` + `tests/fixtures/admission/untrusted-build.sh` —
  extend the probe to emit **per-axis** denial exit codes (so a denial names the
  axis), keeping the existing enforce/control duality.
- `.github/workflows/admission-sandbox.yml` — invoke the audit build+probe under
  each platform's jail and feed the outcome into the index CI verdict.
- `docs/adr/` — a short ADR recording "Tier-2 = differential-confinement, not
  tracing" and the per-platform certify/skip semantics.

### 5.2 Fixtures (positive + negative controls — mandatory)

- **NEGATIVE (must REJECT) — used-but-undeclared:** a package declaring
  `[capabilities] = []` (or only `clock`) whose native wrapper opens a
  `TcpStream` / reads a file at probe time. Under the declared-scoped jail the
  probe hits a denial → `Check::NativeTier2` reject naming the network/fs axis.
  This is the seal's canary; it MUST stay red if the check regresses.
- **NEGATIVE (must REJECT) — declared-but-unused:** a package declaring
  `network` whose native code never touches the network; the tightening pass
  removes `network` and the build+probe still passes → reject (over-broad).
- **NEGATIVE (must REJECT) — sandbox-unavailable:** simulate a missing jail
  primitive on a should-be-wired platform → reject that platform, not skip.
- **POSITIVE (must ACCEPT) — benign native package:** declares exactly the axes
  its native code exercises; build+probe pass under the declared-scoped jail and
  no axis is removable → admit.
- **POSITIVE control (isolation is real):** the existing admission
  enforce-vs-control duality (fixture PROBE_MODE) proves a denial comes from the
  jail, not an unreachable host — reuse it unchanged so a "clean" Tier-2 result
  can never be a false pass from a broken jail.
- **Pure-Ipê package:** Tier-2 skips with a note; Tier-1 still fully gates it.

Fixtures live beside the existing audit tests; the negative used-but-undeclared
fixture is wired as a **standing red canary** in CI so a future weakening of the
admit predicate is caught immediately (the FFI Tier-2 gate uses the same
canary pattern).

### 5.3 Integration with the index admission gate

Tier-1 checks stay universal and run once (admission-gate §2). Only
`native_tier2` fans out across the platform matrix; a version is admitted iff
every *wired, blocking* platform is green **and** Tier-2 reconciles on each. The
set of blocking platforms is recorded with the admitted version, so "admitted"
always names exactly which platforms vouched — inheriting the admission-gate
promotion rule verbatim.

---

## 6. Decomposition (ordered, each independently shippable + fail-closed)

1. **Prereq PR — build-jail outcome primitive.** Add `JailOutcome` and a
   *returning* build-jail entry in `ipe_sandbox` (distinct from the process-
   replacing `exec_in_run_jail`); extend the admission probe to per-axis denial
   exit codes. No audit wiring yet. Ships with unit + fixture tests proving the
   probe names the axis. Fail-closed: an unrecognised failure ⇒ ambiguous ⇒ the
   primitive reports "denied/unknown", never "clean".
2. **Tier-2 Linux, differential-deny.** Wire `Check::NativeTier2` into
   `run_audit` gated on native-bearing; declared-scoped build+probe + tightening
   loop; the two negative + two positive fixtures (§5.2). Advertise Tier-2 for
   `linux-x64` only. This is the first admitting improvement and stands alone.
3. **macOS Tier-2.** Same reconciler over the `sandbox-exec` SBPL jail; promote
   macOS from advisory to blocking; extend the passing line. **Wired.** The macOS
   `build_in_jail` lowers the SAME `SandboxProfile` to a Seatbelt SBPL profile
   (`sbpl_from_profile`) enforced by `sandbox-exec`, decoding the SAME per-axis
   exit-code contract; the reconciler is unchanged. The SBPL lowering is pure
   text, unit-tested on any host; the jail's runtime deny behaviour is the
   `macos-tier2` CI job on a real `macos-latest` runner, which pairs the admission
   enforce-vs-control duality with the `audit_native` E2E through the real jail
   and refuses to certify (hard-fails) if `sandbox-exec` is absent.
4. **Windows / FreeBSD Tier-2.** As each platform's jail is proven
   (Windows-container availability; FreeBSD infra), promote it. Until then
   each is a documented refuse-to-certify (§3.3).
5. **(Optional, later) true tracer.** Should a robust cross-platform
   syscall→axis tracer ever be built, it *replaces* differential-deny as a more
   complete observation — but only tightens, never loosens, the admit predicate.

---

## 7. Open questions / risks (biggest security holes to watch)

- **BIGGEST: the observation is reachability-under-denial, not intent.** A native
  capability that is *compiled in but not exercised by the probe* reads as
  "unused" (→ could even be flagged declared-but-unused and force the author to
  drop the declaration, after which the capability is present in the shipped
  artifact but *undeclared* — a laundering path). Mitigation: the runtime jail
  (ADR 0040) still confines the shipped artifact to the *declared* set at
  `ipe run`, so an undeclared-but-present capability is denied at run regardless;
  and the static wrapper scan (`capability_scan.rs`) independently over-approximates
  reachable axes. Tier-2's declared-but-unused reject should therefore be **cross-
  checked against the static scan**: only flag unused if *both* the tighten and the
  static scan agree the axis is unreached, to avoid pushing an author to under-
  declare a genuinely-present capability. Watch this interaction closely.
- **Probe coverage.** Differential-deny is only as good as the probe's exercise of
  the native surface. A package with an FFI entrypoint the probe never calls is
  under-observed. The probe contract (what the package must expose for Tier-2 to
  exercise) needs a precise, fail-closed spec: a native package that exposes no
  probeable entrypoint should be **rejected or certified only "build-clean, un-
  exercised"**, never silently admitted as "clean".
- **Denial-vs-error ambiguity (§2.4).** If the per-axis denial signal is ever
  scraped from untrusted stderr instead of the wrapper's own exit contract, the
  package could forge a clean result. Keep the signal wrapper-owned.
- **Windows unverified.** Until a Windows-container-capable runner exists,
  Windows native packages are *never* certified. Ensure the admitted-platform set
  cannot silently include windows.
- **Per-platform `cfg` divergence.** The whole reason for the matrix (admission-
  gate §1.3): a `cfg(windows)` native path is invisible to the Linux job. A
  version admitted before its Windows job is wired is admitted *without* that path
  ever confined — hence the refuse-to-certify posture, not advisory-pass.
- **Resource exhaustion by the untrusted build.** The jail's wall-clock + resource
  caps (admission fixture) must bound the O(axes) tightening builds; an untrusted
  `build.rs` that spins must be killed by the cap, not hang the gate.

---

## Links

- Tier-1 gate: `src/ipe-cli/src/audit.rs`; ADR 0044.
- Runtime jail + profile lowering: `src/ipe-cli/src/run_sandbox.rs`,
  `src/compiler/sandbox/src/run_jail.rs`, ADR 0040 / 0041.
- Per-platform admission substrate: `docs/architecture/tbd/admission-gate-per-platform-sandbox.md`,
  `scripts/admission/jail-*`, `.github/workflows/admission-sandbox.yml`.
- Static wrapper capability inference: `src/compiler/ffi/src/capability_scan.rs`
  (FFI Tier-2 §5).
- FFI Tier-2 wrapper model: `docs/architecture/tbd/ffi-tier2-inspect-author-rust.md`.
