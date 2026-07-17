# Efficiency Audit — 2026-07-02

Merged ledger from six performance-audit partitions across the Rust backend:
the type solver (`sky_types`), the Sky→IR lowerer (`sky_lower`), the Rust
emitter (`sky_backend_rust`), the parser/canonicaliser/interner
(`sky_parse` / `sky_canon` / `sky_intern`), the runtime hot paths
(`runtime/src/sky_runtime`), and a data-structure cross-cut (interner /
module-path / solver / collection representation).

Principle order (strict tie-breaker, from `PRINCIPLES.md`):
1 Security · 2 Correctness · 3 Soundness · 4 Efficiency · 5 Completeness ·
6 Readability.

**Efficiency is principle 4.** Every finding below is pursued *only* within the
bounds of principles 1–3. A faster path that weakens a security gate, changes
an observable result (Go-parity / determinism), or opens a soundness / panic
hole is **REJECTED**, not reported as a win — see the dedicated REJECTED
section. Each actionable finding carries a `safety_note` confirming byte-identical
output vs the reference behaviour, no new panic path, and no security
regression.

**These fixes are deferred.** They rank below the in-flight higher-priority work
(exit-0 kernel-scheme class, IR/type shapes, Go-parity closure). None should be
applied blind: each is a candidate to be **measured with a benchmark before and
after** — the impact ratings here are static-analysis estimates, not measured
speedups. Land them behind the existing golden/equivalence suites so the
byte-identical claim is proven, not assumed.

All findings are stated as "the code currently…" facts against HEAD at audit
time; line numbers are approximate and should be re-confirmed before editing.

---

## 0. Burn-down status (2026-07-11)

The strictly-safe ledger burn-down landed as one batch (all gates green:
workspace nextest 1979/1979, runtime `--features live` nextest 771/771, doc
tests, clippy `-D warnings` on both feature sets — the golden/equivalence
suites are the byte-identical proof the header demands).

**LANDED (20 of 27):** every high (3/3), every medium except one (12/13), and
4 lows —

