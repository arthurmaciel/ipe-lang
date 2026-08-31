# bitwise-flags

A `Program`-shape tool showing `Ipe.Bitwise`: a permission set packed into the
low bits of one `Int`, combined with OR, tested with AND, and cleared with the
complement. The worked example for the [Bitwise guide](../../../../docs/guide/bitwise.md).

```
ipe build package.ipe --out out/rust
cargo run --manifest-path out/rust/Cargo.toml
```
