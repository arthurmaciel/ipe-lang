# Readability & Naming Audit — whole-compiler synthesis

> Status: **FUTURE rework backlog (Priority 6 — readability, the lowest).**
> Verified read-only against HEAD `aa8f638`. No code was edited to produce
> this document. Every item below is a *proposal*, not an applied change.

## 1. Framing

Sky's operating principle order is fixed:

> **security > correctness > soundness > efficiency > completeness > readability**

Readability/naming is **P6 — the lowest**. Two design rules sit *above* it and
are the reason several items in this audit are more than cosmetic:

- **PARSE, DON'T VALIDATE** — untrusted/untyped input is parsed once at the
  boundary into a typed value; downstream code cannot reintroduce the failure.
- **MAKE INVALID STATES UNREPRESENTABLE** — if a value can be constructed in a
  state the code must reject, that is a latent correctness/soundness bug, not a
  style nit.

**Prime constraint:** *a readability or naming change must NEVER hurt a higher
principle.* A rename that silently breaks a golden oracle, a cross-crate
contract, a stringly kernel-dispatch key, or an error-code wire form is a
correctness/soundness regression wearing a cosmetic hat. Every proposed rename
below is therefore **RISK-FLAGGED**:

| Flag | Meaning |
|---|---|
| **SAFE-MECHANICAL** | Crate-local identifier, no external contract, no serialized/golden-observable string. `rustc` proves the rename total. |
| **CONTRACT-TOUCHING** | Crosses a crate's public API, OR is a stringly key (kernel qualifier `String`/`Db.Decode`/`idiv`, error code `IPE-L0119`, runtime symbol name, JSON wire field) OR is otherwise golden-oracle-observable. Can silently break correctness/soundness — **coordinate-with-tests**, do not rename in isolation. |

