# Constructs

One page per language construct — the syntactic forms of Ipê, such as `case`
and `do`. Each page is embedded into the compiler and resolved by
`ipe doc <construct>` (for example `ipe doc case`).

Adding a construct page requires a new `docs/constructs/<name>.md` and a
matching entry in the compiler's construct table (`src/ipe-cli/src/doc.rs`).
