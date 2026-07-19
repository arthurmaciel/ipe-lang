# `examples/sky/` — mirrored upstream Sky examples (patch-per-example)

This directory does **not** vendor the upstream Sky example source. It holds the
declarative *patch* that turns each upstream Sky example into a buildable-on-Ipê
example, plus the mirrored+patched trees the sweep materialises at run time.

## Why a rename map, not a `*.patch` per example

The dominant transform from Sky to Ipê is purely syntactic — a module-qualifier
rewrite (`Sky.Core.*` / `Sky.Http.*` / `Sky.Ffi` / `Sky.Test` / `Std.*` →
`Ipe.*`) plus the `.sky` → `.ipe` source-extension rename and the `sky.toml`
`entry` key. A per-file unified diff would rewrite line *positions* and reject on
any shifted context line — upstream makes exactly those small edits between
releases (v0.17.9 alone re-touched 7 examples). A **declarative rename map**
([`rename-map.tsv`](rename-map.tsv)) applied by
[`../../scripts/equivalence-checks/sky-to-ipe-transform.py`](../../scripts/equivalence-checks/sky-to-ipe-transform.py)
rewrites *tokens*, so it survives those edits and stays reviewable as one small
table.

## Syntactic-only guarantee

The transform rewrites CODE only — never a string literal or a comment. Sky
example prose (`"Sky.Live Counter"`, a `"Std.Ui showcase"` label, a window
title) stays byte-identical. That is required twice over:

1. A patch may not change program behaviour (a behavioural need is a
   compiler/stdlib gap to FILE, not to paper over).
2. The Go-vs-Rust equivalence diff compares the Rust build against a Go reference
   built from the ORIGINAL Sky source, which prints those strings verbatim —
   rewriting them would manufacture a false divergence.

## How the sweep uses this dir

`scripts/equivalence-checks/examples-sweep.sh`, when run with `IPE_SWEEP_MIRROR_SKY=1`
(or unconditionally for the `sky/*` set), calls `mirror_sky_examples`
(`scripts/lib/sky_mirror.sh`) per upstream example:

1. Copy `../sky/examples/<name>` (network fallback: fetch from
   `anzellai/sky`) into `examples/sky/<name>/`.
2. Rename every `*.sky` → `*.ipe`; rewrite the `sky.toml` `entry` key.
3. Apply the rename map via the transform.
4. Hand the patched tree to the normal BUILD · RUN · EQUIVALENCE pipeline
   (`lib/examples.sh` derives shape + Go-FFI scope exactly as for the
   first-party `examples/NN-*` set).

The materialised `examples/sky/<name>/` trees are git-ignored (regenerated each
run); only the rename map, this README, and the manifest are checked in.

## Go-FFI scope

The upstream set includes Go-package examples (`02-go-stdlib`, `03-tea-external`,
`05-mux-server`, `11-fyne-stopwatch`, `13-skyshop`, `16-skychess`, `17-skymon`,
`19-skyforum`, and the FFI composites). The existing `is_out_of_scope`
Go-FFI filter (`lib/examples.sh`) excludes those from the Rust build set
unchanged — a Go-package import that resolves to neither an Ipê stdlib module
nor a local project `.ipe` file marks the example out of scope.

`13-skyshop` has a first-class Ipê-NATIVE counterpart at
`examples/13-skyshop/`: the same storefront rebuilt on the shim-free auto-FFI
(real `firestore` / `rs-firebase-admin-sdk` / `async-stripe` crates). The
Go-FFI upstream stays out of mirror scope; the counterpart is a
behaviour-level port, not a token patch, so it lives as an ordinary in-tree
example rather than a rename-map entry.
