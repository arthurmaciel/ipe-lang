# The unified `KernelId` registry

Status: authoritative design (spec only — no code, no build).
Closes audit **F1 / F2 / F3 / F8** (`docs/architecture/principles-audit-2026-07-02.md`).
Provides the **M4 kernel registry** the FFI consumer port blocks on
(`docs/architecture/ffi-subsystem-design.md` D5 / R0.2 / M4-dep).

---

## Executive summary (12 lines)

1. Today kernel identity is an unparsed `(qualifier, name)` string pair re-matched at **three divergent sites** (canon `env.rs`, types `constrain.rs`, lower `lower.rs`) plus four more (naming, `is_*` predicates, `native_ir_type`, the zero-arity classifier) — **seven hand-tables that silently drift**.
2. The types table fails **open**: `constrain.rs:4069 _ => Ty::Var(u32::MAX)` gives any un-schemed kernel a single flexible variable, so `skyc` never checks its args → **exit-0-then-cargo-fail**, ~231 holes across 14 families.
3. Fix: resolve `(qualifier, name) → KernelId` **once at canonicalisation** (parse, don't validate); every downstream stage holds a typed `KernelId` and never re-matches a string.
4. `KernelId = Stdlib(StdlibKernel) | Ffi(FfiKernelId)` — a two-tier sum: a **closed** enum for stdlib (compile-time exhaustive) and an **opaque index** for FFI (open, data-driven — R0.2).
5. The registry lives in a **new leaf crate `sky_kernels`** (deps `sky_intern` + `sky_diagnostics` only), consulted by canon / types / lower / backend with no dependency cycle.
6. Every KernelId **proves an entry exists**: for `Stdlib`, by enum exhaustiveness; for `Ffi`, by a smart constructor that refuses to mint an id without a parsed scheme.
7. The `Ty::Var(u32::MAX)` fallback is **deleted**. A canon-listed-but-unschemed kernel becomes a **compile error** (non-exhaustive match), not a silent runtime hole.
8. Backend dispatch becomes an exhaustive `match KernelId` — the `is_db()/is_ui()+wildcard` routing (F8) is replaced by a stored `class` field with a total FFI default (over-drop keystone).
9. `Origin { Stdlib | Ffi { crate, version } }` is the M4 axis; the FFI port adds `Ffi` entries and **zero** match arms (D5).
10. Migration is family-by-family behind a fail-closed transitional path; the build stays green and Go-parity is golden-pinned at every commit.
11. The ~231 holed kernels receive their **first correct scheme** as each family enters the registry; the act of migrating *is* the act of closing the hole.
12. **First family: `String`** (largest well-schemed family — proves the plumbing byte-for-byte), then `Math`/`List`, then the six no-stdlib-module holed families (`Encoding`/`JsonEnc`/`JsonDec`/`JsonDecP`/`Jwt`/`Uuid`).

---

## Ground truth (verified against HEAD, 2026-07-02)

| Concern | Site | Failure direction today |
|---|---|---|
| Name-list ("what exists") | `sky_canon/src/env.rs:182` `QUALIFIERS: &[(&str,&[&str])]`, iterated `:990` → `VarHome::Kernel(Symbol,Symbol)` | CLOSED (unknown qualifier → error) |
| `(qual,name) → KernelFn` | `sky_lower/src/lower.rs` `lower_callee` (~377 `&str` arms) → SKY-L0108 | CLOSED |
| `(qual,name) → Ty` scheme | `sky_types/src/constrain.rs:1881` `kernel_ty`, fallback **`:4069 _ => Ty::Var(u32::MAX)`** | **OPEN — the F1 root** |
| `KernelFn → runtime name` | `sky_backend_rust/src/naming.rs` `kernel_name` (~416 arms) | — |
| backend dispatch | `emit_expr.rs` keyed on `is_db()/is_ui()/is_server()` + `_ => Ok(None)` (`:279`, `:444`, `:821`) | OPEN (F8 mis-route) |
| `KernelFn → IrType` | `lower.rs` `kernel_native_ir_type` | — |
| zero-arity classifier | `lower.rs` (Uuid.v4 bare vs Time.now `()`) | — |

**Enum + DAG facts that constrain every choice.** `KernelFn` is a closed ~394-variant enum in `sky_ir` (`ir.rs:822`); `Callee::Kernel(KernelFn)` at `ir.rs:806`. Classification predicates `is_db/is_tea/is_server/is_ui/is_live/is_tui/is_webview` at `ir.rs:1882–2127`. Verified crate edges:

```
sky_ir            → { sky_intern, sky_diagnostics }                    (leaf)
sky_canon         → { sky_intern, sky_diagnostics, sky_syntax, sky_parse }   (NOT sky_ir)
sky_types         → { sky_intern, sky_diagnostics, sky_canon, sky_parse }    (NOT sky_ir)
sky_lower         → { sky_canon, sky_types, sky_ir, … }
sky_backend_rust  → { sky_ir, sky_intern, sky_backend, sky_diagnostics }
```

That **canon and types cannot name `KernelFn`** (they do not depend on `sky_ir`) is *exactly why* identity is smuggled as `(Symbol, Symbol)` and why `kernel_ty` re-matches strings. F2 is a symptom of the missing shared leaf crate.

---

## The central tension, resolved

Two mandates pull opposite ways:

- **F1** wants the strongest gate: a canon-listed kernel with no scheme must **not compile** — a *closed-world* exhaustiveness property.
- **R0.2** (ffi-subsystem-design.md:74–91) mandates an **open** registry indexed by an opaque `KernelId`; 76 000 FFI kernels must add **zero** match arms — an *open-world* property.

A single monolith cannot be both. The resolution is a **two-tier `KernelId`**:

```
KernelId = Stdlib(StdlibKernel)   // closed enum → compile-time exhaustive (F1)
         | Ffi(FfiKernelId)       // opaque u32 index → open, data-driven (R0.2)
```

The ~231 holes are **stdlib** holes, and the stdlib arm is closed → F1's compile-error gate lands there. FFI is open → R0.2 is satisfied there. Both arms guarantee "every id has a scheme": stdlib by exhaustiveness, FFI by a mint-time invariant. This is the load-bearing decision; §Q1–Q6 follow from it.

---

## Q1 — `KernelId` + `KernelEntry` shape + crate placement

**DECISION.**

New leaf crate **`sky_kernels`**, deps `sky_intern` + `sky_diagnostics` only. It owns the **identity axis** in dependency-free primitives:

```
// sky_kernels  (leaf)
pub enum StdlibKernel { StringLength, StringFromInt, MathMin, DbExec, LiveApp, … }  // ex-KernelFn
pub struct FfiKernelId(u32);          // private field — see Q4 mint invariant
pub enum   KernelId  { Stdlib(StdlibKernel), Ffi(FfiKernelId) }   // Copy

pub enum KernelClass { Pure, Db, Server, Tea, Ui, Live, Tui, Webview, Ffi }  // DATA, not a predicate
pub enum Origin      { Stdlib, Ffi { krate: CrateRef, version: Version } }
pub struct EmitRef(pub Symbol);       // neutral runtime-fn / wrapper-ref name (no backend types)

pub struct StdlibDecl {               // the identity single-source-of-truth record
    pub id:        StdlibKernel,
    pub qualifier: &'static str,      // "String", "Math", "Db", "Ui" …
    pub name:      &'static str,      // "length", "fromInt", "min" …
    pub arity:     u8,
    pub class:     KernelClass,
    pub emit:      EmitRef,           // absorbs naming.rs for the simple majority
}
impl StdlibKernel {
    pub const fn decl(self) -> StdlibDecl { match self { … } }   // EXHAUSTIVE, wildcard-free
}
// `ALL: &[StdlibKernel]` derived once via strum::EnumIter — the only generated artifact.
```

**Why the literal single `KernelEntry` struct (D5) cannot physically live in the leaf.** D5's `KernelEntry { id, sky_signature: Scheme, origin, emit, fallibility }` names `Scheme` (= `Ty`, owned by `sky_types`), `native_ir_type` (= `IrType`, owned by `sky_ir`), and a backend-typed emit. A single struct in the leaf would force `sky_kernels → { sky_types, sky_ir, sky_backend_rust }`, and those crates already depend on `sky_kernels` → **cycle**.

**Resolution — exhaustiveness is the merge, not one struct.** The registry is realised as *one shared closed id* + *N exhaustive projections, each in the crate that owns the target type*:

| Axis | Home crate (owns the type) | Form |
|---|---|---|
| identity / arity / class / emit-ref / origin | `sky_kernels` (leaf) | `StdlibDecl` via `decl()` |
| HM scheme (`Ty`) | `sky_types` | `stdlib_scheme(StdlibKernel) -> Ty` — exhaustive `match` |
| `native_ir_type` (`IrType`) | `sky_ir` | `native_ir_type(StdlibKernel) -> Option<IrType>` — exhaustive `match` |
| backend emit strategy | `sky_backend_rust` | exhaustive `match StdlibKernel` (simple → `decl().emit`; bespoke → hand arm) |

Each projection is **total over the same closed enum**. "Three (really seven) tables that silently drift" becomes "N exhaustive matches over one shared id that *cannot compile* if they drift." We did not merge the tables into one struct (impossible without a cycle); we made every table total over one shared closed id.

`sky_ir` changes `Callee::Kernel(KernelFn)` → `Callee::Kernel(KernelId)` and gains a **down-edge** to `sky_kernels`. `sky_canon` / `sky_types` / `sky_lower` / `sky_backend_rust` each add the same down-edge. **No cycle, and no crate gains an `sky_ir` edge it did not already have** — the decisive reason `KernelId` lives in the leaf, not in `sky_ir` (which would drag `sky_canon → sky_kernels → sky_ir`, a layering edge that does not exist today).

The literal D5 `KernelEntry` struct survives **only for the FFI tier** (Q4), where `sky_signature` arrives already parsed as `Ty` from `.ipei` and `emit` is a neutral `EmitRef` — no backend-type dependency, so it lives cleanly in `sky_types`.

---

## Q2 — Parse `(qualifier, name) → KernelId` once, at canon

**DECISION.** The **single parse point** is canon name-resolution in `sky_canon` (co-located with the current `QUALIFIERS` existence check, `env.rs:182`).

1. `sky_canon` gains a dep on `sky_kernels`.
2. Build `stdlib_index: BTreeMap<(Symbol,Symbol), StdlibKernel>` by iterating `StdlibKernel::ALL` and interning `decl().qualifier` / `decl().name`. **This replaces the hand-written `QUALIFIERS` block** — the installed qualifier set is now literally the registry's projection.
3. Where canon today emits `VarHome::Kernel(Symbol, Symbol)` / `VarKernel { module, name }`, it now calls `sky_kernels::resolve(qual, name) -> Option<KernelId>`:
   - `Some(id)` → `VarHome::Kernel(id)` / `VarKernel { id }`.
   - `Rust.*` qualifier declared by a loaded `.ipei` → `KernelId::Ffi(fid)` (Q4).
   - `None` → the existing unknown-qualifier / unknown-member error — **stays fail-closed**.
4. `VarHome::Kernel(Symbol, Symbol)` → `VarHome::Kernel(KernelId)`. The raw pair is **consumed at canon and never reconstructed**.

Downstream — every string match deleted:

- `sky_lower::lower_callee` — the ~377-arm `&str` table is **deleted**; lower reads the canon-produced `Callee::Kernel(id)`. `kernel_is_db/tea/server` become `id.class()` field-reads.
- `sky_types::kernel_ty(module, name)` → `stdlib_scheme(id)` (Q3) — no strings.
- `sky_backend_rust` — reads `id`.

This is **parse, don't validate** applied verbatim: the untyped `(qualifier, name)` from the surface AST is parsed once, at the boundary, into a typed `KernelId` that *carries the proof it names a real kernel in its structure*. No downstream code can re-ask "is this a kernel?" because the only way to hold a `KernelId` is to have passed the canon parse. **F2 closed structurally.**

Two `&str`-re-inspection special cases collapse the same way:
- **F9** `Attribute` disambiguation (`lower.rs` substring-scan for `"Html"`) → distinct variants `UiAttribute` vs `HtmlAttribute`; the enum variant *is* the disambiguation, no module path carried.
- **F14** `Math.min`/`Math.max` re-`resolve`d guards → `MathMin` / `MathMax` variants carrying their ordering obligation in the scheme. No `&str` match survives past canon.

---

## Q3 — Migrating scheme / emit / naming in; the compile-error single-source-of-truth

**DECISION — no scheme DSL, no `kernels!` macro; the closed enum *is* the source of truth.**

- **`kernel_ty`'s ~173 arms** → `sky_types::stdlib_scheme(k: StdlibKernel) -> Ty`, an exhaustive `match` whose arm bodies are **byte-identical to today's `kernel_ty` arms**, with native access to `self.builtins` and the interner. Pure relocation keyed by enum variant instead of `&str` pair. **The `_ => Ty::Var(u32::MAX)` fallback is deleted** (at Phase E, §Q5). Go-parity is preserved *by construction* — the arm body is unchanged.
- **`naming.rs` (~416 arms)** → collapses to `StdlibKernel::decl().emit` (`EmitRef`) for the simple majority; genuinely-bespoke emitters stay as hand-written backend arms.
- **`emit_expr.rs` special emitters** (`emit_db_call`, `emit_live`, HTTP builders, app-entry) → coarse-routed by `id.class()` (a *field*, total), then an **exhaustive `match StdlibKernel`** per family. The `is_db()/is_ui()+wildcard` and the three `_ => Ok(None)` tails are **deleted** (F8).
- **`kernel_native_ir_type`** → `sky_ir::native_ir_type(StdlibKernel) -> Option<IrType>`, exhaustive.
- **classification predicates** (`ir.rs:1882–2127`) → deleted as methods; `id.class()` reads `decl().class`.

**Single-source-of-truth, two nested guarantees (compile-time, not test-time):**

1. `StdlibKernel::decl(self)` is an exhaustive wildcard-free `match`. Add a variant → `decl` fails to compile until qualifier/name/arity/class/emit are supplied. Because **both** the canon name-list **and** `stdlib_index` are built by iterating `ALL` and calling `decl()`, the name-list and the resolution table are *the same data* — **drift is structurally impossible**. The onBool / onKeyPress / Ui-vs-Event class of bug (add a name to one table, forget another) cannot be expressed.
2. Every type-carrying projection (`stdlib_scheme`, `native_ir_type`, backend emit) is an exhaustive `match StdlibKernel`. Add a variant → **every** consumer crate fails to compile until an arm exists. A canon-listed kernel with no scheme is therefore **unrepresentable**.

The only generated artifact is `ALL` — one `strum::EnumIter` derive, compiler-checked, no bespoke DSL.

**Why no `SchemeSpec` data-descriptor** (see OPEN DECISION 1): a declarative `Ty`-mirror stored in the leaf was considered, to co-locate schemes with identity. It is rejected for the stdlib tier because it introduces a *new* faithfulness-critical `SchemeSpec → Ty` interpreter — a fresh Go-parity divergence surface — and drags an interner-threading problem, purely to relocate arms that already have native `Ty` access. The scheme stays a hand `match` in the crate that owns `Ty`. FFI needs no `SchemeSpec` either: `.ipei` decodes straight to `Ty` (Q4). `SchemeSpec` is eliminated on both tiers.

---

## Q4 — The M4 origin axis (`Stdlib | Ffi { crate }`)

**DECISION.** `KernelId = Stdlib(StdlibKernel) | Ffi(FfiKernelId)` **is** the coexistence mechanism, matching ffi-subsystem-design.md D5 (`KernelEntry { id, sky_signature, origin, emit, fallibility }`, `Origin { Stdlib | Ffi { crate, version } }`).

The FFI tier is the literal D5 `KernelEntry`, living in `sky_types` (it holds `Ty`), seeded from `.ipei`:

```
// sky_types  (owns Ty)
struct FfiKernelEntry {
    id:          FfiKernelId,
    qualifier:   Symbol, name: Symbol,   // "Rust.<Crate>.<name>"
    scheme:      Ty,                      // parsed once from .ipei — the consumer-side single parse point
    arity:       u8,
    origin:      Origin,                  // always Ffi { krate, version }
    emit:        EmitRef,                 // wrapper_ref_name from kernel.json
    fallibility: Fallibility,             // R0.4 single stored bit
}
struct FfiRegistry(Vec<FfiKernelEntry>);  // populated at type-env build from the .ipei cache
```

Coexistence properties (all per D5 / R0.2):

- **Shared id space, shared path.** Canon resolves `String.length → Stdlib(StringLength)` and `Rust.uuid.new_v4 → Ffi(fid)` through the *same* `VarKernel` path; the only differences are `origin`, the signature source, and `emit` (D5:307–312).
- **The FFI consumer port just adds `Ffi` entries.** `.ipei` load calls `FfiRegistry::register(Rust.<Crate>.<name>, scheme_from_ipei, origin = Ffi{…})`. It adds zero `StdlibKernel` variants and zero match arms: `stdlib_scheme` gains no arm (FFI schemes come from `FfiRegistry`), and dispatch is `Stdlib(k) => match k {…}, Ffi(fid) => emit_ffi(entry)`. This is precisely the "generator off M4's critical path; only consumer wiring (`.ipei` seeding + `KernelId` lowering resolution) blocks on M4" contract (ffi doc:325–332, 456).
- **Over-drop keystone, enforced by construction.** `KernelId::Ffi(_).class() == KernelClass::Ffi` is a total arm → `is_db/is_tea/is_server/is_ui` are structurally false for every FFI kernel. The `register` signature accepts **no** `KernelClass` argument, so an FFI kernel *cannot name* a stdlib fast-path (R0.2:85–88). An FFI binding is never mis-routed into a stdlib emitter.
- **FFI cannot reintroduce the F1 hole.** `FfiKernelId` has a **private** field; the only constructor is a smart constructor that requires an already-parsed `scheme: Ty`. An id without a scheme is unconstructable — parse-don't-validate at the `.ipei` seam. So both tiers guarantee "every `KernelId` has a scheme": stdlib by exhaustiveness, FFI by the mint invariant.

---

## Q5 — Incremental migration (~400 variants, build green family-by-family)

**DECISION.** A five-phase ladder. The build stays green for the currently-green set at every commit; Go-parity is golden-pinned per family; the exit-0 hole-class is closed *loud* at Phase C and made *unrepresentable* at Phase E.

**Phase A — scaffold (zero behaviour change).** Create `sky_kernels`; move `KernelFn`'s variants to `StdlibKernel` with `type KernelFn = StdlibKernel;` alias; introduce `FfiKernelId`, `KernelId`, `KernelClass` (incl. `Ffi`), `Origin`, `EmitRef`. Keep `Callee::Kernel` on the alias. Pure move/rename. Green.

**Phase B — parse-once boundary (closes F2).** `VarHome::Kernel(KernelId)`; relocate the `(qual,name) → StdlibKernel` table from `lower.rs` into `decl()` / `stdlib_index`, consumed by canon; `lower_callee` unwraps the canon id. Behaviour identical (same table, relocated). **F2 closed here, independent of schemes.** Green.

**Phase C — invert the fallback (closes F1's silent-open class + F3's asymmetry).** Stand up the dual-lookup: `stdlib_scheme(id)` serves migrated families from the new exhaustive match and delegates un-migrated ones to legacy `kernel_ty`. **Change the legacy tail `_ => Ty::Var(u32::MAX)` to a fail-closed diagnostic whose shape matches lower's SKY-L0108** (an *unimplemented-kernel* gap — **not** a `CompilerBug`; a holed kernel used by valid Sky code is a missing scheme, not a compiler invariant break). The crate still compiles (the wildcard still exists), but its failure flips from silent-open (exit-0-then-cargo-fail) to loud-closed at type-check. **No new exit-0 hole can appear, and all ~231 existing holes surface at once as fail-closed `skyc` diagnostics** — and because `sky_types` now fails the *same way* lower already does, **F3's lower-closed/types-open asymmetry is closed here, two phases before the wildcard is deleted.** A burndown test enumerates `StdlibKernel::ALL`, calls `stdlib_scheme`, counts legacy-tail hits, and **asserts the count only decreases**.

> Rationale (security > correctness > completeness): the ~231 holes never produced a passing example — they were already red (cargo failure). Fail-closing only moves the failure earlier and louder, which the no-deferral rule *wants*. This is why Phase C precedes any family fill.

**Phase D — per-family fill (schemed-families first, then holed).** Family by family, move arms into `stdlib_scheme` / `native_ir_type` / backend emit; move identity/arity/class/emit into `decl()`; delete the family's arms from the legacy tables and its bits from `is_*`.
- **Already-schemed families first** (`String`, then `Math`, `List`): this proves the registry plumbing reproduces `kernel_ty` **byte-for-byte** against an oracle *before* the risky holed families rely on it. Per-kernel golden: `stdlib_scheme(k) == old kernel_ty(module, name)`, structural equality.
- **Then the holed families**, starting with the six no-stdlib-module ones (`Encoding`/`JsonEnc`/`JsonDec`/`JsonDecP`/`Jwt`/`Uuid`, 42 kernels — no import escape hatch, holed under all user actions). Each holed kernel gets its **first correct scheme** here, authored against the runtime signature (`runtime/src/sky_runtime/*`) + the Go oracle; the previously-failing skyc→cargo build now **succeeds**, and that green build *is* the parity proof. Because `stdlib_scheme` is exhaustive-in-progress, registering a kernel is impossible without authoring its scheme — **the act of migrating is the act of closing the hole**; there is no intermediate registry-resolved-but-scheme-absent state.

**Phase E — seal (makes F1 unrepresentable).** When the burndown count hits **zero**, delete the legacy tail, the dual-lookup, and the `_ => Ty::Var(u32::MAX)` wildcard. `stdlib_scheme` is now a total match; the gate flips from a runtime test to a **compile-time guarantee**. This is the only "atomic" step and it is a one-line deletion — trivial, because by then every arm exists. Flip `KernelId` from alias-backed to final; `KernelFn` alias removed.

Build is green at every phase because every kernel is *either* registry-backed *or* legacy-backed, never neither; the currently-red holes stay red (now loudly) until their family lands.

---

## Q6 — The exhaustiveness gate (drift + `Ty::Var` permanently unrepresentable)

**DECISION.** Compile-time exhaustiveness is the primary gate; tests cover only the axes exhaustiveness cannot reach.

**Primary (compile-time — post-Phase-E):**
- `StdlibKernel::decl` exhaustive, wildcard-free → no variant without identity/arity/class/emit ⇒ canon-list ≡ resolution-table by construction (**F3 drift impossible**).
- `sky_types::stdlib_scheme`, `sky_ir::native_ir_type`, `sky_backend_rust` emit — all exhaustive `match StdlibKernel`, no wildcard, `Ty::Var(u32::MAX)` deleted. Adding a variant fails to compile in every crate until every arm exists ⇒ **F1 (no scheme) unrepresentable**.
- Backend dispatch is `match KernelId` (exhaustive over the `Stdlib` arm; single `Ffi` data arm), not `is_*()+wildcard` ⇒ **F8 mis-route impossible**.

**Secondary (tests — for what exhaustiveness can't reach):**
1. `canon_equals_registry` — the qualifier/name set canon installs == `StdlibKernel::ALL.map(decl)`. Tautological by construction; the tripwire if a future refactor smuggles back a private `&str` table.
2. `ffi_mint_requires_scheme` — `FfiKernelEntry` is unconstructable without a parsed `Ty`; guards the FFI seam (the one arm exhaustiveness cannot see).
3. `no_ty_var_max_sentinel` — a source-level test asserting `Ty::Var(u32::MAX)` appears nowhere; the banned sentinel cannot be reintroduced by a future PR.
4. `arity_matches_scheme` — for every `StdlibKernel`, `decl().arity == arrow_arity(stdlib_scheme(id))`. A stored `arity` that already lives in the scheme is a drift surface; it must be **asserted against the scheme, never trusted**. (This also disambiguates the zero-arity classifier: `Uuid.v4 : String` vs `Time.now : () -> Task` differ in the scheme head shape, not in an arity count — the classifier reads the scheme, and this test pins it.)
5. `every_ffi_id_reaches_ffi_default` — a property test over synthetic `FfiKernelId`s asserting `class() == Ffi` and dispatch never enters a stdlib fast-path (the open-half boundary the closed match cannot cover).

The F3 asymmetry that produced the recurring bugs (lower fails closed, `kernel_ty` fails open, no test asserting agreement) is gone: one identity source, one failure mode, one totality guarantee. **F16** (schemed-but-unresolvable dead arms like `onKeyPress`) is also foreclosed — one registry entry ⇒ resolvable ∧ schemed ∧ emittable; a kernel cannot be schemed without being in `decl()`, hence canon-resolvable.

---

## What this design fixes, explicitly

- **F1 (CRITICAL)** — the `Ty::Var(u32::MAX)` fail-open fallback is deleted; a KernelId always has a scheme (stdlib by exhaustiveness, FFI by mint invariant).
- **F2 (HIGH)** — `(qualifier, name)` is parsed once at canon into a `KernelId`; no downstream `&str` re-match survives.
- **F3 (HIGH)** — the three (seven) tables are projections of one closed enum; drift is a compile error, and the lower/types asymmetry closes at Phase C.
- **F8 (MEDIUM)** — backend dispatch is an exhaustive `match KernelId` + a `class` field with a total FFI default; the `is_*()+wildcard` routing is deleted.

And it **is** the M4 registry the FFI consumer port blocks on: `KernelEntry` / `Origin { Stdlib | Ffi }` per ffi-subsystem-design.md D5, an open `KernelId`-indexed registry per R0.2, so the FFI port adds only `Ffi` entries (ffi doc:442–457).

---

## OPEN DECISIONS

1. **Stdlib scheme storage: hand `match → Ty` (this spec) vs shared `SchemeSpec` descriptor + one translator.**
   The spec adopts the hand-match (majority: soundest — no interpreter, no interner trap, Go-parity by construction, no module-path-loss). The dissent (R1) is legitimate: a shared `SchemeSpec` populated by *both* tiers with one `SchemeSpec → Ty` translator would guarantee an FFI `fun(int,string)` type-checks *identically* to stdlib `String.fromInt` by *construction* rather than by two independent `Ty`-builders agreeing. Counter-argument: there is only one `Ty` type and the `.ipei → Ty` decoder is needed regardless, so a stdlib `SchemeSpec` adds a third representation without removing a need. **Resolution deferred to the M4 implementer:** if, during FFI bring-up, the `.ipei → Ty` and stdlib-arm paths show any structural divergence for the same logical signature, promote to a shared descriptor; otherwise keep the hand-match. Revisit before the first `Ffi` entry lands.

2. **`ALL` generation: `strum::EnumIter` derive vs a minimal `macro_rules!` row-list.**
   The spec assumes `EnumIter` (one derive, no new dep beyond `strum`). A `macro_rules!` single-row-list emitting the enum + `decl()` together would make "variant without decl" physically un-writable in one edit, but cannot host the scheme match (needs `Ty`). Low-stakes; either satisfies the single-source guarantee. Decide at Phase A based on whether `strum` is already a workspace dep.

3. **Retention of `(Symbol, Symbol)` on the canon node.**
   Consumed for resolution; the spec drops it from the semantic path. Whether to retain it as a diagnostics/`sky doc` breadcrumb (nice errors, `sky doc` origin display) or reconstruct from `decl()` on demand is an ergonomics call for Phase B — no soundness impact either way.

---

## Migration order (first family)

1. **`String`** — largest already-schemed family; migrating it first proves the registry plumbing reproduces `kernel_ty` byte-for-byte before any holed family depends on it.
2. **`Math`**, **`List`** — remaining well-schemed scalar families; lock the golden-parity harness.
3. **The six no-stdlib-module holed families** — `Encoding`, `JsonEnc`, `JsonDec`, `JsonDecP`, `Jwt`, `Uuid` (42 kernels): no import escape hatch, holed under all user actions → highest-value first schemes.
4. Remaining holed families (the other eight of the 14), each closing its slice of the ~231 exit-0-then-cargo-fail holes as it lands.
5. App-entry families (`LiveApp` / `TuiApp` / `TuiProgram` / `WebviewApp`) — already hand-schemed in lockstep, structurally unlike the scalar families; migrate last, as the closed-record-cfg proving ground that the registry formalises the lockstep they currently maintain by hand.

---

## Phase C re-review (post-B, 2026-07-02)

Re-validation of the Phase C section against the codebase after Phase A/B
landed. **Verdict: GO WITH ADJUSTMENTS.** The mechanism is sound and the
parsed id is available at the constrain site; five concrete entry conditions
tighten the impl. Refreshed anchors and the adjustments follow.

### Refreshed anchors (drift from the pre-A citations)

| Spec citation | Current anchor | Note |
|---|---|---|
| `constrain.rs:4069 _ => Ty::Var(u32::MAX)` (lines 13, 33) | `constrain.rs:4076` | moved +7 |
| `kernel_ty` at `constrain.rs:1881` (line 33) | `constrain.rs:1888` (`:1881` is now the close of the prior fn) | header moved |
| VarKernel constrain arm | `constrain.rs:1483` (destructures `id: _`); scheme entry `constrain_var_kernel` at `:1435` | new post-B site |
| `KernelFn` closed enum in `sky_ir` (`ir.rs:822`), "~394 variants" (lines 12, 39) | `StdlibKernel` now lives in leaf `sky_kernels` (`lib.rs`), **424** variants; `KernelFn` is an alias | Phase A relocated the enum + count |
| "the ~377-arm `&str` table is **deleted**" from `lower.rs` (line 133) / Q2 | still present as the **id=None** legacy arm (`lower.rs:4050–4065`, tail `SKY-L0108` at `:4065`); dual-backed, not deleted | Q2 describes the Phase D/E end-state, not post-B reality |
| canon `QUALIFIERS` hand-block replaced (line 125) | `stdlib_index` built from `ALL` in `env.rs:1012+`; install fast-path at `env.rs:192` | landed as specified |

The `~231 holes / 14 families / ~173 arms` figures are pre-A estimates and
were not re-counted; treat as approximate.

### Adjustments (tightened Phase C entry conditions)

1. **Build-graph prerequisite — add the `sky_types → sky_kernels` edge FIRST.**
   `sky_types/Cargo.toml` deps are `sky_intern, sky_diagnostics, sky_canon,
   sky_parse` — **no `sky_kernels`**. The canon node already carries
   `id: Option<StdlibKernel>` (`sky_canon/src/ast.rs:138`) and the constrain
   arm already binds it (`id: _`, `constrain.rs:1484`), so threading is
   additive — but to *name* `StdlibKernel` (declare `stdlib_scheme(k:
   StdlibKernel)`, match `Some(k)`) `sky_types` must gain
   `sky_kernels = { path = "../sky_kernels" }`. `sky_kernels` is a leaf
   (`sky_intern` + `sky_diagnostics` only) → clean down-edge, no cycle. The
   spec's edge table (Q1) anticipated this; Phase A/B added it to `sky_canon`
   only.

2. **Fail-close belongs in `constrain_var_kernel`, not in `kernel_ty`.**
   `kernel_ty` returns bare `Ty` (`:1888`); its `_ => Ty::Var(u32::MAX)`
   (`:4076`) cannot raise a diagnostic. `constrain_var_kernel` returns
   `DResult<VarId>` (`:1435`). Phase C must make `kernel_ty` (or its
   successor) return `Option<Ty>` and surface the miss in
   `constrain_var_kernel` as `Err(unsupported(span, Feature::Kernels))` —
   byte-for-shape with lower's `SKY-L0108` (`lower.rs:4065`). Recommend the
   Phase C signature `stdlib_scheme(StdlibKernel) -> Option<Ty>` (None =
   not-yet-migrated) so the dual-lookup is `id.and_then(stdlib_scheme)
   .or_else(|| legacy_kernel_ty(module, name))` and Phase E's seal is the
   single fact "stdlib_scheme is now total (never None)".

3. **Family order: String first is correct and safest; Math carries a hidden
   obligation.** The min/max ordering obligation and the Dict/Set
   `key_obligation` are attached in `constrain_var_kernel`
   (`:1436–1442`, `:1443–1451`) — **outside `kernel_ty`**. Migrating a
   family's scheme into `stdlib_scheme` does **not** carry these bounds; if
   Math lands without re-encoding the min/max `Ord` super-var into
   `stdlib_scheme(MathMin/MathMax)` (or retaining the pre-check), it reopens
   the bare-unbounded-min/max gate (records / functions accepted, then
   `cargo` E0277). `String` and `List` have no obligation pre-check → clean.
   **Recommended first cut: String → List → Math**, with Math's obligation
   encoded deliberately and pinned by the `Math.min f g` / `Math.min recA
   recB` rejection probes. Dict/Set (same class) stay behind Math until the
   `Hash + Eq + Ord` bound is proven in-scheme.

4. **Add a scheme-parity tripwire alongside the burndown.** The forward
   `canon_equals_registry` (`sky_canon/src/lib.rs:1355`) and the reverse
   id-match check (`:1410`) pin canon↔registry identity, and they stay
   meaningful as schemes migrate because they touch identity, not schemes.
   They do **not** guard the relocation of `Ty`. Phase C must add, per
   migrated kernel, `stdlib_scheme(k) ≡ kernel_ty(decl(k).qualifier,
   decl(k).name)` (structural `Ty` equality) — the Go-parity proof that the
   move was byte-faithful — plus the spec's monotone-decreasing legacy-tail
   burndown (Q5). Together these keep the dual state honest: every kernel is
   registry-backed **or** legacy-backed, never mis-typed, never neither.

5. **G1 makes the delegation key provably unambiguous — in three parts.**
   - **Injectivity** (`decl()` maps no two variants to the same `(qual,name)`)
     is delivered by `sky_kernels::tests::no_colliding_qualifier_name_pairs`.
     A collision would make `stdlib_index`'s last-wins insert silently alias
     one variant onto another, breaking the `id = Some(k)` uniqueness
     guarantee.
   - **Decl-equiv-legacy equivalence** (`decl(k).(qualifier,name)` matches
     the legacy string-match arm for every wired kernel) is delivered by
     `sky_lower::tests::decl_equiv_legacy_match`.  That test forces `id = None`
     so the legacy path runs; a wrong or missing arm causes the assert to fail.
   - **Propagation wiring** (the G1 reverse check in `canon_equals_registry`)
     verifies only that `install_prelude_qualifiers` stored the id it read from
     `stdlib_index`, not that `decl()` is injective or that the legacy arm is
     correct.
   Together the three gates make `id = Some(k)` unambiguous and
   `decl(k).(qualifier,name)` provably equal to the node's `(module,name)` —
   the property Phase E needs to drop `module`/`name` from the `VarKernel`
   node entirely and delegate purely by `decl(k)`.

### Soundness of the incremental dual state — confirmed

The split is sound because the constrain arm mirrors lower's dual-backing
exactly: `id = Some(k)` → `stdlib_scheme(k)` then legacy; `id = None` (FFI
`Rust.*` and any name absent from `stdlib_index`) → legacy `(module,name)`.
No family is silently mis-typed: a migrated family is total in
`stdlib_scheme`; an un-migrated one is untouched legacy; a miss on both is
loud (`SKY-L0108`-shaped) rather than the current silent `Ty::Var(u32::MAX)`
exit-0-then-cargo-fail. The one edge the original prose under-specified is
the **un-migrated `Some`** case — it must fall to legacy just like `None`,
which the `Option<Ty>`-returning `stdlib_scheme` (adjustment 2) makes
structural rather than a second conditional.
