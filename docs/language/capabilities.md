# Capabilities — what a program is allowed to do

Ipê tells you exactly what a program can touch on the security-relevant axis —
network, filesystem, database, environment, subprocess, clock, randomness, native
code — **from its code alone, with nothing to declare.** The compiler reads the
answer off the program; you never annotate it, and it cannot be wrong or hidden.

The idea is *verify behaviour, not reputation*: rather than trusting that a
dependency is well-behaved, you can see the precise set of things it is even
*able* to do before you run it.

## The nine capabilities

| Capability | What it covers |
|---|---|
| `network` | Outbound or inbound network — HTTP client/server, WebSocket, email send. |
| `filesystem` | Reading or writing files, directories, an `.env` or config file. Not database access. |
| `database` | Structured database access — SQL queries, migrations, row decoders. |
| `env` | Reading process environment — environment variables, `argv`. |
| `subprocess` | Spawning or controlling a child process. |
| `clock` | Reading wall-clock or monotonic time, sleeping, or firing on a timer. |
| `random` | Drawing non-determinism — RNG, random tokens, UUIDs. |
| `native-ffi` | Crossing into native `Rust.` code, which is opaque to inference (see below). |
| `ffi-raw` | A native crossing whose signature the author asserted via `Rust.Ffi.call`, rather than derived from crate inspection. Always accompanies `native-ffi`; its presence discloses the assertion. |

The vocabulary is closed and coarse for now: `network` means *any* network, not
per-host; `filesystem` means *any* file, not per-path. Finer, per-resource
granularity is a planned refinement.

## Seeing a program's set

```
$ ipe capabilities examples/sky/02-go-stdlib/src/Main.ipe
network
clock
```

`ipe capabilities <entry>` prints the inferred set, one per line — or `none` for
a pure program.

## How it is inferred

Every effect in Ipê flows through a **capability-tagged kernel** — there is no way
to reach the network without going through a kernel the compiler has tagged
`network`. So the compiler walks the program's call graph and takes the **union**
of the capabilities of every kernel it can transitively reach. That union is the
program's capability set.

This gives a guarantee stronger than a runtime check: for pure Ipê code, a
capability a program does not use is **not present in the compiled binary at
all** — there is no code path to it to block, because there is nothing to block.
An unused capability is *unrepresentable*, not merely denied.

## The native boundary

`native-ffi` is the one capability the compiler cannot see through: native Rust
brought in via `Rust.` can make any system call, so its true effects are opaque to
inference. Ipê handles this without a blind spot:

- A package that carries native code **declares** the native capabilities it uses
  in its manifest. There is no secret native capability — the declaration is
  exactly what you are shown.
- `ipe add` prints the resolved set (inferred Ipê capabilities **plus** declared
  native ones) before installing, and is **loud on `native-ffi`**. Installing is
  informed consent to that set.
- The declared set is checked against the code: a capability the program uses but
  did not surface is a compile error, so a malicious effect cannot hide — it must
  appear as a capability you consented to.
- Native FFI wrapper crates (a planned author-supplied-crate tier, tracked as a
  GitHub issue) are **admitted and isolated by the runtime jail** on a target where the jail
  holds (Linux first). A wrapper that reaches a runtime-enforced capability
  (network, filesystem, database, environment, subprocess, native-ffi) is
  installed and then run confined to its declared-plus-inferred set — an
  undeclared effect fails closed at the OS boundary. On a platform in the
  documented refuse-gap (no jail), the older fail-closed posture stays: such a
  wrapper is **refused at install** rather than admitted-and-run-unconfined.
  Native code is **contained, not proven**: the manifest gate cannot see through
  native Rust, so the jail contains an under-declared wrapper (it does not catch
  the under-declaration).

How native Rust is bound — and how its capabilities are established — is covered by
the FFI subsystem ([ADR 0033](../adr/0033-ipe-rust-ffi-subsystem.md)). Two extensions
are planned and tracked as GitHub issues: a declarative `provide.*` type-creation
surface (whose shapes stay capability-inferable) and an author-supplied
wrapper-crate tier (which declares capabilities, is inference-checked, and is
refused unless its effects are containable).

## Where enforcement lives

- **Pure Ipê code** → the capability is absent from the binary. Nothing to
  enforce at run time; the guarantee is structural.
- **Packages** → the declared set must match the inferred set (checked at
  compile), and installing is consent to it (`ipe add`).
- **Native code (build)** → the RCE build sandbox isolates the compile of an
  untrusted crate (fresh empty net namespace, read-only `/`, scrubbed env), so a
  malicious build script or proc-macro is contained while inspecting/building.
- **Native code (run)** → the emitted-app **runtime jail** confines the running
  program to its declared-plus-inferred capability set (`ipe run`, and `ipe exec`
  for a built artifact). On Linux it is an OS-level, fail-closed jail: a fresh
  network namespace when `network` is absent, a scoped filesystem when
  `filesystem` is absent, a scrubbed environment, a `seccomp` filter denying
  subprocess creation (and, unconditionally, `ptrace`/`io_uring`/mount-family
  escape primitives), plus a fresh `/proc` and `no_new_privs`. An unavailable
  primitive **refuses to run** rather than running unconfined; the narrow
  `IPE_ALLOW_UNSANDBOXED=1` override is a hard error for any high-value native
  axis. macOS and other platforms are a documented refuse-gap: a native-capability
  program refuses to run there rather than running unconfined. A built artifact
  carries its enforcement — an `ipe.profile` plus a capability floor embedded in
  the binary — so `ipe exec` re-applies the jail wherever the artifact runs; a
  tampered profile that requests less isolation than the embedded floor is
  refused.

## Not covered: resource quotas

Capabilities answer *what* a program may touch, not *how much* it may consume.
Bounding a program's memory, CPU time, or I/O throughput is a separate concern
that native compilation does not provide at the language level — a native binary
can loop or allocate without limit. The sandbox has partial time/`prlimit` hooks,
but a general per-program quota model is deferred to the deployment layer
(OS-level `cgroups`/`prlimit`) rather than the language. This is a deliberate
trade for native speed; a VM-based runtime would bound these for free.

## Going deeper

- The package-level trust model — how capabilities feed the package index, the
  install-time consent flow, and the supply-chain gate — is decided in
  [ADR 0044](../adr/0044-package-coordination-manifest-index-gate.md).