- §2 solver: `Rc<CtorScheme>` ctor table · `Rc<Ty>` top-level schemes (≡ §7
  cross-cut #1) · `root_content` by-ref peek for field accesses AND record
  updates · `unify` 6→2 union-find traversals (`equivalent` now
  `#[cfg(test)]`).
- §3 lowerer: nine kernel-family walks → ONE `KernelUsage` traversal
  (early-exit) · `collect_records_in_ty` HashSet dedup (`IrType: Hash`) ·
  `lower_callee` peek threaded through `Intercepted::Fallthrough` · positional
  `FuncId` in `lower_def` (drops the per-def `Vec<Symbol>` key).
- §4 emitter: single `Callee::Kernel` gate before the 8 dispatch probes ·
  `emit_program` capacity hint (`GOLDEN.len() + 4096`).
- §5 canon/interner/parser: `Env` kernel/ctor/qual/wildcard/stdlib tables
  behind `Rc` + `Rc::make_mut` setup writes · interner single `Arc<str>` per
  unique string (≡ §7) · qualified-ident `rfind('.')` slice split.
- §6 runtime: single-pass HTML escaper (`escape_html`, metachar-free fast
  path) · borrowed `&str` attr-diff maps · borrowed sky-id in `diff_node` ·
  `render_children` shared accumulator via `pub(crate) render_into` ·
  `build_style_string` single buffer with running `;`.
- §2 low `kernel_ty` eager `task_unit`: found ALREADY closed at HEAD
  (`let task_unit = || Ty::Con { … }` — the closure form this row asked for).

**RESIDUAL (7 of 27) — each gated by the audit's own safety analysis, kept as
a narrowed BACKLOG row:**

- §7 medium `ModPathId` module-path interning — determinism caveat (mandatory
  `Ord`-by-resolved-path guard vs Go-parity emission order).
- §2 low scope-entry persistent map — explicitly profile-gated ("not applied
  blind"), adds a dep.
- §3 low `lower_callee` Symbol-keyed kernel table — logged
  low-priority-not-rejected (§8 note), broad mechanical diff.
- §4 low Http field-name `&'static` consts — small, safe; batched with the
  fieldset-keying row below since both touch the same lookup helper.
- §4 low `record_by_fieldset` interned keying — conditionally safe (a
  mismatched key silently resolves the wrong struct).
- §5 low lexer streaming — gated behind a byte-for-byte lexer golden suite.
- §6 low `SafeCssValue::parse` lazy buffers — security-gated (must re-prove
  the exact match set; security > efficiency).

---

## 1. Roll-up (class × impact)

Actionable findings only (the four REJECTED entries are excluded and listed
separately). Two physically-identical findings surfaced in two partitions each
— the typed top-level-reference clone (`constrain.rs:1382`, solver F4 ≡
cross-cut #1) and the interner double-allocation (`sky_intern/src/lib.rs:59-60`,
parser ≡ cross-cut) — and are counted once.

| Class                    | High | Medium | Low | Total |
|--------------------------|:----:|:------:|:---:|:-----:|
| needless-clone-alloc     |  2   |   6    |  4  |  12   |
| string-rebuild           |  1   |   2    |  2  |   5   |
| data-structure           |  0   |   1    |  3  |   4   |
| recompute-in-loop        |  0   |   2    |  1  |   3   |
| algorithmic-complexity   |  0   |   1    |  0  |   1   |
| linear-scan-vs-map       |  0   |   1    |  0  |   1   |
| other                    |  0   |   0    |  1  |   1   |
| **Total**                |  3   |  13    | 11  | **27** |

Impact totals: **3 high · 13 medium · 11 low = 27 actionable** (+ 4 REJECTED).

---

## 2. Solver — `crates/sky_types`

The union-find + unify core is already lean (iterative find/occurs/zonk,
non-allocating empty Vecs, `VarId: Copy`). The recurring waste is the
"deep-clone a scheme/descriptor to release a `&self` borrow before a `&mut self`
call" pattern on the hottest constraint-gen and post-solve paths.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `constrain.rs:1681,1714` | medium | Constructor-ref check deep-clones the whole `CtorScheme` per use | `ctors: BTreeMap<Symbol, Rc<CtorScheme>>` — `.cloned()` becomes an Rc bump; `instantiate_ctor` reads through it. Fully internal to Builder. | Rc holds byte-identical data; same fresh vars, same constraints, same errors. No result/panic/security change. |
| `constrain.rs:1381-1382` (≡ cross-cut #1, `558`/`664` decls) | medium | Top-level-ref check deep-clones the scheme `Ty` + allocs a `Vec` key per use | Hold schemes as `Rc<Ty>`; drop the `module.to_vec()` key alloc by keying on an interned FQ symbol. Keep public `env` shape via a final unwrap. | Rc<Ty> read identically; identical fresh vars + SchemeApp. Lookup-encoding swap only. Same CompilerBug on truly-unknown name. |
| `lib.rs:375-376` | medium | `resolve_field_accesses` clones the entire record field map to read one field var | Add `UnionFind::root_content(&self, root) -> Option<&T>`; match `&Record(fields)` by ref, `fields.get(&fa.field).copied()`. | `find()` still runs first (path-compression preserved); identical `Option<VarId>`, unchanged None/NoSuchField path. |
| `lib.rs:408-411` | medium | `resolve_record_updates` clones the whole base-record map even when few fields change | Peek only the K needed field vars via `root_content` into a small `Vec<(Symbol,Option<VarId>)>`, then run the unify loop over that. | Same field vars resolved, same unify order, same span blamed. Pre-copy releases the arena borrow exactly as the map clone did. |
| `unify.rs:91-97` | low | `unify` performs up to 6 union-find traversals where 2 suffice | Replace `equivalent(a,b)?` + two finds with `let ra=find(a)?; let rb=find(b)?; if ra==rb {…}`. Optional `content_of_root` skips one re-find. | `equivalent` ≡ `find(a)==find(b)`; short-circuit identical, path compression preserved. Redundant-call elimination. |
| `constrain.rs:1884-1888` | low | `kernel_ty` unconditionally heap-allocates `task_unit` on every kernel ref | Build `task_unit` lazily (closure like existing `list`/`maybe`) so the 1-elem Vec allocs only in the arms that use it. | Constructed Ty identical eager-or-lazy; only alloc timing moves. |
| `constrain.rs:1505,1566,1660` | low | Scope entry clones the full local env per let / lambda / case-branch | If profiling confirms hot, back the scope map with a persistent map (`im`/`rpds`) for O(1) copy-on-write scope entry. Opt-in; adds a dep. | Persistent map exposes same lookups; identical shadowing + constraints. Flagged pending a profile, not applied blind. |

---

## 3. Lowerer — `crates/sky_lower/src/lower.rs`

Four strictly-safe findings, all producing byte-identical IR.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `898,908,914,918-921` | medium | `run()` walks every function body 7× for kernel-usage flags | One traversal ORs all seven `k.is_*()` predicates into a 7-field flag struct (early-exit once all set), replacing seven independent `.any()` full-AST passes. | Same seven booleans (OR over same per-kernel predicates); enum injection + `uses_*` flags unchanged. Read-only, no alloc growth. |
| `1105-1147` (esp. `1135,1144`) | medium | `collect_records_in_ty`: O(n²) `out.contains` dedup + redundant `ir_type_from_ty` on duplicate region types | Gate insertion with a `HashSet<IrType>` (derive `Hash` on `IrType`) alongside the ordered `out` Vec → O(1) dedup, kills the quadratic scan. | `out` keeps same ordering + same element set (HashSet only gates insertion). `#[derive(Hash)]` is inert, emits no IR. |
| `2544` (peek) + `2638` (resolved) | medium | `lower_callee` (large string-dispatch) invoked twice per kernel/top-level call | Thread the already-computed `peek: Callee` into the fall-through arm instead of re-calling `lower_callee`. Halves dispatch for the common call. | `lower_callee` is pure/deterministic; second call provably returns the same `Callee`. Byte-identical emit; error propagation unchanged. |
| `1170-1173` | low | `lower_def` allocs a throwaway `Vec<Symbol>` key to look up its own `FuncId` | Iterate `defs.iter().enumerate()`, pass idx, set `id = FuncId::from_raw(idx)` — positional id equals the map-resolved id under the unique-(home,name) invariant. | In the no-collision case (module invariant) positional id ≡ map id. `from_raw` on an index is total. Only the self-lookup changes. |
| `3532-4053` | low | `lower_callee` resolves kernels via a hundreds-arm `(&str,&str)` match per call site | Pre-intern fixed kernel module/name symbols in `new()` into a `(Symbol,Symbol) -> KernelFn` map; dispatch on Symbol eq / HashMap probe. Larger mechanical refactor. | Table returns the same `KernelFn` the string arms return; identical fallback. Lower-priority (broad diff, must stay in sync), **not** rejected. |

---

## 4. Emitter — `crates/sky_backend_rust/src`

The emitter's dominant cost is one heap String per IR node plus a `Vec<String>`
+ join at each composite node — inherent to the return-String-per-node design,
kept deliberately (`#[inline(never)]` helpers bound stack depth for the
IPE-L0200 depth-guard soundness test). A writer-based rewrite is a high-risk
architectural change, **not** a quick win, and is left as a deferred
opportunity. Four strictly-safe wins found.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `emit_expr.rs:2285-2326` (helpers guard at `167,316,485,866,961,1106,2133`) | medium | Every `Expr::Call` node probes 7 `inline(never)` kernel-dispatch helpers, even plain user-function calls | Wrap the seven probes in a single `if let Callee::Kernel(_) = callee {…}` block; Func-callee falls straight through. Optionally dispatch by kernel group inside. | Each helper provably returns `Ok(None)` for `Callee::Func` (Kernel-only guard); skipping them for Func changes nothing. Kernel calls still traverse all seven in order. |
| `project.rs:175` | low | `emit_program` assembles `main.rs` into a zero-capacity String | `String::with_capacity(GOLDEN.len() + 4096)` — GOLDEN (in scope at `:30`) is a sound floor for the fixed prelude/epilogue/runtime-bindings. | Capacity is a hint only; bytes pushed are identical. One-shot per compile. |
| `emit_expr.rs:176;233-241;331-339` | low | Http emitters rebuild fixed field-name key Vecs (3 and 7 heap Strings) on every call | Hoist to `&'static [&str]` consts (`RESP_FIELDS`/`REQ_FIELDS`); look up `record_struct_by_key` via a `&[&str]` adapter or cache the resolved index in `EmitCtx`. | Key contents are identical alphabetical constants; only alloc site moves. Emitted struct name unchanged. Http-only path. |
| `lib.rs:189` (lookups `emit_expr.rs:3020,2973`; `lib.rs:457,508`) | low | `record_by_fieldset` keyed by `Vec<String>`; every record node re-resolves+clones+sorts field names to look it up | Key by an interned/positional fieldset identity computed once at `EmitCtx::build`; share ONE keying helper between build + all lookup sites. | **Conditionally safe** — identity is defined by field names sorted *as strings*; symbol-id order ≠ string order, so the sort key MUST stay the resolved NAME on both build + lookup. Gated on shared keying; a mismatched key silently resolves the wrong struct (correctness regression). |

---

## 5. Parser / Canon / Interner — `crates/sky_parse`, `sky_canon`, `sky_intern`

Kernel `(qualifier,name)` resolution in canon is already map-based (BTreeMap,
O(log n)) and dependency name-resolution uses BTreeSet/BTreeMap — **no
linear-scan-vs-map win exists there**, contrary to the initial hypothesis. The
dominant defect is the per-scope full-`Env` clone.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `sky_canon/src/resolve.rs:789,972,990,1007` (Env def `env.rs:35-45`) | **high** | Per-scope full `Env` clone deep-copies the ~600-entry immutable kernel table | `qual_vars: Rc<BTreeMap<…>>` + `ctors: Rc<…>`; build via `Rc::make_mut` (refcount 1 → in-place, no copy). Each scope `env.clone()` becomes an Rc bump + a small clone of only `vars`+`home`. | Same maps read with same BTreeMap ordering → identical resolution + diagnostic order. `make_mut` is total, no copy during single-owner setup. No untrusted input crosses this path. |
| `sky_intern/src/lib.rs:59-60` (≡ cross-cut) | medium | Interner heap-allocates each unique string twice (`strings.push` + `map.insert`) | Store one `Arc<str>`: `map: HashMap<Arc<str>,Symbol>`, `strings: Vec<Arc<str>>`; second store is a refcount bump. `resolve` returns `&str` via deref. | Identical Symbol assignment order + dedup + resolve output, incl. None-on-forged-symbol contract. `Arc` (not `Rc`) keeps `Interner: Send+Sync`. `forbid(unsafe_code)` preserved. |
| `sky_parse/src/parser.rs:1339-1346` | low | Qualified-ident split reconstructs the qualifier via Vec+Vec+join instead of one slice | `let idx = text.rfind('.')…; qualifier=&text[..idx]; last=&text[idx+1..]` — drops the two Vecs + the join String. | split→join of init segments ≡ `text[..last_dot]`; same interned qualifier + name. rfind at an ASCII '.' is a char boundary (safe slice). |
| `sky_parse/src/lexer.rs:133-140` | low | Lexer eagerly materialises the whole source as `Vec<(usize,char)>` | Stream over `char_indices()` with a 3-slot lookahead ring buffer; `offset()` reads head slot or `src.len()` at EOF. Genuine refactor — gate behind lexer golden tests. | Token sequence / spans / line-col / every ParseError variant depend on exact peek+offset; only safe once the lexer suite passes byte-for-byte. Memory-only win, careful equivalence required. |

---

## 6. Runtime hot paths — `runtime/src/sky_runtime`

Hottest paths reachable from emitted programs: `html.rs` render/escape,
`live/diff.rs` SSE diff, `ui/render.rs` Std.Ui→Html, `tea.rs`/`list.rs` kernels.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `html.rs:417-426` (escape_text), `428-433` (escape_attr) | **high** | HTML escapers do 4–5 sequential allocating `.replace()` passes per string | Single pass: if no `&<>'"` present, return `t.to_owned()` (or borrow via `Cow`); else `String::with_capacity` and push entity-or-char per source char. Emit `&amp;`/`&lt;`/`&gt;`/`&#39;`/`&#34;` exactly as today. | Multi-pass order only mattered so `&` from `&amp;` isn't re-escaped — a single original→final map never re-scans output, so bytes are identical. Same escape set (Go subset). No panic, same chars neutralised. |
| `live/diff.rs:145-176` (diff_attrs collect) | **high** | SSE attr diff clones every key AND value of every element into two owned HashMaps per diff | Borrow: `HashMap<&str,&str>` over the attr slices; comparison works unchanged; only `insert_safe_attr` (already copying) allocs, and only for changed attrs. | Same changed/added/removed key set regardless of owned-vs-borrowed; same `insert_safe_attr` XSS gate on same pairs. Iteration order already unordered → no observable shift. Lifetimes sound (slices outlive the local maps). |
| `live/diff.rs:88` | medium | `diff_node` eagerly allocs the sky-id String for every element pair, even when no patch is emitted | `let id: &str = sky_id(old).unwrap_or("");` — `Patch::for_id(&str)` already does the single `.into()` only when a Patch is built. | Identical patches; only eager materialisation deferred to the points that already copy it. Borrow bounded by `old`. |
| `live/diff.rs:235-241` (render_children) | medium | Whole-subtree replace renders each child into its own String before concatenating | Expose `pub(crate) render_into(node, &mut String)` and write children directly into the shared accumulator — no per-child String. | `render_html` = new String + `render_into`; concat order + content unchanged. Same recursion, same `MAX_HTML_DEPTH` cap. |
| `ui/render.rs:160-348` (build_style_string) | medium | Std.Ui style builder allocs one String per CSS declaration then joins | Write directly into a single String with a running `;` separator (helper prepends `;` when non-empty); drop the `Vec<String>` + join. | `;`-joined declarations in same order → byte-identical. CSS security gates (SafeCssPropertyName/Value, dangerous-URL, saturating_add) unchanged. |
| `ui/render.rs:81,98` (SafeCssValue::parse) | low | CSS value gate allocs two Strings per user style value (`to_ascii_lowercase` + filtered `collect`) | Run the cheap breakout-char checks against the borrowed original; build the whitespace-stripped lowercase buffer lazily only if those pass. | **Must preserve the exact match set** (`;{}`, `</`, `/*`, `@import`, whitespace-insensitive script-sink list). Verify against `css_midvalue_injection_dropped`/`css_dangerous_key_dropped` before landing — else leave as-is (security > efficiency). |

---

## 7. Cross-cutting data structures

Interner / module-path / solver / runtime-collection representation. Two entries
here are the same physical findings as §2 and §5 (noted); the net-new
actionable entry is the module-path interning, which carries a determinism
caveat.

| file:line | impact | title | fix | safety-note summary |
|---|---|---|---|---|
| `sky_types/constrain.rs:1382` (≡ solver F4) | medium | Type solver deep-clones the full annotated `Ty` tree on every typed top-level ref | `top_level: BTreeMap<(Vec<Symbol>,Symbol), Rc<Ty>>` (+ `Generated.top_level`); `.cloned()` clones an Rc; `instantiate_tracked(&rc)` reads through. | Rc shares an immutable value; instantiate only reads `ty`. Resolved types byte-identical, keys + BTreeMap order untouched. Single-threaded → `Rc` suffices. |
| `sky_types/ty.rs:30,223` (`Con` module); `constrain.rs:1381`; `lower.rs:682`; `ir.rs:13` | medium | Module paths are `Vec<Symbol>` cloned/`to_vec()`'d pervasively; interning to a Copy `ModPathId` removes lookup allocs + shrinks `Con` clones | Add a module-path interner minting Copy `ModPathId(u32)`; replace `module: Vec<Symbol>` in `Con` + map keys with `(ModPathId, Symbol)`. | **NOT strictly free — determinism caveat**: several `BTreeMap<(Vec<Symbol>,_),_>` rely on lexicographic path-sorted iteration for Go-parity emission order. `ModPathId` MUST impl `Ord` by the resolved path (or verify no emission pass iterates in path order). Mandatory guard reported. |
| `sky_intern/src/lib.rs:59-60` (≡ §5) | low | Interner allocates + stores every string twice | Share one `Arc<str>` between `map` key and `strings` entry. | See §5 row; `Arc` keeps `Send+Sync`. |

---

## 8. REJECTED — would trade a higher principle

These speedups are tempting but weaken principle 1/2/3. They stay as-is and are
recorded so a future pass does not "optimize" them into a regression.

- **Runtime `list_filter` per-element clone** (`runtime/src/sky_runtime/list.rs:120-122`).
  `list.into_iter().filter(|x| f(x.clone()))` clones each element to feed the
  predicate. Passing `&T0` would require a codegen-wide closure-ABI change (Sky
  predicates lower as `Fn(T0)->bool`, by value — same reason `list_sort_by`
  carries `A: Clone`), and could alter move/borrow semantics of user closures.
  **Rejected:** trades soundness of the closure ABI (principle 3) for speed.

- **Runtime `Set`/`Dict` BTreeSet/BTreeMap → HashMap** (`set.rs`, `dict.rs`).
  HashMap gives faster average insert/lookup, but `BTreeSet`'s sorted iteration
  is the documented *conforming, deterministic* order (Go's map iteration is
  randomized; Rust picks a stable order to be a reproducible implementation).
  **Rejected:** swapping changes observable Set/Dict iteration order (principle
  2 / Go-parity determinism) for a principle-4 gain. A private HashSet
  accelerator is permissible *only* if all observable output still flows through
  the sorted structure.

- **Solver `unify_flat` inner `as1/es1/fs1.clone()`** (`sky_types/unify.rs`).
  The merge-first ordering (needed so recursive references resolve — a soundness
  requirement) forces one copy to survive the union while the other is iterated.
  Every alternative (pre-collecting `VarId` pairs) trades one equal-sized alloc
  for another, so it is **not strictly free**. Dropped.

- **Solver `kernel_ty` `(&str,&str)` → interned-Symbol dispatch**
  (`sky_types/constrain.rs`). A real linear-ish string-compare table walked per
  kernel ref, but `&str`-eq short-circuits on length/first byte, so impact is
  modest while the rewrite risk over a ~1000-line declarative table is real.
  Not a principle trade — left as a low note, **not applied** rather than
  rejected. (Same reasoning applies to `lower_callee`'s arm match, §3, which is
  logged as low-priority-not-rejected.)

---

## 9. TOP 10 strictly-safe wins (ranked by impact / effort)

Byte-identical, no principle trade, no caveat. Ranked so trivial one-line guards
with medium impact outrank larger-diff high-impact rewrites.

1. **SSE attr diff → borrowed `&str` maps** — `live/diff.rs:145-176` · high impact,
   low effort. Removes ~2N string allocs per element per diff.
2. **HTML escapers → single pass** — `html.rs:417-433` · high impact, medium
   effort. 4–5 allocs+scans → 1 (or a borrow on the clean path).
3. **Emitter 7-probe `Callee::Kernel` gate** — `emit_expr.rs:2285-2326` · medium
   impact, trivial effort (one `if let` wrap). Kills 7 non-inlined calls per
   user-function call node.
4. **`lower_callee` reuse the `peek` Callee** — `lower.rs:2544/2638` · medium
   impact, trivial effort. Halves the large string-dispatch per call.
5. **`diff_node` borrow the sky-id** — `live/diff.rs:88` · medium impact, trivial
   effort. Defers/avoids a String alloc per unchanged element pair.
6. **Per-scope `Env` clone → `Rc` kernel/ctor tables** — `sky_canon/resolve.rs:789,972,990,1007`
   · high impact, small–medium effort. Largest allocation sink in canon.
7. **`run()` 7 kernel-flag walks → 1 pass** — `lower.rs:898-921` · medium impact,
   small effort. ~7× AST traversals → 1.
8. **`Rc<CtorScheme>` ctor table** — `sky_types/constrain.rs:1681,1714` · medium
   impact, small effort. Fully internal to the crate; deep-clone → Rc bump.
9. **`collect_records_in_ty` HashSet dedup** — `lower.rs:1105-1147` · medium
   impact, small effort. Removes the O(n²) `out.contains` scan.
10. **`render_children` render-into-accumulator** — `live/diff.rs:235-241` ·
    medium impact, small effort. Drops one throwaway String per direct child on
    subtree replace.

Runners-up (strictly safe, slightly lower ROI): `Rc<Ty>` top-level schemes
(`constrain.rs:1382`), interner `Arc<str>` single-alloc (`sky_intern:59-60`),
`build_style_string` single-buffer (`ui/render.rs:160-348`), `unify` redundant
find elimination (`unify.rs:91-97`).

---

*Measure before/after. These are principle-4 deferrals — do not preempt the
exit-0 kernel-scheme work or Go-parity closure to land them.*
