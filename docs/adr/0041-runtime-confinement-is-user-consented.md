# 41. Runtime confinement is user-consented, not mandatory

Date: 2026-07-25

## Status

Accepted; the `--native` consent surface is **not yet implemented**. Amends ADR
0040 (and, through it, ADR 0038). ADR 0040 scoped the runtime jail to
native-bearing programs but kept it *mandatory* for them — fail-closed where a
jail exists, refuse where none does. This ADR removes the mandate: at run time the
jail is never forced. The admission responsibility of ADR 0040 is unchanged.

The native-scoping half of this design (pure Ipê runs free) is implemented. The
consent surface described here — the `--native <prompt|allow|jail>` control, the
interactive `[y/N]`, and the non-interactive fail-closed default — is the tracked
follow-up; until it lands, a native-bearing program on a jail-less platform still
follows ADR 0040's refuse-unless-`IPE_ALLOW_UNSANDBOXED` behavior.

## Context

Confining the emitted binary at run time requires an OS jail, and every platform
exposes a different one (Linux namespaces + seccomp + Landlock, macOS Seatbelt,
FreeBSD Capsicum, OpenBSD pledge, Windows AppContainer). Making the jail
mandatory for native-bearing programs therefore makes Ipê's runnability depend on
building and shipping a correct jail backend for every platform a user might run
on — an open-ended obligation, with Windows alone a large one, gating whether an
ordinary native program can run at all.

That obligation buys less than it appears. A tool's real trust boundary is
**admission** — vetting code before it enters the package index. Once a user
chooses to run code on their own machine, that is their decision, exactly as it
is for every other language toolchain: none sandboxes the program you ask it to
run. A runtime jail we impose is us asserting a responsibility that is the user's,
and (for native code) one we could only partly honor — the jail was always
"contained, not proven".

The two responsibilities are distinct and should be treated differently:

- **Admission is ours.** The build sandbox that isolates an untrusted crate's
  compilation, and the capability diagnostic that measures declared-vs-demanded
  (ADR 0040), gate what enters the index. These stay mandatory.
- **Execution is the user's.** Whether a program on their machine runs confined
  is their call — our duty is to make sure they *know* when opaque native code is
  involved and to obtain their consent, not to decide for them.

## Decision

**At run time the jail is never mandatory. A native-bearing program runs only
with the user's consent; a pure program runs freely; anyone who wants OS
confinement opts into it.**

The whole decision is one axis — how to treat a native-bearing program — surfaced
as a single enumerated control, **`--native <prompt|allow|jail>`** (env
`IPE_NATIVE`), defaulting to `prompt`. Policy for `ipe run` / `ipe exec`:

- **Pure Ipê → runs directly, regardless of the setting.** No jail, no prompt, no
  warning (structural guarantee, ADR 0040). `--native` governs native-bearing
  programs only.
- **`prompt` (default), interactive → warn and ask.** Warn that the program
  reaches native Rust whose effects are not proven safe, then prompt `[y/N]`,
  defaulting to **N**. Yes runs unconfined; No refuses.
- **`prompt` (default), non-interactive (no TTY) → refuse.** No terminal to prompt
  ⇒ fail closed with remediation. It does **not** assume yes.
- **`allow` → run unconfined** after printing the warning. This is the
  non-interactive / scripted path (CI, pipelines, `ipe exec` in production).
- **`jail` → confine** under an OS jail where a backend exists; where none does,
  refuse (the user asked to confine and we cannot honor it here).

**One enum, not two flags.** A single control on one axis makes the contradictory
state — "run unconfined *and* confine" — unrepresentable, per
make-invalid-states-unrepresentable. Two independent flags (an `--allow-native`
plus a `--jail`) could both be set, an incoherent combination a parser would have
to arbitrate. `prompt|allow|jail` is a closed set parsed once (parse, don't
validate); an unknown value is a hard error, never a permissive default.

Load-bearing invariant: **no channel silently runs native code.** The only ways a
native-bearing program runs unconfined are `prompt`'s interactive `y` or an
explicit `--native allow` — both a deliberate human act. The absence of a TTY
under `prompt` is treated as "cannot obtain consent" (refuse), never as consent.

*Rejected — keep the ADR 0040 mandatory jail for native code.* It predicates
running an ordinary native program on shipping a jail backend for the user's
platform, and asserts a responsibility that is the user's once code is on their
machine. The confinement it forced was partial for native code anyway.

*Rejected — two independent flags (`--allow-native` and `--jail`).* They can both
be set at once, an incoherent state the parser must arbitrate; and a symmetric
`--jail-native` spelling is false symmetry, since the OS jail confines the whole
process, not "the native part." One enum on one axis removes the invalid state
and reads as one decision.

*Rejected — proceed silently with only a warning (no consent).* A warning nobody
must acknowledge is indistinguishable from no gate. Running opaque native code
must be a deliberate act, not a default with a printed caveat.

*Rejected — treat a missing TTY as consent.* This is the one reading that opens a
real hole: every scripted, piped, or CI run would execute unproven native code
with no gate. Non-interactive is fail-closed; the consent flag is the explicit
opt-in for those contexts.

## Consequences

- Ipê's runtime no longer depends on a per-platform jail. The default path needs
  no external jail binary and no platform backend, so an ordinary program — pure
  or native-consented — runs anywhere. BSD/Windows/macOS jail backends become
  **opt-in hardening** behind `--native jail`, added when someone wants them,
  never a blocker.
- The trust boundary is stated honestly: we vet at admission and we obtain
  consent at execution; we do not silently confine or silently run. The README
  and capability docs must describe runtime confinement as user-consented and
  opt-in, not automatic.
- Residual risk, named so it is a decision and not a drift: a package that passes
  admission but is malicious in a way the diagnostic misses runs unconfined on a
  consenting user's machine. This is bounded — admission is the real filter and
  `--native jail` is available to anyone who wants belt-and-suspenders — and is the same
  bet every mainstream toolchain makes. Strengthening admission is the lever,
  not a mandatory runtime jail.
- Prompt fatigue is a real cost of the interactive `[y/N]`: a developer iterating
  on a native program should not answer it every run. The consent flag/env
  handles automation; remembering consent per project is a tracked refinement if
  the prompt proves noisy, not a first-cut requirement.
- `ipe exec` on a deployed artifact is non-interactive by nature, so a native
  artifact runs in production only under `--native allow` (or `IPE_NATIVE=allow`)
  or `--native jail`. The embedded capability floor still classifies
  native-vs-pure (ADR 0040); the jail machinery and profile emission are retained
  so `--native jail` can honor them.
- The prior narrow override (`IPE_ALLOW_UNSANDBOXED`, a hard refuse-unless-set for
  native code) is replaced by this consent model. There is one control at run
  time — `--native <prompt|allow|jail>` (env `IPE_NATIVE`) — and its `prompt`
  default carries the interactive `[y/N]`.
