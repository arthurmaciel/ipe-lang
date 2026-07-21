# Runtime capability sandbox — spec + implementation plan

A program's capability set is *inferred* from pure Ipê and *declared* for native
`Rust.` code, and the manifest gate proves the declaration is honest. But there is
today **no jail around the emitted binary when it runs**: `ipe_sandbox` confines
the *build-time* RCE surface (compiling and inspecting untrusted foreign crates),
not the app the compiler produces. So for pure Ipê the capability guarantee is
structural (an unreachable capability is not in the binary), but a Tier 2 wrapper
crate — arbitrary native Rust that can make any syscall — is bound only by its own
good behavior once the process starts.

This plan closes that gap: an OS-level, fail-closed jail that runs the emitted
binary confined to *exactly* its declared-plus-inferred capability set, so declared
effects work and undeclared ones are impossible.

## Where this sits relative to what exists

- **Build-time jail (`ipe_sandbox`, present).** Bubblewrap-primary, refuse-if-absent,
  wraps `cargo`/`rustdoc` over untrusted crates. Reusable primitives: PATH probe
  for `bwrap`/`prlimit`/`timeout`, `select_mechanism` (bwrap-or-refuse), a pure
  `bwrap_argv` builder, `NetworkPolicy`, `ResourceLimits`, `JailSpec`, a bounded-output
  jailed runner, and the `IPE-F4410` refusal taxonomy. This plan **reuses that argv
  vocabulary**; it does not fork a second jail.
- **Capability inference (present).** `program_capabilities` (over the entry's
  reachable kernels) and the package-wide union both yield a `BTreeSet<Capability>`;
  `verify_capabilities` proves a declared set equals the inferred one. That set is
  the input to profile generation here.
- **Runtime jail around the app (absent — this plan).** Nothing wraps the binary
  `ipe run` execs, nor the artifact `ipe build` writes. The Tier 2 design and the
  capability doc both *refer to* "runtime capability scoping, fail-closed" as the
  load-bearing enforcement layer; this plan is that layer's spec.

## The capability→isolation map (coarse, first cut)

Each `Capability` (from `src/compiler/kernels/src/capability.rs`) maps to an OS
control. The posture is **deny-by-default**: a capability that is *not* in the set
is isolated away; a capability that *is* in the set has its control relaxed to
"allowed" (still coarse — any host, any path — for the first cut).

| Capability | In the set → allow | Not in the set → isolate (fail-closed) | Primitive |
|---|---|---|---|
| `network` | Network namespace shared with host (egress/ingress possible) | Fresh empty network namespace — no interface but loopback; native `connect`/`bind`/`socket` cannot reach a real endpoint | `--unshare-net` (unshared only when absent), user namespace |
| `filesystem` | Read-write bind of the working tree + a scoped writable tempdir | `/` read-only, `$HOME`/`/root`/`/tmp` masked by tmpfs, one scoped writable tempdir only | `--ro-bind / /`, `--tmpfs`, `--bind <scoped>` |
| `database` | Resolved to `network` **or** `filesystem` per the `ipe.toml` driver (a TCP driver → network; a file/SQLite driver → filesystem path) | Neither the network nor the file path the driver needs is present | (delegates to the two above) |
| `env` | Manifest-named variables re-exported into the scrubbed env | `--clearenv`; only a fixed minimal allowlist (`PATH`, `TMPDIR`, `LANG`) re-enters; secrets in the host env are invisible | `--clearenv` + `--setenv` allowlist |
| `subprocess` | `fork`+`exec` permitted; still under a **fresh PID namespace** even when allowed (no host-PID visibility) | seccomp-bpf denies the whole task-creation family — `fork`/`vfork`/`clone`/**`clone3`**/`execve`/`execveat` → `EPERM` (see the clone3 note); fresh PID namespace | seccomp-bpf syscall filter + `--unshare-pid` |
| `clock` | (always allowed) time syscalls are not isolated | not isolated — reading time is not a confinement axis in the first cut | none (see note) |
| `random` | (always allowed) `getrandom`/`/dev/urandom` available | not isolated in the first cut | none (see note) |
| `native-ffi` | The *marker* that inference is blind; does not itself open a control — it forces the declared set to be treated as the ceiling | n/a (it never widens the OS surface on its own) | none directly; it gates *which* of the above are trusted from declaration vs inference |

