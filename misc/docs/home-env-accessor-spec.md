# Spec — one typed accessor for HOME and path-valued environment variables

Crates: `ipe_sandbox` (`home.rs`), `ipe-cli` (`env_dir.rs`), runtime
(`system.rs`). Principles: Security (1), parse-don't-validate, SSOT. Lane tier:
high (trust boundary).

## Class-closing property

A path-valued environment variable (`HOME`, `XDG_*`, `IPE_*_DIR`) is read in
exactly one place per variable and parsed there into a typed absolute path
(`HomeDir(AbsPath)`), or refused with a typed error: unset, empty, relative,
containing NUL, or non-UTF-8 → refused. No other site calls `std::env::var` on
it, and the variable name is one `pub const`.

## Why the instances exist (origin/main 983fdc404)

- `src/ipe-cli/src/env_dir.rs:32`/`:35`: a private `HOME_VAR` and its own
  parse; the runtime and the sandbox read `HOME` independently.
- `ipe_sandbox::home` with `pub const HOME_VAR` exists only on in-flight
  `lane/2993-2940-paths` (#2940 / #2993); main has no shared accessor.
- The runtime `home_dir` lookup is not at the `system.rs:62-71` anchor cited by
  #3005; it has moved. Re-anchor after #2940 lands.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #3005 one HOME accessor across cli / sandbox / runtime | real on main | `env_dir.rs:32` private const |
| #3006 (HOME parser part: relative / empty HOME accepted) | real | `env_dir.rs:35` |

Related in-flight (do not duplicate): #2940, #2993 (`lane/2993-2940-paths`).
This spec starts after that lane lands.

## Implementation plan

1. After lane/2993 lands, `ipe_sandbox::home::HomeDir::from_env() ->
   Result<HomeDir, HomeError>` is the only reader; `HomeError { Unset, Empty,
   Relative, NotUtf8 }`.
2. `env_dir.rs` deletes its private `HOME_VAR` and consumes `HomeDir`.
3. The runtime cannot depend on `ipe_sandbox`: mirror the const and the parse
   in the runtime and assert equality (`const` on the name; shared table test
   on the refusals) — never hand-sync.
4. `rg 'env::var\("HOME"' src` returns only the accessor (CI grep gate in the
   existing panic-scan/grep lane, or a unit test that scans the tree).

## Prove the refusals

- `HOME=` (empty), `HOME=relative/dir`, `HOME` unset, non-UTF-8 bytes → each
  typed variant, in cli and runtime;
- cache/config dir derived from a refused HOME → the command fails closed with
  the typed error, never falls back to `.` or `/`.

Lane tier: high. Guardian: required.
