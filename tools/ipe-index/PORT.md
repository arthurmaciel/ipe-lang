# ipe-index — Rust code-relation index

**Status: SHIPPED.** `tools/scripts/ipe-index` is a thin wrapper execing the Rust
binary at `tools/ipe-index/target/release/ipe-index`.

## What it is

A single-repo, sqlite-backed code-relation index. Agents query it instead of
`rg` for "where is X defined / who imports Y / which examples exercise a
module".

### Single-repo, path-prefixed

Every file/symbol path is prefixed with a repo tag. A single repo is indexed
today, but the tag keeps results self-describing and leaves room for a future
multi-repo setup without a schema change:

| Tag  | Repo            | Roles indexed |
|------|-----------------|---------------|
| `ipe`| `.` (this repo) | `compiler-rs` (crates/), `runtime-rs` (runtime/), `tool-rs` (tools/), `stdlib-ipe` (*.ipe), `example`, `fixture`, `console-ts`, `script-sh` |

### Languages

- **tree-sitter defs+imports:** Rust (fn/struct/enum/trait/type/const/static/macro/mod
  + impl targets), TypeScript/JS.
- **line-scan defs+imports:** Bash (`name()` funcs + `source`/`.`).
- **custom scan:** Ipê (bindings + imports).

## Commands

```
ipe-index index                 # rebuild → .ipe-index/index.db
ipe-index update                # incremental: git-diff last_sha..HEAD
ipe-index locate <name>         # every def site (file:line:col)
ipe-index roles|pipeline|wakeup
ipe-index deps <m> | rdeps <m> | covers <m>
```

Default repos: `ipe:.`. Override with repeatable `--repo tag:path`. DB defaults
to `.ipe-index/index.db` (gitignored).

## Auto-update on every commit

- `.git/hooks/post-commit`: `ipe-index index` (bg, quiet, no-build).

Hooks are local (`.git/hooks`, not committed). Re-install after a fresh clone.

## Notes

- **Incremental `update`** — per-repo `last_sha:<tag>..HEAD` git diff, re-extract
  only changed files. Falls back to a full `index` when the DB is absent or a
  repo has no recorded sha. Zero drift vs a fresh full index.
- **Rust `impl`-target capture** — `impl Foo` / `impl Trait for Foo` (incl.
  `impl Vec<T>`) stored as kind `impl` so `locate Foo` surfaces impl sites.

Usage manual: `README.md`.
