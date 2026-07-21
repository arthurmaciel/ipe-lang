# `examples/sky/` — upstream Sky examples, mirrored + patched for Ipê

This directory does **not** vendor the upstream Sky example source. It holds the
tracked **control surface** that turns each upstream Sky example into a
buildable-and-runnable Ipê example. The mirror trees themselves are transient:
`scripts/examples-sweep.sh` re-materialises `examples/sky/<name>/` from upstream
on every run, and they are git-ignored so they can never drift from upstream.

## Tracked control surface

| file | purpose |
| --- | --- |
| `manifest.toml` | classifies which upstream examples are in scope + Go-FFI |
| `rename-map.tsv` | the shared Sky→Ipê token rewrite (base patch, all examples) |
| `ipe-patches/<name>.patch` | OPTIONAL per-example semantic delta (absent = none) |
| `README.md` | this file |
| `BLOCKERS.md` | honest ledger of defects the mirror + sweep surfaced |

Everything else under `examples/sky/` — the mirrored `<name>/` trees — is
git-ignored and regenerated each sweep.

## The patch mechanism — two ordered steps

Each example is transformed from Sky to Ipê by `scripts/lib/mirror.sh` in two
ordered steps:

1. **`rename-map.tsv`** (via
   [`../../scripts/lib/sky-to-ipe-transform.py`](../../scripts/lib/sky-to-ipe-transform.py))
   — the shared, drift-resistant token rewrite: `Sky.Core.*` / `Sky.Http.*` /
   `Sky.Ffi` / `Sky.Test` / `Std.*` → `Ipe.*`, plus the `.sky` → `.ipe`
   source-extension rename and the `sky.toml` `entry` key. This is the *base*
   patch, applied to every example.
2. **`ipe-patches/<name>.patch`** — an OPTIONAL per-example unified diff applied
   on top, for a semantic delta the token rewrite cannot express. It is **absent**
   for any example whose transform is purely syntactic (the common case — every
   example today).

### Why a token rewrite for the base, not a full `*.patch` per example

The dominant Sky→Ipê difference is purely syntactic — a module-qualifier
rewrite. A per-file unified diff would rewrite line *positions* and reject on any
shifted context line, and upstream makes exactly those small edits between
releases. A declarative token rewrite (`rename-map.tsv`) survives those edits and
stays reviewable as one small table. The `ipe-patches/<name>.patch` slot exists
for the rare case where a real, byte-level semantic delta is unavoidable; a patch
that fails to apply is surfaced as a RED sweep row, never silently ignored.

### Syntactic-only guarantee for the base rewrite

The token transform rewrites CODE only — never a string literal or a comment.
Sky example prose (`"Sky.Live Counter"`, a `"Std.Ui showcase"` label, a window
title) stays byte-identical. A behavioural NEED that the transform cannot express
is a compiler/stdlib gap to FILE in `BLOCKERS.md`, never a hack: a patch may not
paper over a real gap.

## How the sweep uses this dir

`scripts/examples-sweep.sh` calls `mirror_sky_examples` (`scripts/lib/mirror.sh`)
per in-scope example:

1. Fetch the current upstream example from `anzellai/sky` into
   `examples/sky/<name>/`. A local `../sky` sibling checkout is used only as an
   offline fallback — the network fetch comes first, so each refresh tracks the
   live upstream.
2. Rename every `*.sky` → `*.ipe`; rewrite the `sky.toml` `entry` key.
3. Apply `rename-map.tsv` (the token rewrite), then `ipe-patches/<name>.patch`
   if present.
4. Hand the patched tree to the BUILD (`ipe build` + `cargo build`) + RUN
   pipeline. `lib/examples.sh` derives the shape + Go-FFI scope exactly as for
   the first-party `examples/NN-*` set.

There is **no** Go build and **no** cross-compiler comparison: the sweep proves
that OUR compiler builds and runs the real (patched) upstream examples.

## Go-FFI scope

The upstream set includes Go-package examples (`02-go-stdlib`, `03-tea-external`,
`05-mux-server`, `08-notes-app`, `11-fyne-stopwatch`, `13-skyshop`, `16-skychess`,
`17-skymon`, `18-job-queue`, `19-skyforum`, and the FFI composites). The
`is_out_of_scope` Go-FFI filter (`lib/examples.sh`) excludes those from the Rust
build set — a Go-package import that resolves to neither an Ipê stdlib module nor
a local project `.ipe` file marks the example out of scope. They are listed in
`manifest.toml` with `go_ffi = true` so the sweep knows they exist and does not
FAIL LOUD for them, but never tries to build them.

`13-skyshop` has a first-class Ipê-NATIVE counterpart at `examples/13-skyshop/`:
the same storefront rebuilt on the shim-free auto-FFI (real `firestore` /
`rs-firebase-admin-sdk` / `async-stripe` crates). The Go-FFI upstream stays out
of mirror build scope; the counterpart is a behaviour-level port, not a token
patch, so it lives as an ordinary in-tree example.
