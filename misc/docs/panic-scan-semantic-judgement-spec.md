# Spec — panic-scan judges by resolved paths and the cargo target set, not spellings

Crate: `tools/panic-scan` (`main.rs`, `lib.rs`, `test_path.rs`). Principles:
Soundness (3), defend in depth, prove the refusals. Lane tier: standard.

## Class-closing property

panic-scan's inventory is the set of files cargo compiles (from `cargo metadata`
targets + followed `mod`/`include!`), and a banned call is recognised by its
resolved path (after `use` aliases), never by literal spelling. A file or call
the scanner cannot classify is a typed `Unauditable` finding (fail closed),
never a skip.

## Why the instances exist (origin/main 983fdc404)

- `main.rs:493` `walk` crawls `src/` and already records unreadable dirs as
  `Unauditable::Io` (#2960 item 1 largely fixed by #2996). Still missed:
  autobins (`src/bin/*.rs` outside the crawl root rules), `include!` of a
  renamed file, workspace members whose sources live outside `src/`.
- `lib.rs:814` `visit_path` matches literal `process` + `exit`/`abort`;
  `use std::process as p; p::exit(1)` passes (#2992).
- `test_path.rs:65` `is_template_path` exempts by path shape.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2960 inventory gaps (autobin, renamed include!, out-of-src) | item 1 FIXED (#2996, `main.rs:493`); rest real | code read |
| #2992 module-alias bypass of banned calls | real | `lib.rs:814` |

Standalone (not this class): #2977 — sandbox `unsafe` (`build_jail.rs:344`,
`:1903`; `lib.rs:469`, `:524`; `run_jail/linux.rs:166`) versus AGENTS.md's
"exactly ONE sanctioned block". LIMIT: rewrite on `rustix` safe wrappers, or
sanction the sandbox blocks in PRINCIPLES with a ledger. Maintainer decision.

## Implementation plan

1. Inventory from `cargo metadata --no-deps` target `src_path`s, then follow
   `mod` and `include!` (literal-arg only; non-literal → `Unauditable`).
2. Per-file alias table from `use` items (incl. `as` renames and glob of
   `std::process`); `visit_path` resolves the head through it before matching.
3. Test/template exemptions keyed by cargo target kind (`test`, `bench`), not
   path shape.

## Prove the refusals

- fixture crate with `src/bin/x.rs` calling `.unwrap()` → flagged;
- `include!("renamed.rs")` carrying a panic → flagged;
- `use std::process as p; p::exit(1)` and `use std::process::exit as bye` →
  flagged;
- `include!(concat!(...))` → `Unauditable`.

Lane tier: standard.
