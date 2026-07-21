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
- Native FFI wrapper crates ([Tier 2](architecture/tbd/ffi-tier2-inspect-author-rust.md))
  are held to a stricter, fail-closed bar while the runtime sandbox is being
  built. A wrapper's Rust runs with the process's full authority at `ipe run` —
  there is **not yet** a runtime jail around the emitted app — so a wrapper that
  declares or is inferred to reach a runtime-enforced capability (network,
  filesystem, database, environment, subprocess, native-ffi) is **refused at
  install** rather than admitted unenforced. Only wrappers confined to the
  containable axes (clock, random) or to pure compute install today. This is the
  honest posture until the runtime jail lands, at which point those axes re-open
  one at a time as each is actually scoped.

How native Rust is bound — and how its capabilities are established — is covered by
the FFI docs: the declarative [`provide.*`](architecture/tbd/ffi-rust-type-creation-and-coverage.md)
surface (whose shapes stay capability-inferable) and the
[wrapper-crate tier](architecture/tbd/ffi-tier2-inspect-author-rust.md) (which
declares capabilities, is inference-checked, and is refused unless its effects
are containable).

## Where enforcement lives

- **Pure Ipê code** → the capability is absent from the binary. Nothing to
  enforce at run time; the guarantee is structural.
- **Packages** → the declared set must match the inferred set (checked at
  compile), and installing is consent to it (`ipe add`).
- **Native code (build)** → the RCE build sandbox isolates the compile of an
  untrusted crate (fresh empty net namespace, read-only `/`, scrubbed env), so a
  malicious build script or proc-macro is contained while inspecting/building.
- **Native code (run)** → the emitted-app runtime jail is **not yet built**. Until
  it is, a Tier-2 wrapper that would reach a runtime-enforced capability is
  *refused at install* (above) rather than run uncontained — fail-closed, not
  fail-open.

## Going deeper

- The package-level trust model — how capabilities feed the package index, the
  install-time consent flow, and the supply-chain gate — is designed in
  [package coordination & capability-based trust](architecture/tbd/package-coordination-and-capabilities-design.md).
