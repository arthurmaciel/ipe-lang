# Capabilities — what a program is allowed to do

Ipê tells you exactly what a program can touch on the security-relevant axis —
network, filesystem, database, environment, subprocess, clock, randomness, native
code — **from its code alone, with nothing to declare.** The compiler reads the
answer off the program; you never annotate it, and it cannot be wrong or hidden.

The idea is *verify behaviour, not reputation*: rather than trusting that a
dependency is well-behaved, you can see the precise set of things it is even
*able* to do before you run it.

## The eight capabilities

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
- At run time the sandbox **enforces** the set, fail-closed: if a package did not
  declare `network`, the network namespace is left unshared and native network is
  simply impossible. High-value capabilities (network, filesystem, environment,
  subprocess) are isolated by the sandbox.

How native Rust is bound — and how its capabilities are established — is covered by
the FFI docs: the declarative [`provide.*`](architecture/tbd/ffi-rust-type-creation-and-coverage.md)
surface (whose shapes stay capability-inferable) and the
[wrapper-crate tier](architecture/tbd/ffi-tier2-inspect-author-rust.md) (which
declares capabilities and is sandbox-enforced).

## Where enforcement lives

- **Pure Ipê code** → the capability is absent from the binary. Nothing to
  enforce at run time; the guarantee is structural.
- **Packages** → the declared set must match the inferred set (checked at
  compile), and installing is consent to it (`ipe add`).
- **Native code** → the sandbox isolates the declared high-value capabilities,
  fail-closed, so undeclared effects cannot occur.

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
  install-time consent flow, and the supply-chain gate — is designed in
  [package coordination & capability-based trust](architecture/tbd/package-coordination-and-capabilities-design.md).
