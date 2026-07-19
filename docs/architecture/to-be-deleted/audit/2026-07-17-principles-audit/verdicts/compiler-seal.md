# compiler-seal verdicts

Theme: the claimed SEAL breaches across co-types / co-backend / co-front.
Each verdict was reached by tracing the reachable path in
`src/compiler/{types,lower,backend,parse,canon}` — read-only, no builds.

## CO-TYPES-001 · CONFIRMED
- final severity: high (SEAL — genuine exit-0-then-cargo-fail)
- reachability: any well-typed program whose `case` mentions a Prelude
  builtin ADT ctor other than `Just`/`Nothing`/`Ok`/`Err`, non-exhaustively
  over the nested/whole shape and with no catch-all. Author-controlled
  source; no exotic capability.
- reasoning: verified the full three-link chain.
  1. `canon::env::install_builtin_ctors` (env.rs:255-321) registers
     `ErrorKind` (11 variants), `SqlValue`/`SqlField`, `ChunkEvent`, `Error`
     — so patterns over them RESOLVE and the program type-checks.
  2. `exhaust::Sigs::build` (exhaust.rs:80-106) seeds ONLY Maybe/Result into
     `ctor_to_union`/`union_ctors`. `pattern_uses_unknown_ctor`
     (exhaust.rs:238-256) recurses into ctor args, so `Just Io` sees `Io`
     absent from `ctor_to_union` → returns true. `check_case`
     (exhaust.rs:466-471) then does an early `return Ok(())` for the WHOLE
     case — no usefulness/witness analysis runs.
  3. Lowering's `Match::new` (ir.rs:2415-2455) backstop is by its own
     documented contract TOP-constructor-only: it inserts each arm's top
     ctor into `covered` and requires `covered == expected`. For a
     `Maybe ErrorKind` scrutinee the top set is `{Just, Nothing}` = full, so
     it passes; the nested `IpeErrorKind` arm set is never checked
     ("proven UPSTREAM" — which is exactly the false premise here).
  The emitted Rust `match` is genuinely non-exhaustive on `IpeErrorKind`
  → rustc E0004 after `ipe` exit 0. THE SEAL is broken.
- repro (SEAL):
  ```elm
  module Main exposing (main)
  import Ipe.Prelude exposing (..)
  import Ipe.Log exposing (println)

  describe : Maybe ErrorKind -> String
  describe m =
      case m of
          Just Io      -> "io"
          Just Network -> "net"
          Nothing      -> "none"
          -- 9 remaining ErrorKind variants under `Just` are unmatched,
          -- no catch-all: exhaust skips the case, Match::new sees
          -- Just+Nothing (complete top set) and passes.

  main = println (describe Nothing)
  ```
  `ipe build` exits 0; `cargo build` fails E0004 (non-exhaustive
  `IpeErrorKind` match). A top-level `case kind of Io -> …; Network -> …`
  is the (b) variant — caught, but presented as `Diagnostic::CompilerBug`
  ("top constructors cover 2 of 11") instead of the user-facing IPE-T0010:
  a correctness/UX defect, not a second SEAL breach.
- dup-of: —

## CO-BACKEND-001 · CONFIRMED
- final severity: high (SEAL for the value-typed local; silent-miscompile
  for the fn-typed local — the worse of the two)
- reachability: any program with a `let`/lambda binder whose source spelling
  equals an emitted top-level name. `_` is a legal ident char, so
  `main_update`, `log_println`, `string_from_int` are all writable Ipê
  locals; `main_update` is a NATURAL local in module `Main`.
- reasoning: locals emit verbatim (only keyword-mangled) via
  `EmitCtx::emit_ident` (lib.rs:1217) → `let {name_s} = …`
  (emit_expr.rs:5953-5956). Top-level fns fold `Main.update → main_update`
  via `naming::module_value` (lib.rs:581) and every call site emits the
  BARE unqualified name — `callee_name` (emit_expr.rs:941-949) returns
  `ctx.func_name(id)` for `Callee::Func` and `kernel_name(k)` for
  `Callee::Kernel`, with NO `crate::` qualifier (contrast `Callee::Ffi`
  which DOES qualify `crate::ffi::…`). The top-level collision gate at
  lib.rs:590 only compares `func_names.values()` against each other —
  local-vs-top-level shadowing is entirely outside its view. In Rust, a
  `let` binder shadows a top-level `fn` of the same name for the rest of
  the block, so:
  - value-typed local (`let main_update = update model msg`): a later
    `update m2 x` call emits `main_update(m2, x)`; the local (a model
    value) is not callable → E0618 at cargo, ipe already exited 0. SEAL.
  - fn-typed local (a lambda → `Box<dyn Fn…>`): the emitted call
    type-checks and invokes the LOCAL — silent wrong-call, no cargo error.
    Strictly worse than a SEAL break (no failing signal at all).
