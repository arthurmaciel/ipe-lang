# ipe-index — manual

A fast, sqlite-backed **code-relation index** over the Ipê repo. Ask it "where is
X defined", "who imports Y", "which examples exercise a module" — one answer,
instantly, instead of grepping and guessing.

**Rule of thumb:** reach for `ipe-index` before `rg` for structural questions
(defs, imports, dependents). Use `rg` only for free-text / substring hunts.

---

## Quick start

```bash
# From the repo root. The wrapper builds the Rust binary on first run.
tools/scripts/ipe-index index          # build the index → .ipe-index/index.db
tools/scripts/ipe-index locate Lowerer # where is `Lowerer` defined / impl'd?
tools/scripts/ipe-index wakeup         # one-screen digest of the whole index
```

`tools/scripts/ipe-index` is a thin wrapper that execs the compiled binary at
`tools/ipe-index/target/release/ipe-index`. Anywhere the docs say `ipe-index`,
run `tools/scripts/ipe-index` (or the binary directly).

The index auto-refreshes after **every commit** (git `post-commit` hook) — you
rarely run `index`/`update` by hand.

---

## Build & install

**Prerequisite:** a Rust toolchain (`cargo`). The crate is edition 2024, so use a
recent stable Rust (`rustup update`).

`ipe-index` is a standalone crate — its own `target/`, detached from the compiler
workspace. You don't have to build it by hand: the `tools/scripts/ipe-index`
wrapper compiles it on first run and execs the release binary. To build it
explicitly:

```bash
cd tools/ipe-index && cargo build --release
# → tools/ipe-index/target/release/ipe-index
```

Nothing installs to your `PATH`: invoke the wrapper `tools/scripts/ipe-index`
(which finds the repo root and execs the binary) or run the binary directly.

**Auto-refresh (recommended):** a local git `post-commit` hook runs
`ipe-index index` after each commit so the index never drifts. Hooks live in
`.git/hooks/` and are not tracked, so after a fresh clone seed the index once:

```bash
tools/scripts/ipe-index index      # builds the binary + indexes the repo
```

To keep it fresh automatically, add a `.git/hooks/post-commit` that runs
`tools/scripts/ipe-index index` (and make it executable).

---

## The mental model

### One repo, tag-prefixed paths

Every path is prefixed with a repo tag so results tell you where they came from
and so a future multi-repo setup never collides:

| Tag   | Repo | What lives there |
|-------|------|------------------|
| `ipe:`| `.`  | Rust compiler (`src/compiler/`), runtime (`src/runtime/`), tooling (`tools/`), stdlib/examples (`*.ipe`) |

So a result reads `ipe:crates/ipe_lower/src/lower.rs:2518` — the prefix is the
repo.

### Languages, real defs

| Language          | Defs captured | Imports |
|-------------------|---------------|---------|
| Rust              | fn, struct, enum, trait, type, const, static, macro, mod, **impl targets** | `use` |
| TypeScript / JS   | function, arrow-const | `import` / `export … from` |
| Bash              | `name()` functions | `source` / `.` |
| Ipê               | bindings | `import` |

---

## Commands

### Finding things

```bash
ipe-index locate <Name>      # every def/impl site of a symbol
ipe-index deps <module>      # what <module> imports (substring match)
ipe-index rdeps <module>     # who imports <module> (exact; --subtree, --count)
ipe-index covers <module>    # which examples/fixtures exercise a module
```

`locate` example (each site carries the unit's qualified name + uid, so it
feeds the review commands below directly):

```
$ ipe-index locate Lowerer
ipe:src/compiler/lower/src/lower.rs:7758:12  def   crate::Lowerer  4a6585e9…
ipe:src/compiler/lower/src/lower.rs:8584:10  impl  crate::Lowerer  c8fdaf51…
```

`rdeps` example (who depends on a module, exact — won't fold in `Data.List`):

```bash
ipe-index rdeps Ipe.List --subtree   # also matches Ipe.List.*
ipe-index rdeps ipe_ir --count       # just the number
```

### Reviewing a change

The review path: start from what a diff touched, then follow each unit's blast
radius. Every command takes a symbol name, qualified name, or uid, and prints
clickable `file:line-line  kind  qualified  uid` coordinates.

```bash
ipe-index changed main..HEAD   # the units a git range touched (the review scope)
ipe-index context <unit>       # review card: location, kind/facing, purpose, blast counts
ipe-index callers <unit>       # who calls it — "what breaks if this changes?"
ipe-index callees <unit>       # what it calls — "what does this rely on?"
```

`changed` example (drives a branch/PR review — each uid pipes into `context`):

```
$ ipe-index changed HEAD~3..HEAD
ipe:src/compiler/lower/src/lower.rs:7758-7958   struct  crate::Lowerer     4a6585e9…
ipe:src/compiler/lower/src/lower.rs:19232-19472 fn      crate::lower_case  eb30de67…
```

`--repo <path>` points `changed` at the git repo to diff (default: current dir).

### Unit-level links (by uid)

```bash
ipe-index links <uid>        # outgoing links + calls of one unit
ipe-index neighbors <uid>    # links + callgraph edges around a unit, both directions
```

### Situational awareness

```bash
ipe-index wakeup     # digest: file/symbol/edge counts + role breakdown
ipe-index roles      # file counts per role (compiler-rs, runtime-rs, stdlib-ipe, …)
ipe-index pipeline   # module counts per compiler stage (parse/canon/type/build/generate)
```

### Rebuilding

```bash
ipe-index index      # full rebuild from scratch, ~1-2 s
ipe-index update     # incremental: git-diff last_sha..HEAD, re-extract only
                     #   changed files. Falls back to a full index when the DB
                     #   is absent. Zero drift vs `index`.
```

Both are wired into the `post-commit` hook, so the index stays fresh
automatically.

### The change queue (what the code-review app consumes)

Each `update` records per-unit `new`/`modified`/`deleted` events in a
`change_queue` table — the review backlog the sibling `code-review` app reads.

```bash
ipe-index pending                 # queued unit changes as JSON lines
ipe-index pending --since <sha>   # exclude rows enqueued by that update run
ipe-index pending --limit N       # cap the output
```

### Rename planning (read-only)

```bash
ipe-index rename-path <old> [--to <new>]      # every edit site for a path rename
ipe-index rename-symbol <old> [--to <new>]    # every occurrence of a symbol name
    # rename-symbol also: --preserve <regex>… (skip matches), --map k=v,… (correlated)
```

Both emit JSON-line edit sites (`{kind,path,line,col,context,replacement?}`) and
never write. `<old>` for `rename-path` is the untagged repo-relative path
(e.g. `tools/ipe-index`), matched whole-segment across any repo tag.

---

## Flags

- `--db <path>` — index location (default `.ipe-index/index.db`, gitignored).
- `--repo <tag:path>` — repeatable; override the indexed repo set. Default is
  `ipe:.`.

---

## How auto-update works

- `.git/hooks/post-commit` → `ipe-index index` (background, quiet).

Hooks are local (`.git/hooks/`, not committed). After a fresh clone, re-run
`tools/scripts/ipe-index index` once (or reinstall the hook) to seed the index. See
`PORT.md` for design/status.
