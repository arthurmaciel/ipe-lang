# ipe-index — Rust code-relation index (ported from skydex)

**Status: SHIPPED (Rust port complete).** The Python v0 is retired —
`scripts/ipe-index` is now a thin wrapper execing the Rust binary at
`tools/ipe-index/target/release/ipe-index`.

## What it is

A two-repo, six-language, sqlite-backed code-relation index. Adapted from
`../sky/tools/skydex` (tree-sitter walk/store/parity/query, ~1600 LOC) for the
Ipê two-repo layout. Agents query it instead of `rg` for "where is X defined /
who imports Y / which Sky kernels still need a Rust impl".

### Two-repo, path-prefixed

Every file/symbol path is prefixed with a repo tag so the two repos never
collide (both have `Cargo.toml`, `README.md`, `scripts/*`, `tools/*`) and parity
can compare across them:

| Tag  | Repo            | Roles indexed |
|------|-----------------|---------------|
| `ipe`| `.` (this repo) | `ipe-compiler-rs` (crates/), `ipe-runtime-rs` (runtime/), `ipe-tool-rs` (tools/), `ipe-stdlib-sky` (*.sky) |
| `sky`| `../sky`        | `compiler-hs` (src/Sky/), `runtime-go` (runtime-go/), `runtime-rust` (ancestor), `stdlib-sky`, `console-ts`, `example`, `fixture` |

### Six languages

- **tree-sitter defs+imports:** Rust (fn/struct/enum/trait/type/const/static/macro/mod),
  Go (func/method + string-registered kernels), TypeScript/JS.
- **line-scan defs+imports:** Haskell (`name ::` sigs + data/newtype/type/class +
  `import`), Bash (`name()` funcs + `source`/`.`).
- **custom scan:** Sky (bindings, imports, `Ffi.kernel` decls).

### Cross-repo kernel parity (the headline feature)

`ipe-index parity --gaps` reconciles Sky-Go kernel impls (`../sky/runtime-go`)
against Ipê-Rust kernel impls (`crates/`, `runtime/`) in one table, keyed by the
Sky `Ffi.kernel` decl set + `Kernel.hs` routes. Classifies each kernel
`go-only` (real Rust gap) / `rust-only` / `orphan-route` / `ok` — directly feeds
the kernel-porting backlog. Every row carries `route=` (Haskell), `go=`, `rust=`
source locations, tag-prefixed.

## Commands

```
ipe-index index                 # rebuild across both repos → .ipe-index/index.db
ipe-index update                # full reindex (alias of index for v1)
ipe-index locate <name>         # every def site (file:line:col), both repos
ipe-index parity [--gaps]       # cross-repo kernel parity
ipe-index roles|pipeline|wakeup
ipe-index deps <m> | rdeps <m> | covers <k>
```

Default repos: `ipe:.` + `sky:../sky`. Override with repeatable
`--repo tag:path`. DB defaults to `.ipe-index/index.db` (gitignored).

## Auto-update on every commit (both repos)

- `.git/hooks/post-commit` (this repo): `ipe-index index` (bg, quiet, no-build).
- `../sky/.git/hooks/post-commit`: rebuilds the SAME shared index from the Sky
  side (`--repo sky:. --repo ipe:../sky-rust`, DB in the Ipê repo). Binary + DB
  live here; the Sky hook points back across the sibling path.

Hooks are local (`.git/hooks`, not committed). Re-install after a fresh clone.

## Phase gate

Until Ipê goes green + FFI-complete: run BOTH `skydex` (Sky-Rust ancestor
reference, used as-is) and `ipe-index`. After: drop `skydex`; `ipe-index` only,
re-pointed at Sky Haskell+Go as the reference. See memory
`sky-rust-is-ipe-ancestor-not-upstream`.

## Deferred (v2, off critical path)

- Per-repo incremental `update` from git diff (`walk::changed` +
  `reconcile_from_store` scaffolding retained, `#[allow(dead_code)]`). v1 does a
  full reindex — bounded ~1-2 s.
- Rust `impl` block target capture; Haskell equation-def lines (only sigs indexed).
