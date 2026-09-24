Status: Living (consolidated)
Date: 2026-09-18
Archive: misc/docs/archive/ADR/

# 0004. Capabilities, trust boundary & confinement

How Ipê reaches outside its pure core — native Rust, in-tree kernels, untrusted
JS — and how each crossing is named, gated, and confined. One foreign-interface
boundary parameterized by target; a trust boundary the module name makes
mechanical; capability enforcement that is native-code-scoped, per-axis,
user-consented, and fail-closed at every gap.

## Decisions

### One foreign boundary, parameterized by target

Every direction Ipê reaches outward — in-tree kernel, native Rust crate, JS port,
browser custom element, future wasm component — instantiates one boundary spine,
never a per-direction subsystem. The directions differ only in *who is on the far
side*, not in *what the boundary must guarantee*. Five invariants hold across all
targets: one closed capability vocabulary the compiler owns (a package declares
against it, cannot mint axes); one boundary predicate — the SEAL — with an ingress
decode gate (untrusted bytes must parse into a concrete, declared, monomorphic ADT,
fail-closed and bounded) and an egress restriction (no function, open row, or
`Secret` leaves the pure core); one trust level keyed on the far side; one coverage
surface checking every foreign symbol carries its level's discipline; and one
statically-resolvable boundary — the foreign target is a compile-time string
literal (`Kernel.kernel "…"`, `Rust.fn "…"`), never a runtime-computed value, so
the reachable foreign surface is statically enumerable.

Adding a target adds a level arm and a namespace under `Ipe.Ffi.<Target>`, not a
subsystem; the security review of any target reduces to which conjuncts of its
level it satisfies, checked mechanically.

### Trust is a level, not a binary

The far side carries one of a closed, compiler-owned set of levels:

| Level | Far side | Guarantee | Mechanism |
|---|---|---|---|
| `proven` | in-tree kernel, type-checked | compiler owns it end to end | none |
| `contained` | native Rust, opaque | declared-and-contained, not proven | capability jail on effects |
| `sealed` | untrusted JS | nothing until parsed | SEAL ingress decode gate |

`contained` jails *effects, not data*: a returned value is ordinary untrusted
content once in the pure core, handled by the same downstream injection-safety as
a `proven` kernel's value. `Secret` egress is per-level: into `proven` freely (crypto
kernels take keys), **never** into `sealed`, into `contained` only behind an
explicitly declared capability (fail-closed default: forbid). `contained` is a
binding obligation, not a label —
`contained ⟺ inspector-verified ∧ caps-declared ∧ glue-compiles ∧ jailed` — enforced
across independent stages (declaration → constrain → lower → emit → coverage
re-check → runtime) as a total match over the closed level enum, so no arm can emit
`contained` without the jail or `sealed` without the decode gate.

The build-time inspector (Rust) and the runtime decode gate (JS) are the same job
at different times; which time is forced by whether the toolchain can *see* and
*trust* the far side at build. Rust compiles the far side in — its shape is
guaranteed at build (rustc backstops a mis-shaped inspection), so a runtime gate
would be redundant. JS is a runtime host the toolchain can neither see nor trust,
so its shape can only be guaranteed at the moment the value exists. Foreign code
calling back into Ipê crosses as an opaque correlation token, never a function
pointer: the runtime holds the real closure keyed by that id, invocation is a
checked lookup, and an unknown or expired id is dropped fail-closed — keeping
"foreign code holding an Ipê callable" unrepresentable and avoiding the
use-after-free class a trampoline would reintroduce.

### The module namespace encodes the trust boundary

Exactly two module prefixes are compiler-meaningful, each naming a side of the
trust boundary. **`Ipe.*` is reserved and compiler-owned** — it resolves only to
the blessed first-party stdlib; the resolver rejects any user or third-party module
declared under it (IPE-N0025 / IPE-N0026). Trust is the *tag*, not the spelling: a
hostile `module Ipe.Palette` is rejected precisely because it is not blessed. This
is fail-closed by construction — absent proof a module is blessed, resolving it
under `Ipe.` is rejected. **`Rust.*` is the FFI boundary, an invariant not a
convention** — *every* native crossing is spelled `Rust.`, regardless of which
library ships it, and only `Rust.*` reaches the native-call lowering path where the
FFI rules (sandbox admission, `Task Error a` effect typing, unsafe audit) apply. The
payoff is an audit primitive: a plain textual scan for `Rust.` enumerates every
native crossing in a program, no semantic analysis required.

