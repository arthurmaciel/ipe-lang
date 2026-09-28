# code-review

A small Ipê/TEA web app that reads an `ipe-index` `index.db` as its review
backlog: it lists the units the index has queued for review, shows each unit's
source slice and context, and drains a unit's `change_queue` row once decided.
`main : Task Error ()` serves the embedded TEA app over `Ipe.Server.Http`, so
the program stands on its own — no wrapper script.

## Prerequisites

This app is written in Ipê, so the `ipe` compiler must be installed and on your
`PATH`. Install it from the repo root with `./install.sh`; confirm with `ipe
version`.

The app reviews an `ipe-index` database, so you also need one. Build it once
from the repo root:

```bash
tools/scripts/ipe-index index      # → .ipe-index/index.db
```

See `tools/ipe-index/README.md` for that tool.

## Configuration

Two environment variables are **required** — `main` checks both at startup and
exits with an actionable message if either is unset, so a misconfigured run
fails closed rather than opening a wrong-path database:

| Variable         | Meaning                                              |
|------------------|------------------------------------------------------|
| `IPE_INDEX_DB`   | Path or `sqlite://` URL of the `ipe-index` DB.       |
| `IPE_INDEX_ROOT` | Repo root the index's `tag:relative` paths join to.  |

The index DB is opened read-only for listing and read-write only to delete a
consumed `change_queue` row. `IPE_REVIEW_DB` is optional and defaults to
`review.db` (the app creates and owns this database).

## Running

There is no `package.ipe` at the repo root — this app lives in
`tools/code-review`, so run it from there (or `cd` into it first):

```bash
(cd tools/code-review && IPE_INDEX_DB="$(git rev-parse --show-toplevel)/.ipe-index/index.db" IPE_INDEX_ROOT="$(git rev-parse --show-toplevel)" ipe run)
```

`ipe run` builds and serves on <http://localhost:8000>. `ipe type-check` runs a
fast check with no runtime, and `ipe build` compiles to a native binary.

The queue view loads at most one bounded page of pending units at a time
(`pageSize` = 100, see `src/Lib/Index.ipe`) rather than the whole
`change_queue` table, so a large backlog stays fast; the header shows how many
of the total queued units the current page holds.

If you run from inside a compiler checkout, `ipe` may auto-discover the
checkout's vendored runtime snapshot instead of its own version-matched one,
failing the build with a version skew. Point the build at the installed
compiler's runtime to avoid it:

```bash
ver=$(ipe version | awk '{for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\.[0-9]+\.[0-9]+$/) print $i}')
IPE_RUNTIME_DIR="$HOME/.ipe/runtime/$ver/rust" ipe run
```
</content>
</invoke>
