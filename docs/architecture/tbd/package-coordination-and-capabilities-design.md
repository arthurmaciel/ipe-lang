# Ipê package coordination & capability-based trust — design

> **Status: Accepted (design) — not yet implemented.** Becomes an ADR once landed.
>
> Scope: coordination of third-party **Ipê** packages — how they are published,
> resolved, verified, and trusted. **Supersedes decision D6** of
> `namespace-imports-and-packaging-spec.md` (git-first / no central gate). Keeps
> D1–D5 (the `Ipe.` first-party and `Rust.` FFI namespace boundary; crates.io as
> the FFI registry).

## Why this supersedes D6

D6 chose a fully decentralized, Go-style model — "any git repo resolves, integrity
by lockfile, no central gate" — explicitly to escape Elm's centralization pain:
one gatekeeper, one server, no private/git/fork deps.

The realization that changes the calculus: **a curated index can be a GitHub
repository gated by CI**, so curation costs no bespoke server and no human
bottleneck. Under the principle order — Security is the first, strict tie-breaker —
a *vetted* install path dominates an unvetted one: with D6, any git repo resolves
as a public dependency with no wall, which is precisely an attacker foothold
(expired or weak-password domains handing over a dependency, typosquatting). A
CI-gated index makes an unvetted public package unrepresentable in the install
path — `make-invalid-states-unrepresentable` applied to packaging, the same shape
as the FFI decode-boundary newtypes and the SEAL itself. The freedoms D6 protected
(private, fork, experimental, offline) survive via an explicit git/path escape
hatch. So: an authoritative CI-gated index **plus** the loud escape hatch — not a
free-for-all, and not a single-server gatekeeper either.

## The surface: a "Package authoring" command group

A new help section, `Package authoring`, alongside Development / FFI / Tools:

- `ipe add <name>[@<version|latest>]` — install a public Ipê package resolved
  through the curated index. `ipe remove <name>` uninstalls.
- `ipe package publish` — publish the current package: validate locally, then open
  a pull request against the index.
- `ipe package audit` — run the full gate toolchain locally and report what is
  missing or would fail. A pre-flight that reduces bounced submissions and teaches
  the gate (the compiler-as-kind-teacher ethos extended to packaging).

The existing crate-FFI `ipe add` / `ipe remove` (which bind a crates.io Rust crate)
are renamed **`ipe rust add` / `ipe rust remove`**, mirroring the `Ipe.` vs `Rust.`
namespace split: `ipe add` is for Ipê packages, `ipe rust …` for the native FFI
boundary.

## The authoritative index

A public git repository is the index. Each entry maps a canonical short name to:
source URL, per-version content hash, the package's capability set, and the
publisher identity.

- `ipe add <name>` resolves **only** through the index: one canonical version line
  per name (no squatting), reproducible, content-hash pinned.
- Private, forked, or experimental dependencies use an explicit
  `{ git = … }` / `{ path = … }` entry in `ipe.toml` — the loud, visible escape
  hatch. It carries lockfile + checksum integrity but bypasses the gate by design;
  the reader sees exactly which dependencies are un-vetted.

