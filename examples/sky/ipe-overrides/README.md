# `examples/sky/ipe-overrides/`

Per-example **whole-port override trees**. When `ipe-overrides/<name>/` exists,
`scripts/lib/mirror.sh` uses it as the `examples/sky/ipe/<name>/` port verbatim —
the `rename-map.tsv` token rewrite and any `ipe-edits/<name>.edits` are skipped
entirely for that example.

An override lives here when the Ipê port is a **structural rebuild** of the
upstream example rather than a transform of it — the two share too little for a
per-line edit to express the delta, and files are added or removed. The raw
upstream stays in `examples/sky/original/<name>/` as the reference the rebuild
diverges from; diff against it on demand:

```sh
diff -ru examples/sky/original/<name> examples/sky/ipe-overrides/<name>
```

## Current overrides

| name | why it is a rebuild, not a transform |
| --- | --- |
| `13-skyshop` | upstream is a Go-FFI storefront (`Net.Http`, `Github.Com.…`); the Ipê port is a from-scratch rebuild on the shim-free Rust-crate FFI (`firestore` / `rs-firebase-admin-sdk` / `async-stripe`) with a different module layout. Not buildable in the per-commit sweep (needs `ipe install` to generate the `Rust.*` bindings); `manifest.toml` keeps the upstream classified `go_ffi = true`. |
