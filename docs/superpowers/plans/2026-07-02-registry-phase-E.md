# Kernel registry — Phase E (the seal)

> Implementation plan (superpowers *writing-plans* grade). Read-only design
> artifact; the tasks below are executed later, in order, by an implementer.
> Source spec (follow, do not redesign):
> `docs/architecture/kernel-registry-design.md` §Q5 "Phase E", §Q6, and the
> "Phase C re-review" point 5 (G1 three-part unambiguity proof).
> Umbrella task: **#45** ("Make constrain kernel-scheme table exhaustive over
> canon lists").

---

## Goal

Turn the kernel-scheme gate from a **runtime test** into a **compile-time
guarantee**. When every stdlib family is migrated (Phase D done, burndown = 0),
Phase E performs three irreversible deletions so that "a canon-listed kernel
with no scheme" becomes **unrepresentable by construction**:

1. **Delete the `Ty::Var(u32::MAX)` fallback** in `sky_types/src/constrain.rs`
   and make `stdlib_scheme` **total** (`-> Ty`, no `Option`). The legacy
   `kernel_ty` string table and both `u32::MAX` sentinel sites are removed.
2. **Drop `module`/`name` (and the `Option`) from the `VarKernel` node** so a
   kernel reference is carried purely as a `StdlibKernel` id; every downstream
   consumer delegates to `id.decl()`. Valid because (a) the full subset gate
   (deliverable 3) proves every installed module-kernel has an id, (b)
   `decl()` is injective (`no_colliding_qualifier_name_pairs`), and (c) a new
   `some_node_decl_equals_node` assert proves `decl(id).(qualifier,name)`
   equals the node's `(module,name)` for every resolved reference — so the two
   symbols are provably redundant.
3. **Make the forward `canon_equals_registry` check the full
   QUALIFIERS-subset-registry gate** — add the canon→registry direction (every
   installed module-qualifier member resolves to `id = Some`), turning the
   one-directional `registry ⊆ canon` check into `registry ≡ canon` on module
   qualifiers. This gate turning green **is** the machine-checked precondition
   for deliverable 2.

Done-state: `stdlib_scheme`, `native_ir_type`, and backend emit are total
`match StdlibKernel` with no wildcard; adding a variant fails to compile in
every consumer crate until an arm exists. Drift is a compile error; the F1
hole is unrepresentable.

---

## Architecture

Identity lives in the leaf crate `sky_kernels` (`StdlibKernel` + `decl() ->
StdlibDecl`, deps: none today — add `sky_intern`/`sky_diagnostics` only if a
task needs them, it does not). N exhaustive projections live in the crate that
owns each target type:

| Axis | Home crate | Form (post-E) |
|---|---|---|
| identity / arity / class / emit | `sky_kernels` | `StdlibDecl` via `decl()` — exhaustive, wildcard-free |
| HM scheme (`Ty`) | `sky_types` | `stdlib_scheme(StdlibKernel, &Builtins) -> Ty` — **total** |
| canon resolution | `sky_canon` | `stdlib_index: BTreeMap<(Symbol,Symbol), StdlibKernel>` built from `ALL` |
| callee dispatch | `sky_lower` | `lower_callee` reads `Callee::Kernel(id)` — no string table |

Phase E is the *seal*: it removes the transitional dual paths (registry ∨
legacy) that Phases B–D maintained, leaving only the registry path.

**Data-flow after E3** (module/name dropped):

```
surface AST  --canon-->  Expr_::VarKernel { id: StdlibKernel }   (parse, don't validate)
                              |
             sky_types  ->  constrain_var_kernel(id, span)  ->  stdlib_scheme(id) : Ty   (total)
             sky_lower  ->  lower_callee  ->  Callee::Kernel(id)                          (no &str match)
             sky_ir     ->  Callee::Kernel(StdlibKernel)                                  (alias optional, E4)
```

---

## Tech Stack

- Rust (workspace at repo root; `edition.workspace`, `version.workspace`).
- Crates touched: `sky_types`, `sky_canon`, `sky_lower`, `sky_kernels`
  (and optionally `sky_ir` in E4).
- Test runner: `cargo test -p <crate> <filter>`. No new deps
  (`strum` is **not** a workspace dep; `ALL` is a hand-written `const` array —
  keep it that way; the `no_colliding_qualifier_name_pairs` test already
  guards it).
