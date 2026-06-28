# Diagnostics Hardening + Error-Code System: Design

**Date:** 2026-06-27
**Status:** Approved by directive; design authored by `security-soundness-guardian` (design authority)
**Depends on:** Milestone 0 spine (complete). Governed by `PRINCIPLES.md` (Security > Correctness > Soundness > Efficiency > Completeness > Readability) and the parse-don't-validate law.

## Goal

Ill-formed Sky source must **fail fast** with **precise, informative, helpful** messages — rustc/Elm grade. Every diagnostic carries a **stable error code** (`SKY-XNNNN`), renders with a source-span caret + a structured help line, and points at `skyc explain <CODE>`. Each code has an embedded **explain page** (explanation + ≥3 `sky` snippets). All compiler code is hardened to the 6 principles and passes guardian scrutiny.

## Core principles for this work

- **Parse-don't-validate:** errors fire at the stage boundary where looser input becomes a stricter type; downstream never re-validates. Diagnostics carry **owned, structured payloads** (already-resolved data), never a stringly-typed channel. `CompilerBug.detail` is the *only* free-form String.
- **Additive:** the frozen `Diagnostic`/`Span`/`Located`/`DResult` shapes are extended additively; existing call sites keep compiling.
- **Totality:** `code()`, `severity()`, `primary_span()`, `help()` are total (explicit arms, no `_ =>`), so a new variant fails to compile until it gets a code + page.
- **Determinism:** did-you-mean ranked by stable `(Levenshtein, name)` sort; no observable HashMap iteration in rendered text.
- **No panic / no raw slicing:** byte→line/col is checked/clamped; DUMMY/OOB spans render gracefully.

## Error-code taxonomy (authoritative)