Bare names are not globally reserved (only the `Ipe.` *prefix* is): a bare `List`
resolves to its canonical `Ipe.*` module, a colliding third-party module never
silently shadows it (a silent shadow is a compile error), and a deliberate
`import Acme.List as List` rebind is allowed. Adding a first-party stdlib module is
therefore non-breaking.

### Ipê → Rust FFI subsystem

Native binding is fully automatic — no hand-authored shims. Untrusted rustdoc JSON
crosses into Ipê at exactly two `TryFrom<wire> → Result<Domain, Diagnostic>` decode
points (`PkgInfo`, `Call`): parse, don't validate. A `Call` that has not passed
`Call::validate` (run inside the `Call::decode` gate) is unconstructible, and between over-drop (silent omission) and
under-bind (a binding `cargo` then rejects, breaking THE SEAL), the subsystem always
over-drops. The inspector runs post-macro-expansion rustdoc-JSON analysis inside an
RCE build sandbox (no network, read-only filesystem, explicit argv quoting) and
produces typed `PkgInfo`; the generator produces the `.ipei` type-env and kernel
registry entries; an FFI call lowers to `Call { callee: Kernel(Ffi(id)), args }`
exactly like a stdlib call, so no IR changes are needed. A crate whose inspection
fails is over-dropped with a diagnostic, never silently bound; a dependency upgrade
that changes a signature is caught by a content-hash drift fence and re-inspected.

### Runtime confinement is native-code-scoped and user-consented

A program whose capability union reaches no opaque native (`Rust.`) code runs
**directly — no jail, no warning**: its capability set is a compile-time structural
fact (an unreachable capability is absent from the binary), so there is nothing to
enforce at run time. A jail around pure code could only forbid what the binary
already cannot do, and mandating one everywhere would make an ordinary program
unrunnable on any host without a jail primitive — a portability tax on all code to
buy security that matters only for native code. The native-vs-pure split is drawn
from the *same* capability inference the manifest gate uses, so a program is
classified native-bearing iff its union reaches declared native code — never a
source heuristic that could under-approximate and skip the jail.

A native-bearing program's confinement is the **user's** decision, obtained as
consent, not imposed: admission (vetting code before it enters the index) is Ipê's
responsibility and stays mandatory; execution on the user's own machine is theirs,
as it is for every other toolchain. The design is one axis — how to treat a
native-bearing program — surfaced as a single enumerated control
**`--native <prompt|allow|jail>`** (env `IPE_NATIVE`), default `prompt`. `prompt`
interactive warns and asks `[y/N]` (default N); `prompt` non-interactive (no TTY)
**refuses** — a missing TTY is "cannot obtain consent", never assumed consent;
`allow` runs unconfined after the warning (the scripted / `ipe exec` path); `jail`
confines where a backend exists and refuses where none does. One enum on one axis
makes the contradictory "run unconfined *and* confine" state unrepresentable; the
closed set is parsed once and an unknown value is a hard error, never a permissive
default. The load-bearing invariant: **no channel silently runs native code** —
the only unconfined paths are an interactive `y` or an explicit `allow`, each a
deliberate human act.

> **NEEDS UPDATE AFTER IMPLEMENTATION** — the `--native <prompt|allow|jail>` /
> `IPE_NATIVE` consent surface (the interactive `[y/N]` and non-interactive
> fail-closed default) is not yet coded; `IPE_NATIVE` and `--native` appear nowhere
> in the workspace. The runtime still follows the prior narrow override: a
> native-bearing program on a jail-less platform refuses unless
> `IPE_ALLOW_UNSANDBOXED` is set (`src/ipe-cli/src/run_sandbox.rs:42`,
> `OVERRIDE_ENV`), which then downgrades the refusal to a loud recorded-consent
> warning. Tracked: untracked. Once landed, replace the `IPE_ALLOW_UNSANDBOXED`
> override with the `--native`/`IPE_NATIVE` enum here and delete this banner.

### The run-jail confines per axis, fail-closed

Where a native-bearing program *is* jailed, the jail confines the emitted binary to
its `inferred ∪ declared` capability set — union, not inferred-only, so a
legitimately-declared effect can never be false-denied. The capability→profile
lowering is an exhaustive `match Capability` with no catch-all (a new variant fails
to compile until classified; the empty set is the maximally-isolated profile), and
the empty profile is the deny-by-default floor. `SandboxProfile` carries four
platform-independent axes — `network`, `filesystem`, `env`, `subprocess` — plus
resource limits (run-tuned, never the build defaults that would kill a long-lived
server); `database` lowers to network/filesystem; `clock`/`random` carry no OS
control.

