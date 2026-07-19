# CO-BACKEND findings

7 findings: 0 critical, 1 high, 2 medium, 4 low.

Audited: `src/compiler/backend/src/lib.rs`, `src/compiler/backend/rust/src/{project.rs, preamble.rs, crate_specs.rs, static_build.rs, emit_types.rs, rust_file.rs, naming.rs, lib.rs, emit_live.rs (targeted), emit_expr.rs (targeted: literal emission, let/lambda binders, callee naming, fallback sites, depth guard)}`, `src/compiler/ir/src/{ir.rs, pretty.rs, lib.rs}` — plus cross-checks into `src/compiler/parse/src/{parser.rs, lexer.rs}` (identifier charset) and `src/compiler/ffi/src/bindings.rs` (wrapper-region shape) to establish reachability only.

## CO-BACKEND-001 · Emitted local binders can shadow top-level fn / kernel-wrapper names — silent wrong-call or cargo fail after exit 0
- severity: high
- axis: soundness
- principle: THE SEAL — no exit-0-then-cargo-fail; "make invalid states unrepresentable" (injective emitted-name mapping)
- location: `src/compiler/backend/rust/src/lib.rs:1217` (`emit_ident` — verbatim local spelling), `src/compiler/backend/rust/src/emit_expr.rs:5953-5956` (`let {name_s} = …` emission), `src/compiler/backend/rust/src/emit_expr.rs:941-950` (`callee_name` — `Callee::Func`/`Callee::Kernel` emit BARE unqualified names)
- reachability: any user program with a local `let`/lambda binder whose spelling coincides with an emitted top-level name. Locals are emitted with their source spelling (only keyword-mangled); top-level fns emit as `<snake_module>_<snake_fn>` (`Main.update` → `main_update`) and kernel wrappers as fixed bare names (`log_println`, `string_from_int`, …), and every call site emits the bare unqualified name. The lexer permits `_` in identifiers (`lexer.rs:255`), so `main_update` / `log_println` are legal Ipê local names — and `main_update` is a *natural* local name in module `Main` holding the result of `update`.
  - `let main_update = update model msg in … update m2 x …` — the inner `update` call emits `main_update(m2, x)`; the local (an updated-model value) is not callable → E0618, cargo fails after ipe exit 0 (SEAL).
  - If the local is function-typed (a lambda emits as `Box<dyn Fn…>`, which Rust calls directly by name), the emitted call type-checks and invokes the LOCAL instead of the top-level function — a silent miscompile with no cargo failure.
  - Related non-injectivity in the same namespace: `naming.rs:223` `mangle_reserved` maps `match` → `match_`, colliding with a user ident literally spelled `match_` (both legal Ipê). Two same-scope lambda params `\match match_ ->` emit duplicate Rust params (E0415); record fields `match`/`match_` emit duplicate struct fields (E0124, via `emit_types.rs:652`); sequential lets silently rebind.
- problem: emitted locals, module-prefixed top-level fns, kernel wrappers, and keyword-mangled names all share one flat textual namespace with no injectivity guarantee and no shadow-proof qualification. The func/enum name folds have fail-closed collision gates (`lib.rs:544`, `lib.rs:590`), but those gates only cover top-level-vs-top-level — local-vs-top-level shadowing is entirely unguarded, and its worst case is not a build failure but wrong behaviour.
- fix direction: emit top-level fn references qualified (`crate::main_update(...)` — shadow-proof from every scope, matching the `Callee::Ffi` precedent) and make `mangle_reserved` injective (escape user idents landing in the mangled image, e.g. trailing-underscore doubling).
- prior: new

## CO-BACKEND-002 · `assert_mod_idents_unique` is a dead gate — colliding module idents reach emission; comment claims the gate runs
- severity: medium
- axis: soundness
- principle: THE SEAL; "invariant asserted in comments but not enforced by types" (completeness of the fail-closed gate set)
- location: `src/compiler/backend/rust/src/rust_file.rs:82-100` (gate, no production caller — module is `#![allow(dead_code)]`, rust_file.rs:9), `src/compiler/backend/rust/src/project.rs:766-787` + `project.rs:1433-1442` (split paths that should call it; the comment at `project.rs:770-772` states its uniqueness is "already guaranteed" by the gate)
- reachability: `module Std_Ui` and `module Std.Ui` in one program. The lexer allows `_` in identifier continuation (`lexer.rs:255`) and `parse_module_name` splits segments on `.` (`parser.rs:372-375`), so both are legal and distinct `ModPath`s; `mod_ident` folds both to `ipe_mod_std_ui` (join-with-`_` then snake_case, `rust_file.rs:39-44`). Two distinct homes ⇒ the ≥2 split branch runs. The func/enum collision gates do NOT catch this when the two modules declare differently-named items (`Std_Ui.foo` + `Std.Ui.bar` → `std_ui_foo` / `std_ui_bar`, no overlap).
- problem: the split branch emits two identical `#[path = "ipe_mods/ipe_mod_std_ui.rs"] mod ipe_mod_std_ui;` barrel pairs (E0428) and pushes two `src/ipe_mods/ipe_mod_std_ui.rs` entries whose second `files.insert` silently overwrites the first — one module's items vanish from the emitted tree. ipe exits 0; cargo fails (or would mis-link if the duplicate `mod` were ever tolerated). The exact gate built for this (`assert_mod_idents_unique`, with its own collision test at `rust_file.rs:244-267`) is never invoked on any production path.
- fix direction: call `assert_mod_idents_unique(&module_homes, interner)` in both `emit_program`'s split branch and `assemble_split_manifest` before emitting barrel lines.
- prior: new