- Diagnostics: `Diagnostic::Lower { span, msg: LowerError::Unsupported(Feature::Kernels) }`
  = **IPE-L0108** (`crates/sky_diagnostics/src/render.rs`: "this kernel
  function is not available yet [feature: kernels]"). Fail-closed everywhere;
  never `panic!`, never a silent wildcard.

---

## Global Constraints

- **PRINCIPLES order (highest wins):** security > correctness > soundness >
  efficiency > completeness > readability. Phase E is a soundness/correctness
  seal; it never trades those for readability or a smaller diff.
- **PARSE, DON'T VALIDATE.** `(qualifier, name)` is parsed **once**, at canon,
  into a typed `StdlibKernel`. After E3 no downstream stage can re-ask "is this
  a kernel?" — the only way to hold the id is to have passed the canon parse.
- **MAKE INVALID STATES UNREPRESENTABLE.** After E2/E3 the type system forbids
  "kernel id without a scheme" (`stdlib_scheme` total over the closed enum) and
  "kernel reference without an id" (`VarKernel { id: StdlibKernel }` — no
  `Option`, no fallback string pair).
- **Fail-closed, not panics/wildcards.** Any residual miss surfaces as an
  IPE-L0108-shaped `Err`, never `Ty::Var(u32::MAX)` and never `unreachable!()`.
- **Go-parity is golden-pinned.** No arm body changes value in Phase E — it is
  pure deletion of transitional paths. The per-kernel parity tripwires
  (`stdlib_scheme(k) ≡ legacy kernel_ty(decl(k).qualifier, decl(k).name)`) that
  Phase D authored are the proof; E2 removes them only *after* the legacy
  oracle they compare against is itself deleted.
- **../sky reference note.** Where the Haskell compiler at `../sky` is a parity
  reference, it is a capability reference only. The relevant difference: the
  Haskell backend re-matches `(qualifier, name)` strings at each stage
  (`Compile.hs` kernel dispatch); the Rust backend replaces those N hand-tables
  with N exhaustive `match` projections over one shared closed id — a
  compile-time totality the string-keyed design cannot express. Stated as a
  difference, no value judgment.

---

## Preconditions & parallel-safety (READ BEFORE STARTING)

**Phase E is strictly downstream of Phase D. Do not begin any task below until
all four hold:**

1. **Phase D complete — burndown = 0.** Every `StdlibKernel` in `ALL` has a
   real arm in `stdlib_scheme` (no `_ => return None`). Verify with **Task E0**.
   Today only String (34) + List (10) + Math (37) ≈ 81 of the ~424 variants are
   schemed; the rest still fall through to legacy. E1–E3 are unsound until this
   is 0.
2. **Full subset gate green** — **Task E1** must pass (canon ≡ registry on
   module qualifiers) so that every `VarKernel` carries `id = Some`. This is the
   machine-checked precondition for dropping the `Option` in E3.
3. **HEAD is quiescent on the registry files.** At the time this plan was
   written the working tree was **actively churning** under the in-flight
   Phase C agent: `crates/sky_types/src/constrain.rs` oscillated between the
   committed Phase-B signature `constrain_var_kernel(&mut self, module, name,
   span)` and an in-progress Phase-C signature that threads `id: Option<StdlibKernel>`
   and adds `stdlib_scheme(...) -> Option<Ty>` (a +516-line WT diff that then
   vanished; HEAD stayed at `691e275` "…parse-once seam … Phase B"). **All line
   numbers in this plan are therefore anchored by `file:fn`, not by line.**
   Re-grep each anchor immediately before editing.
4. **No concurrent edit in flight** on: `constrain.rs`, `sky_canon/src/{ast,env,resolve,lib}.rs`,
   `sky_lower/src/lower.rs`, `sky_kernels/src/lib.rs`. These are **co-owned**
   with task #45 (Phases C/D). Phase E takes exclusive ownership of them for the
   duration.

**File overlap with #49 (Port TailCallOpt):** #49 adds +2 variants to
`sky_ir` and edits `sky_lower/src/lower.rs` (`emit_expr.rs` too). Phase E's E3
edits `lower.rs` (`lower_callee`, deleting the ~399-arm table) and E4 (optional)
edits `sky_ir` (`Callee::Kernel` / `KernelFn` alias). **Do not run E3/E4
concurrently with #49.** Land one, rebase the other. Recommended sequence:
finish Phase E (E0–E3) first — it is a large mechanical deletion in `lower.rs`;
then rebase #49 onto the smaller post-E `lower_callee`. E4 (alias removal) is
optional and should be sequenced *after* both.

**Ground-truth anchors verified against HEAD `691e275` (2026-07-02):**

| Symbol | File | Current shape |
|---|---|---|
| `stdlib_scheme` | `sky_types/src/constrain.rs` | `fn stdlib_scheme(k: StdlibKernel, b: &Builtins) -> Option<Ty>`; tail `_ => return None` |
| legacy scheme | `sky_types/src/constrain.rs` | `fn kernel_ty(&self, module: Symbol, name: Symbol) -> Ty`; tail `_ => Ty::Var(u32::MAX)` |
| kernel constrain | `sky_types/src/constrain.rs` | `fn constrain_var_kernel(...)` (signature in flux — id threaded in Phase C); dual-lookup + fail-closed on `Ty::Var(u32::MAX)` |
| burndown test | `sky_types/src/constrain.rs` | `mod stdlib_scheme_tests::burndown_none_count_does_not_exceed_phase_c_ceiling` |
| node | `sky_canon/src/ast.rs` | `Expr_::VarKernel { id: Option<StdlibKernel>, module: Symbol, name: Symbol }` |
| home | `sky_canon/src/env.rs` | `VarHome::Kernel(Option<StdlibKernel>, Symbol, Symbol)`; built in `install_prelude_qualifiers` (QUALIFIERS + `stdlib_index`) and `install_builtin_vars` (Basics) |
| producers | `sky_canon/src/resolve.rs` | two `Ok(canon::Expr_::VarKernel { id: *id, module: *m, name: *f })` sites (`resolve_var`, `resolve_qual_var`) |
| gate test | `sky_canon/src/lib.rs` | `mod tests::canon_equals_registry` — forward (registry→canon) + G1 reverse |
| callee | `sky_lower/src/lower.rs` | `fn lower_callee`; VarKernel arm: `if let Some(sk) = id { return Ok(Callee::Kernel(*sk)) }` then ~399-arm `match (self.resolve(module), self.resolve(name))` legacy table, tail `(_,_) => Err(unsupported(callee.span, Feature::Kernels))` = IPE-L0108 |
| legacy tripwire | `sky_lower/src/lower.rs` | `mod tests::decl_equiv_legacy_match` (forces `id = None`) |
| identity | `sky_kernels/src/lib.rs` | `StdlibKernel`; `pub const fn decl(self) -> StdlibDecl`; `pub const ALL`; `no_colliding_qualifier_name_pairs`; `is_db/is_tea/is_server/is_ui/is_live/is_tui/is_webview` |
| ir alias | `sky_ir/src/ir.rs` | `pub type KernelFn = sky_kernels::StdlibKernel;`  `Callee::Kernel(KernelFn)` |

---

## Task E0 — Precondition gate: `stdlib_scheme` is total (burndown = 0)

Verification-only. Converts the monotone burndown ceiling into a hard
`== 0` gate. **If this task's test does not pass, STOP — Phase D is
incomplete and E1–E3 must not run.**

**Files**
- `crates/sky_types/src/constrain.rs` (test module `stdlib_scheme_tests`)

**Interfaces**
- Consumes: `stdlib_scheme(k: StdlibKernel, b: &Builtins) -> Option<Ty>`
  (pre-E2 signature), `sky_kernels::StdlibKernel::ALL: &'static [StdlibKernel]`.
- Produces: test `stdlib_scheme_is_total` asserting the `None` count over `ALL`
  is exactly 0.

**Steps**

1. Add the gate test next to `burndown_none_count_does_not_exceed_phase_c_ceiling`
   in `mod stdlib_scheme_tests`:

   ```rust
   /// Phase-E entry gate: every StdlibKernel has a scheme. `None` count MUST be 0.
   /// While this is red, Phase D is unfinished and E1–E3 are unsound — do not proceed.
   #[test]
   fn stdlib_scheme_is_total() {
       let (_interner, b) = make_builtins();
       let missing: Vec<StdlibKernel> = StdlibKernel::ALL
           .iter()
           .copied()
           .filter(|&k| stdlib_scheme(k, &b).is_none())
           .collect();
       assert!(
           missing.is_empty(),
           "{} StdlibKernel variants still have no scheme (Phase D incomplete): {missing:?}",
           missing.len(),
       );
   }
   ```

2. Run it — **expected to FAIL until Phase D lands** (this is the gate, not a
   bug in the test):

   ```
   cargo test -p sky_types stdlib_scheme_is_total
   ```
   Expected while Phase D incomplete:
   ```
   ---- constrain::stdlib_scheme_tests::stdlib_scheme_is_total stdout ----
   thread '...' panicked at 'NNN StdlibKernel variants still have no scheme (Phase D incomplete): [Encoding..., Uuid..., ...]'
   test result: FAILED. 0 passed; 1 failed
   ```

3. **Gate:** proceed to E1 only when this prints:
   ```
   test constrain::stdlib_scheme_tests::stdlib_scheme_is_total ... ok
   test result: ok. 1 passed; 0 failed
   ```

4. Commit (the gate test itself is a durable artifact):
   ```
   git add crates/sky_types/src/constrain.rs
   git commit -m "Phase E gate: assert stdlib_scheme is total over StdlibKernel::ALL"
   ```

---

## Task E1 — Full QUALIFIERS-subset-registry gate + decl==node assert

Deliverable 3, plus the injectivity/equivalence proofs E3 depends on. Strengthen
`canon_equals_registry` from one-directional (`registry ⊆ canon`) to
bidirectional on module qualifiers (`registry ≡ canon`), and add the
`some_node_decl_equals_node` assert.

**Files**
- `crates/sky_canon/src/lib.rs` (`mod tests::canon_equals_registry`, and a new
  sibling test)
- (read-only) `crates/sky_kernels/src/lib.rs` (`no_colliding_qualifier_name_pairs`)

**Interfaces**
- Consumes: `Env::initial`, `Env.qual_vars: BTreeMap<Symbol, BTreeMap<Symbol, VarHome>>`,
  `Env.stdlib_index: BTreeMap<(Symbol,Symbol), StdlibKernel>`,
  `VarHome::Kernel(Option<StdlibKernel>, Symbol, Symbol)`,
  `StdlibKernel::decl()`.
- Produces: strengthened `canon_equals_registry` with a canon→registry subset
  loop; new test `some_node_decl_equals_node`.

**Steps**

1. **Add the subset direction** inside `canon_equals_registry`, after the
   existing forward loop and G1 reverse loop. For every `(qual_sym, members)`
   in `env.qual_vars` whose resolved qualifier is a *module* qualifier (i.e.
   NOT in the sanctioned alias/prelude exclusion set already declared in that
   test — `Basics`, `Attr`, `Event`, `Ipe.*`, `Ipê.*`), assert every member
   that is a `VarHome::Kernel` carries `id = Some`:

   ```rust
   // ── Full subset gate (Phase E, deliverable 3): canon ⊆ registry ──────────
   // Every kernel installed under a real MODULE qualifier must resolve to a
   // StdlibKernel id. A `None` here is a canon-listed-but-unregistered kernel —
   // exactly the state E3 forbids by dropping the Option. The sanctioned
   // exclusions (Basics helper aliases, Ipe.*/Ipê.* namespace aliases) are the
   // same `excluded_quals` set used by the G1 reverse loop.
   for (qual_sym, members) in &env.qual_vars {
       let qual_str = interner.resolve(*qual_sym).unwrap_or("<unknown>");
       if excluded_quals.contains(qual_str) {
           continue;
       }
       for (name_sym, home) in members {
           if let VarHome::Kernel(id, _m, _f) = home {
               let name_str = interner.resolve(*name_sym).unwrap_or("<unknown>");
               assert!(
                   id.is_some(),
                   "canon ⊄ registry: qual_vars[{qual_str:?}][{name_str:?}] is a \
                    kernel with no StdlibKernel id; add a decl() entry (Phase D) \
                    or move the qualifier into the sanctioned exclusion set",
               );
           }
       }
   }
   ```

   > Reuse the existing `excluded_quals` binding; if it is scoped inside the
   > G1 reverse block, hoist it above both loops (pure refactor).

2. Run it — while Phase D leaves any module-kernel unregistered it **lists the
   exact residual names**, which is the burndown-by-name for E3:

   ```
   cargo test -p sky_canon canon_equals_registry
   ```
   Expected while residuals remain:
   ```
   thread '...' panicked at 'canon ⊄ registry: qual_vars["Encoding"]["base64Encode"] is a kernel with no StdlibKernel id; ...'
   test result: FAILED. 0 passed; 1 failed
   ```
   Fix each by ensuring the family's `decl()` + `stdlib_index` entry exists
   (Phase D work) — **not** by widening the exclusion set unless the name is a
   genuine non-kernel prelude alias.

3. **Add `some_node_decl_equals_node`** — the proof that `module`/`name` are
   redundant with `decl(id)` (the exact property E3 relies on). This closes the
   loop the spec's Phase-C re-review point 5 calls the "decl==node assert":

   ```rust
   /// For every installed kernel that carries an id, the id's decl() reproduces
   /// the node's own (module, name). Proves the two symbols on VarKernel are
   /// redundant → E3 may drop them and reconstruct via decl() with zero loss.
   #[test]
   fn some_node_decl_equals_node() {
       use crate::env::VarHome;
       use sky_intern::Interner;
       let mut interner = Interner::new();
       let env = Env::initial(vec![], &mut interner).expect("Env::initial");
       for members in env.qual_vars.values() {
           for home in members.values() {
               if let VarHome::Kernel(Some(k), m, f) = home {
                   let decl = k.decl();
                   let m_str = interner.resolve(*m).unwrap_or("<unknown>");
                   let f_str = interner.resolve(*f).unwrap_or("<unknown>");
                   assert_eq!(
                       (decl.qualifier, decl.name), (m_str, f_str),
                       "decl({k:?}) = ({:?},{:?}) but node stores ({m_str:?},{f_str:?}); \
                        E3 cannot drop module/name until decl() reproduces them",
                       decl.qualifier, decl.name,
                   );
               }
           }
       }
   }
   ```

   > Note the alias subtlety already documented in `canon_equals_registry`:
   > for `FUNC_ALIASES` the node stores the **canonical** `(m, f)` (not the
   > alias key), so `decl().(qualifier,name)` matches `(m, f)` — the assert is
   > over `(m, f)`, never the `qual_vars` key. This is why the assert reads `*m`
   > / `*f`, not the loop keys.

4. Run both green:
   ```
   cargo test -p sky_canon canon_equals_registry some_node_decl_equals_node
   ```
   Expected:
   ```
   test tests::canon_equals_registry ... ok
   test tests::some_node_decl_equals_node ... ok
   test result: ok. 2 passed; 0 failed
   ```

5. Confirm the injectivity support is present (read-only — do not modify):
   ```
   cargo test -p sky_kernels no_colliding_qualifier_name_pairs
   ```
   Expected `test result: ok. 1 passed`. Together {subset gate, decl==node,
   injectivity} are the three-part unambiguity proof (spec Phase-C re-review
   point 5) that makes E3 sound.

6. Commit:
   ```
   git add crates/sky_canon/src/lib.rs
   git commit -m "Phase E: full canon≡registry subset gate + decl==node redundancy proof"
   ```

---

## Task E2 — Total `stdlib_scheme`; delete `Ty::Var(u32::MAX)` + legacy `kernel_ty`

Deliverable 1. Make `stdlib_scheme` return `Ty` (no `Option`), delete its
`_ => return None` tail, delete the legacy `kernel_ty` fn and its
`_ => Ty::Var(u32::MAX)`, and remove the now-dead fail-closed branch and dual
lookup in `constrain_var_kernel`. Add the sentinel-ban and arity tests.

**Files**
- `crates/sky_types/src/constrain.rs`

**Interfaces**
- Consumes: `StdlibKernel`, `Builtins`, `Ty`, `Ty::Fun(Box<Ty>, Box<Ty>)`.
- Produces:
  - `fn stdlib_scheme(k: StdlibKernel, b: &Builtins) -> Ty` (total).
  - `constrain_var_kernel` routes **only** through `stdlib_scheme` (+ the
    Math.min/max Ord and Dict/Set key obligations, unchanged in behaviour).
  - Deletes: `fn kernel_ty`, the `_ => Ty::Var(u32::MAX)` arm, the
    `if id.is_some() { if Ty::Var(u32::MAX) ... Err }` fail-closed block, the
    `_ => return None` arm.
  - Tests: `no_ty_var_max_sentinel`, `arity_matches_scheme`; retire the
    burndown/None-oracle tests superseded by E0's totality test.

**Steps**

1. **Write the sentinel-ban test first** (Q6 secondary #3). It fails now because
   `Ty::Var(u32::MAX)` still appears in the source:

   ```rust
   /// The banned fail-open sentinel must appear nowhere in constrain.rs.
   /// Guards against a future PR silently reintroducing the F1 hole.
   #[test]
   fn no_ty_var_max_sentinel() {
       let src = include_str!("constrain.rs");
       // Allow this test's own mention by excluding the string in a comment-free way:
       let hits = src.matches("u32::MAX").count();
       assert_eq!(
           hits, 0,
           "`u32::MAX` still present in constrain.rs ({hits} occurrence(s)); \
            the Ty::Var(u32::MAX) fail-open sentinel must be fully deleted",
       );
   }
   ```

   > The literal `u32::MAX` inside this test body would self-trigger, so write
   > the needle as `concat!("u32", "::MAX")` in the `matches` call, or place the
   > test in a sibling file. Chosen approach: `src.matches(concat!("u32","::MAX")).count()`.
   > Rewrite step 1's `matches(...)` accordingly.

2. Run — expected FAIL (sentinel still present):
   ```
   cargo test -p sky_types no_ty_var_max_sentinel
   ```
   ```
   thread '...' panicked at '`u32::MAX` still present in constrain.rs (2 occurrence(s)); ...'
   test result: FAILED. 0 passed; 1 failed
   ```

3. **Make `stdlib_scheme` total.** Change the signature and drop the `Some(...)`
   wrapper + `None` tail:
   - `fn stdlib_scheme(k: StdlibKernel, b: &Builtins) -> Ty` (was `-> Option<Ty>`).
   - The body's outer `Some(match k { ... })` becomes `match k { ... }`.
   - Delete the final `// All other variants → not yet migrated ... _ => return None,`
     arm. Every variant now has an explicit arm (guaranteed by E0 = green).
   - The match is now exhaustive and wildcard-free; the compiler enforces it.

4. **Delete legacy `kernel_ty`** entirely (the whole `fn kernel_ty(&self,
   module: Symbol, name: Symbol) -> Ty` including its `_ => Ty::Var(u32::MAX)`).

5. **Simplify `constrain_var_kernel`.** After E0/E2 the only paths are:
   - Math.min/max Ord early-return (keep; still keyed on module/name here — E3
     re-keys it off the id).
   - Dict/Set `key_obligation` (keep; E3 re-keys off `decl().qualifier`).
   - `stdlib_scheme(k)` → `self.instantiate(&ty)`.
   Delete the fail-closed block:
   ```rust
   // DELETE — kernel_ty and its Ty::Var(u32::MAX) are gone; a miss is impossible.
   if id.is_some() { if let Ty::Var(n) = &ty { if *n == u32::MAX { return Err(...); } } }
   ```
   and the `let ty = self.kernel_ty(module, name);` step. The `id` is now
   always resolvable (E0). Any call reaching this fn without a scheme is a
   compiler-invariant break, not a user feature gap → if a residual guard is
   wanted, use `Diagnostic::CompilerBug`, **not** IPE-L0108 and **not**
   `Ty::Var`.

6. **Retire superseded tests.** Delete
   `burndown_none_count_does_not_exceed_phase_c_ceiling` and the `None`/`Some`
   arms of `new_string_kernels_are_schemed` (its `Ty::Var(u32::MAX)` match arm
   no longer type-checks — `stdlib_scheme` returns `Ty`, not `Option<Ty>`).
   Keep the per-family parity tests **only** if the legacy oracle they call
   (`legacy_ty` → old `kernel_ty` shape) still stands alone in the test module;
   if `legacy_ty` referenced the now-deleted `kernel_ty`, delete `legacy_ty`
   and those tests — their guarantee (byte-faithful relocation) was already
   discharged when each family landed in Phase D. `E0::stdlib_scheme_is_total`
   subsumes the burndown.

7. **Add `arity_matches_scheme`** (Q6 secondary #4) — pins `decl().arity`
   against the scheme's arrow spine and doubles as the zero-arity classifier
   pin (`Uuid.v4 : String` arity 0 vs `Time.now : () -> Task` arity 1 differ in
   the scheme head, not a count):

   ```rust
   #[test]
   fn arity_matches_scheme() {
       let (_interner, b) = make_builtins();
       fn arrow_arity(t: &Ty) -> u8 {
           match t { Ty::Fun(_, r) => 1 + arrow_arity(r), _ => 0 }
       }
       for &k in StdlibKernel::ALL {
           let ty = stdlib_scheme(k, &b);
           assert_eq!(
               k.decl().arity, arrow_arity(&ty),
               "arity drift: decl({k:?}).arity = {} but scheme arrow-arity = {}",
               k.decl().arity, arrow_arity(&ty),
           );
       }
   }
   ```

   > If a family stores arity that legitimately differs from the top-level
   > arrow count (none known today — String/List/Math all match), pin the
   > exception explicitly with a comment; do not weaken the assert.

8. Build + run the whole crate's kernel tests:
   ```
   cargo test -p sky_types
   ```
   Expected:
   ```
   test constrain::stdlib_scheme_tests::stdlib_scheme_is_total ... ok
   test constrain::stdlib_scheme_tests::no_ty_var_max_sentinel ... ok
   test constrain::stdlib_scheme_tests::arity_matches_scheme ... ok
   test result: ok. N passed; 0 failed
   ```
   And confirm no wildcard survives — a deliberate probe: temporarily add a new
   dummy variant to `StdlibKernel` and confirm `cargo build -p sky_types` fails
   with `E0004 non-exhaustive patterns: ... not covered` in `stdlib_scheme`;
   revert the probe. (This is the compile-time gate replacing the runtime test.)

9. Commit:
   ```
   git add crates/sky_types/src/constrain.rs
   git commit -m "Phase E: make stdlib_scheme total; delete kernel_ty + Ty::Var(u32::MAX) fallback"
   ```

---

## Task E3 — Drop `module`/`name` (and `Option`) from `VarKernel`

Deliverable 2. Collapse the kernel node to `VarKernel { id: StdlibKernel }`;
delete the legacy `lower_callee` string table and IPE-L0108 kernel tail; re-key
the constrain obligations off the id. Sound because E1 (subset gate) +
`no_colliding` (injectivity) + `some_node_decl_equals_node` (redundancy) all
pass.

**Files**
- `crates/sky_canon/src/ast.rs` (`Expr_::VarKernel`)
- `crates/sky_canon/src/env.rs` (`VarHome::Kernel`, `install_prelude_qualifiers`,
  `install_builtin_vars`)
- `crates/sky_canon/src/resolve.rs` (two producers)
- `crates/sky_canon/src/lib.rs` (`canon_equals_registry` G1 reverse + subset loop)
- `crates/sky_types/src/constrain.rs` (`constrain_var_kernel`, VarKernel arm)
- `crates/sky_lower/src/lower.rs` (`lower_callee` VarKernel arm; delete legacy
  table; delete `decl_equiv_legacy_match` test)

**Interfaces (post-E3 exact signatures)**
- `sky_canon::ast::Expr_::VarKernel { id: StdlibKernel }`
- `sky_canon::env::VarHome::Kernel(StdlibKernel)`
- `sky_canon::resolve`: `Ok(canon::Expr_::VarKernel { id: *id })`
- `sky_types::constrain`: `fn constrain_var_kernel(&mut self, id: StdlibKernel, span: Span) -> DResult<VarId>`
- `sky_lower::lower`: VarKernel arm ⇒ `Ok(Callee::Kernel(*id))` (no string match)

**Steps**

1. **Failing test first — node shape.** In `sky_canon/src/lib.rs` tests, add a
   compile-level expectation that `VarHome::Kernel` takes exactly one field:

   ```rust
   #[test]
   fn var_home_kernel_is_id_only() {
       use crate::env::VarHome;
       use sky_kernels::StdlibKernel;
       // If VarHome::Kernel still carries symbols this line fails to compile.
       let _h = VarHome::Kernel(StdlibKernel::StringLength);
       assert!(matches!(_h, VarHome::Kernel(StdlibKernel::StringLength)));
   }
   ```
   Run — **expected COMPILE ERROR** (arm still has 3 fields):
   ```
   cargo test -p sky_canon var_home_kernel_is_id_only
   ```
   ```
   error[E0023]: this enum variant takes 3 arguments but 1 argument was supplied
   ```

2. **`env.rs`:** change `Kernel(Option<StdlibKernel>, Symbol, Symbol)` →
   `Kernel(StdlibKernel)`. In `install_prelude_qualifiers`, the two/three
   `VarHome::Kernel(id, qual_sym, canonical_sym)` inserts become
   `VarHome::Kernel(id.expect("subset gate (E1) guarantees id"))`. Prefer an
   explicit fail-closed unwrap over silent drop:
   ```rust
   let id = self.stdlib_index.get(&(qual_sym, canonical_sym)).copied()
       .ok_or_else(|| /* CompilerBug: canon lists a kernel the registry lacks */ ...)?;
   module.insert(canonical_sym, VarHome::Kernel(id));
   ```
   In `install_builtin_vars` (Basics), any kernel install must likewise resolve
   to an id; if a Basics helper legitimately has no `decl()` (e.g. an alias
   mapping to `MathMin`), give it that canonical id via `stdlib_index` — E1's
   green state confirms none are left `None`. `stdlib_index` itself is retained
   (canon still needs `(Symbol,Symbol) → StdlibKernel` for resolution).

3. **`ast.rs`:** `Expr_::VarKernel { id: Option<StdlibKernel>, module, name }` →
   `Expr_::VarKernel { id: StdlibKernel }`. Update the doc comment: module/name
   are reconstructable via `id.decl()`; the node no longer stores them.

4. **`resolve.rs`:** both producers become
   ```rust
   Some(VarHome::Kernel(id)) => Ok(canon::Expr_::VarKernel { id: *id }),
   ```

5. **`constrain.rs`:** VarKernel arm ⇒ `self.constrain_var_kernel(*id, span)?`.
   Rewrite `constrain_var_kernel(&mut self, id: StdlibKernel, span: Span)`:
   - Math.min/max Ord early-return keyed on the id:
     ```rust
     if matches!(id, StdlibKernel::MathMin | StdlibKernel::MathMax) {
         let s = self.super_var(TyBounds::ord(), span)?;
         let inner = self.structure(FlatType::Fun(s, s))?;
         return self.structure(FlatType::Fun(s, inner));
     }
     ```
   - Dict/Set `key_obligation` keyed on `id.decl().qualifier` (interned to the
     module symbol) instead of the dropped `module` param — or on explicit id
     variant groups if that reads cleaner. The obligation logic is otherwise
     unchanged; only its *key* moves from the node's symbol to `decl()`.
   - Then `let ty = stdlib_scheme(id, &self.builtins); self.instantiate(&ty)`.

6. **`lower.rs` `lower_callee`:** replace the whole VarKernel arm with:
   ```rust
   canon::Expr_::VarKernel { id } => Ok(Callee::Kernel(*id)),
   ```
   **Delete** the ~399-arm `match (self.resolve(*module)?, self.resolve(*name)?)`
   legacy string table and its `(_, _) => Err(unsupported(callee.span,
   Feature::Kernels))` [IPE-L0108] tail. The `self.resolve(...)` calls in that
   arm vanish with it. **Delete** the `decl_equiv_legacy_match` test (it forces
   `id = None`, a state that no longer exists) and any helper that constructs a
   `VarKernel` with `id: None`.

   > This is the large deletion that overlaps #49. Do it as its own commit so a
   > #49 rebase has a clean anchor.

7. **`lib.rs` gate:** update `canon_equals_registry`'s G1 reverse loop and E1's
   subset loop — `VarHome::Kernel(Some(actual_sk), m, f)` becomes
   `VarHome::Kernel(actual_sk)`. Reconstruct the `stdlib_index` key by interning
   `actual_sk.decl().qualifier` / `.decl().name` (the node no longer carries
   `m`/`f`):
   ```rust
   if let VarHome::Kernel(actual_sk) = home {
       let decl = actual_sk.decl();
       let q = interner.intern(decl.qualifier).expect("intern");
       let n = interner.intern(decl.name).expect("intern");
       assert_eq!(Some(actual_sk), env.stdlib_index.get(&(q, n)), "...");
   }
   ```
   `some_node_decl_equals_node` (E1) becomes trivially true (node = id, decl
   reproduces qualifier/name) — keep it as the anti-regression tripwire; adjust
   its body to no longer read `m`/`f`.

8. Build the whole downstream chain and run tests:
   ```
   cargo build -p sky_canon -p sky_types -p sky_lower
   cargo test  -p sky_canon -p sky_types -p sky_lower
   ```
   Expected: clean build (the compiler now proves every `VarKernel` carries a
   valid id end-to-end) and:
   ```
   test tests::var_home_kernel_is_id_only ... ok
   test tests::canon_equals_registry ... ok
   test tests::some_node_decl_equals_node ... ok
   test result: ok. ...
   ```

9. Full workspace regression (Go-parity + examples unaffected — pure deletion):
   ```
   cargo test --workspace
   ```
   Expected `test result: ok` across all crates; zero behaviour change (no arm
   value changed, only transitional paths removed).

10. Commit as two logical steps:
    ```
    git add crates/sky_canon/src/ast.rs crates/sky_canon/src/env.rs crates/sky_canon/src/resolve.rs crates/sky_canon/src/lib.rs crates/sky_types/src/constrain.rs
    git commit -m "Phase E: collapse VarKernel to { id: StdlibKernel } (drop module/name/Option)"
    git add crates/sky_lower/src/lower.rs
    git commit -m "Phase E: delete lower_callee legacy string table; dispatch purely on kernel id"
    ```

---

## Task E4 — (OPTIONAL, sequence after #49) Retire the `KernelFn` alias

Not required by the three deliverables. Spec §Q5 Phase E mentions "Flip
`KernelId` from alias-backed to final; `KernelFn` alias removed" as the broader
end-state. This is a mechanical rename touching `sky_ir` + `sky_backend_rust`
and **overlaps #49's sky_ir edits** — do it only when both Phase E (E0–E3) and
#49 have landed and the tree is quiescent.

**Files**
- `crates/sky_ir/src/ir.rs` (`type KernelFn = sky_kernels::StdlibKernel;`,
  `Callee::Kernel(KernelFn)`)
- every `KernelFn` reference across `sky_ir`, `sky_lower`, `sky_backend_rust`

**Steps**
1. Replace `Callee::Kernel(KernelFn)` → `Callee::Kernel(sky_kernels::StdlibKernel)`;
   delete `pub type KernelFn = ...`.
2. `rg -n '\bKernelFn\b' crates/` → rewrite each to `StdlibKernel` (import
   `sky_kernels::StdlibKernel`).
3. `cargo build --workspace && cargo test --workspace` → expected clean.
4. Commit:
   ```
   git commit -am "Phase E: remove KernelFn alias; use sky_kernels::StdlibKernel directly"
   ```

---

## Definition of done

- `stdlib_scheme` is `-> Ty`, total, wildcard-free; adding a `StdlibKernel`
  variant fails `cargo build -p sky_types` until an arm exists.
- No `u32::MAX` / `Ty::Var(u32::MAX)` anywhere in `constrain.rs`
  (`no_ty_var_max_sentinel` green).
- `Expr_::VarKernel` and `VarHome::Kernel` carry `StdlibKernel` only — no
  `Option`, no symbols. `lower_callee` has no kernel string table.
- Gates green: `stdlib_scheme_is_total`, `canon_equals_registry` (bidirectional
  on module qualifiers), `some_node_decl_equals_node`, `arity_matches_scheme`,
  `no_colliding_qualifier_name_pairs`.
- `cargo test --workspace` green; Go-parity fixtures unchanged (pure deletion).
- F1 (no-scheme kernel) is unrepresentable; F3 drift is a compile error.
