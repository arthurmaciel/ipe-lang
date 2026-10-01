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

Two environment variables are **required**. `main` parses both at startup and
exits with an actionable message if either is unset or unusable, so a
misconfigured run fails closed rather than opening a wrong-path database or
joining a stored path outside the repo:

| Variable         | Meaning                                                       |
|------------------|---------------------------------------------------------------|
| `IPE_INDEX_DB`   | Path or `sqlite://` URL of the `ipe-index` DB.                |
| `IPE_INDEX_ROOT` | Repo root the index's `ipe:relative` paths join to; must be an existing directory. |
| `IPE_REVIEW_DB`  | Optional path or `sqlite://` URL of the review DB; defaults to `review.db`. |

A relative path in any of the three resolves against the working directory. A
file path containing `?`, `#` or `%` is refused — pass such a location as a
percent-encoded `sqlite://` URL. A `sqlite://` URL must carry no `?` query and
must name a database: the app appends the open mode itself. A `sqlite:` or
`file:` value without `//` is refused. A refused database location is reported
by variable name only, never by value, since a mistyped URL can carry a
password.

`IPE_INDEX_ROOT` binds the repo tag `ipe`, the tag of `ipe-index`'s default
`--repo ipe:.`. A unit stored under any other tag (an index built with a second
`--repo`) is refused rather than read from that root, so the page never shows a
different file than the one the index recorded. A tag is the text before the
first `:` when no `/` precedes it, matching how `ipe-index` reads it back.

Every stored `tag:relative` path is sealed with `Ipe.Path.fromString` and joined
with `Ipe.Path.under`, so it must land strictly under its tag's root: an
empty, absolute, or NUL-bearing stored path, or one whose `..` climbs out of it,
is refused with an error naming it and the `Ipe.Path` reason. The check is lexical — a symlink inside the repo is
followed, so it can point a read outside the root. A source file larger than
16 MiB is refused rather than read.

The index DB is opened read-only for listing and read-write (never created) only
to delete a consumed `change_queue` row. The app creates and owns the review DB.

## Running

From this directory, against an index built at the repo root:

```bash
IPE_INDEX_DB=../../.ipe-index/index.db IPE_INDEX_ROOT=../.. ipe run
```

`ipe run` builds and serves on <http://localhost:8000>. `ipe type-check` runs a
fast check with no runtime, and `ipe build` compiles to a native binary.

The queue view loads one page of at most 200 units (`pageSize` in `src/Lib/Index.ipe`).

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
