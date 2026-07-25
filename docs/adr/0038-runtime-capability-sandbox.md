# 38. Runtime capability sandbox around the emitted binary

Date: 2026-07-21

## Status

Accepted and **implemented** (first cut), **amended by ADR 0040**. The jail
mechanism, lowering, and artifact machinery here stand; ADR 0040 narrows *when*
the jail runs — it applies only to native-bearing programs (a `Rust.` crossing),
while pure Ipê runs directly. The fail-closed, deny-by-default lowering below
governs the case where a jail is present. The Linux jail, `ipe run` wiring, the
`ipe build` artifact profile + `ipe exec` launcher, and the per-target
admit-and-isolate hand-off are in place; macOS/other platforms are the documented
refuse-gap. Implementation lives in `ipe_sandbox` (`run_jail`, `seccomp`) and the
CLI (`run_sandbox`, `ffi`); the design is in
`docs/architecture/tbd/runtime-capability-sandbox-plan.md`.

## Context

Ipê infers a program's capability set from pure Ipê and requires native `Rust.`
code to declare its axes, and a manifest gate checks the declaration. But the only
sandbox that exists confines the *build-time* RCE surface (`ipe_sandbox`, wrapping
`cargo`/`rustdoc` over untrusted crates) — there is **no jail around the app the
compiler emits when it runs**. For pure Ipê this is fine: an unreachable capability
is absent from the binary, so the guarantee is structural. For a Tier 2 wrapper
crate — arbitrary native Rust that can make any syscall — nothing enforces the
declared set at runtime. That gap is why real-capability Tier 2 wrappers are
hard-refused at install today (refuse-until-jail).

Two facts constrain the design. The manifest gate (`verify_capabilities`) proves
`declared == inferred` over the **Ipê-inferable set only** — it is blind to native
code by construction, so it cannot *prove* a native wrapper's true set. And no
sandbox primitive is available on every platform, so a portable design must decide
what happens when one is absent.

## Decision

Ship an **OS-level, fail-closed runtime jail** that runs the emitted binary confined
to `inferred ∪ declared`. Union (not inferred-only) guarantees *no false-deny*: a
legitimately-declared effect can never be blocked. Each capability maps to a coarse
OS control (network → net namespace, filesystem → mount scope, subprocess →
seccomp-denied task-creation family, env → scrubbed environment); `database` lowers
to network/filesystem; `clock`/`random` carry no OS control (denying them breaks more
than it contains and neither is a high-value axis).

Load-bearing invariants:

- **Fail-closed, always.** An unavailable primitive refuses to run — never runs
  unconfined. The override is narrow, distinct from the FFI-compile override, and a
  hard error (not a warning) when the set includes native/high-value axes.
- **Deny-by-default, structurally.** The capability→profile lowering is an exhaustive
  `match Capability` with no catch-all, so a new variant fails to compile until
  classified; the empty set is the maximally-isolated profile.
- **Baseline denials** independent of the set: fresh `--proc /proc` (the reused
  build-jail argv exposes the host `/proc`, defeating `--clearenv`), `clone3` and the
  whole task-creation family, `ptrace`/`process_vm_*`, `io_uring`, `no_new_privs`,
  unconditional IPC/net namespace unshare.
- **Native code is contained, not caught.** The gate cannot prove a native wrapper's
  set, so the jail is containment: an undeclared syscall fails closed. A static source
  scan is a best-effort, defeatable honesty check, never the boundary.
- **The artifact carries its enforcement.** `ipe build` emits a strictly-parsed
  profile and a launcher; the authoritative capability floor is embedded in the
  binary, so a tampered profile cannot under-isolate. A bare `./ipe-app` is a
  documented, deliberate deployer escape.

Platform posture: Linux is airtight-coarse first (bubblewrap + seccomp-bpf +
`prlimit`/`timeout` with run-tuned values); macOS is Seatbelt-or-refuse; other
platforms refuse. No platform runs a native-capability binary unconfined by default.

Once the jail is present on a target, the refuse-until-jail posture is lifted to
**admit-and-isolate per-target** — never globally, since admitting on a refuse-gap
platform would mean admit-and-run-unconfined.

## Consequences

- Real-capability Tier 2 wrappers become installable and are isolated at runtime,
  closing the gap that forced the refuse-until-jail posture.
- The guarantee is honestly tiered: pure Ipê is structural, native is contained. The
  README and capability docs must state "contained, not proven" for native code.
- Coarse whole-capability axes (any host / any path) are the first cut; per-host,
  per-path (Landlock), a full seccomp capability→syscall map, and manifest-declared
  resource quotas are tracked refinements.
- The jail reuses `ipe_sandbox`'s argv *mechanism* but not its resource-limit
  *values* (the build defaults would kill a long-lived server).
- The `ipe.profile` deserialization is a new parse-don't-validate boundary the
  implementation must honor (parse failure ⇒ refuse-to-run).