Each platform arm confines **every axis it claims**, assembling primitives rather
than trusting one blanket mechanism, and confinement composes only downward — a
profile may be established by an arm at least as isolated *per axis* as the profile
demands, checked axis by axis, never as an aggregate:

- **Linux** — network namespace (`network`), read-only root bind plus tmpfs masks
  (`filesystem`), `--clearenv` + re-export (`env`), seccomp deny of the clone/fork
  family (`subprocess`), atop baseline denials (fresh `/proc`, `ptrace`/`process_vm_*`,
  `io_uring`, `no_new_privs`, IPC/net namespace unshare).
- **macOS** — a `sandbox-exec` Seatbelt SBPL profile for `network`, `filesystem`,
  `subprocess`; the launcher performs the `env` scrub, because Seatbelt does not.
- **Windows** — a Job Object (`subprocess`) wrapping an AppContainer lowbox-tokened
  child (`filesystem` + `network`), with the launcher scrubbing `env`.

Where a primitive carries a precondition admit-time cannot verify, the arm closes it
with a runtime probe that **fails closed**: on Windows the AppContainer filesystem
boundary is a no-op on a volume without `FILE_PERSISTENT_ACLS`, so the arm probes the
volume flags and **refuses to launch** rather than run with an unconfined filesystem
axis. No-over-claim is the governing rule: an arm reports it `Holds` a profile only
when every axis it claims is actually enforced; an axis it cannot confine is a
refuse-gap, never a silent downgrade to best-effort. The standing invariant: for any
target the jail reports `Holds`, each of the four axes is confined by a concrete
primitive whose preconditions held at launch — verified per axis, fail-closed on any
gap. The `ipe build` artifact carries its enforcement: the authoritative capability
floor is embedded in the binary, so a tampered profile cannot under-isolate.

> **NEEDS UPDATE AFTER IMPLEMENTATION** — the run-jail has Linux, macOS, and Windows
> arms (`src/compiler/sandbox/src/run_jail/{linux,macos,windows}.rs`); there is no
> FreeBSD run-jail arm. On FreeBSD (and every other unwired target) the run-jail is
> a documented refuse-gap: `exec_in_run_jail` returns
> `RunJailDefect::UnsupportedPlatform` (`src/compiler/sandbox/src/run_jail/mod.rs`
> ~L670). Tracked: untracked. Once a FreeBSD run-jail arm lands, add it to the Linux/
> macOS/Windows arm list above and drop this banner.

### Tier-2 native admission is differential confinement, not tracing

The package gate proves a declared capability set equals the compiler-inferred set —
exact only for pure Ipê. When a package crosses into native `Rust.` code, inference
cannot see past the marker, so a package could declare `[]` and open a socket from
native code. Tier-2 closes that hole by **differential confinement**: it builds and
exercises the package's native code inside a jail scoped to *exactly* the declared
set, then reads a typed outcome — **used-but-undeclared** (a probe action denied
under the declared-scoped jail → reject, naming the axis), **declared-but-unused** (a
per-axis tightening pass drops an axis and the build still passes → reject, but only
when the static wrapper scan also agrees the axis is unreached, so an author is never
pushed to under-declare a compiled-in capability), **sandbox-unavailable** (reject
that platform — never run the untrusted build unconfined), **build-fails-in-jail**
(an ordinary compile error, reject), or **clean on every wired platform** (the only
admit path).

Two structural properties make it sound. The denial signal is **wrapper-owned**: the
untrusted build runs as a child of a probe wrapper Ipê authors, which owns the
per-axis exit-code contract, so the untrusted build cannot forge a clean result — it
does not own the exit the decoder reads. The confinement is **not forked**: the
declared-scoped profile is lowered by the same `profile_from_capabilities` the
run-jail runs under, so what Tier-2 confines a build to and what the shipped artifact
is confined to cannot drift. Differential confinement is strictly weaker than a
tracer — it observes reachability-under-denial, not intent — but only in the
fail-closed direction: it can over-reject a package a tracer would admit, never admit
one a tracer would reject, which is the correct bias for a supply-chain gate. The
admit predicate is a single conjunction over the typed outcome; a standing red-canary
(a native package that opens a socket while declaring `[]`) must always reject,
naming the axis. The audit tree materialized under `IPE_HOME` is same-user-trusted,
outside the package-adversary scope; the declared-scoped jail means even a tampered
runtime tree cannot forge a certified verdict.