Notes on the map:

- **`clock`/`random` are not OS-isolated in the first cut.** Coarsely denying the
  time or RNG syscalls breaks far more than it contains (allocators, TLS, hashing all
  draw randomness; almost everything reads time), and neither is a high-value
  exfiltration axis. They remain in the capability *vocabulary* for honesty and
  inference, but carry no first-cut sandbox control. A determinism/replay jail that
  fakes them is a possible later refinement, not a security control.
- **`database` is not a primitive** — it *lowers* to `network` or `filesystem`
  before the profile is built, using the same `ipe.toml` driver selection the
  compiler already resolves. The profile generator expands `database` into the
  concrete axis so the jail never sees a `database` control it cannot enforce.
- **`native-ffi` widens nothing by itself.** Its role is epistemic: its presence
  means inference cannot see the true set, so the *declared* set (consented at
  `ipe add`) becomes the authoritative ceiling and the jail is built from
  `inferred ∪ declared`. Without `native-ffi`, inferred already equals the true set.

Later refinements (per-resource, out of first-cut scope): per-host network
allowlists (seccomp/`connect` address filtering or a userspace proxy), per-path
filesystem scoping (Landlock LSM / finer bind sets), and a seccomp
capability→syscall map that denies whole syscall families per absent capability
rather than only the process axis.

### Baseline denials — regardless of the capability set

The following are denied *unconditionally* (no capability grants them), because they
are escape/exfiltration primitives no legitimate effect needs. These close holes the
reused build-jail argv does not, and are part of the first cut, not a later map:

- **`--proc /proc` (fresh proc mask).** The reused `bwrap_argv` does `--ro-bind / /`,
  which exposes the *host* `/proc`. That defeats the `env` guarantee outright
  (`/proc/<pid>/environ` of sibling host processes leaks secrets past `--clearenv`),
  re-enters masked paths via `/proc/<pid>/{root,cwd,fd}`, and is the classic
  user-namespace escape lever. The run jail MUST mount a fresh `--proc /proc` and
  never inherit the host one.
- **`clone3` and the whole task-creation family.** seccomp cannot dereference
  `clone3`'s pointer argument, so a flags-based "threads yes, processes no" allowlist
  is not reliably expressible. The first cut therefore denies the *entire* create
  family (`fork`/`vfork`/`clone`/`clone3`/`execve`/`execveat`) when `subprocess` is
  absent, accepting that in-process threads collateral-share the deny (Rust std
  threads use `clone` with `CLONE_VM`); the app runs single-process. A fixture must
  call `SYS_clone3` *raw*, not just `Command::new`, to prove the raw path is closed.
- **`ptrace`, `process_vm_readv`/`process_vm_writev`** — cross-process memory read /
  code injection against jail siblings and (on a misconfigured user namespace) the
  launcher. No capability needs them; baseline-denied.
- **`io_uring_setup`/`enter`/`register`** — perform file and network I/O *without* the
  `openat`/`connect` syscalls a naive filter watches, bypassing the filesystem-bind
  view. Baseline-denied (network is separately contained by the empty netns).
- **`no_new_privs` set; no setuid.** Make the "no privilege gain" claim mechanical,
  not incidental; a setuid-binary fixture must fail to elevate.
- **`--unshare-ipc` and `--unshare-net` stay unconditional** even when their axis is
  granted-elsewhere-shaped, because SysV shmem / message queues / abstract sockets are
  covert channels that ride IPC/net namespaces independent of the `network` axis.