## CO-BACKEND-003 · Live route `:param` decoding defaults on failure — unparseable/missing segment becomes 0 / 0.0 / "" / false
- severity: medium
- axis: correctness
- principle: "Parse, don't validate" — present-but-wrong input defaulted to a trusted value; P2 correctness
- location: `src/compiler/backend/rust/src/emit_live.rs:472-479` (`route_param_get`)
- reachability: any routed Ipe.Live app whose Page constructor carries an `Int`/`Float`/`Bool` payload (`route "/apps/:id" AppDetailPage` with `AppDetailPage : Int -> Page`). A remote client controls the URL.
- problem: the emitted conversion is `params.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or_default()` — a request for `/apps/abc` silently constructs `AppDetailPage 0` instead of routing to `notFound`. Entity id 0 may be a real row, so an attacker-controlled unparseable segment lands the user on a page they did not address; a missing param likewise becomes `""`/`0`. The failure is swallowed at the trust boundary rather than surfaced as a route miss.
- fix direction: on parse failure fall through to the `notFound` page (route non-match) instead of `unwrap_or_default()`.
- prior: new

## CO-BACKEND-004 · Raw slice `&rest[pos + MARK.len()..]` violates the workspace `indexing_slicing` deny with no per-site allow
- severity: low
- axis: soundness
- principle: Mechanical enforcement — "when a lint fires, fix the code"; clippy deny-set applies workspace-wide
- location: `src/compiler/backend/rust/src/project.rs:2162` (`reached_ffi_idents`)
- reachability: compile-time gate only — the offset is `find`-derived over an ASCII needle, so the slice cannot actually panic; but `ipe_backend_rust` has `[lints] workspace = true` and `indexing_slicing = "deny"`, so the full clippy gate (`--workspace --all-targets -D warnings`) fails on this line with no sanctioned `#[allow]`.
- problem: an un-allowed deny-set violation on the emission path; either the full gate goes red or the gate is not being run against this file — both are enforcement breaches.
- fix direction: `rest.get(pos + MARK.len()..)` with the existing `while let` reshaped around the `Option`.
- prior: new

## CO-BACKEND-005 · Defensive emitter fallbacks emit invalid Rust instead of failing closed
- severity: low
- axis: soundness
- principle: THE SEAL — "every new acceptance path fails closed at ipe time, never open at cargo time"
- location: `src/compiler/backend/rust/src/emit_expr.rs:5877-5883` (`Expr::Char` multi-char fallback emits a *string* literal in char position), `src/compiler/backend/rust/src/emit_expr.rs:6678-6682` (`emit_tuple_arm_head` — `cols.get(c)…unwrap_or(ColMode { no coercion })` on a tuple pattern wider than the column table)
- reachability: both guard *internal* invariants (single-char lexeme from the lexer; tuple arity from the lowerer) — not reachable from a well-formed pipeline today, hence low / smell.
- problem: if either invariant ever breaks, the chosen "total" fallback ships Rust that cannot type-check (E0308) — the failure surfaces at cargo, not at ipe, exactly the fail-open posture the SEAL forbids. The sibling paths in the same files use `Diagnostic::CompilerBug` for identical situations, so these two are inconsistent outliers.
- fix direction: return `Diagnostic::CompilerBug` in both fallback arms.
- prior: new

## CO-BACKEND-006 · IR pretty-printer recursion is unbounded (emitter is depth-capped; `--emit-ir` is not)
- severity: low
- axis: soundness
- principle: P3 soundness — no input-proportional native-stack recursion (same rationale as `MAX_EMIT_DEPTH`)
- location: `src/compiler/ir/src/pretty.rs:1505` ff. (`write_expr` recurses per nesting level with no depth guard; `write_pat`/type rendering likewise)
- reachability: `ipe build --emit-ir` on a deeply-nested program. The emitter caps at `MAX_EMIT_DEPTH = 96` (`emit_expr.rs:27`, typed IPE-L0200 refusal) precisely because deep IR is representable; the pretty printer walks the same tree first/independently with no equivalent bound, so a program the emitter would refuse can still overflow the compiler's stack (abort) in the dev-flag path. Smell: depends on upstream parser nesting limits I did not fully trace.
- problem: the module doc claims "pure and total: it never panics" but totality holds only up to native stack depth; the depth invariant enforced one stage later is absent here.
- fix direction: thread the same depth counter and render a `<depth limit>` placeholder past the bound.
- prior: new

## CO-BACKEND-007 · FFI shake keep-decision is last-`pub fn`-wins — latent wrapper drop if a region ever carries two fns
- severity: low
- axis: completeness
- principle: THE SEAL (latent); "fix the structure, not the symptom"
- location: `src/compiler/backend/rust/src/project.rs:2200-2207` (`shake_ffi_by_fn_ident` — `*keep = ident.is_empty() || reached.contains(&ident)` overwrites the prior decision)
- reachability: not reachable today — `ipe_ffi::bindings::emit_bindings` emits exactly one `pub fn` per BEGIN/END region (verified against `src/compiler/ffi/src/bindings.rs:1490-1502`). The invariant lives only in the generator's current output shape; the backend deliberately does not depend on `ipe_ffi` and restates the wire format, so nothing on this side enforces it.
- problem: a future generator region containing a reached `pub fn` followed by an unreached helper `pub fn` flips `keep` back to false and drops a wrapper the program calls — E0425 at cargo after ipe exit 0.
- fix direction: accumulate with `*keep = *keep || …` (a region stays kept once any of its fns is reached).
- prior: new
