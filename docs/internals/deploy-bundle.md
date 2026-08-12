# Deploy bundle

`ipe deploy` produces a **single self-jailing binary**: a launcher
(`ipe-wrapper`) with the app binary and its capability profile fused in at
compile time. The launcher extracts the app, verifies the profile against the
capability floor embedded in the app binary, and execs the app inside the
sandbox jail — all without `ipe`, `cargo`, or any compiler on the target server.
Because the app is reachable only through the launcher, the jailed path is the
only path.

`--bundle` opts out of fusion and lays the wrapper, app, and profile out as
sibling files instead. That form is less safe: the `ipe-app` binary sits on
disk beside the wrapper and an operator can run it directly, bypassing the
sandbox. Prefer the default single-file form for production; reach for
`--bundle` only when the pieces must be inspected or replaced independently.

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

## Artifact layout (default: single self-jailing binary)

The default artifact is one file, with `ipe-app` and `ipe.profile` fused into
the wrapper at compile time (via `include_bytes!`):

```text
deploy/bundle/ipe-wrapper   # wrapper + app + profile, all baked in
```

The wrapper writes the app bytes to a temp file (owner-execute only, `0o700` on
Unix), verifies them with the same floor scan, execs under the jail, and removes
the temp file if exec fails. There is no separate app binary on disk, so the
jailed launcher is the only way to run the app.

On a target server with `bwrap` + `prlimit` installed, run the artifact as
(illustrative):

```text
./ipe-wrapper -- <app-args>
```

## Multi-file layout (`--bundle`)

`ipe deploy --bundle` lays the pieces out as siblings instead of fusing them:

```text
deploy/bundle/
├── ipe-wrapper   # the jailed launcher
├── ipe-app       # the statically-linked app binary (musl-static)
└── ipe.profile   # the capability manifest (plain text, auditable)
```

Running through `ipe-wrapper -- <app-args>` is jailed identically to the
single-file form. The difference is a security one: `ipe-app` is a native
ELF/Mach-O/PE binary sitting beside the wrapper, and an operator with local
access can run it directly, bypassing the wrapper and the jail. That bypass is
the reason the fused single-file form is the default — removing the standalone
app binary removes the un-jailed path entirely. Choose `--bundle` only when the
app and profile must be inspected or swapped independently, and treat the
directory as capable of running the app un-jailed.

## Inspecting the capability model (`--capabilities` / `--show-profile`)

`ipe deploy --capabilities` (alias `--show-profile`) prints the capability model
the app would enforce and exits without building or writing anything. It accepts
`--plain` (bare names, one per line) and `--json` (a stable object) alongside the
default human-readable report (illustrative):

```text
ipe deploy --capabilities --json
```

Once deployed, the running launcher exposes the same inspection via
`./ipe-wrapper --show-profile`.

## Relationship to `ipe exec`

`ipe exec <dir>` runs a build artifact jailed from a local build output
directory. It requires `cargo metadata` (and therefore `cargo`) to locate the
binary. `ipe deploy` extends that to a server with no toolchain by:

1. Building statically-linked (`--static --target <musl-triple>`) binaries —
   zero runtime dynamic-library dependencies.
2. Fusing the wrapper + app + profile into a single binary designed to be
   `scp`'d (or, under `--bundle`, a directory of the three files).
3. Having the wrapper resolve the app from its fused-in bytes (or, under
   `--bundle`, by a FIXED RELATIVE PATH) rather than via `cargo metadata`.

The jail mechanics are identical.

## Related

- `src/compiler/sandbox/src/run_jail.rs` — the SSOT jail implementation
- `src/ipe-wrapper/` — the wrapper crate
- `src/ipe-cli/src/run_sandbox.rs` — the CLI glue (`jail_and_exec`,
  `artifact_is_native`, `load_and_verify_artifact`)
- ADR 0040 — the native-bearing program jail policy
- Issue 703 — the env-name floor exactness refinement