## Mechanism, per platform (fail-closed everywhere)

The invariant across every platform: **if the required primitive is unavailable,
refuse to run — never run unconfined.** An emitted binary with a non-empty
native/declared capability set that cannot be jailed is an `IPE-F44xx`-class refusal,
mirroring the build jail's `IPE-F4410`.

The override is deliberately narrow. `IPE_ALLOW_UNSANDBOXED=1` (a **distinct** env
var from the FFI-compile `IPE_FFI_ALLOW_UNSANDBOXED`) covers only the pure-Ipê /
low-value-axis cases and prints a trust warning. Once the install gate flips to admit
real-capability Tier 2 wrappers, this override becomes an "arbitrary native code runs
with the invoking user's full privileges" switch — so it is a **hard error, not a
warning**, when the resolved set includes `native-ffi` or a high-value native axis
(network/filesystem/subprocess/env). There is no flag that runs admitted native code
unconfined; that combination refuses, full stop.

- **Linux (the first target).** Bubblewrap user + namespaces (net/pid/uts/ipc/cgroup)
  for the coarse capability axes, `prlimit` for resource caps, `timeout` for a wall
  clock, plus a **seccomp-bpf** filter for `subprocess` denial (the one axis a
  namespace alone does not close — a shared PID namespace still permits `fork`).
  This reuses `bwrap_argv`'s existing flag vocabulary; the new piece is a seccomp
  program applied via `bwrap --seccomp <fd>` (bubblewrap already accepts a compiled
  BPF program on an fd). Fail-closed when `bwrap` is absent (as today).
- **macOS.** No bubblewrap. Two honest options, in order of preference:
  1. `sandbox_init(3)` / Seatbelt with a generated `.sb` profile expressing the
     same allow/deny axes (`(deny network*)`, `(deny file-write*)` with a scoped
     allow, `(deny process-fork)`), plus `setrlimit` for caps. This is the
     documented, if deprecated, sandbox primitive and maps cleanly to the coarse
     axes.
  2. If (1) is not pursued at first, macOS is a **documented gap**: a binary with a
     non-empty native capability set **refuses to run** under `ipe run` on macOS
     unless the unsandboxed override is set. Pure Ipê (empty native set) is
     unaffected — its guarantee is structural and needs no jail.
- **Other platforms (Windows, BSDs).** Documented gap at first: same
  refuse-unless-override posture. Windows job objects + AppContainer are a plausible
  later mapping; not the first cut.

The cross-platform story is deliberately honest: **Linux is airtight-coarse first;
macOS is Seatbelt-or-refuse; everything else is refuse.** No platform runs a
native-capability binary unconfined by default.

## Profile generation and wiring

### From capability set to a concrete profile

1. **Collect the set.** `inferred = program_capabilities(entry)`. If the project
   carries native/Tier 2 declarations, `declared = manifest.capabilities`. The
   **profile set is `inferred ∪ declared`** — the same union the package design
   specifies. Union (not intersection, not inferred-only) is what guarantees the
   *no-false-deny* property: every capability anyone claims the program has is
   relaxed, so a legitimately-declared effect can never be blocked.

   **What the manifest gate does and does not prove (do not overstate this).**
   `verify_capabilities` proves `declared == inferred` over the **Ipê-inferable set
   only** — it is *blind to native `Rust.` code* by construction. So for native code
   the union is **containment, not proof**: an under-declared wrapper is *contained*
   by the jail (an undeclared syscall fails closed), not *caught* at the gate. The
   static source scan (below) is a best-effort, defeatable smell test, never the
   boundary. Do not claim the gate has proven `declared ⊇ inferred` for native code —
   it has not, and cannot.
2. **Lower `database`** to `network`/`filesystem` per the `ipe.toml` driver.
   **Fail-closed on an unknown/missing/ambiguous driver** — never silently drop the
   axis (dropping it would *tighten* the jail past what the program needs → a
   false-deny; and mis-lowering to the wrong axis under-isolates).
