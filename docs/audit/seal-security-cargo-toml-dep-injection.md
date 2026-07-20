# Cargo.toml dependency-line injection via ungated feature/name splice

## The flaw

`render_dep_line` (ffi/src/driver.rs) interpolates a dependency's feature
strings and package name verbatim into TOML lines of the generated
`Cargo.toml`:

- `features = [ "<f>" ]` — each feature rendered as `format!("\"{f}\"")`.
- `<name> = "=<version>"` — the package name as the dependency key.

The `version` splice is already closed: it flows through `CrateVersion`, a
decode-boundary newtype whose charset gate rejects quote/bracket/brace/newline.
But two sibling values reach the SAME splice ungated:

- `PkgInfo.features` was stored as raw `Vec<String>` (`features: w.features`).
- `TransitiveDep.name` was stored as raw `String` (`name: dep.name`); only
  `TransitiveDep.ident` went through `RustIdent`.

A feature or package-name string carrying `"`, `]`, `}` and a newline breaks
out of the array / inline table and injects arbitrary manifest content
(`[dependencies.evil]` with `path =`/`git =` and a `build.rs`) that runs at the
user's next `cargo build`. The module even DEFINED a `FeatureName` gate for
this position, but it had zero production callers.

## The trigger

Craft or tamper a `<slug>.pkg.json` whose `features` (or a transitive
`name`) carries a TOML-breakout payload, e.g.
`"features": ["std\"]}\n[dependencies.evil]\npath = \"/tmp/evil\nx = [\""]`.
It decoded cleanly (no gate), `cargo_dep_lines` emitted the line, and
`ffi_cargo_toml` appended it verbatim. The downstream duplicate-key filter
(split on first `=`) does not neutralize an injected `[dependencies.evil]`
table.

## The fix

Parse-don't-validate at the decode boundary — make the injection-bearing value
unrepresentable past decode:

- `FeatureName` moved to `pkginfo.rs` as a decode-boundary newtype with a
  `WireDefect::InvalidFeature` charset gate; `PkgInfo.features` is now
  `Vec<FeatureName>`, gated in `TryFrom<WirePkgInfo>`.
- `TransitiveDep.name` is now `PackageName` (the existing `[A-Za-z0-9_-]+`
  gate the primary crate name already used), gated at decode.
- `render_dep_line` takes typed `&[FeatureName]` and `&str` (from the gated
  name), so a raw string cannot reach the splice — the type, not a runtime
  escape, closes the class.

A malformed feature or name now fails the WHOLE package decode loudly, exactly
like the version case, rather than passing through to the manifest emitter.