- repro (SEAL, value case):
  ```elm
  module Main exposing (main)
  import Ipe.Prelude exposing (..)
  import Ipe.Log exposing (println)

  update : Int -> Int -> Int
  update a b = a + b

  main =
      let
          main_update = update 1 2      -- emits `let main_update = main_update(1,2)`?
      in                                 -- no: value binds fine; the shadow bites the NEXT call
      println (String.fromInt (shadowed main_update))

  shadowed : Int -> String
  shadowed n =
      let
          main_update = n               -- local Int shadows top-level fn `main_update`
      in
      String.fromInt (update main_update 5)
                                        -- `update` is a DIFFERENT fn, fine; but a call to
                                        -- `main_update`-named top-level fn here would emit
                                        -- `main_update(...)` binding to the Int → E0618.
  ```
  The minimal true trigger is: a local named exactly like a top-level fn
  that is CALLED within the same scope. e.g. two top-level fns `a`/`b`
  where module is `Main`, fn `Main.b` folds to a name a local also spells,
  and `b` is called after the local binding — emits `<local>(args)` on a
  non-fn value → E0618. The `mangle_reserved` sub-claim (`match`→`match_`
  colliding with a user `match_`) is a real secondary non-injectivity
  (naming.rs) but a narrower same-scope case (E0415/E0124); folded here.
- dup-of: —

## CO-BACKEND-002 · CONFIRMED
- final severity: medium (SEAL — E0428, plus a silent item-drop via
  `files.insert` overwrite; needs a two-module program, one of them a
  `_`-in-name module, so slightly narrower than 001)
- reachability: a program with two distinct modules whose homes fold to the
  same `mod_ident` AND at least two DISTINCT `IpeModule` homes overall (so
  the `module_homes.len() >= 2` split branch runs). Canonical trigger:
  `module Std.Ui` and `module Std_Ui` (both legal — `_` is an ident char,
  `parse_module_name` splits on `.`), giving distinct `ModPath`s that both
  fold to `ipe_mod_std_ui`.
- reasoning: `mod_ident` (rust_file.rs:39-44) =
  `format!("ipe_mod_{}", to_snake_case(module_prefix(home)))`.
  `module_prefix(["Std","Ui"])` = `"Std_Ui"`; `module_prefix(["Std_Ui"])`
  = `"Std_Ui"` — IDENTICAL string → identical `to_snake_case` output
  `std_ui` → identical `ipe_mod_std_ui`. The split branch
  (project.rs:817-843) iterates `module_homes`, calls `resolve_mod_ident`
  per home, and `push_str`es a `#[path=…] mod ipe_mod_std_ui;` barrel pair
  for EACH — no dedup, no uniqueness check — then pushes a
  `src/ipe_mods/ipe_mod_std_ui.rs` source per home. Two identical `mod`
  decls → rustc E0428; the second source-file entry silently overwrites the
  first module's items. `assert_mod_idents_unique` (rust_file.rs:82), the
  gate built for exactly this, is confirmed to have NO production caller:
  `rg assert_mod_idents_unique` finds only its definition, doc-references,
  and a `#[cfg(test)]` call (rust_file.rs:261/278). The comment at
  project.rs:770-772 asserts its uniqueness is "already guaranteed" — false;
  nothing calls it. The `module_homes.len()>=2` disjointness collection at
  project.rs:774-786 folds into a `BTreeSet<String>`, which SILENTLY DEDUPES
  the collision rather than catching it, so even that path does not fail
  closed. The func/enum collision gates don't help: `Std.Ui.foo` +
  `Std_Ui.bar` fold to distinct `std_ui_foo`/`std_ui_bar`.