3. **Emit a `SandboxProfile`** — a small serializable value (network on/off,
   filesystem scope, env allowlist, subprocess allow/deny, resource limits). This
   is the platform-independent description; a per-platform *builder* turns it into
   a `bwrap` argv + seccomp program (Linux) or a `.sb` profile (macOS).

   **Deny-by-default, structurally.** `profile_from_capabilities` MUST be an
   exhaustive `match Capability` with **no `_` catch-all** (mirroring the exhaustive
   `as_str`/`FromStr` in `capability.rs`), so a newly-added capability variant fails
   to *compile* until it is classified — and an unclassified variant can never
   default to "allowed". `native-ffi`'s no-op control is still an explicit arm.
   `SandboxProfile` has no `Default` that yields an all-allowed value: an empty set
   lowers to the *maximally isolated* profile, or the type is simply not
   `Default`-constructible.

### Two wire points

- **`ipe run` (wrap the exec).** Today `run_run` ends by `cmd.exec()`-ing the emitted
  `ipe-app` binary directly. The jail inserts *before* that exec: build the profile
  from the entry's capability set, construct the platform jail argv wrapping the
  binary, and exec *that*. On Unix the `exec` replacement is preserved (the jail
  launcher is what gets exec'd, and it in turn execs the app inside the namespaces).
  An empty native set + empty inferred high-value set may run without a jail (nothing
  to confine) — but a non-empty set with no available primitive **refuses**.
- **`ipe build` (a deployable profile the artifact carries).** A built artifact can
  be copied off the build host and run elsewhere, so the enforcement must travel
  with it. `build_emit_manifest` writes the emitted project; this plan adds a
  generated **`ipe.profile`** file (the serialized `SandboxProfile`) alongside the
  binary, plus a thin **launcher** (`ipe-run` shim, or a documented `ipe exec
  <artifact>` subcommand) that reads the profile and applies the same jail before
  exec'ing the app. The artifact is enforceable wherever an Ipê-aware launcher runs;
  a bare `./ipe-app` invocation is the escape hatch a deployer must consciously choose
  (documented, and refused by the launcher path).

  **`ipe.profile` is untrusted input to the launcher — parse, don't validate.** A
  tampered profile that requests *fewer* isolations builds a no-op jail around a
  high-capability binary, which would make the mechanism *weaker* than the
  bare-`./ipe-app` escape it closes. So: the launcher strictly *parses* the profile
  into the typed `SandboxProfile` (parse failure ⇒ refuse-to-run, never a permissive
  fallback), and the authoritative capability set is **embedded in the binary itself**
  (so the profile cannot claim fewer axes than the binary was built for) — the profile
  file is a convenience mirror, not the source of truth. A profile weaker than the
  embedded floor is a refuse-to-run, not an honored request.

### Where the two sets come from — no new inference

Profile generation consumes *only* existing outputs: `program_capabilities` for
inference and the manifest `[capabilities]` block for declaration. There is no new
capability-analysis pass; this plan is enforcement plumbing over an already-computed
set.

## Interaction with capability-enforcement (the refuse-until-jail hand-off)

The capability-enforcement work established a **refuse-until-jail** posture: because
no runtime jail exists, a Tier 2 wrapper crate that needs a *runtime-enforced* axis
(network/filesystem/database/env/subprocess/native-ffi) is **hard-refused at
install** — admitting it would mean trusting native Rust to self-limit at runtime,
which nothing enforces.

**Once this jail lands, that posture is LIFTED to admit-and-isolate.** The hand-off:

- The install-time gate stops hard-refusing a real-capability Tier 2 wrapper. Instead
  it records the declared capability set (consent, loud on `native-ffi`) and admits
  the crate.
- At `ipe run` / from the built artifact's profile, the wrapper runs inside the jail
  built from `inferred ∪ declared`. A declared axis works; an *undeclared* syscall
  the wrapper attempts fails closed at the OS boundary.
- The build-time RCE jail (`ipe_sandbox`) is unchanged and still gates the wrapper's
  compile. This plan adds the *runtime* half the enforcement design named but did not
  have.

Concretely, the enforcement design's "runtime capability scoping, fail-closed" and
the Tier 2 doc's "sandbox enforcement (guarantees)" step are satisfied by this jail.
The implementing work should update those docs' forward-references from "planned" to
"provided by the runtime sandbox" and flip the install gate.

**The flip is conditioned on the jail actually being present where the artifact
runs — this is the most dangerous step in the plan.** Flipping refuse-until-jail →
admit-and-isolate *globally* would, on a platform in the refuse-gap (macOS without
the Seatbelt path, Windows, BSD), mean admit-and-run-**unconfined** — strictly worse
than today. So the flip is per-target: a real-capability Tier 2 wrapper is admitted
only for a run/deploy target that has a working jail; on a refuse-gap target the
refuse-until-jail posture *stays*. The admission decision is "is this binary going to
run somewhere the jail holds?", not "does a jail exist on the build host?". A static
source scan of the wrapper (reusing the existing capability-gate path rules) runs at
admission as a best-effort honesty smell test on the declaration — surfacing an
obvious under-declaration — but it is defeatable and is never the boundary; the jail
is.

## Resource quotas — carried, but honestly scoped

The capability doc defers a general per-program quota model (memory/CPU-time/I/O) to
the deployment layer. This jail **carries the hooks** rather than the policy:
`prlimit` (address space, CPU-seconds, open FDs, process count, file size) and a
`timeout` wall clock are already in the build-jail argv and are reused as the
*mechanism* for the run jail. **The values are not reused** — the build-jail defaults
(10 GiB address space, 900-second CPU and wall clock) are calibrated to kill a giant
one-shot rustdoc build and would wrongly kill a legitimate long-lived server (a
false-deny). The run jail defines its own `ResourceLimits`: no wall-clock kill by
default for a long-running app, generous CPU, sane FD/memory ceilings. Reuse the
mechanism, not the numbers. What is **not** carried at first: a per-program *quota policy* surfaced in the manifest (e.g.
`memory = "512M"`), cgroup v2 accounting, or I/O-throughput limits. The first cut
ships sane default `ResourceLimits` for the run jail and leaves a *policy* knob
(manifest-declared quotas, cgroup enforcement) as the deferred quota story,
cross-referenced from `docs/capabilities.md`. The jail is where that story will land
when it is picked up; it does not block the first cut.

## Threat model

Adversary: a **malicious Tier 2 wrapper crate** (arbitrary native Rust, admitted
after this jail lands) attempting to exceed its declared capability set at runtime.

| Threat | Containment | Residual (first cut) |
|---|---|---|
| Undeclared network exfiltration | Fresh empty net namespace when `network` absent — no interface but loopback, no route off-host, native `connect` to a real endpoint fails | Coarse: a *declared* `network` crate may reach any host (per-host is a later refinement); loopback still exists inside the namespace |
| Undeclared filesystem read/write | `/` read-only, home/tmp masked, one scoped writable dir when `filesystem` absent | Coarse: a declared `filesystem` crate may touch any path (per-path/Landlock is a later refinement) |
| Undeclared subprocess spawn | seccomp-bpf denies the whole create family incl. **`clone3`** → `EPERM` when `subprocess` absent; fresh PID namespace | Requires seccomp available; refuse-if-absent closes the fallback; single-process (thread collateral) |
| I/O via `io_uring` bypassing the syscall filter | `io_uring_setup`/`enter`/`register` baseline-denied — the fs-bind view and the netns still bound it, but the direct-I/O bypass is closed | Baseline deny; not axis-conditional |
| Cross-process memory read / injection (`ptrace`, `process_vm_readv`) | Baseline-denied unconditionally — no capability grants them | — |
| Host-secret theft via env or `/proc` | `--clearenv` + fixed allowlist **and** a fresh `--proc /proc` mask (host `/proc/<pid>/environ` is not exposed) | A declared `env` var is exposed in full |
| Namespace/jail escape (user-ns tricks, `/proc`, setuid) | fresh `--proc /proc`, `no_new_privs`, `--die-with-parent`, `--new-session`, read-only `/`, fresh dev, no setuid; seccomp denies `mount`/`pivot_root`/`setns`/`unshare`/`ptrace` re-escape | User-namespace kernel CVEs are out of scope (host kernel trust) |
| Launcher tampering (a doctored `ipe.profile` requesting a no-op jail) | The profile is strictly parsed (parse-fail ⇒ refuse); the authoritative set is embedded in the binary; a profile below that floor refuses | A deployer editing the binary itself is the raw-binary escape, already documented |
| Covert channels (shmem, abstract sockets, timing/RNG) | `--unshare-ipc`/`--unshare-net` unconditional close shmem/abstract-socket channels | Timing/RNG side channels via `clock`/`random` remain (accepted by design — not a confinement axis) |
| Capability *false-deny* (a legitimately declared effect blocked) | Profile is built from `inferred ∪ declared`; a declared axis is *relaxed*, so a declared effect must work — a false-deny is a correctness bug, tested by positive fixtures | Requires the map to relax exactly and only the declared axes |
| Bypass by running the bare binary off a build artifact | `ipe.profile` + launcher travel with the artifact; the launcher applies the jail; bare `./ipe-app` is a documented, deliberate deployer escape | A deployer who runs the raw binary opts out — documented, not silent |
| Primitive unavailable → silent unconfined run | **Fail-closed refusal** (`IPE-F44xx`), override only via loud `IPE_ALLOW_UNSANDBOXED=1` | Override exists for CI/dev; prints a trust warning |

Two load-bearing soundness properties the implementation and its fixtures must prove:

1. **No false-deny.** For every declared/inferred capability, a positive fixture that
   *exercises* that effect must succeed inside the jail. A jail that breaks a
   legitimately-declared effect is a correctness bug, not a security win.
2. **Fail-closed on the undeclared.** For each isolatable axis, a negative fixture
   whose capability is *absent* must have the effect fail at the OS boundary (not by
   the app's own choice). And an unavailable primitive must refuse, never degrade to
   unconfined.

## First cut vs later refinements

**First cut (this plan):**
- Linux: bwrap namespaces (net/fs/env/pid/ipc) + fresh `--proc /proc` + seccomp-bpf (subprocess incl. `clone3`, plus baseline `ptrace`/`process_vm_*`/`io_uring` denial) + `no_new_privs` + run-tuned `prlimit`/`timeout`, reusing the `ipe_sandbox` argv vocabulary (mechanism, not values).
- Coarse whole-capability axes (any host / any path).
- `database` lowered to network/filesystem (fail-closed on an unknown driver); `clock`/`random` not OS-isolated.
- `ipe run` wrap + `ipe build` artifact profile (strictly parsed, capability floor embedded in the binary) + launcher.
- Deny-by-default via an exhaustive `match Capability` (a new variant fails to compile until classified).
- Fail-closed refusal when a primitive is absent; narrow override env that hard-errors on native/high-value axes.
- macOS: Seatbelt profile (`sandbox_init`/`.sb`, a consciously-chosen deprecated primitive) **or** documented refuse-unless-override gap; other platforms refuse.
- The capability-enforcement hand-off: lift refuse-until-jail → admit-and-isolate **only per-target where the jail holds**; the honest posture that native under-declaration is *contained, not caught*.

**Later refinements (tracked, not built here):**
- Per-host network allowlist, per-path filesystem scope (Landlock).
- A seccomp capability→syscall map denying whole syscall families per absent capability.
- Manifest-declared resource quotas + cgroup v2 enforcement.
- macOS Seatbelt if the first cut chose the documented-gap route; Windows AppContainer/job objects.
- Determinism jail for `clock`/`random` (replay), if wanted.

## Implementation steps (bite-sized, independently landable)

A. **`SandboxProfile` type + the capability→profile lowering.** A serializable
   profile value in a new module (near `ipe_sandbox` or a sibling `ipe_runtime_jail`
   crate), plus `profile_from_capabilities(inferred, declared, driver) -> SandboxProfile`
   built from `inferred ∪ declared`, lowering `database` (fail-closed on an unknown
   driver) and dropping the non-isolated axes. The match over `Capability` is
   exhaustive with no `_`; the empty set yields the maximally-isolated profile; no
   all-allowed `Default`. Pure, fully unit-tested. No process spawned.

B. **Linux profile → jail argv.** A builder that turns a `SandboxProfile` into a
   `bwrap` argv reusing `bwrap_argv`'s flag vocabulary, adding the run-jail
   allow/deny toggles (net shared vs unshared, fs scope, env allowlist), the fresh
   `--proc /proc` mask, `no_new_privs`, and run-tuned `ResourceLimits` (distinct from
   the SDK-inspection defaults). Pure/testable like the existing argv builder.

C. **seccomp-bpf baseline + `subprocess` filter.** Compile a BPF program denying the
   whole create family (incl. `clone3`) when `subprocess` is absent, plus the
   unconditional baseline (`ptrace`/`process_vm_*`/`io_uring`/`mount`-family), applied
   via `bwrap --seccomp`. Behind a probe (refuse if seccomp unavailable). Unit-test the
   program shape; end-to-end-test the deny with a *raw* `SYS_clone3` fixture (not just
   `Command::new`).

D. **Wire `ipe run`.** Insert profile-build + jail before the final `exec` in
   `run_run`. Fail-closed refusal path + the narrow override (hard-error on
   native/high-value axes). Positive fixture (declared effect works) and negative
   fixture (undeclared effect fails at the OS boundary) under `IPE_E2E`.

E. **Wire `ipe build` artifact.** Emit `ipe.profile` into the manifest and add the
   launcher (`ipe exec <artifact>` or an `ipe-run` shim) that strictly *parses* it
   (parse-fail ⇒ refuse) and applies the jail; embed the authoritative capability
   floor in the binary so a weakened profile cannot under-isolate. Copy-off-host
   fixture proving enforcement travels and a tampered-profile fixture proving it
   refuses.

F. **macOS Seatbelt path _or_ documented-gap refusal.** Either a `.sb` generator +
   `sandbox_init` wire (a consciously-chosen deprecated primitive), or the
   refuse-unless-override gate with a clear diagnostic. Decide up front; do not leave
   macOS silently unconfined.

G. **Capability-enforcement hand-off.** Flip the install gate from refuse-until-jail
   to admit-and-isolate for real-capability Tier 2 wrappers **per-target where the
   jail holds** (never globally — a refuse-gap target keeps refusing). Add the
   best-effort static-scan honesty check on the declaration. Correct the Tier 2 and
   capability docs' forward-references to say the jail *contains* native code (not
   that the gate *proves* the native set). Guarded by the full jail being green.

H. **Resource-quota hooks.** Confirm the run jail carries `prlimit`/`timeout` with
   run-tuned defaults; leave the manifest-quota *policy* as the cross-referenced
   deferred story.

Steps A–C are pure/testable with no CLI changes; D is the first behavior-visible
wire; G is gated on the whole jail being green.

---

*Fenced blocks and the map rows are illustrative — the authoritative shapes are the
`Capability` enum, `program_capabilities`, and the `ipe_sandbox` argv builder.*
