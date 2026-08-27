# Sky behavior-parity harness

The parity harness cross-checks Ipê ports against the upstream compiler by running
both side by side and byte-comparing their output. This document covers the
`parity` manifest field, the compare script, and how to run the check locally.

## What is compared

Increment 1 covers ports that are:

- `status = "green"` in `examples/sky/manifest.toml`
- `verify = "run"` (exits with code 0 headlessly)
- `shape` is `program` or `console` (deterministic stdout programs)
- `go_ffi = false`

Web, TUI, server, and webview shapes are later increments (they require a
browser or a live HTTP probe to validate output).

## The `parity` field

Each in-scope example carries a `parity` field that tells the compare script how
to treat its output:

| Value | Meaning |
|-------|---------|
| `exact` | Stdout and exit code must be byte-identical between the upstream run and the Ipê port. |
| `normalized` | Stdout is pre-processed to strip known nondeterministic tokens (ISO 8601 timestamps, 13-digit epoch-ms integers) before comparison. Structural output must still match. |
| `skip` | Output is intrinsically nondeterministic (random tokens, UUIDs, wall-clock timestamps that appear in the output). The port is printed as SKIP with a one-line reason; it is never a FAIL. Only skip on genuine nondeterminism — do not weaken a deterministic port to skip to silence a mismatch. |

When `parity` is absent the default is `exact`.

A `parity_skip_reason` string is required alongside `parity = "skip"`.

## Running locally

**Step 1 — install the upstream toolchain (one-time).**

The installer downloads a pinned prebuilt binary from the upstream GitHub release.
The pinned version is `v0.19.13`; update `PINNED_VERSION` in
`tools/scripts/install-sky-toolchain.sh` when the example corpus moves to a newer
release.

To install into `~/.local/bin` (the default):

```bash
# verified: downloads sky v0.19.13 for linux/x64 or darwin/arm64
bash tools/scripts/install-sky-toolchain.sh
export PATH="$HOME/.local/bin:$PATH"
sky --version   # prints: sky v0.19.13
```

To specify a version and a custom destination:

```bash
# verified: installs to /tmp/sky-bin/sky
bash tools/scripts/install-sky-toolchain.sh v0.19.13 --dest /tmp/sky-bin
```

**Step 2 — build ipe** (requires the Rust toolchain; deferred to CI when not
available locally):

```bash
# illustrative — requires cargo + Rust toolchain
cargo build --release -p ipe
```

**Step 3 — run the parity check.**

With `ipe` on PATH or built at the default target location, and `sky` on PATH:

```bash
# verified: runs the two skip ports (no ipe build required for skip policy)
bash tools/scripts/check-sky-parity.sh --names 35-composite-generics,03-tea-external
```

Full run (requires ipe binary from Step 2):

```bash
# illustrative — requires built ipe binary
bash tools/scripts/check-sky-parity.sh
```

Useful flags (all verified):

```bash
# Run a single port
bash tools/scripts/check-sky-parity.sh --names 01-hello-world

# Comma-separated subset
bash tools/scripts/check-sky-parity.sh --names 01-hello-world,04-local-pkg

# Point at a specific sky binary
bash tools/scripts/check-sky-parity.sh --sky-bin /tmp/sky-bin/sky --names 01-hello-world

# Show more diff context on a mismatch (default 40 lines)
bash tools/scripts/check-sky-parity.sh --diff-lines 80 --names 01-hello-world
```

The script exits 0 on full pass, 1 on any mismatch, 2 on setup error (missing
binary, disk under 5 GB).

## CI job

`.github/workflows/sky-parity.yml` runs nightly at 05:47 UTC and on manual
`workflow_dispatch`. It is **not** a required PR gate — the fast offline gate is
`sky-ports-gate.yml`. The nightly job:

1. Installs the pinned upstream toolchain via `install-sky-toolchain.sh`.
2. Builds ipe (`cargo build --release -p ipe`).
3. Runs `check-sky-parity.sh` and reports the summary.

The job is a hard gate (`continue-on-error: false`): any sky-vs-ipe stdout or
exit-code divergence on an in-scope port fails it. A newly-added port that does
not yet match must be classified `parity = "skip"` (with a reason) until fixed,
rather than left to redden the gate.

## Adding a new port to the parity set

When an example is promoted to `status = "green"` and `verify = "run"`:

1. Add `parity = "exact"` to its manifest entry (or `"skip"` with a
   `parity_skip_reason` if the output is genuinely nondeterministic).
2. Run `check-sky-parity.sh --names <name>` locally to confirm the comparison
   passes.
3. Commit the manifest change.