| Code | Phase | Title | When |
|---|---|---|---|
| SKY-P0001 | Parse | unexpected token | Lexer/parser hits a token the grammar disallows at this position. Payload carries the found TokenKind + an ExpectedSet. Covers the generic 'expected X, found Y' family (expect Module/Exposing/'('/')'/'='/'of'/'->' etc. all funnel here with a precise expected set). |
| SKY-P0002 | Parse | unexpected end of file | Input ends while a token is still required (truncated module/incomplete construct). Distinguished from SKY-P0001 by a Construct payload naming the enclosing construct (module header, case branch, type, paren group). |
| SKY-P0003 | Parse | input nests too deeply | Recursion-depth guard trips (limit 256) on pathological nesting of case/parens/types/exprs. Deliberate fail-fast against stack-exhausting adversarial source; payload carries construct + limit. |
| SKY-P0010 | Parse | unknown character | A byte that is not a recognised M0 symbol/ident-start/digit (e.g. '@','#',';','"','{'). Payload carries the char. |
| SKY-P0011 | Parse | stray '.' | A lone '.' not part of '..' and not attached to an identifier; help suggests '..' or 'Module.name'. |
| SKY-P0012 | Parse | number joined to a name | A digit immediately followed by an identifier character (e.g. 123abc); help: separate with a space. Payload carries the offending char. |
| SKY-P0013 | Parse | integer literal out of range | Integer literal does not fit in i64 (max 9223372036854775807). Span must cover the whole literal. |
| SKY-P0020 | Parse | malformed module header | File does not begin with 'module', module name missing/non-identifier, or 'exposing' missing. Help shows 'module Main exposing (main)'. |
| SKY-P0021 | Parse | malformed exposing list | Missing '(', bad item separator (not ',' or ')'), non-identifier exposed name, or malformed Type(..) constructor list. |
| SKY-P0030 | Parse | missing '=' in definition | A value binding's patterns are not followed by '=' before its body. Payload carries the binding name. |
| SKY-P0031 | Parse | malformed type declaration | After 'type': missing type name, missing '=' before constructors, or a non-identifier where a constructor name is required (after '=' or '|'). Constructors must be uppercase. |
| SKY-P0040 | Parse | only a type constructor can take arguments | Type arguments applied to a type variable or function type ('a Int', '(A -> B) C'). |
| SKY-P0041 | Parse | expected a type | A token that cannot begin a type (after ':', inside parens). Help: a name, a type variable, or a parenthesised type. |
| SKY-P0050 | Parse | unclosed delimiter | A '(' opened a type/expression/pattern that never closes with ')'. Secondary span points at the opener. |
| SKY-P0060 | Parse | malformed case expression | 'of' missing after scrutinee, missing '->' in a branch, zero branches, or first branch not indented past 'case'. CaseDefect payload selects the exact wording. |
| SKY-N0001 | Name | cannot find this value in scope | A bare value name resolves to no constructor/local/top-level/kernel. Payload carries the name + a deterministic did-you-mean over in-scope values; hint when the closest match is in another namespace. |
| SKY-N0002 | Name | cannot find this type in scope | A type name in an annotation/type decl is undefined. Did-you-mean over in-scope type names. |
| SKY-N0003 | Name | cannot find this constructor | A constructor used in a pattern or expression is undefined/misspelled. Did-you-mean over in-scope constructor names (mirrors the Haskell canonicaliser). |
| SKY-N0004 | Name | unknown module or import | A qualified name's qualifier ('Qual.name') names no module/import alias in scope. Did-you-mean over import aliases + kernel modules. |
| SKY-N0005 | Name | module has no such member | The qualifier resolves but the member is absent/unexposed ('String.frobnicate'). Did-you-mean over that module's members ('fromInt'). |
| SKY-N0010 | Name | value defined more than once | Two top-level value bindings share a name. Points at both definition spans (secondary = first). Closes the current silent last-wins. |
| SKY-N0011 | Name | constructor defined more than once | Two constructors (same or different unions) share a name. Points at both spans. Closes silent last-wins in register_union. |
| SKY-N0012 | Name | type defined more than once | Two unions/type declarations share a name. Points at both spans. Prevents the downstream cross-module bare-Symbol type-name collision in the backend. |
| SKY-T0001 | Type | type mismatch | Two types fail to unify. Payload carries pretty-printed expected + found (owned TyDoc), the use span (primary), an optional definition span (secondary), and the diverging field/row path when applicable. |
| SKY-T0002 | Type | infinite type | Occurs-check fails: a flexible var would have to satisfy 'a = a -> b' (e.g. self-application). Carries the real offending span (today wrongly DUMMY + miscategorised as Mismatch). |
| SKY-T0003 | Type | type inference exceeded its step budget | HM solver step budget (N) exhausted before inference settles. Help: raise via SKY_SOLVER_BUDGET=<n> (0 disables). Best-effort span of the last-blamed constraint. |
| SKY-T0004 | Type | more parameters than the type signature describes | A typed binding has more parameter patterns than its annotation has arrows (e.g. 'f a b = ...' with 'f : Int'). Was a CompilerBug; now user-facing with the binding span + the annotation rendered. |
| SKY-T0010 | Type | this case does not handle every possibility | A case does not cover every constructor of the scrutinee's enum. Lists the MISSING constructors; M0 has no wildcard '_'. Emitted by a new exhaustiveness check at end of type-checking, with a source span — must exist so the lowering Match::new ICE is unreachable. |
| SKY-T0011 | Type | redundant case branch | Two arms cover the same constructor (or an arm is unreachable). Warning severity; names the duplicated constructor with the arm span. User-facing analogue of the lowerer's duplicate-arm ICE. |
| SKY-L0100 | Lower | pattern kind not supported yet | A case arm uses a wildcard/variable/literal pattern; M0 matches only nullary constructor patterns. [feature: case-pattern-kinds] |
| SKY-L0101 | Lower | operator not supported yet | A binary operator other than '+'/'-' is used ('*','/','==','++'). Reported with the operator's span; payload carries the operator. [feature: binops] |
| SKY-L0102 | Lower | polymorphic value's type could not be determined | A value stays fully polymorphic — the solver never pinned it to a concrete instance (e.g. 'let f = identity' with f never applied), so the lowerer cannot monomorphise it. M2a emits generic functions, but cannot yet represent an under-determined polymorphic value. [feature: polymorphism] |
| SKY-L0103 | Lower | function-valued parameters/returns not supported yet | A function type appears in argument/return position of a value annotation. [feature: higher-order-values] |
| SKY-L0104 | Lower | only Task () is supported yet | A Task type other than 'Task ()' is used ('Task Int'). [feature: task-results] |
| SKY-L0105 | Lower | parameter destructuring not supported yet | A function parameter is a non-variable pattern ('f (Just x) ='); M0 params must be plain names. [feature: param-patterns] |
| SKY-L0106 | Lower | top-level function needs a type signature | An unannotated top-level binding has parameters ('f x = x + 1' with no sig). Add 'f : Int -> Int'. [feature: untyped-functions] |
| SKY-L0107 | Lower | first-class functions not supported yet | A function is referenced as a bare value, or a call's callee is not a kernel/top-level name (lambda/computed callee). [feature: first-class-functions] |
| SKY-L0108 | Lower | kernel function not available yet | A kernel call other than Log.println / String.fromInt is used ('Time.now'). Payload carries the name + did-you-mean. Ideally validated at name-resolution. [feature: kernels] |
| SKY-L0200 | Lower | expression nests too deeply for the backend | A deeply nested BinOp/Call/Match from untrusted source exceeds the Rust backend's bounded emit depth. Fail-fast user-facing message instead of a native stack overflow. |
| SKY-I0001 | Internal | internal compiler error | Generic violated invariant (illegal IR, missing region type, unbound local that resolver should have caught). Renders with 'this is a bug in Sky, please report'. The only free-form String channel (CompilerBug.detail). |
| SKY-I0010 | Internal | intern: unresolved symbol | Interner::resolve hit a Symbol not backed by the interner (forged via from_raw / post-overflow collision). Surfaced via Option<&str> + CompilerBug{where_:"intern"} rather than the silent "" that leaks an empty identifier into generated Rust. |
| SKY-I0011 | Internal | intern: symbol table exhausted | More than u32::MAX distinct strings interned. Fail fast instead of saturating to u32::MAX and aliasing distinct names to one Symbol. |
| SKY-I0100 | Internal | ICE: match on unknown variant | An IR Match arm names a constructor not in the scrutinee enum's variant set. Detail resolves names (not raw ids). Unreachable once SKY-T0010 exists. |
| SKY-I0101 | Internal | ICE: duplicate match arm | Two IR arms cover the same variant. Unreachable once SKY-T0011 exists upstream. |
| SKY-I0102 | Internal | ICE: non-exhaustive match | IR arm set does not cover every variant; detail names the MISSING constructors. Unreachable once SKY-T0010 exists. |
| SKY-I0103 | Internal | ICE: match arm enum mismatch | An arm's pattern constructor type does not match the scrutinee enum while sharing a colliding variant Symbol (Pat::Ctor.ty never validated). Currently passes silently — must be detected. |
| SKY-I0200 | Internal | ICE: no Rust name for symbol | sky_backend_rust EmitCtx has no name entry for an enum-type/func Symbol the lowerer referenced (enum_name/func_name miss). |
| SKY-I0201 | Internal | ICE: dangling value/variant symbol | emit_expr resolves a Var/Ctor/param Symbol to an empty string (lowerer emitted a dangling Symbol). Must fail fast instead of emitting an empty Rust identifier. |
| SKY-I0202 | Internal | ICE: cross-module type-name collision | Two modules' types resolve to the same bare-Symbol key in EmitCtx::build; one mapping was silently overwritten. Prefer module-qualifying the key so it cannot occur; else fail fast. (SKY-N0012 guards the user-facing case.) |
| SKY-I0203 | Internal | ICE: golden anchor missing | A preamble/project golden anchor (USER TYPES banner, Ffi.kernel polyfill, runtime_bindings START/END) not found in the embedded golden. Replaces the silent-empty fallback so a future golden edit fails loudly. |

