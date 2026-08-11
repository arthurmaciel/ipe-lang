# Deploy bundle

`ipe deploy` produces a **self-contained, toolchain-free jailed deploy bundle**:
a small launcher binary (`ipe-wrapper`) that locates the app binary (`ipe-app`)
and its capability profile (`ipe.profile`), verifies the profile against the
capability floor embedded in the app binary, and execs the app inside the
sandbox jail — all without `ipe`, `cargo`, or any compiler on the target server.

All command examples in this document are **illustrative** — `ipe deploy` is
built by this crate and requires the musl toolchain and (on the target server)
`bwrap`/`prlimit` at runtime; they are not runnable as-is from the repo root.

## Trust boundary

The wrapper trusts two things:

1. **The embedded capability floor** — a `#[used]` static in the app binary's
   `.rodata`, emitted by `ipe deploy` and scanned passively (the binary is
   never executed to read it). It records the precise axis grants (network,
   filesystem, subprocess, env var names) the binary was built expecting.
   `strip` cannot remove it (`.rodata` is allocated); a linker or attacker
   cannot forge it without rebuilding the binary.

2. **The `ipe.profile`** — a strictly-parsed plain-text manifest that describes
   the capability axes the running app is permitted. It is a *convenience
   mirror* of the floor; its job is to let an operator read and audit the
   sandbox policy without disassembling the binary.

The wrapper verifies: `profile.satisfies_capfloor(floor)`. A profile that
grants **more** than the floor allows is refused — a doctored `ipe.profile`
cannot widen the jail below what the binary was built for. A missing profile
is refused (fail-closed). A missing floor marker is refused (fail-closed).

## SSOT guarantee

The jail enforcement path is **shared with `ipe exec`**. Both call into the
same `ipe_sandbox::run_jail` primitives:

- `scan_capfloor` — passive binary scan for the embedded floor
- `satisfies_capfloor` — profile-vs-floor comparison
- `exec_in_run_jail` — the OS jail (bubblewrap + seccomp on Linux,
  `sandbox-exec` on macOS, Job Object + AppContainer on Windows)

There is no second jail implementation. Any future change to the jail mechanism
automatically applies to both the `ipe exec` path and the wrapper.

## Fail-closed points

| Condition | Outcome |
|---|---|
| `ipe.profile` absent or unreadable | refuse, exit non-zero |
| `ipe.profile` fails strict parse | refuse, exit non-zero |
| app binary has no `ipe-capfloor` marker | refuse, exit non-zero |
| profile grants more than the floor allows | refuse, exit non-zero |
| jail primitive absent (no `bwrap`/`prlimit`) | refuse, exit non-zero |
| jail establishment fails | refuse, exit non-zero |

None of these conditions has a permissive fallback. The wrapper either execs
the app under a verified jail or exits non-zero with a typed message.

## Bundle layout (default mode)

The bundle directory layout (illustrative):

```text
deploy/bundle/
├── ipe-wrapper   # the jailed launcher
├── ipe-app       # the statically-linked app binary (musl-static)
└── ipe.profile   # the capability manifest (plain text, auditable)
```

On a target server with `bwrap` + `prlimit` installed, run the bundle as
(illustrative):

```text
./ipe-wrapper -- <app-args>
```

## Embed mode (`--embed`)

`ipe deploy --embed` bakes `ipe-app` and `ipe.profile` into the wrapper binary
at compile time (via `include_bytes!`). The bundle is a single file:

```text
deploy/bundle/ipe-wrapper   # wrapper + app + profile, all baked in
```

The embed-mode wrapper writes the app bytes to a temp file (owner-execute only,
`0o700` on Unix), verifies them with the same floor scan, execs under the jail,
and removes the temp file if exec fails.

To audit the embedded profile without running the wrapper (illustrative):

```text
./ipe-wrapper --show-profile
```

## Honest limit

The inner `ipe-app` is a native ELF/Mach-O/PE binary. An operator with
sufficient local access can run it directly, bypassing the wrapper and the jail.
This is inherent to native executables — the wrapper makes the sanctioned,
jailed, profile-verified path the easy toolchain-free one; it does not prevent
a sufficiently privileged local operator from running the binary bare. The
security guarantee is: **any run through `ipe-wrapper` is jailed exactly as
tightly as the embedded floor requires**.

## Relationship to `ipe exec`

`ipe exec <dir>` runs a build artifact jailed from a local build output
directory. It requires `cargo metadata` (and therefore `cargo`) to locate the
binary. `ipe deploy` extends that to a server with no toolchain by:

1. Building statically-linked (`--static --target <musl-triple>`) binaries —
   zero runtime dynamic-library dependencies.
2. Packaging the wrapper + app + profile into a directory (or single file)
   designed to be `scp`'d.
3. Having the wrapper locate the app by a FIXED RELATIVE PATH rather than via
   `cargo metadata`.

The jail mechanics are identical.

## Related

- `src/compiler/sandbox/src/run_jail.rs` — the SSOT jail implementation
- `src/ipe-wrapper/` — the wrapper crate
- `src/ipe-cli/src/run_sandbox.rs` — the CLI glue (`jail_and_exec`,
  `artifact_is_native`, `load_and_verify_artifact`)
- ADR 0040 — the native-bearing program jail policy
- Issue 703 — the env-name floor exactness refinement