**Coordination / anti-double-churn:** this backlog is intended to ride the
**Tier-3 mechanical passes already scheduled** — the `Sky → Ipê` rename (task
**#59**) and the post-parity **simplification sweep**. Doing these renames as a
separate pass would touch the same files twice. The naming work should be folded
into #59 so the tree is re-tokenised once. **Until the compiler reaches Go
parity and the exit-0 seal is in place, none of the P6 items below should be
started** — they are the last mechanical polish, not release-blocking.

**Exception — the higher-principle items in §4 are NOT P6.** Several findings
surfaced while reading are genuine rule-1/rule-2/fail-closed correctness issues
that merely *manifest* as a bad name. Those deserve their own before-sweep tasks
and must not wait on the cosmetic pass.

---

## 2. Main table — sorted name-truthfulness + higher-principle FIRST, cosmetic de-abbrev LAST

Legend — Lens: `HP` higher-principle · `NT` name-truthfulness · `DA` de-abbreviation.

| # | Location | Current | Proposed | Lens | Principle | Risk | Rationale |
|---|---|---|---|---|---|---|---|
| **H1** | `sky_types/constrain.rs:5123` (def), consumed `:1512`,`:2708`,`:2869`,`:4646`,`:4752` | `Ty::Var(u32::MAX)` sentinel = "unbacked kernel / hole" | `kernel_ty -> Option<Ty>`; absence = `None` | HP | soundness/correctness (representable-invalid-state; exit-0-then-cargo-fail class) | SAFE-MECHANICAL but **coordinate — in-flight (task #45)**; gates `first_schemed_were_holes`/`both_miss_is_fail_closed` assert on it | A real domain ctor (a type *variable*) is overloaded with a magic id to mean "no scheme". Flagship invalid-state of the crate. |
| **F1** | `sky_lower/lower.rs:3213` (`callee_arity`, Kernel arms) vs `sky_kernels/lib.rs:555` (`decl().arity`) | Hand-maintained ~400-line per-kernel arity `match`, parallel to `decl().arity` | `Callee::Kernel(k) => Ok(k.decl().arity as usize)` | HP | correctness (two hand-maintained tables **already disagree** — drift class of task #58) | **CONTRACT-TOUCHING** (changes saturation/eta decisions); add `callee_arity(k)==decl().arity` tripwire | Verified drift: `decl()`=0 for `TimeNow`/`TimeUnixMillis`/`SystemArgs`/`SystemCwd`/`IoReadLine`, but `callee_arity`=1; `RandomFloat` `decl()` vs `callee_arity`=2. Latent exit-0-then-cargo-fail. |
| **F2** | `sky_kernels/lib.rs:846-847` (`UuidV4/V7`→arity **1**) vs `:874,876,878` (`TimeNow`/`TimeUnixMillis`/`SystemArgs`→arity **0**) | `decl().arity` counts the unit arg inconsistently for `() -> Task a` kernels | Pick ONE rule for `() -> Task a`; document it on the `arity` field | NT | correctness (precondition for F1; scheme "arrow-count == decl().arity" tripwire keys off this) | **CONTRACT-TOUCHING** (golden/scheme tripwire) | Confirmed at HEAD: `Uuid.v4 => d(..,1,..)`, `Time.now => d(..,0,..)`. The coexisting conventions are *what force* F1 to diverge. |
| **H2** | `sky_types/unify.rs:45` `super_concrete_ok` vs `lib.rs:286` `concrete_super_ok` (+ `lib.rs:258` `emitted_bound_satisfied`) | Two near-anagram names for a soundness pair doing *different* jobs (head pin-check vs deep resolved-check) | `head_pin_satisfies_bounds` / `resolved_ty_satisfies_bounds` / `generic_use_satisfies_bounds` | NT | correctness (editing the wrong one of a soundness pair is a live hazard) | SAFE-MECHANICAL (`super_concrete_ok` private; `concrete_super_ok` `pub(crate)`, single caller `unify.rs:171`) | The transposition actively defeats the reader's ability to tell head-check from deep-check. |
| **H1d** *(diagnostics)* | `sky_diagnostics/code.rs:25` `struct Code(&'static str)`; wildcards `code.rs:307`,`:400` | Stringly newtype forces wildcard `_ =>` arms in `title`/`explain_page` | `enum Code { P0001, … }` with exhaustive `as_str()`/`title()`/`explain_page()` | HP | correctness/soundness (rule-2; project "no `_ ->` catchall" rule) | **CONTRACT-TOUCHING** — `as_str()` wire form `"IPE-T0001"` is `skyc explain`/golden-observable, byte-preserve | Newtype gives forgeability protection but *not* exhaustiveness. Enum makes "code without title/page" a compile error and kills H2d at the root. |
| **H2d** *(skyc)* | `skyc/lib.rs:41` `ALL_CODES` vs authoritative `code.rs:409` `ALL` (`#[cfg(test)]`-only) | Second hand-maintained taxonomy copy; **has drifted** | Promote `pub const ALL: &[Code]` from diagnostics; skyc consumes it; delete local copy | HP | correctness (live user-visible bug) | **CONTRACT-TOUCHING** — restores 8 codes to `explain`/index output | **Verified drift at HEAD:** `ALL_CODES` omits `IPE-P0016,P0017,T0014,L0114,L0115,L0116,L0117,L0119` — all have `title` arms *and* `include_str!` pages. `skyc explain IPE-L0117` currently returns `UnknownCode`. |
| **A1** *(runtime)* | `runtime/.../live/diff.rs:8` `Patch.attrs: HashMap<String,String>` | Empty-string value overloaded: "set attr to `\"\"`" vs "remove attr" are byte-identical | in-Rust `enum AttrDelta { Set(String), Remove }` + hand-written `Serialize` emitting identical wire bytes | HP | correctness (rule-2; `value=""` vs remove) | **CONTRACT-TOUCHING** (JSON wire consumed by `live/client.js` + Go-parity golden) — freeze wire, change only in-Rust repr | Representable-but-invalid state inherited from Go parity. Latent live-diff correctness hole. |
| **N2** | `sky_types/constrain.rs:1269` `normalize_annotation_ty` | Named "normalize" but also *rejects* (IPE-T0001) non-`Error` Task channel | `normalize_and_check_annotation` (or document the reject in the header) | NT | correctness (a parse-don't-validate boundary; name hides the validate half) | SAFE-MECHANICAL (private) | Under-informs: reader expects a total normalizer, not a fallible parser. |
| **N1d** | `sky_diagnostics/code.rs:224`,`311` doc | Doc claims `title`/`explain_page` "Total over the taxonomy" — the signature cannot enforce it | Soften doc to name the wildcard as a drift-risk fallback, OR fix via H1d | NT | correctness (doc vs behaviour) | SAFE-MECHANICAL (doc-only) | Until H1d lands the "total" claim is false; a reader trusts it. |
| **F5** | `sky_ir/ir.rs:844` `pub type KernelFn = sky_kernels::StdlibKernel;` | Alias reads as "a function value"; it is a closed tag enum | (eventual) `StdlibKernel`/`Kernel` | NT | readability | **CONTRACT-TOUCHING** (every `KernelFn::` call-site across crates) — batch with Phase-B migration | Alias intentionally keeps hundreds of call-sites compiling during Phase A→B; flag, don't do standalone. |
| **F7** | `sky_lower/lower.rs:976` `types: &'a SolvedTypes` | Reads as a type list; holds solver output | `solved` / `solved_types` | NT | readability | SAFE-MECHANICAL (crate-local field) | — |
| **N3** | `sky_types/solve.rs:31` `Constraint { lhs, rhs }` | Positional-neutral fields actually encode found(`lhs`)/expected(`rhs`) blame polarity | `found` / `expected` | NT | readability (polarity is load-bearing for diagnostics) | SAFE-MECHANICAL (crate-internal; `Constraint` `pub` but consumed in-crate) | Aligns with `unify.rs:355` mismatch builder convention. |
| **P1** *(parse)* | `sky_parse/lexer.rs:24` `enum Tok`, `:115` `Token.kind: Tok`; vs `sky_diagnostics::TokenKind` | `Tok`/`TokenKind` near-homonyms; `token.kind: Tok` contradicts the existence of `TokenKind` | `Tok` → `Lexeme` (and `Token.kind` → `Token.lexeme`) | NT | readability | SAFE-MECHANICAL (`Tok` crate-local; zero `Tok::` use outside `sky_parse`). **Do NOT touch `sky_diagnostics::TokenKind`** | Highest-value clarity win in the parse area; the one genuinely misleading name. |
| **C1** *(canon)* | `sky_canon/resolve.rs:1604` `fn name_zero() -> Symbol` = `Symbol::from_raw(0)` | Name claims "empty-string symbol"; body mints an unchecked raw sentinel on "unreachable" paths | `unreachable_placeholder_symbol` (or return `Diagnostic::CompilerBug` at the two sites) | NT (+ latent HP) | readability + soundness (P3) latent | SAFE-MECHANICAL (crate-local private) | Both callers are `unwrap_or_else(name_zero)` on "cannot occur in grammar" branches; a minted `Symbol(0)` later `resolve()`s to `None`. Fail-closed would be more honest. |
| **F3** | `sky_ir/pretty.rs:206-659` `kernel_name` | Third hand-maintained per-kernel string table duplicating `decl().qualifier`+`.name` | `let d = k.decl(); format!("{}.{}", d.qualifier, d.name)` | HP (SSOT) | correctness (low — debug pretty-printer) | SAFE-MECHANICAL unless pretty output is a golden — **verify first** | Collapses the third arity/name table onto `decl()`. |
| **N4d** *(runtime)* | `runtime/.../http_header.rs:28` `fn canonical_header(k)` | Canonicalizes a header *name*, not a header | `canonicalize_header_name` | NT | readability | SAFE-MECHANICAL (`pub(crate)`, 2 call-sites) | Very low value; doc already disambiguates. |
| **N1** | `sky_types/constrain.rs:560` `Builder`, `:680` `Generated`, `:699` `run` | Names the pattern, not the job (HM constraint generation) | `ConstraintGenerator` / `GeneratedConstraints` / `generate` | DA/NT | readability | SAFE-MECHANICAL (`pub` inside private `mod constrain`; consumer `lib.rs:45,101`) | `Builder::run` cannot predict "generates HM equality constraints". |
| **C2** *(canon)* | `sky_canon/env.rs:28` `VarHome::Kernel(Option<StdlibKernel>, Symbol, Symbol)` destructured `(id,m,f)` | Positional tuple; asymmetric with the *named-field* `canon::Expr_::VarKernel { id, module, name }` | struct variant `Kernel { id, module, name }` | DA/NT | readability (+ names the `id:None`=legacy-fallback invariant) | SAFE-MECHANICAL (`VarHome` crate-internal to `sky_canon`) | Two representations of the same concept disagree; eliminates `m`/`f`. |
| **C3** *(canon)* | `sky_canon/env.rs:59` `stdlib_index: BTreeMap<(Symbol,Symbol),StdlibKernel>` | "index" is generic | `kernel_by_qualified_name` | DA/NT | readability | SAFE-MECHANICAL (crate-internal field; in-crate tripwire only) | States key shape + lookup direction. |
| **P2** *(parse)* | `sky_parse/parser.rs` ~20 sigs (`parse_expr:984`, `parse_type:801`, …) `threshold: u32` | Bare magnitude word; means "layout minimum column" (offside rule) | `min_column` (or `block_col`) | NT/DA | readability | SAFE-MECHANICAL (all crate-local) | Layout-sensitive parsing is the subtlest part; name the column as a column. |
| **F6** | `sky_lower/lower.rs:975` `m: &'a canon::Module` | Single-letter field for the canonical module | `module` | DA | readability | SAFE-MECHANICAL (6 sites) | — |
| **D1** | `sky_types/ty.rs:19-43` `Ty`, `Ty::Var`, `Ty::Fun`, `Ty::Con` | Flagship abbreviations | `Type`, `Type::Variable`, `Type::Function`, `Type::Constructor`/`App` | DA | readability | **CONTRACT-TOUCHING** (`pub use ty::{Ty,TyBounds}` `lib.rs:43`; lowerer/backend read `Ty`). Not golden-observable (never serialized). Coordinate downstream + `rustc` | See §5. `Con`/`FlatType`/`Content` are deliberate 1:1 mirrors of elm/GHC — port-fidelity vs newcomer clarity is an **OPEN** crate-wide judgement. |
| **D2** | `sky_types/ty.rs:183-213` `Content::Flex`, `Content::Super{..}` | `Super` reads as the keyword; `Flex` outlier | `Content::Flexible`, `Content::SuperTyped{..}` | DA/NT | readability | SAFE-MECHANICAL (`Content`/`FlatType` crate-private, never `pub use`) | `Rigid`/`Structure` already spelled out. |
| **D3** | `sky_types/ty.rs:65` `TyBounds` (re-exported `lib.rs:43`); doc calls contents "obligations" | Abbrev + doc/name mismatch | `TypeBounds` (DA) or `Obligations`/`SuperBounds` (NT) | DA/NT | readability | **CONTRACT-TOUCHING** (cross-crate; lowerer emits trait-bounds) | Bundle with D1. |
| **D4** | `sky_types/constrain.rs:5169` `zonk`, `:5133` `ZonkTask`, `:5282` `zonk_underflow` | GHC jargon | `read_back` / `ReadBackTask` / `read_back_underflow` (or `resolve_var`) | DA | readability | SAFE-MECHANICAL (crate-internal; `pub fn` in private `mod constrain`) | Module doc has to gloss it (`constrain.rs:14`). Same port-fidelity **OPEN** call as D1. |
| **DA1** *(skyc)* | `skyc/lib.rs:864` `let a = line.trim()` | Trimmed yes/no reply | `answer` | DA | readability | SAFE-MECHANICAL (fn-local) | — |
| **DA2** *(skyc)* | `skyc/lib.rs:881` `let tmp = …` | Sibling temp-file `PathBuf`, not scratch | `temp_path` | DA | readability | SAFE-MECHANICAL (fn-local) | — |

---

## 3. SAFE-MECHANICAL vs CONTRACT-TOUCHING split (counts)

**SAFE-MECHANICAL (17):** H2, N2, N1d, F7, N3, P1, C1, F3, N4d, N1, C2, C3, P2, F6, D2, D4, DA1, DA2 — *(18 listed; H1 is mechanical-but-in-flight, see below)*.

**CONTRACT-TOUCHING (9):** F1, F2, H1d, H2d, A1, F5, D1, D3 + the frozen stringly-key set in §3.1.

**Special — mechanical shape but must coordinate (2):** **H1** (`Ty::Var(u32::MAX)` → `Option<Ty>`; in-flight task #45, gates assert on it) and **F3** (verify pretty output is not a golden before editing).

> Precise tally: **18 SAFE-MECHANICAL**, **8 CONTRACT-TOUCHING renames**, plus
> **1 frozen-do-not-touch contract set** (§3.1) and **H1** in-flight.

### 3.1 Frozen stringly keys — DO NOT rename (record so future renamers know)

These are intentionally stringly *at their layer* and are the compiler's
cross-crate dispatch contract. Renaming a value here is a silent
correctness/soundness break (an unmatched arm falls through to the
`Ty::Var(u32::MAX)` fail-closed hole → skyc-OK then cargo-fail, the
exit-0-then-cargo-fail class).

- **Kernel `(qualifier, name)` primary key** — `sky_kernels/lib.rs:55-71`,
  consumed by `decl()` and the `no_colliding_qualifier_name_pairs` tripwire
  (`:1849`); mirrored in the canon `QUALIFIERS` table. Values `"String"`,
  `"Db.Decode"`, `"_internal_"`, `"PubSub"`, the `emit` symbols `"tui_app"` /
  `"tui_app_ui"` — **frozen** (canon-, runtime-symbol-, and E2E-golden-observable).
  The intentional `TuiProgram→"tui_app"` / `TuiApp→"tui_app_ui"` asymmetry
  (`:1114-1115`) is correct-but-surprising; the existing comment is adequate.
- **`sky_canon/env.rs` `qual_vars` / `QUALIFIERS` / `FUNC_ALIASES` /
  `QUALIFIER_ALIASES`** (`:51`, `install_prelude_qualifiers` `:202`–`:1010`),
  plus `resolve_op_func` (`resolve.rs:1368`) and `op_precedence` (`:1211`)
  operator spellings (`"idiv"`, `"fdiv"`, …). Must byte-match `lower.rs` arms and
  `constrain.rs` `kernel_ty`. **Frozen.** (The *field* name `qual_vars` is
  accurate; a `type QualVarTable = …` alias is optional low-value SAFE-MECHANICAL.)
- **Error-code wire forms** — `Code::as_str()` → `"IPE-T0001"` etc. are
  `skyc explain`/golden-observable. Any H1d/H2d work must byte-preserve them.
- **Runtime Sky-facing kernel names** — `list_foldr`, `sky_list_cons`,
  `dict_union`, `strip_style_close`, `canonical_header`, `render_html`, … are
  emitted-by-name and Go-wire-parity-observable. **Frozen.**
- **Cross-crate `StdlibKernel`** — used at `constrain.rs:1426,1459,1477` but
  owned by `sky_kernels`; any rename is contract-touching against that crate.

---

## 4. Higher-principle spots found while reading (candidate before-sweep tasks, NOT P6)

These are rule-1 / rule-2 / fail-closed observations that outrank the naming
backlog. Each should be considered for its own task, independent of the Tier-3
cosmetic pass.

1. **H1 — `Ty::Var(u32::MAX)` unbacked-kernel sentinel** *(in-flight, task #45).*
   A domain constructor overloaded with a magic id to mean "hole". Represent
   absence as `Option<Ty>` end-to-end (`legacy_kernel_ty` `:1544` began this).
   Verified present at HEAD (`constrain.rs:5123`, gates at `:5615`+).

2. **F1/F2 — arity SSOT drift** *(live latent bug).* `callee_arity`
   (`lower.rs:3213`) contradicts `decl().arity` for six kernels
   (`TimeNow`/`TimeUnixMillis`/`SystemArgs`/`SystemCwd`/`IoReadLine`,
   `RandomFloat`). **Reconcile F2 first** (pick one `() -> Task a` arity
   convention, document it), **then collapse F1/F3 onto `decl()` behind a
   `callee_arity(k)==decl().arity` tripwire.** This is exit-0-then-cargo-fail
   class — the disagreement is the finding, not the duplication.

3. **H1d/H2d — `Code`-as-string forces non-exhaustive dispatch + a drifted
   duplicate taxonomy** *(H2d is a live user-visible `explain` bug).* Verified:
   `skyc/lib.rs:41 ALL_CODES` omits 8 real page-backed codes
   (`IPE-P0016,P0017,T0014,L0114,L0115,L0116,L0117,L0119`), so
   `skyc explain IPE-L0117` returns `UnknownCode` today. **H2d must be fixed on
   its own even if H1d (enum-ify `Code`) is deferred.** Making `Code` an enum
   structurally eliminates the wildcard mislabel (N1d) and the duplication in one
   move — highest single-fix leverage in the diagnostics/skyc area.

4. **A1 — `Patch.attrs` empty-string overload** *(rule-2, contract-touching).*
   `live/diff.rs:8` cannot distinguish `value=""` from attribute-removal on the
   wire. Introduce an internal `enum AttrDelta { Set, Remove }` with a
   hand-written `Serialize` that emits byte-identical wire output — coordinate
   with `live/client.js` + the Go-parity golden; **do not change the wire
   encoding.**

5. **C1 — `name_zero()` mints an unchecked `Symbol(0)` on "unreachable" paths**
   *(latent soundness, P3).* `resolve.rs:1604`. The two callers are
   `unwrap_or_else(name_zero)` on grammar-impossible branches; a fail-closed
   `Diagnostic::CompilerBug` is more honest than a sentinel a later stage will
   `resolve()` to `None`. Worth doing when the fn is next touched.

**Verified-clean, recorded for the record (no action):**
runtime `css_safety.rs` / `html.rs` / `core.rs` are model parse-don't-validate
types (`SafeCssValue`, `SafeAttrName`, sole-constructor smart types); `sky_ir`
IR nodes enforce invalid-states-unrepresentable structurally (`Match` private
fields + `Match::new`/`new_flat`, `BoundSet` bitset); parse/canon/intern error
paths are total `DResult`; emit dispatch (`naming.rs::kernel_name`,
`emit_expr.rs`) is exhaustive with no `_ =>` catchalls and routes `IntDiv →
sky_int_div` (panic-soundness). No security/soundness *naming* defects outside
the five spots above.

---

## 5. Flagship de-abbreviation set (worked example — one coordinated pass, OPEN decision)

The `sky_types` type-representation vocabulary is the user's headline example.
Treat it as a **single crate-wide pass**, not piecemeal, and only inside the
#59 rename so the tree is re-tokenised once:

| Current | Proposed | Risk |
|---|---|---|
| `Ty` | `Type` | CONTRACT-TOUCHING (`pub use`, downstream crates) |
| `Ty::Var` | `Type::Variable` | CONTRACT-TOUCHING |
| `Ty::Fun` | `Type::Function` | CONTRACT-TOUCHING |
| `Ty::Con` | `Type::Constructor` (or `Type::App`) | CONTRACT-TOUCHING |
| `TyBounds` | `TypeBounds` | CONTRACT-TOUCHING (lowerer trait-bound emission) |
| `Content::Flex` | `Content::Flexible` | SAFE-MECHANICAL (crate-private) |
| `Content::Super` | `Content::SuperTyped` | SAFE-MECHANICAL |
| `zonk` / `ZonkTask` | `read_back` / `ReadBackTask` | SAFE-MECHANICAL |

None of these are golden-oracle-observable (they are Rust types/functions, never
serialized), so the risk is purely `rustc` + downstream-crate compilation — a
mechanical rename `rustc` proves total.

> **OPEN DECISION (must be made crate-wide, once):** `Con` / `FlatType` /
> `Content` / `zonk` are *deliberate* 1:1 mirrors of elm/compiler + the GHC
> `FlatType`/`Content` vocabulary (module docs `ty.rs:6-9`, `:216`;
> `constrain.rs:14`). De-abbreviating trades **port-fidelity** (easy diffing
> against the Haskell/elm reference during the ongoing port) for
> **newcomer clarity**. Decide D1+D3+D4 together and consistently — do not
> half-abbreviate. Recommended: **defer until after Go parity + the port is no
> longer diffed line-for-line against the reference**, then apply as one pass
> inside #59.

**Explicitly KEEP (de-abbreviating = pure churn, verified conventional):**
`uf`/`UnionFind` internals (`ra`/`rb`/`winner`/`loser`), `unify.rs` HM locals
(`fa`/`fb`/`ca`/`cb`, `occurs`/`occurs_guard`, `instantiate*`), parser vocab
(`lx`/`tok`/`bump`/`peek`/`lo`/`hi`/`depth`), AST `P`-/`T`-prefixed variants
(`PCtor`/`TLambda`), `home`/`VarHome`/`CtorHome`, runtime parity-traced
`fmt_g_exponent`/`dp`/`nd` (mirror Go `fmtE`/`fmtF`), `submgr`/`tx`/`rx`/`om`/`nm`,
`cmp_total`/`sort_by_total`/`str_err`/`ok_res`.

---

## 6. Coordination note

- **Fold into task #59 (`Sky → Ipê` rename)** and the **post-parity
  simplification sweep** — both Tier 3, both re-tokenise the whole tree. Running
  the P6 renames as a separate pass doubles the churn on every file.
- **Do not start any P6 item before the compiler reaches Go parity + the exit-0
  seal.** These are last-mile polish.
- **The §4 higher-principle items are exempt** from the "wait for Tier 3" rule —
  H2d is a live bug, F1/F2 is latent exit-0-then-cargo-fail, H1 is already
  in-flight. File them independently.
- **Every CONTRACT-TOUCHING rename must land with its tripwire/golden update in
  the same commit.** In particular: F1 needs the `callee_arity==decl().arity`
  tripwire; F2 needs the arrow-count/scheme tripwire re-baselined; H1d/H2d must
  byte-preserve `Code::as_str()`; A1 must byte-preserve the live-diff wire.
