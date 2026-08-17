# code-review

A small Ipê/TEA web app that reads an `ipe-index` `index.db` as its review
backlog: it lists the units the index has queued for review, shows each unit's
source slice and context, and drains a unit's `change_queue` row once decided.

## Prerequisites

This app is written in Ipê, so the `ipe` compiler must be installed and on your
`PATH`. Install it from the repo root with `./install.sh` — it builds and installs
`ipe` to a bin dir on your `PATH`. Confirm with `ipe version`.

The app reviews an `ipe-index` database, so you also need one. Build it once from
the repo root:

```bash
tools/scripts/ipe-index index      # → .ipe-index/index.db
```

See `tools/ipe-index/README.md` for that tool.

## Building and running

```bash
ipe type-check      # fast, no runtime needed
ipe build           # compile to a native binary
ipe run             # build + serve on http://localhost:8000
```

### The compiler ⇄ runtime pairing

`ipe build`/`ipe run` links the emitted Rust against the Ipê runtime, and the
two **must be the same version** — a program emitted by compiler `X` cannot link
a runtime `Y != X`. The matching runtime for the installed `ipe` re-materializes
on demand at `~/.ipe/runtime/<version>/rust`; build against that one.

This directory lives inside a full compiler checkout whose vendored runtime
snapshot (`../../src/runtime/rust`) is pinned to that checkout's own version.
The installed `ipe` auto-discovers a runtime by walking up from the working
directory, so from here it finds the enclosing checkout's snapshot instead of
its own — a version skew that fails the build with:

```
ipe: the Ipe runtime at .../src/runtime/rust is version A, but this compiler is B
```

Point the build at the installed compiler's own runtime to avoid the skew:

```bash
ver=$(ipe version | awk '{for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\.[0-9]+\.[0-9]+$/) print $i}')
IPE_RUNTIME_DIR="$HOME/.ipe/runtime/$ver/rust" ipe build
```

`ipe version` reports the installed compiler version; the runtime under
`~/.ipe/runtime/<that version>/rust` is its guaranteed match.

## Configuration

The app reads its index at runtime from environment variables (all optional):

| Variable         | Default                 | Meaning                                        |
|------------------|-------------------------|------------------------------------------------|
| `IPE_INDEX_DB`   | `.ipe-index/index.db`   | Path or `sqlite://` URL of the `ipe-index` DB. |
| `IPE_INDEX_ROOT` | current directory       | Root the index's `tag:relative` paths join to. |

The index DB is opened read-only for listing and read-write only to delete a
consumed `change_queue` row.