- repro (SEAL): a two-file project with `src/Std/Ui.ipe`
  (`module Std.Ui exposing (foo)`) and a module literally declaring
  `module Std_Ui exposing (bar)`, both imported by `Main` alongside a third
  module (to guarantee ≥2 IpeModule homes). `ipe build` exits 0; `cargo
  build` fails E0428 (duplicate `mod ipe_mod_std_ui`).
- dup-of: —

## CO-FRONT-001 · CONFIRMED
- final severity: high (soundness-invariant breach the crate advertises it
  does NOT have; compiler-crash / DoS, NOT exit-0-then-cargo-fail)
- reachability: a single source expression with a long RIGHT-associative
  operator chain — `"a" ++ "a" ++ … ++ "a"`, `x :: x :: … :: []`,
  `True && True && …`, `f <| g <| …` — of a few hundred thousand
  operators. Author/generator-controlled source (a hosted playground
  compile service makes it a remote DoS surface).
- reasoning: `parse_expr` (parser.rs:989-1008) gathers the chain in a FLAT
  `while` loop into `ops: Vec` — nesting depth stays 1, so the
  `depth > MAX_DEPTH` (MAX_DEPTH=256) guard NEVER trips regardless of chain
  length. `canonicalise_binops` (resolve.rs:2946) hands the flat chain to
  `climb_binops` (resolve.rs:2983). `op_precedence` (resolve.rs:2916-2931)
  assigns `++`,`::` (prec 5), `&&` (3), `||` (2), `<|` (0) all
  `Assoc::Right`. In `climb_binops`, a right-assoc op sets
  `next_min = prec` (resolve.rs:3005-3007), so the recursive call at
  :3009 re-enters with the SAME min-prec and consumes the next equal-prec
  operator one frame deeper — native call depth == chain length N. A few
  hundred thousand operators overflows the thread stack → SIGSEGV/abort,
  not a coded diagnostic. The parser module doc's promise ("Recursion is
  bounded by MAX_DEPTH so adversarial input cannot overflow the stack") is
  bypassed because the chain is flat in the AST and the recursion happens
  in canon, one stage past the guard.
- repro (crash, not SEAL): a source file whose `main` body is
  `"a" ++ "a" ++ … ++ "a"` repeated ~300k times. `ipe build` crashes
  (stack overflow) instead of emitting a diagnostic or exiting cleanly.
  This is NOT exit-0-then-cargo-fail — `ipe` never exits 0 — so it does
  not break THE SEAL's cargo contract; it breaks the P3 no-input-stack-
  overflow invariant the crate explicitly advertises. Kept HIGH on that
  basis; downgraded from "SEAL" framing to "advertised-invariant breach /
  DoS".
- dup-of: —

## Push-blocking assessment

- **CO-TYPES-001 — PUSH-BLOCKING.** A well-typed, non-exotic program
  (`case` over `Maybe ErrorKind`/`SqlValue`/`ChunkEvent`) reaches cargo as
  E0004. This is the canonical SEAL violation the audit exists to catch and
  is reachable from ordinary author code (the documented `forEachChunk`
  `ChunkEvent` handler shape is squarely in the blast radius).
- **CO-BACKEND-001 — PUSH-BLOCKING.** The fn-typed-local case is a SILENT
  miscompile (no cargo error), which is strictly worse than a SEAL break;
  the value-typed case is a plain SEAL break. Both from natural source
  (`let main_update = …` in `Main`).
- **CO-BACKEND-002 — should-fix, borderline push-blocking.** Real SEAL
  break with a dead gate whose comment lies about being enforced, but the
  trigger (two modules colliding under the `.`-vs-`_` fold) is narrower and
  unlikely in hand-written code. The dead gate + false comment is itself a
  principles violation regardless of trigger likelihood.
- **CO-FRONT-001 — should-fix, not a cargo-contract SEAL break.** Genuine
  advertised-invariant breach and DoS surface, but `ipe` crashes rather
  than exiting 0, so it does not ship broken Rust. High severity, not
  push-blocking on the SEAL axis specifically.

Confirmed: 4 (0 crit / 3 high / 1 med / 0 low) · Refuted: 0 · Downgraded: 0 (CO-FRONT-001 re-FRAMED from SEAL to advertised-invariant/DoS, severity kept high) · Dup: 0