Ranges: `SKY-P####` parse, `SKY-N####` name resolution, `SKY-T####` type, `SKY-L####` lower / not-yet-supported, `SKY-I####` internal (compiler bug / ICE).

## sky_diagnostics changes (additive, leaf crate)

- New `code.rs`: `Code(&'static str)` + taxonomy constants; `Severity { Error, Warning, Bug }`; `title(Code) -> &'static str`; `explain_page(Code) -> Option<&'static str>`.
- Grow `ParseError` / `NameError` / `TypeError` with payload variants (owned `Box<str>` / POD enums — `TokenKind`, `Expected`, `ExpectedSet`, `Construct`, `HeaderDefect`, `ExposingDefect`, `TypeDeclDefect`, `CaseDefect`); add owned `TyDoc` for type rendering (producer zonks + builds it, so the reporter needs neither interner nor VarIds).
- New `Diagnostic::Lower { span, msg: LowerError }` with `LowerError`/`Feature` — "not supported yet" gets its own channel, distinct from `CompilerBug` ("compiler is broken").
- Total methods on `Diagnostic`: `code()`, `severity()`, `primary_span()`, `help() -> Vec<HelpLine>` (structured, payload-derived). `CompilerBug.where_` maps to a stable `SKY-I####`.
- New `render.rs`: `render(&Diagnostic, file, source) -> String` — 4-band rustc/Elm layout (header `error[CODE]: title`, ` --> file:line:col`, source snippet with `^` primary + `-` secondary underline, `= help:` + `= note: run skyc explain <CODE>`). Pure, deterministic, clamped, NO_COLOR-aware.
- `crates/sky_diagnostics/explain/SKY-*.md` pages + embedding table + a CI `#[test]` asserting: every emitted `code()` has a page; page line 1 == `# <CODE>: <title>`; ≥3 ` ```sky ` blocks per page.

## Cross-crate (one-directional; sky_diagnostics stays a leaf)

- `sky_intern`: `resolve` → `Option<&str>`; `intern` capacity-guarded → `CompilerBug` on `u32::MAX` exhaustion (no silent saturation/aliasing). `sky_intern` gains a one-way dep on `sky_diagnostics`. Closes SKY-I0010/I0011.
- Producers (`sky_parse`/`sky_canon`/`sky_types`/`sky_lower`/`sky_backend_rust`) switch coarse variants → payload variants at every audited site; resolve `Symbol`s at the failure point into owned payload data.
- `sky_types`: add the end-of-checking exhaustiveness + redundancy pass (SKY-T0010/T0011) so the lowerer's `Match::new` failure becomes a genuinely unreachable ICE.
- `sky_backend_rust`: bounded emit-depth guard (SKY-L0200); reserved-Rust-name mangling; checked ident resolver (SKY-I0201); module-qualified type-name keys (SKY-I0202); golden-anchor-missing → SKY-I0203 (no silent empty fallback).
- `skyc`: `CliError::Pipeline { file, src, diag }` rendering via `render(...)`; new `skyc explain <CODE>` subcommand (embedded pages, did-you-mean on unknown code, no-arg index).

## Explain page format (CI-enforced)

Line 1 `# <CODE>: <title>`; 1-3 plain-language paragraphs (Elm tone, address the reader); ≥3 ` ```sky ` blocks illustrating the error AND ≥1 fix. `SKY-L*` pages say "not supported yet" + `[feature: <tag>]`, never blame the user. `SKY-I*` pages state it is a compiler bug, what to attach (`skyc --version` + source), where to report. Defensive-bound pages (`SKY-P0003`, `SKY-T0003`, `SKY-L0200`) explain the bound is deliberate and how to raise/avoid it.

