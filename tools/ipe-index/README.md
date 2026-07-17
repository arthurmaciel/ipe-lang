# ipe-index — manual

A fast, sqlite-backed **code-relation index** over BOTH the Ipê repo (this one)
and the Sky reference repo (`../sky`). Ask it "where is X defined", "who imports
Y", "which Ipê kernels still need a Rust impl" — one answer, instantly, instead
of grepping and guessing.

**Rule of thumb:** reach for `ipe-index` before `rg` for structural questions
(defs, imports, parity). Use `rg` only for free-text / substring hunts.

---

## Quick start

```bash
# From the Ipê repo root. The wrapper builds the Rust binary on first run.
scripts/ipe-index index          # build the index (both repos) → .ipe-index/index.db
scripts/ipe-index locate Lowerer # where is `Lowerer` defined / impl'd?
scripts/ipe-index parity --gaps  # which kernels differ between Ipê-Go and Ipê-Rust?
scripts/ipe-index wakeup         # one-screen digest of the whole index
```

`scripts/ipe-index` is a thin wrapper that execs the compiled binary at
`tools/ipe-index/target/release/ipe-index`. Anywhere the docs say `ipe-index`,
run `scripts/ipe-index` (or the binary directly).

The index auto-refreshes after **every commit** (git `post-commit` hook, both
repos) — you rarely run `index`/`update` by hand.

---

## The mental model

### Two repos, one index, tag-prefixed paths

Every path is prefixed with a repo tag so the two repos never collide and
results tell you which repo they came from:

| Tag   | Repo      | What lives there |
|-------|-----------|------------------|
| `ipe:`| `.`       | Ipê Rust compiler (`crates/`), runtime (`runtime/`), tooling (`tools/`), Ipê stdlib (`*.ipe`) |
| `sky:`| `../sky`  | Sky Haskell compiler (`src/Sky/`), Go backend+runtime (`runtime-go/`), Sky stdlib, the Rust ancestor (`runtime-rust/`) |

So a result reads `ipe:crates/sky_lower/src/lower.rs:2518` or
`sky:runtime-go/rt/rt.go:7146` — the prefix is the repo.

### Six languages, real defs

| Language          | Defs captured | Imports |
|-------------------|---------------|---------|
| Rust              | fn, struct, enum, trait, type, const, static, macro, mod, **impl targets** | `use` |
| Go                | func, method, string-registered kernels | `import` |
| TypeScript / JS   | function, arrow-const | `import` / `export … from` |
| Haskell           | signatures + `data`/`newtype`/`type`/`class` + **equation defs** (deduped) | `import` |
| Bash              | `name()` functions | `source` / `.` |
| Ipê               | bindings, `Ffi.kernel` decls | `import` |

---

## Commands

### Finding things

```bash
ipe-index locate <Name>      # every def/impl site of a symbol, both repos
ipe-index deps <module>      # what <module> imports (substring match)
ipe-index rdeps <module>     # who imports <module> (exact; --subtree, --count)
ipe-index covers <kernel>    # which examples/fixtures exercise a kernel/module
```

`locate` example:

```
$ ipe-index locate StdlibKernel
ipe:crates/sky_kernels/src/lib.rs:112:10  def
ipe:crates/sky_kernels/src/lib.rs:1174:6  impl
```

`rdeps` example (who depends on a module, exact — won't fold in `Data.List`):

```bash
ipe-index rdeps Ipe.List --subtree   # also matches Ipe.List.*
ipe-index rdeps sky_ir --count            # just the number
```

### Cross-repo kernel parity (the headline feature)

Reconciles Ipê-Go kernel impls against Ipê-Rust kernel impls in one table —
this IS the kernel-porting backlog.

```bash
ipe-index parity            # every kernel + its parity verdict
ipe-index parity --gaps     # only the real gaps
```

Reading a `--gaps` row:

```
go-only    Math.isNaN   go=1 rust=0  route=sky:src/Ipê/Generate/Go/Kernel.hs:441  go=sky:runtime-go/rt/rt.go:7146  rust=<missing>
rust-only  Basics.abs   go=0 rust=1  route=…  go=<missing>  rust=ipe:runtime/src/sky_runtime/basics.rs:120
```

| Verdict        | Meaning |
|----------------|---------|
| `go-only`      | Ipê-Go has it, Ipê-Rust does not → **a real port TODO** |
| `rust-only`    | Ipê-Rust has it, Ipê-Go doesn't |
| `orphan-route` | routed in `Kernel.hs` but neither backend implements it |
| `ok`           | both sides implement it |

### Situational awareness

```bash
ipe-index wakeup     # digest: file/symbol/edge/kernel counts, role breakdown, gap count
ipe-index roles      # file counts per role (ipe-compiler-rs, runtime-go, compiler-hs, …)
ipe-index pipeline   # module counts per compiler stage (parse/canon/type/build/generate)
```

### Rebuilding

```bash
ipe-index index      # full rebuild from scratch (both repos), ~1-2 s
ipe-index update     # incremental: git-diff last_sha..HEAD per repo, re-extract
                     #   only changed files, re-reconcile parity. Falls back to a
                     #   full index when the DB is absent. Zero drift vs `index`.
```

Both are wired into the `post-commit` hooks, so the index stays fresh
automatically.

---

## Flags

- `--db <path>` — index location (default `.ipe-index/index.db`, gitignored).
- `--repo <tag:path>` — repeatable; override the indexed repo set. Default is
  `ipe:.` + `sky:../sky`. (The `../sky` hook uses `sky:. ipe:../sky-rust`.)

---

## How auto-update works

- `.git/hooks/post-commit` (this repo) → `ipe-index index` (background, quiet).
- `../sky/.git/hooks/post-commit` → rebuilds the SAME shared index from the Sky
  side. The binary and DB live in this repo; the Ipê hook reaches back across
  the sibling path.

Hooks are local (`.git/hooks/`, not committed). After a fresh clone, re-run
`scripts/ipe-index index` once (or reinstall the hooks) to seed the index.

---

## Phase note

Until Ipê is green on all example sweeps + FFI-complete, we run BOTH `ipe-index`
and the upstream `skydex` (the Sky-Rust ancestor's own index, used as-is for
reference). After that milestone we drop `skydex` and re-point `ipe-index` at the
Sky Haskell + Go reference. See `PORT.md` for design/status.
