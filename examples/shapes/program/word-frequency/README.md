# word-frequency

A `Program`-shape batch tool: turn a paragraph into its three most common words.
It is one `List` pipeline end to end — split, normalise, tally, rank, take — where
every step returns a new list rather than mutating the last. The worked example
for the [`Ipe.List` guide](../../../../docs/modules/Ipe.List.md).

```
ipe build package.ipe --out out/rust
cargo run --manifest-path out/rust/Cargo.toml
```