## Hardening backlog (guardian audit — 20 should-fix, 0 blockers)

- **[Correctness] sky_intern + sky_diagnostics** @ `crates/sky_intern/src/lib.rs:28` — Do not saturate. Either (a) make the capacity bound an explicit invariant: `debug_assert!(self.strings.len() < u32::MAX as usize)` plus a documented upstream source-size gate, and on the impossible overflow return a `Diagnostic::CompilerBug` (requires `intern` to become fallible / the crate to depend on sky_diagnostics), or (b) at minimum replace `unwrap_or(u32::MAX)` with an explicit overflow branch that yields a CompilerBug instead of a colliding id. Never let two distinct strings map to one Symbol.
- **[Correctness] sky_intern + sky_diagnostics** @ `crates/sky_intern/src/lib.rs:37` — Return `Option<&str>` (`self.strings.get(sym.0 as usize).map(String::as_str)`) and let the caller turn `None` into `Diagnostic::CompilerBug { where_, detail }`. Keeps the no-panic guarantee while making the missing-symbol case a typed, surfaced error rather than a silent empty name.
- **[Completeness] sky_intern + sky_diagnostics** @ `crates/sky_diagnostics/src/diagnostic.rs:9` — Co-design these payloads with the stable error-code scheme: give each variant structured fields (e.g. `Unexpected { found: Symbol, expected: SmallVec<TokenKind> }`, `Unknown { name: Symbol, suggestions: Vec<Symbol> }`, `Mismatch { expected: TypeId, found: TypeId }`) plus a stable code, so messages are rendered from data and remain machine-greppable. Keep `CompilerBug.detail` as the sole free-form String.
- **[Correctness] sky_syntax + sky_parse** @ `sky_parse/src/parser.rs:467-481 (parse_type_atom, uppercase branch) and crates/sky_syntax/src/ast.rs:131-140 (TypeAnnotation)` — Either (a) add `TTypeQual(Symbol, Symbol, Vec<Self>)` to TypeAnnotation and have parse_type_atom split a dotted upper ident into (qualifier, name) exactly as the reference does, rejecting 3+ segments; or (b) if qualified types are explicitly deferred past M0, reject a dotted upper-ident in type position with a typed error (a new ParseError variant) so unmodelled input fails fast instead of producing a non-reference AST. Mirror the same audit for Pattern_::PCtor vs the reference's PCtor/PCtorQual split (ast.rs:120-128; parser.rs:680-693).
- **[Completeness] sky_syntax + sky_parse** @ `crates/sky_diagnostics/src/diagnostic.rs:9-15 (ParseError) and ~30 raise sites across lexer.rs + parser.rs` — Grow ParseError into per-construct variants carrying the expected set and (where cheap) the found token, e.g. ParseError::Expected{what: ExpectKind, found: Option<TokKind>}, plus dedicated ParseError::IntLiteralOverflow and ParseError::UnexpectedChar(char). Thread them through the sites cataloged in diagnostic_sites. This is purely additive to the existing typed-diagnostic infrastructure; no soundness change.
- **[Correctness] sky_canon** @ `crates/sky_canon/src/resolve.rs:89-93 (canonicalise_value) + :311-321 (canonicalise_type)` — Order free vars by resolved name string, not Symbol id. In canonicalise_value, after collecting: `let mut fv: Vec<Symbol> = free_vars.into_iter().collect(); fv.sort_by(|a,b| interner.resolve(*a).cmp(interner.resolve(*b)));` and store fv. (Interner is already in scope.) Add a regression test with a multi-tyvar annotation whose alphabetical order differs from source/intern order.
- **[Correctness] sky_canon** @ `crates/sky_canon/src/resolve.rs:58 (register_union env.ctors.insert) and :38 (env.vars.insert for top-levels)` — Detect collisions at registration: before inserting into env.ctors / env.vars, check presence and on a hit return a new `NameError::Duplicate`-style diagnostic carrying both spans (the M0 NameError enum currently only has `Unknown`; add a variant). Verify exact reference shape against src/Sky/Canonicalise/Module.hs duplicate handling, but the silent-overwrite behaviour is a divergence regardless.
- **[Efficiency] sky_canon** @ `crates/sky_canon/src/resolve.rs:75 (body_env = env.clone()) and :169 (arm_env = env.clone())` — Use a scoped overlay instead of cloning: either push the pattern-bound locals into `env.vars`, recurse, then remove them (restore prior values), or thread an immutable parent `Env` plus a small per-scope `BTreeMap<Symbol, VarHome>` consulted first in lookups. Keeps determinism while making scope entry O(bound names) not O(env).
- **[Soundness] sky_types** @ `crates/sky_types/src/constrain.rs:355-379 (zonk_depth) and crates/sky_types/src/unify.rs:125-156 (occurs)` — Rewrite zonk and occurs as iterative walks over an explicit heap-allocated work stack (mirroring the iterative `find` in unionfind.rs), bounded by the existing depth limit and budget. At minimum, tick the Budget inside zonk_depth and lower both depth limits well under the native-stack ceiling (e.g. a few thousand) so the guard fires before the stack overflows.
- **[Correctness] sky_types** @ `crates/sky_types/src/constrain.rs:382-390 (peel_arrow), reached from constrain.rs:187-193` — Either validate param/arrow arity during canonicalisation and reject with a Name/Type error there, or change peel_arrow to return a real TypeError variant carrying the binding's span and the written annotation, e.g. `f expects 1 argument but its definition binds 2`.
- **[Correctness] sky_ir + sky_backend** @ `crates/sky_ir/src/ir.rs:162 (Match::new) and :131 (Pat::Ctor)` — Thread the scrutinee's enum name (Symbol) into Match::new and require every `arm.pat`'s `ty` to equal it (return CompilerBug otherwise); OR remove the unchecked `ty` field from `Pat::Ctor` so it cannot encode a false claim. Prefer the former — it lets codegen rely on `pat.ty` being trustworthy.
- **[Soundness] sky_ir + sky_backend** @ `crates/sky_ir/src/ir.rs:90-98 (Expr), :141 (Match.scrutinee: Box<Expr>)` — Document the load-bearing dependency on the parser depth cap at the `Expr` definition, and ensure that cap is provably below the stack-overflow threshold of the heaviest recursive op (drop of a boxed spine). For defence-in-depth, consider a manual iterative `Drop` for `Expr` (drain the spine into a worklist) so the IR is self-protecting regardless of the parser's limit — same class as the zonk/occurs recursion risk already tracked in sky_types.
- **[Security] sky_ir + sky_backend** @ `crates/sky_backend/src/lib.rs:22-28 (EmittedProject.files)` — Either (a) make the path key a validated newtype (`RelPath`) whose constructor rejects absolute paths, `..` components, and leading `/`/drive letters, returning a Diagnostic; or (b) document in the contract that the on-disk materialiser MUST reject such keys, and ensure that writer does so. Do not trust the lexer invariant silently at the disk boundary.
- **[Soundness] sky_backend_rust** @ `crates/sky_backend_rust/src/emit_expr.rs:42` — Thread a depth counter through emit_expr and return Diagnostic::CompilerBug (or a dedicated 'expression too deeply nested' diagnostic) past a fixed ceiling, OR rewrite the BinOp/Call/Match descent as an explicit work-stack so depth is heap-bounded. Add a regression test that emits a deeply-nested BinOp chain and asserts a graceful Err, not an abort.
- **[Correctness] sky_backend_rust** @ `crates/sky_backend_rust/src/lib.rs:74-81` — Key enum_names by a module-qualified identity (e.g. (ModuleName, Symbol) or a per-def unique id mirroring FuncId) rather than the bare Symbol. If the IR cannot distinguish two same-named types by Symbol alone, raise that as an IR-boundary fix; at minimum detect a duplicate insert in build() and return Diagnostic::CompilerBug instead of overwriting.
- **[Correctness] sky_backend_rust** @ `crates/sky_backend_rust/src/naming.rs:88-104` — Add a `reserved_rust_names` set and rewrite colliding emitted identifiers via raw identifiers (`r#name`) or a trailing-underscore mangle, mirroring the Go backend's reservedGoNames. Apply at module_value/enum_name and at the var/param emit sites (emit_expr.rs:45, emit_expr.rs:87). Cover the full Rust 2018/2021 keyword list including reserved-for-future (`become`, `priv`, `typeof`, `unsized`, `virtual`, `macro`).
- **[Correctness] sky_backend_rust** @ `crates/sky_backend_rust/src/emit_expr.rs:45` — Add a checked resolver on EmitCtx (e.g. resolve_ident(sym) -> DResult<&str>) that returns Diagnostic::CompilerBug when interner.resolve yields "" for a non-empty-intended symbol, and route Var (emit_expr.rs:45), Ctor variant (emit_expr.rs:47), Match arm variant (emit_expr.rs:69), enum variant (emit_types.rs:50) and param names (emit_expr.rs:87) through it.
- **[Correctness] sky_lower + skyc** @ `crates/sky_types/src/constrain.rs:272-282 (consumed at crates/sky_lower/src/lower.rs:302 → crates/sky_ir/src/ir.rs:184)` — Add an exhaustiveness check in sky_types (or a dedicated pass) keyed off the union's full ctor set, emitting a `Diagnostic::Type` with the scrutinee span and the missing variants. Then `Match::new` failing becomes a genuine unreachable CompilerBug contract.
- **[Completeness] sky_lower + skyc** @ `crates/sky_lower/src/lower.rs:96-100,124-127,138-141,150-157,174-177,227-230,253-256,264-266,275-279,289-293` — Add a `Diagnostic::Unsupported{span, feature: &'static str}` variant to sky_diagnostics and reclassify these sites to it (carrying the node's span). Reserve CompilerBug strictly for the genuinely unreachable cases (lower.rs:88, 106, 179-182, 202-206, 284).
- **[Correctness] sky_lower + skyc** @ `crates/sky_lower/src/lower.rs:195-207 (con_name_to_ir) and 163-192 (ir_type_from_ty)` — Confirm sky_canon rejects builtin-shadowing type/ctor names (as the Haskell §3.2 gate does); if not, reject here. At minimum add a comment pinning the invariant to the canon gate so the precedence isn't read as a deliberate override.

Plus 21 nits (efficiency/readability) tracked but lower priority; addressed where they touch files already being changed. Full audit JSON archived in the run transcript.

## Guardian gates (blocking, per phase)

1. `#![forbid(unsafe_code)]` + clippy-hardest green workspace-wide; Miri clean on changed crates.
2. `code()/severity()/primary_span()/help()` total (no `_ =>`).
3. CI page-coverage test green (every code → conforming page with ≥3 snippets).
4. No String error channel except `CompilerBug.detail`; payloads owned + structured.
5. Renderer panic-free on DUMMY/OOB spans (proptest/fuzz).
6. Determinism: stable did-you-mean ordering; no HashMap in rendered output.
7. M0 golden E2E still byte-identical + prints 1 (no regression).

## Backlog (added 2026-06-27, post-directive)

- **`--emit-ir` (debug aid).** `skyc build --emit-ir` runs parse→canon→types→lower and prints the `sky_ir::Program` via a **dedicated pretty-printer** (readable indented tree, not raw `Debug`), then stops before codegen. No serde/RON dep; in-memory IR only. Purpose: localize a divergence to before/after the `sky_ir` boundary, complementing the golden byte-diff oracle. Lands in Phase F.
- **Formatting + edition standing gates.** All compiler source MUST be `cargo fmt --all --check` clean (rustfmt `style_edition = "2024"`); compiler crates are edition 2024. Add a `fmt` job to CI (Phase F) alongside clippy/test/miri. NOTE: emitted Sky projects stay edition 2021 (golden + runtime-rust contract) — independent of the compiler's edition.
- **Kernel representation decision (pre-stdlib-breadth).** `sky_ir::KernelFn` is a flat enum (M0: 2 variants). It does NOT scale to the full stdlib (the Haskell reference handles ~76k FFI symbols via dispatch tables). Before widening kernels, decide between: (a) flat enum — simplest, compile-time-total, but the enum + every backend `match` churns per kernel; (b) two-level enum grouped by stdlib module (`Kernel::String(StringFn::FromInt)`) — mirrors the stdlib but leaks the frontend module taxonomy into the IR boundary and adds nested matches; (c) **registry/table-driven** `Callee::Kernel(KernelId)` where `KernelId` is a resolved handle and a single shared kernel table maps it to {Sky signature, per-backend emission} — IR stays stable as the stdlib grows, exhaustiveness becomes a registry-coverage test + the `SKY-L0108` fail-fast for unimplemented kernels. Recommendation: keep flat for M0; adopt (c) the registry model when kernels widen (matches the Haskell's dispatch-table architecture). Guardian to ratify when that milestone opens.

## Addenda (2026-06-27) — suggested fixes, humble messaging, issue link

### A. Machine-applicable suggestions + opt-in source patching
- Every did-you-mean / structured fix becomes a typed **`Suggestion { span, replacement: Box<str>, applicability }`** where `Applicability ∈ { MachineApplicable, MaybeIncorrect, HasPlaceholders }` (rustc model). The renderer shows it (`help: replace `lenght` with `length``); `MachineApplicable` single-candidate suggestions are eligible for auto-patch.
- **`skyc fix`** (+ a `--fix` flag on `build`): applies suggestions to the source. **Default is interactive + confirm** — patching source is a hard-to-reverse, mutating action, so the compiler MUST ask ("Replace `lenght` with `length` at Main.sky:12:18? [y/N]") unless `--fix`/`--yes` gives durable authorization. Never silent.
- **Soundness of patching (guardian gate):** apply only non-overlapping spans, back-to-front, write atomically (temp file + rename); re-parse after patching and refuse to keep a patch that doesn't compile/parse (offer rollback); only `MachineApplicable` is auto-applied — `MaybeIncorrect`/`HasPlaceholders` are shown but require explicit per-edit confirm. No raw byte indexing; spans validated against current source length.

### B. Human-friendly, limitation-exposing messages (Elm tone)
- All messages and explain pages address the reader plainly, name the exact conflict, and avoid jargon dumps.
- When the compiler is **uncertain / hits its own limitation** (an unclassified internal state, an ICE, a not-yet-modelled shape it can't phrase precisely), it says so honestly, Elm-style, and points to the issue tracker — never a raw backtrace, never false confidence. Canonical wording:
  > `I'm not sure what went wrong here — sorry about that. This is likely a gap in the Sky Rust compiler. Please report it (with this source + `skyc --version`) at: <ISSUE_TRACKER_URL>`
- `SKY-I*` (internal/ICE) pages already must state "this is a compiler bug, not your fault," what to attach, and where to report. Extend that with the explicit apology + the issue link. `SKY-L*` (not-yet-supported) pages stay "not supported yet" + `[feature: …]`, also linking the tracker for "please nudge us."
- A single constant `ISSUE_TRACKER_URL` (Codeberg, e.g. `https://codeberg.org/<owner>/sky-rust/issues`) is the source of truth, embedded once and referenced by every humble/ICE message + page footer. (Placeholder until the repo's Codeberg home is fixed.)

## Addendum (2026-06-28) — the compiler is a KIND TEACHER (MUST)

Error explanations are not just correct and friendly — they **teach**, so the
reader (human or AI agent) leaves more capable than they arrived. This is a
non-negotiable requirement for every `explain/SKY-*.md` page and, where space
allows, for inline diagnostic help.

### Progressive, layered structure (every page)
A page reads top-to-bottom from newcomer to expert. `skyc explain <CODE>` prints
the whole progression so the reader naturally descends as far as they want:

1. **🧒 In plain words (for anyone)** — explain it to a curious 10-year-old:
   concrete analogy, zero jargon. The reader must understand the *shape* of the
   problem from this alone.
2. **🛠️ A bit deeper (everyday terms)** — the actual rule in working-developer
   language, with the fix.
3. **�the code** — ≥3 `sky` snippets (the existing requirement): the error and at
   least one fix.
4. **🔬 Under the hood** — the real mechanism / algorithm / theory.
5. **📖 Names & where they come from** — a glossary: EVERY named concept used on
   the page (Skolem, Maranget, Hindley–Milner, monomorphisation, eta-expansion,
   exhaustiveness, usefulness, unification, rigid var, …) gets: a plain
   definition, **its etymology / origin** (the person or root the name comes
   from), and *why it matters here*. Never name-drop a term without teaching it.

Tone: kind, encouraging, never condescending. `SKY-L*` (not-yet) pages say "not
your fault, not supported yet." `SKY-I*` (ICE) pages say "this is our bug, please
report" + the issue link. A humility line belongs on hard pages: *if the compiler
can't explain itself well, that's our bug.*

### CI enforcement (extend the page-coverage test)
The existing coverage test gains assertions:
- Each page contains the tier section markers (plain-words, deeper, under-the-hood)
  in order, plus the ≥3 `sky` blocks.
- **Jargon gate:** maintain a `TEACHABLE_TERMS` list (skolem, maranget,
  hindley-milner/hm, monomorphi, eta, exhaustiv, usefulness, unification, rigid,
  union-find, …). If a page body contains a term, it MUST have a glossary entry
  defining it (term + origin). A page that uses jargon without teaching it fails CI.

### Roll-out
- The progressive template + jargon gate is established now; **every NEW page from
  here on uses it** (added to the workflow agent preamble).
- A dedicated **page-upgrade batch** rewrites the existing ~56 pages to the
  progressive format + builds the shared glossary. Because each page is a disjoint
  `.md` file, this fans out in PARALLEL (per the disjoint-files rule) and is
  mechanical/cheaply-verified → use **Sonnet** (model-selection policy). The
  shared CI-gate + glossary-infra change (touches sky_diagnostics) is one
  sequential owner. Schedule it as a quality batch (not blocking the roadmap; can
  run parallel to code milestones since pages ≠ crate source).

## Addendum (2026-06-28) — reader-facing pages are TIMELESS (no project archaeology)

Every `explain/SKY-*.md` page (and future user/stdlib docs) explains the
*concept and behaviour*, NEVER the compiler's development history. The reader is a
Sky programmer or agent learning the language — they do not care how or when the
compiler was built.

**Forbidden in reader-facing pages** (FAIL CI): references to phases / milestones
(e.g. "M3b-2", "v0.15 Stage A"), sessions, epics, "walls" (e.g. "Wall #2"), task
numbers, internal problem IDs, and first-person dev narrative ("we ported it…",
"this was added in…", "previously this failed…"). The glossary teaches a concept
timelessly — e.g. Maranget's algorithm is "the standard exhaustiveness/usefulness
method from Luc Maranget's 2007 paper (INRIA)", with NO "we ported it" tail.

Allowed: the concept, the rule, the fix, the theory, the names + their real-world
origins, and (for SKY-L*/SKY-I*) the "not yet"/"our bug, please report" framing
(that's product behaviour, not archaeology).

**CI gate:** the page-coverage test adds an archaeology denylist (case-insensitive
regex over page bodies): `\bM[0-9]+[a-z]?\b` (milestone tags), `\bWall #?[0-9]`,
`\bphase\b`, `\bepic\b`, `\bsession\b`, `\btask #?[0-9]`, `we ported`, `was added
in`, `v0\.[0-9]+ stage`. A reader-facing page matching any of these fails CI.

**Code comments:** apply the same spirit going forward — comments explain WHAT and
WHY, not WHEN/which-phase/which-task. Existing archaeology comments ("M3a
residual", "Wall #2", "v0.15 Stage A", "fixes M2c BLOCKER") are pruned
opportunistically when a file is touched, and swept in a dedicated cleanup pass
before any public release. (This is a *code-readability* item, not CI-gated yet.)

## Addendum (2026-06-28) — explain-page structure refinement (supersedes earlier tier order)

Canonical order for every `explain/SKY-*.md` page (technical-first; analogy late + optional):

1. **The short version** — 1–2 PRECISE sentences: what's wrong + the fix. No
   analogy. This is the agent-first / skim-first TL;DR; everyone reads it.
2. **What's happening** — the technical rule/mechanism, going as deep as the error
   warrants (working-dev → theory). The source of truth.
3. **the code** — ≥3 `sky` snippets (error + ≥1 fix).
4. **A simple comparison** — an OPTIONAL real-world analogy, clearly labelled and
   opened with a skip cue ("If that felt abstract, here's an everyday parallel…").
   Register: **young-adult / common-people-with-responsibilities** (jobs, money,
   policies, deadlines, routing) — NOT childish (no toys/games). Tight + accurate;
   it illustrates, it does not redefine.
5. **Names & where they come from** — glossary + etymology (per the kind-teacher
   addendum).

Rationale: leading with a metaphor delays the precise signal an AI agent / expert
wants and a strained metaphor can mislead. Placing the analogy AFTER the technical
truth and marking it optional makes it neutral-to-helpful for agents (skippable)
and grounding for human learners, while the technical section stays the
authoritative explanation. Agents primarily consume the rendered diagnostic; they
reach the page to LEARN, so depth is welcome as long as precision comes first.

(The kind-teacher addendum's CONTENT requirements — progressive depth, glossary
with etymology, timeless/no-archaeology, kind tone — all still hold; only the
section ORDER and the analogy register/placement are refined here.)