### Per-platform returning build-jail arms; certify only what ran

The vehicle is the **returning** build-jail — `build_in_jail(profile, …) -> JailOutcome`,
one of `Clean` (the only admit-eligible value), `Denied { axis }`, `BuildFailed`, or
`Unavailable` — the returning counterpart to the run-jail's non-returning `exec`.
Every arm satisfies one contract: lower the shared unforked profile; decode the
wrapper-owned exit contract (`0` ⇒ `Clean` the only positive-proof branch, a
recognised per-axis code ⇒ `Denied`, anything else ⇒ `BuildFailed`) without
inspecting the payload's stdout; **fail closed on every establishment failure**
(missing primitive, unsatisfiable precondition, spawn failure ⇒ `Unavailable`, the
untrusted build never run unconfined); release every kernel object on every path
(the build-jail is called once per removable axis in a long-lived audit process, so
a leak accrues per call); and claim `Clean` only when every withheld axis was
genuinely denied.

Per-platform lowering: **Windows** — a Job Object (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS = 1`
when subprocess is withheld, `KILL_ON_JOB_CLOSE` so no orphan survives the audit
call) for `subprocess`, an AppContainer lowbox token for `filesystem` + `network`
(internet-client capability SID granted or omitted; scratch DACL-ACLed to the
container SID), launcher-side `env` scrub, and the `FILE_PERSISTENT_ACLS` volume
probe that refuses before launch on a non-ACL volume. **FreeBSD** — an entry wrapper
that pre-opens exactly the scratch (and the working tree when filesystem is granted)
then enters `cap_enter` capability mode, denying every ungranted namespace at the
kernel boundary (or a network-less `jail(2)` where a broader bounded view is needed);
launcher-side `env` scrub. The FreeBSD subprocess axis must be a genuine kernel
denial (plain `fork` survives `cap_enter`, so denial comes from `exec`/`fexecve`
being unreachable and `pdfork` ungranted); subprocess and env are confined but not
differentially probed, so a denied withheld-subprocess surfaces as the killed child's
`BuildFailed`, not a named `Denied { axis }`.

A landed jail arm promotes its platform from refuse-to-certify to certifying — the
audit surface then names exactly the platform whose jail ran, never claiming a
certification it did not run. The admit predicate is unchanged as arms are added: a
platform arm adds a lowering, never a new admit branch, and no non-`Clean` outcome
may ever reach admit.

### No authored abrupt-failure; two-gate enforcement; provenance-attributed scan

Authored abrupt-failure is forbidden in production Rust — the whole `panic!` /
`unreachable!` / `todo!` / `unimplemented!` / `assert*` / `.unwrap()` / `.expect()`
family, `panic_any`, `process::abort`, `unreachable_unchecked`, and indexing panics;
`process::exit` is boundary-only (the CLI `main`). Std/dependency internal panics are
the documented boundary of the claim ("no *authored* panic", not "cannot panic"). A
test's `assert!` is the harness working, not a defect, and is not policed the same
way. Two independent gates enforce it: **clippy** on the workspace (`[workspace.lints]`
denies the unwrap/expect/panic/indexing/unreachable/todo/unimplemented family;
`clippy.toml` adds `unwrap_unchecked`, `process::abort`, `panic_any`,
`unreachable_unchecked`), and **`tools/panic-scan`**, a proc-macro2 token scanner
(not a grep — string- and comment-mentions are invisible, line-split constructs are
still caught) that is region-aware and covers what clippy cannot: the `assert!` family
in production, and any Rust *text* including generated and third-party code.

The package gate is attributed by provenance: a pure-Ipê package's *emitted* Rust is
scanned and a hit is a **compiler bug** (our codegen must never emit abrupt failure
from pure Ipê) failing CI; an Ipê + Rust (FFI) package's *author-supplied* Rust
(identified by the `ModuleOrigin::FfiInterface` boundary) is scanned lexically and a
hit is a **user error**, a rejecting diagnostic. A documented `#[allow]` is tracked
debt, not an accepted state — the target is zero. The one principled exception is a
construct provably dead whose removal would reduce security (a loud assertion on a
structurally-dead HMAC key-derivation branch: Security > Correctness > Soundness),
retained by explicit decision.
