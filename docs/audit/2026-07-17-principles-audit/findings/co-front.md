# co-front findings

5 findings: 0 critical, 1 high, 2 medium, 2 low

Audited: `src/compiler/parse/src/{lexer,parser,layout,lib}.rs`,
`src/compiler/canon/src/{resolve,env,target_gate,lib,link,ast}.rs`,
`src/compiler/kernels/src/lib.rs`,
`src/compiler/diagnostics/src/{code,diagnostic,render,span,lib}.rs`,
plus the wasm-gate + stdlib-injection call sites in
`src/ipe-cli/src/{lib,project,stdlib}.rs`.

The reserved-namespace gate (IPE-N0025), the reserved-builtin-type gate
(IPE-N0026), the kernel-alias fail-closed gate (IPE-N0028), the wasm target
gate (IPE-N0029 / IPE-L0129), the default-deny wasm allowlist, the interner
overflow guard, and the 99-code diagnostic taxonomy were all examined and found
structurally sound; the notable defects are unbounded native recursion paths in
canonicalisation that the parser's own depth guard does not cover.

## co-front-001 · Unbounded native recursion in `climb_binops` — stack overflow on a long right-associative operator chain
- severity: high
- axis: soundness
- principle: P3 soundness (no stack overflow on input) / P1 no unbounded resource a caller can exhaust
- location: `src/compiler/canon/src/resolve.rs:2983` (`climb_binops`, recursive call at :3009); reachable from `canonicalise_binops` :2946; fed by `src/compiler/parse/src/parser.rs:989` (`parse_expr`)
- reachability: `parse_expr` gathers a binary-operator chain in a `while` loop into a FLAT `ops: Vec` (parser.rs:996–1001). Its `MAX_DEPTH` guard checks only *nesting* depth (`depth > MAX_DEPTH` at entry), never the chain LENGTH — a single expression `"a" ++ "a" ++ … ++ "a"` (or `True && True && … `, or `x :: x :: … :: []`) of N operators is nesting-depth 1 but produces an N-element flat chain. `canonicalise_binops` then calls `climb_binops`, which for a right-associative operator (`++ :: && || <|`, all `Assoc::Right`) sets `next_min = prec` and recurses once per operator, so the native call depth equals the chain length N. A source file with a few hundred thousand right-associative operators overflows the thread stack → SIGSEGV / abort, not a diagnostic.
- problem: the parser module doc explicitly promises "Recursion is bounded by `MAX_DEPTH` so adversarial input cannot overflow the stack", but that guard is bypassed for operator-chain length because the chain is flat in the AST and the recursion happens later in canon. Crashes the compiler (a hosted/playground compile service is a remote DoS surface; even locally it is a soundness-invariant breach the crate advertises it does not have).
- fix direction: cap chain length in `parse_expr` (emit `NestingTooDeep`/a new coded error past a bound), or rewrite `climb_binops` to an explicit heap work-stack (as `target_gate::check_expr` already does for expression walks).
- prior: new

## co-front-002 · Unbounded native recursion in `resolve_interp_ref` — stack overflow on a many-word interpolation body
- severity: medium
- axis: soundness
- principle: P3 soundness (no stack overflow) / P1 no unbounded resource
- location: `src/compiler/canon/src/resolve.rs:3716` (`resolve_interp_ref`, self-call at :3727–3728)
- reachability: reached from `canonicalise_expr` → `desugar_multiline` (:3842) → `chunk_to_expr` (:3674) for every `{{…}}` interpolation in a triple-quoted string. `resolve_interp_ref` splits on the FIRST space and recurses on the trimmed remainder, so native call depth equals the whitespace-token count in one interpolation body. A triple-quoted literal `"""{{a a a … a}}"""` with hundreds of thousands of words recurses to full depth → stack overflow. Interpolation bodies have no length bound at parse time (the lexer stores the raw triple-string content verbatim).
- problem: another adversarial-input stack-overflow path not covered by the parser `MAX_DEPTH` guard; a crash instead of a bounded error.
- fix direction: bound the interpolation-body token count (fall back to the existing "too complex → literal `{{…}}`" policy past a small cap), or make the `func arg` splitter iterative.
- prior: new

