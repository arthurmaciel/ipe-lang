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

## The mental model

### One repo, tag-prefixed paths

Every path is prefixed with a repo tag so results tell you where they came from
and so a future multi-repo setup never collides:

| Tag   | Repo | What lives there |
|-------|------|------------------|
| `ipe:`| `.`  | Rust compiler (`crates/`), runtime (`runtime/`), tooling (`tools/`), stdlib/examples (`*.ipe`) |

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

`locate` example:

```
$ ipe-index locate Lowerer
ipe:crates/ipe_lower/src/lib.rs:112:10  def
ipe:crates/ipe_lower/src/lib.rs:1174:6  impl
```

`rdeps` example (who depends on a module, exact — won't fold in `Data.List`):

```bash
ipe-index rdeps Ipe.List --subtree   # also matches Ipe.List.*
ipe-index rdeps ipe_ir --count       # just the number
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