Reproducibility comes from the lockfile, not from the registry being reachable
(D6's one keeper): a build resolves against the lockfile's pinned hashes.

## Publishing: `ipe package publish`

`ipe package publish` validates the package locally (manifest well-formed, source
present, tests present, inferred capabilities, correct semver bump), then opens a
pull request that adds or updates the package's index entry. GitHub Actions is the
infrastructure — there is no bespoke package server to run or defend.

## The gate

The gate runs on the index pull request. It downloads the package at the proposed
version, verifies the content hash, and runs in a sandbox.

### Universal tier — every package, including pure Ipê

Every package's emitted Rust is held to the same quality bar the compiler holds
itself to — because on a *well-typed* program, missing that bar is the **emitter's**
fault, not the user's. Universal stages, all on the emitted crate:

- `ipe build` → `cargo build` → run → the package's tests (the SEAL).
- `cargo clippy` with the workspace's strict lints — no `unwrap`/`expect`/`panic`/
  raw indexing, no `dyn Any`.
- `cargo fmt --check` — the emitted Rust is rustfmt-clean.
- capability consistency (below) and enforced-semver.

Each stage catches a *distinct* class of compiler bug on real-world Ipê that the
first-party goldens do not exercise: a `cargo build` failure is a SEAL break; an
emitted `unwrap`/`panic`/`dyn Any` or a lint-failing construct is an emitter
soundness/quality regression; unformatted output is an emitter formatting bug (the
class the native formatter closes by construction). None can be a user error — the
program is well-typed.

### Native tier — packages that cross `Rust.`

A package whose source contains any `Rust.` crossing additionally runs: miri (UB in
the unsafe/FFI surface — no signal on safe pure-Ipê output, so native-only),
cargo-audit + cargo-deny (the crate dependency graph), the declared-native-capability
sandbox check, a visible "contains native code" label on the index entry, and a
longer review path (manual sign-off / verified publisher). `rg '\bRust\.'`
enumerates every native crossing — the audit primitive from D1 doubles as the
security primitive.

### Failure classification + tickets

A gate failure is classified and reported:

- **User-code error** (a type error, a missing test, a wrong semver bump): the PR
  fails with the compiler's own diagnostic and its explain page. The author fixes
  and re-pushes.
- **Ipê compiler bug**: the gate auto-opens a ticket, priced by class. A **SEAL
  break** — a *well-typed* package whose `ipe build` exits 0 but whose emitted Rust
  fails `cargo build` — is filed **CRITICAL** (it violates the core invariant). An
  emitter-quality failure on a well-typed program — a strict-lint hit (`unwrap`/
  `panic`/`dyn Any`) or unformatted output — is filed **High** (an emitter
  regression, not un-buildable). Both are the compiler's fault, not the author's.

An emergent property: the index becomes a **continuous regression corpus for the
whole class of Ipê compiler bugs the project hardens against** — SEAL breaks,
emitted `unwrap`/`panic`/`dyn Any`, unformatted output, and future unknowns — not
just the SEAL. Every published real-world program is a free stress test of emitter
soundness *and* quality, and any regression is detected and escalated
automatically by the ecosystem itself.

## Capabilities: the trust core

Security is gated on *what code may do*, not *what language it is written in* — the
correct axis, because a malicious effect (network exfiltration, filesystem read) is
just as dangerous from Ipê as from Rust. Ipê is unusually well positioned for this:
its effects flow through capability-tagged kernels, so the Ipê runtime plays the
role of a capability platform (the model Roc uses), while `Rust.` is the single
escape hatch (the analogue of a permission system's raw-FFI escape).

### Ipê code — inferred, nothing to declare

The compiler walks the call graph and computes the capability set from the kernels
a program transitively uses (effect-inference style). The manifest's capability
block is **generated, not hand-written**, and cannot drift: a call to a kernel whose
capability the package has not surfaced is a compile error, and the gate verifies
the declared set equals the actually-used set. Zero authoring cost; exact by
construction. `ipe package audit` prints the inferred set for review; the same set
is shown at `ipe add` for consent.

### Native code — declared, sandbox-enforced, fail-closed

Native code is opaque to capability inference, so a native-bearing package declares
the extra capabilities its `Rust.` code needs. The declaration is *consent*; the
**sandbox is the enforcement**, and it is fail-closed:

> `ipe_sandbox` is configured to the **union of (inferred Ipê capabilities) +
> (declared native capabilities)** and denies everything else. If the package's
> Ipê side uses no network and the author did not declare native network, the
> package runs with the network namespace unshared — native network is impossible,
> not merely discouraged. A package that declares less than its native code
> attempts is denied at the OS boundary, not trusted. Native can never exceed the
> sandbox ceiling, and the ceiling equals exactly what the user consented to.

There is no secret native capability: the declaration is what the user sees; the
sandbox is what actually holds.

### Consent

`ipe add` shows the resolved capability set (inferred + declared) before install
and is loud on `native-ffi`. Installing is informed consent to that set.

### v1 scope

- v1 ships: inferred Ipê capabilities (exact, compiler-verified) + declared native
  capabilities + **coarse** `ipe_sandbox` enforcement using what it already
  provides — network-namespace isolation, filesystem / environment / pid
  isolation, and resource limits (`prlimit`). This is fail-closed today for the
  high-value capabilities (network, filesystem, environment, subprocess).
- Fine-grained per-capability syscall filtering (a seccomp capability→syscall map)
  is a tracked v2. Airtight native enforcement is genuinely hard — comparable
  permission systems are only partially sound — so v1 is honest about the tier
  rather than overclaiming.

## Enforced semver

The gate computes the API delta between the submitted version and the prior indexed
version (`ipe diff`, reusing the type / interface surface) and rejects an incorrect
version bump. Pre-1.0 rules: a compatible change is a patch, a breaking change is a
minor; 1.0 stays a deliberate stability promise. This is Elm's genuinely great idea
— a breaking change cannot ship as a patch — made compatible with decentralization:
it runs in CI and needs no authority beyond the index providing the canonical prior
version.

## Manifest + lockfile

`ipe.toml`:

- `[dependencies]` — index name → version; `{ git = … }` / `{ path = … }` escapes.
- `[rust.dependencies]` — crates.io FFI crates (the `Rust.` boundary, per D5).
- `[capabilities]` — the capability set (generated for the Ipê part; the native
  part is authored).

The lockfile pins exact resolved versions and content hashes for both dependency
kinds. Renames and upgrades touch the manifest, never source (D4).

## Threat model & honest limits

- **Pure-Ipê packages** carry no native foothold: their effects are kernel-mediated
  and capability-inferred, so a malicious effect must appear as a declared
  capability the user consents to — it cannot hide. This tier is Elm-grade safe.
- **Native-bearing packages** are the residual surface. Declaration + consent +
  fail-closed sandbox + the full audit gate + the visible label make native code
  loud, attributable, gated, and opt-in — but not magically safe. The v1 coarse
  sandbox covers the high-value capabilities; the fine-grained seccomp map (v2) is
  where full airtight enforcement lands.
- The gate protects the index and the CI host at submission; the sandbox protects
  the end user at install/run. Both are needed: a submission-only check would say
  nothing about what a package does on a user's machine.

## Relationship to sibling specs

- Supersedes **D6** of `namespace-imports-and-packaging-spec.md`; that spec's D1–D5
  stand and are relied on here (the `Ipe.`/`Rust.` boundary, crates.io as the FFI
  registry, origin/version out of module names).
- The universal SEAL gate rests on the existing SEAL guarantee; the native sandbox
  rests on the existing FFI `ipe_sandbox`.

## Open questions (decide during implementation)

- The exact capability list and its granularity (e.g. `network` whole vs
  per-host; `filesystem` whole vs per-path).
- The index-repository entry schema and how `ipe add` resolves + verifies it.
- Auto-merge wait times per tier and the verified-publisher mechanism for native
  packages.
- How `ipe diff` computes the API delta — reuse the LSP / typed-interface surface.
- The compiler-bug ticketing target (tracked with the move off the local backlog to
  an issue tracker, brainstormed separately).