## co-front-003 · `canonicalise_type` native recursion is bounded only by the number of aliases in scope, not by a fixed depth
- severity: low
- axis: soundness
- principle: P3 soundness / P1 no unbounded resource
- location: `src/compiler/canon/src/resolve.rs:3155` (`canonicalise_type`), alias-expansion recursion at :3288/:3290 guarded by `visited` (:3237)
- reachability: the `visited` set correctly prevents a *cyclic* alias from looping, but a long *acyclic* alias chain (`type alias A1 = A2`, `A2 = A3`, … `A99999 = Int`) expands by recursing once per link — native depth equals the chain length. Each alias declaration is shallow, so the parser depth guard does not bound it; the depth is bounded only by how many alias decls the user writes.
- problem: a pathological (or generated) source with a very long alias chain overflows the stack during type canonicalisation. Much harder to hit than 001/002 (needs thousands of distinct top-level alias decls) — filed as a smell for the same structural reason: canon recursion depth is a function of program size, not a fixed constant.
- fix direction: thread the existing depth/`visited` length into a `MAX_DEPTH`-style ceiling and emit a coded error past it.
- prior: new

## co-front-004 · Reserved-namespace gate (IPE-N0025) covers `Ipe`/`Rust` but the taxonomy doc-comment claims `Std`
- severity: low
- axis: correctness (documentation vs. enforcement drift)
- principle: fundamental rule "comments say WHAT the rule IS"; P2 correctness of the stated gate
- location: `src/compiler/canon/src/resolve.rs:473` (doc: "first path segment is `Ipê` or `Std`") vs. the actual gate at :571–598 (checks `Ipe` and `Rust`, never `Std`)
- reachability: not an exploit — the gate itself is sound (it keys on the unforgeable `ModuleOrigin`, and the embedded stdlib lives entirely under `Ipe.*`, confirmed in `src/ipe-cli/src/stdlib.rs` `COMPILED_STD_MODULES`, so there is no `Std.*` trusted module a user could shadow). The defect is that the function doc-comment names a reserved segment (`Std`) that the code does not reserve, so a reader auditing "is `Std` protected?" gets a false yes from the doc.
- problem: doc/enforcement drift on a security-tier gate; if a future `Std.*` embedded module is added, the doc would imply protection that is absent.
- fix direction: correct the doc-comment to name the actual reserved first segments (`Ipe`, `Rust`), or add `Std` to the gate if `Std.*` is meant to be reserved.
- prior: new

## co-front-005 · Wasm target gate string-fallback deny relies on `interner.resolve` never being consulted for the allow decision — verified fail-closed, noted for completeness
- severity: low
- axis: completeness
- principle: P1 security (default-deny) / make-invalid-states-unrepresentable
- location: `src/compiler/canon/src/target_gate.rs:53-61` (`VarKernel { id, .. }` arm); allowlist at `src/compiler/kernels/src/lib.rs:4844` (`wasm_client_available`)
- reachability: the gate denies any `VarKernel` whose `id` is `None` (a kernel resolved only by the string-match fallback) — correct fail-closed behaviour. This is NOT a defect; it is filed to record that the wasm gate's soundness depends entirely on `id: Option<StdlibKernel>` being populated for every genuinely-available kernel at resolution time (`env.rs:1665` builds `stdlib_index` from `StdlibKernel::ALL`, and `install_builtin_vars` threads the id in). A future kernel registered in the `QUALIFIERS` value table but absent from `StdlibKernel::ALL` would resolve with `id: None` and be denied on wasm even though it is safe — a completeness (false-negative) risk, never a security hole. The `decl()` injectivity test (`kernels/src/lib.rs:4976`) partially guards the inverse. No action required; documented so the judge tier can confirm the invariant.
- problem: none (invariant holds today); recorded as the load-bearing property the wasm allowlist rests on.
- fix direction: keep the `StdlibKernel::ALL` ⇔ `QUALIFIERS` correspondence gated by a test (a kernel reachable by qualifier must carry a `Some(id)`).
- prior: new
