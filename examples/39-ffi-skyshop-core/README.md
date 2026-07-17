# 39-ffi-skyshop-core — the shim-free Rust-crate FFI acceptance example

The `13-skyshop` domain core with its `["go.dependencies"]` converted to
Rust crates through the automatic FFI subsystem — **no hand-written shims**:

| skyshop Go dependency | here |
|---|---|
| `github.com/google/uuid` | crates.io **`uuid`**, auto-bound (`Rust.Uuid`) |
| `cloud.google.com/go/firestore`, `firebase.google.com/go/v4` | `Ipe.Db` (SQLite) — async Rust SDKs sit outside the sync FFI ladder (M-G bridge) |
| `encoding/*`, `strings`, `strconv`, `fmt`, `os`, `io`, `context`, `net/http` | Ipê stdlib |
| `github.com/stripe/stripe-go/v84` | not converted — async SDK, same M-G boundary |

The flow: seed a SQLite catalog → place an order whose **order ID is minted
by the real `uuid` crate** (`Uuid.new_v4_from_uuid () |> to_string`) →
verify persistence → print `[SKYSHOP-CORE OK]`.

## Run

```bash
cd examples/39-ffi-skyshop-core
ipe run src/Main.ipe
```

The `.ipe/cache/ffi/rust/` artifacts are checked in so the build is
network-free and reproducible. Regenerate them with:

```bash
ipe add uuid --features v4,v7 --allow-build-scripts --yes
```

(`ipe add` runs the `ipe-ffi-inspector` inside the bubblewrap jail; the
crate's build scripts execute confined.)
