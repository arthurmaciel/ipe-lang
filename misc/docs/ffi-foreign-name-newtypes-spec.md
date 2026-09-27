# Spec — FFI foreign names and requirements parse into cargo/rustc-valid newtypes

Crates: `ipe_ffi` (`driver.rs`, `interface.rs`, `bindings.rs`, `naming.rs`,
`emit.rs`, `transparency.rs`, `pkginfo`), `ipe_canon` (`asserted.rs`),
`ipe_backend_rust` (`lib.rs`), `ipe-cli` (`ffi.rs`). Principles: SEAL,
parse-don't-validate, make invalid states unrepresentable, SSOT. Lane tier:
high (emit / SEAL).

## Class-closing property

Every string that crosses from a foreign crate (or a cache that mirrors one)
into emitted Rust or an emitted `Cargo.toml` is parsed ONCE into a newtype
whose constructor proves it is legal in its destination position:
- `RustModIdent` (ASCII, keyword-free, non-leading-digit, not `_`) for a
  module slug;
- `CargoPackageName` (normalised `-`/`_` form, never `_`, never a
  truncated keyword, never a reserved bin dir) for a package name;
- `CrateRequirement` (the cargo semver-requirement grammar) for a version
  requirement;
- `RustIdent` via the lexer's ASCII `is_ident_start`/`is_ident_continue` for
  every member-name classification;
- typed error enums (`AssertedPathError`, `TransparencyError`) instead of
  `Result<_, String>`.

A collision between two parsed names is a typed refusal at ipe time.

## Why the instances exist (origin/main 983fdc404)

- `src/compiler/ffi/src/driver.rs:393` `slugify`: maps non-alphanumerics to
  `_`, lowercases; no keyword/leading-digit guard, no collision check. The
  slug lands in `ffi.rs:238` `pub mod {slug} { … } pub use {slug}::*;` →
  cargo error on `type`, `2d`, or two crates slugging alike (#2980).
- `src/compiler/backend/rust/src/lib.rs:289` `sanitize_cargo_name`: guards
  `["ipe","union"]` and `["build","deps","examples","incremental"]`; input
  `_` passes (#2981).
- `interface.rs:184`, `:256`, `:410`; `bindings.rs:55`, `:1487`;
  `emit.rs:264`; `naming.rs:351`/`:363`: Unicode `char::is_alphanumeric`
  where the lexer is ASCII (#2995).
- `src/compiler/ffi/src/pkginfo/mod.rs:419` `CrateVersion(String)` with a
  charset gate (`version_char_is_legal`) — `==1`, `=*` pass, cargo rejects
  (#3014).
- `direct_crate_names` compares `slug.replace('_','-')` against raw
  `PackageName` keys (`ffi.rs:141` lane/2944, `:163` lane/2959) — two spellings
  of one fact (#3013).
- `canon/src/asserted.rs:61`, `:124`; `ffi/src/transparency.rs:157`, `:248`,
  `:376`: `Result<_, String>` (#2982).
- Crate-ref seal is a denylist; string attributes and macro input are not
  scanned (#3011).

Fix commits for #2980 (492f81c2e) and #2981 (b0b8fb1b8) exist on
`origin/lane/2980-cargo-names` (52 behind main, no PR). They conflict with
main now; re-apply, do not rewrite.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2980 slugify keyword/digit/collision | real; fix stranded on lane/2980-cargo-names | `driver.rs:393` |
| #2981 `sanitize_cargo_name("_")` | real; fix stranded on lane/2980-cargo-names | `lib.rs:289` |
| #2995 Unicode `is_alphanumeric` + `foreign` e2e | real | `git grep is_alphanumeric` |
| #3013 `direct_crate_names` `_` normalisation | real in in-flight lanes' code | lane/2944 `ffi.rs:141` |
| #3014 `CrateVersion` admits cargo-invalid requirement | real | `pkginfo/mod.rs:419` |
| #3011 crate-ref allowlist, string attrs | real (unreachable today per issue) | issue review of #2956 |
| #2982 typed `AssertedPathError` / `TransparencyError` | real | `asserted.rs:61` |
| #2951 items 1, 5 (typed catalog-conflict refusals Refuse in LSP) | real | `ffi_define_opaque_collision` / `ffi_dependency_pin_conflict` built as untyped messages |

Related in-flight: #2956 (transitive dep refusal), #2991.

## Implementation plan

1. Cherry-pick 492f81c2e + b0b8fb1b8 onto main; resolve against the keyword
   SSOT that landed in #2972/#2975 (`ipe_intern` reserved-keyword list) —
   the slug guard must consume that list, not a copy.
2. `RustModIdent::parse(&str) -> Result<_, FfiNameError>`; `slugify` returns
   it. Collision: build a `BTreeMap<RustModIdent, CrateName>` over the catalog;
   a second insert → `FfiNameError::SlugCollision { a, b }`.
3. `CargoPackageName` owns the `-`/`_` normalisation; `direct_crate_names`
   and the dependency keys both use it (#3013); `sanitize_cargo_name` returns
   it.
4. `CrateRequirement::parse` over the cargo grammar (comparators `=,>,>=,<,<=,~,^`
   + bare/wildcard version, comma-separated). Replace `CrateVersion`'s charset
   gate; keep empty = "unresolved probe" as a separate enum variant, not an
   empty string.
5. Route every `is_alphanumeric` in `ipe_ffi` through
   `ipe_parse::is_ident_start/is_ident_continue` (#2995).
6. `AssertedPathError { Empty, Whitespace, NoSeparator, EmptySegment,
   BadCharset, Wildcard, Keyword }`, `TransparencyError`; catalog text is
   rendered only at `NameError::AssertedCallMalformed`.
7. Catalog-conflict refusals become typed `FfiPrepError` variants the LSP
   classifies Refuse (#2951.1/.5).
8. Crate-ref seal (#3011): invert to an allowlist — a path head must be a
   declared dependency ident or `std|core|alloc|crate|self|super|ipe_runtime`;
   scan string-literal attribute values that parse as paths.

## Prove the refusals

Table tests, one row per illegal input, each asserting the typed variant:
- slug of crates `type`, `2d`, `a-b` + `a_b` (collision), `_`;
- member name `é` (non-ASCII), member `foreign` (with `IPE_E2E=1`);
- requirements `==1`, `=*`, `>=`, `1..2`, `^`, `"1.0" ` (quote);
- underscore crate in a version conflict → refused, not unpinned;
- `#[serde(with = "undeclared::x")]` → refused;
- every `AssertedPathError` variant.

Lane tier: high. Guardian: required.
