# Compiled-source stdlib modules (Ipe.Css, Ipe.Error, …)

Status: accepted design. Reconciles three independent fresh designs with the
upstream Sky v0.17.2 mechanism, under the strict principle order
**(1) security · (2) correctness · (3) soundness · (4) efficiency · (5)
completeness · (6) readability** and the two verbatim rules **PARSE, DON'T
VALIDATE** and **MAKE INVALID STATES UNREPRESENTABLE**.

This is the enabler for port tasks #98 (pure-Ipê-source stdlib north star), #47
(`Ipe.Css`), and #85 (rich `Ipe.Error` ADT).

---

## 0. The load-bearing invariant

> A stdlib source module either resolves to **exactly the same pipeline result
> as a user module** — parse → canonicalise → infer → lower → emit →
> `cargo build` — **or it produces a clean `IPE-N…`/`IPE-T…` diagnostic**.
> There is no third state. In particular: **never exit-0-then-cargo-fail.**

Everything below is engineered to keep that invariant total. The three
mechanisms that make it unrepresentable to violate:

1. **Presence-XOR-registry, fail-closed resolution** (§2) — an import resolves
   to *either* an on-disk/embedded source module *or* a closed kernel-registry
   qualifier, else it is a hard `IPE-N` error. No silent "assume kernel".
2. **Fully-annotated-stdlib, fail-closed gate** (§3) — a compiled-source module
   with any un-annotated top-level binding is a compiler-internal error at
   canonicalisation, before inference can produce a surprising deep-stdlib
   unification failure.
3. **Security-kernel scheme exhaustiveness** (§4) — every leaf security kernel
   the pure-Ipê Css calls is schemed in `sky_kernels`, so a reference cannot
   pass canon and then fail at `cargo`.

---

## 1. Current state (verified against HEAD)

- `crates/ipe/src/stdlib.rs` embeds `sky-stdlib/**` via `include_str!` into
  `MODULES: &[StdModule]`. The `every_embedded_module_parses` test proves each
  embedded module parses with the real front end — but **nothing is compiled
  from source today**. The embedded `Ipe.*` files are a *parse-test
  fixture / shadow copy*; the real implementations are Rust kernels resolved by
  qualifier. `List.map` works because its ctors are prelude builtins and every
  combinator is a kernel.
- `crates/sky_canon/src/resolve.rs` (~line 244) short-circuits **every**
  import whose first path segment is `Ipê` or `Std`:
  ```rust
  if dep_path.first().copied().is_some_and(|s| s == sky_sym || s == std_sym) {
      continue;   // qualifiers pre-installed by Env::initial
  }
  ```
  So a `.ipe` source under the `Ipe.`/`Ipê.` namespace can never be injected —
  this is the exact line that must be relaxed.
- The port already ships a **Design-2 half-build** of CSS:
  `runtime/src/sky_runtime/css.rs` defines runtime `CssProp`/`CssRule`
  reflection enums + builder/sink kernels (`css_property_`, `css_rule_`,
  `css_stylesheet_`, `css_styles_`); `runtime/src/sky_runtime/css_safety.rs`
  defines `SafeCssPropertyName`/`SafeCssValue`/`SafeCssSelector` (full
  per-declaration gating: rejects `@import`, `expression(...)`,
  `url(javascript:…)`, comment digraphs, tag-breakout) plus a
  fixpoint + case-insensitive `strip_style_close`.
- No `crates/ipe/stdlib/Std/Css.ipe` exists yet. `Css`/`Error` are **not** in
  `env.rs`'s `STDLIB_MODULE_QUALIFIERS`.

The upstream Sky v0.17.2 mechanism (for reference): TH-embed `sky-stdlib/`,
materialise it to `<outDir>/.ipe-stdlib/` (cleared-first), append that dir as
the **last** discovery root, and resolve imports by *file presence* — present ⇒
source module, absent ⇒ kernel. Type-checking is per-module (topo order, a
two-pass fixpoint solve), **annotation-driven** generalisation — **not**
whole-program let-generalisation. `Std/Css.ipe` upstream is 1510 lines of 100%
pure Ipê, zero `Ffi.kernel`.

Two upstream choices we deliberately **do not** copy: (a) stdlib root placed
*last* so a stray user `Std/Auth.ipe` silently shadows the audited
implementation — a supply-chain hazard we reject (§2); (b) the general
`Ipe.Html` `<style>` render path emits raw bodies verbatim with **no** breakout
floor — a real gap we close at the render sink (§4).

---

## 2. Module-graph injection + the resolve.rs relaxation (chosen mechanism)

### 2.1 Embedding

Add a table in `stdlib.rs` **disjoint** from both `MODULES` (parse fixtures) and
from `env.rs`'s `STDLIB_MODULE_QUALIFIERS` (kernel qualifiers):

```rust
pub struct CompiledStdModule { pub dotted: &'static str, pub source: &'static str }
pub const COMPILED_STD_MODULES: &[CompiledStdModule] = &[
    CompiledStdModule { dotted: "Ipe.Css",        source: include_str!("../stdlib/Std/Css.ipe") },
    CompiledStdModule { dotted: "Ipe.Error", source: include_str!("../stdlib/Ipê/Core/Error.ipe") },
];
pub fn compiled_std_source(path: &[Symbol], interner: &Interner) -> Option<&'static str> { … }
pub fn is_compiled_source(path: &[Symbol], interner: &Interner) -> bool { … }
```

A `#[test]` asserts `COMPILED_STD_MODULES` is **empty-intersection** with
`STDLIB_MODULE_QUALIFIERS`. This is a hard, load-bearing invariant (risk in
§7): a name in both would be pre-installed as a kernel qualifier *and* injected
as a source dep, giving ambiguous resolution / silent miscompile. Also extend
`every_embedded_module_parses` over `COMPILED_STD_MODULES` — the
PARSE-DON'T-VALIDATE floor: a module cannot enter any graph until it is proven
to parse with the same front end.

### 2.2 Transitive-closure injection

We do **not** blindly add all embedded stdlib as roots (upstream's approach);
we inject **only what is transitively imported**, so an unused-Css build pays
nothing. In `crates/ipe/src/project.rs`:

```rust
pub fn inject_compiled_std_closure(
    sources:    &mut BTreeMap<Vec<Symbol>, (PathBuf, String)>,  // module_path -> (path, src)
    discovered: &mut Vec<DiscoveredModule>,
    interner:   &Interner,
) {
    // 1. Seed worklist from every import across current sources
    //    (reuse extract_imports_from_source). Short-circuit: if no import
    //    matches is_compiled_source, do nothing — zero cost when unused.
    // 2. Pop a candidate compiled-Std module path P:
    //      - if `sources` already has key P -> skip  (BTreeMap key = free dedup)
    //      - else insert (synthetic path "<embedded>/Std/Css.ipe", embedded_src)
    //        into `sources` AND push DiscoveredModule { path, module_path: P }.
    // 3. Scan the embedded source's OWN imports; enqueue any that are
    //    themselves compiled-Std (Std->Std to fixpoint). Kernel imports inside
    //    the embedded source (Ipe.String, Ipe.Db) are NOT enqueued — they
    //    stay kernel-resolved by qualifier.
}
```

The injected nodes now sit in `discovered` + `sources`
**indistinguishably from user modules**.

### 2.3 Graph wiring

In `crates/ipe/src/lib.rs` `build_project`, call
`inject_compiled_std_closure` **immediately after** the user-source read loop
fills `sources`, and **before** `project::topological_order`. Because
`topological_order` builds its `module_set`/`module_map` from `discovered` and
`imports_of` reads `sources`, the injected Std nodes are first-class: a user
edge `Main -> Ipe.Css` becomes a real graph edge; the three-colour DFS orders
`Ipe.Css` **dep-first** (edge direction is user→Std, so Std is finalised first)
and detects Std↔user cycles for free. The dep-first canonicalise loop inserts
`Ipe.Css`'s `ModuleExports` under key `[Std, Css]` in `dep_exports` **before**
any importer is processed. `link()` concatenates all canon modules; the
module-prefixed names (`Std_Css_*`, keyed by `union.home`) cannot collide with
user defs. `infer`/`lower`/`emit` treat them as ordinary code. Nothing bespoke
downstream.

### 2.4 The resolve.rs:244 relaxation (fail-closed)

Change the unconditional `Ipê.`/`Ipe.` continue to gate on **presence in
`deps`**:

```rust
if dep_path.first().copied().is_some_and(|s| s == sky_sym || s == std_sym)
    && !deps.contains_key(dep_path)
{
    continue;   // genuine KERNEL module: qualifier path, untouched
}
// else fall through to deps.get(...) + inject_dep_exports — a COMPILED-SOURCE
// Std module resolves exactly like a user dep.
```

- A **kernel** module (`Ipe.Log`, `Ipe.Prelude`) is absent from `deps`
  (never injected) ⇒ still `continue`s (qualifier path untouched).
- A **compiled-source** module (`Ipe.Css`) *is* in `deps` (injected as a
  synthetic dep) ⇒ falls through to the existing `deps.get` +
  `inject_dep_exports`, which already registers `qual_vars[Css][name] =
  TopLevel([Std,Css])` for qualified access (`Css.px`) **and** the unqualified
  `exposing (rule, px)` / `exposing (..)` names. `Css.px` lowers to
  `VarTopLevel { module: [Std, Css] }` identically to a user module — **no new
  qualifier plumbing**, and `register_stdlib_import_aliases` is a no-op for Css
  (`canonical_stdlib_qualifier` returns `None`, so no double registration).

**Fail-closed refinement (upstream tightening).** Upstream's "no source ⇒
silently assume kernel" is fail-*open*: a typo resolves to a phantom kernel that
only errors at `cargo`. The port keeps the existing behaviour that an unresolved
non-`Ipê`/`Std` import is `IPE-N0020 ModuleNotFound` (with did-you-mean), and
additionally: an unresolved `Ipê.`/`Ipe.` import that is **neither** injected as
source **nor** a member of the closed kernel qualifier registry must surface a
clean `IPE-N` diagnostic rather than a phantom kernel. Import resolution is
therefore total: source **XOR** registered kernel **XOR** clean error.

### 2.5 ModuleOrigin — trust is an unforgeable value (MAKE INVALID STATES UNREPRESENTABLE)

Two canonicaliser gates would reject a legitimate compiled-source module:

- **`IPE-N0025 ReservedNamespace`** (`resolve.rs:198`) rejects any module whose
  *own* home path starts with `Ipê`/`Std`. `Ipe.Css` declares `module Ipe.Css`
  → would be rejected.
- **`reject_reserved_builtin_type` / `RESERVED_BUILTIN_TYPES`**
  (`resolve.rs:56,96`) rejects a user `type` shadowing a builtin. `Ipe.Css` is
  the canonical definer of `type Length` (and `type Color`, the #75 hole) → its
  own ADTs would be rejected.

Fix: thread a typed **`ModuleOrigin { User, EmbeddedStdlib }`** into
`canonicalise_module`. Fire `ReservedNamespace` **only** for `Origin::User`;
exempt `Origin::EmbeddedStdlib` from the reserved-name gates **only for the
names it owns**. The trust tag is a value the *build driver* constructs — set to
`EmbeddedStdlib` **only** when the source came from `stdlib.rs`'s embed table,
**never** derivable from module text. So a hostile user file literally named
`Ipe.Css` (or `Ipe.Auth`) stays `EmbeddedStdlib`-untagged and is still rejected
by N0025. This is the security answer to upstream's "user shadows stdlib"
hazard: **bundled stdlib is authoritative; a user file in the `Ipe.`/`Ipê.`
namespace is a hard error, not a silent win.** Keep the exemption *tight* — only
members of `COMPILED_STD_MODULES`, never any `Std`/`Ipê` self-name in general.

Threading `ModuleOrigin` changes `canonicalise_module`'s signature (ripples to
`build`, `build_project`, tests, LSP). Mitigate with a thin wrapper defaulting to
`User`.

### 2.6 The single-file `build()` hole

`build()` (`lib.rs:182`) compiles one entry with no module graph, so a
single-file program importing `Ipe.Css` cannot inject and would 404. Extract the
graph core (`sources` map → closure → topo → dep-first canon → link) into one
`compile_modules(sources, entry, interner)` helper that **both** `build` and
`build_project` call; `build` seeds it with a single synthetic entry then runs
the identical closure. The two paths must never diverge, and this must land
**with** the feature (not after), or single-file imports silently 404. For
programs with no compiled-source import, `compile_modules` must be
emit-byte-identical to today (regression-tested).

---

## 3. Let-generalisation verdict

**VERDICT: NOT a blocker for Css/Error. Whole-program let-generalisation is an
incremental completeness item, never a soundness or security gate.**

The current inference already gives annotated top-level bindings real
per-use-site let-polymorphism: each `Def::Typed` annotation is registered as a
scheme and every reference instantiates fresh union-find vars per call site (the
`CForeign` path). This matches upstream exactly — upstream type-checks
per-module in topo order with a two-pass fixpoint solve and **annotation-driven**
generalisation (`generaliseToAnnotation`), *not* whole-program let-gen. The only
thing not yet supported is generalising an **un-annotated** top-level used at two
distinct concrete types in one program (rank-based generalisation).

`Ipe.Css` is fully annotated and largely **monomorphic** (`CssRule -> String`,
`px : Int -> Length`, `rule : String -> List CssProp -> CssRule`); its 19 ADTs
are concrete. `Ipe.Error` likewise. So both compile under today's inference
with **zero solver changes**.

**Soundness note.** The absence of rank-based generalisation is a *completeness*
gap, never a soundness hole: if the annotation discipline were violated, the two
use-types unify under one mono var and unification **rejects** the mismatch — a
sound `IPE-T0001`, never a silent miscompile, and it *cannot by itself* cause
exit-0-then-cargo-fail.

**Fail-closed gate (the minimal change, zero solver work).** In
`canonicalise_module`, when `origin == EmbeddedStdlib`, reject any top-level
`Def::Untyped` with a compiler-internal diagnostic ("compiler stdlib module M
binding b must carry a type annotation"). It can never fire for user code; it
converts the fully-annotated precondition from an *assumption* into a
*machine-checked contract* at the exact boundary, turning a would-be confusing
deep-stdlib unification error into an explicit build-time invariant. Rank-based
generalisation stays a documented later item that only unlocks *un-annotated
polymorphic stdlib helpers* — needed one day for a broader pure-Ipê stdlib, not
on the Css/Error critical path.

---

## 4. CSS security recommendation

**RECOMMENDATION: Design-1 (pure-Ipê `Ipe.Css`) + primitive-typed `css_safety`
leaf kernels gating ALL three free-string entry points at construction
(drop-on-fail) + an unconditional `strip_style_close` breakout floor at every
`<style>` render sink.** This is a refined middle path — stronger than the
orchestrator's "single gate on `Css.property`", and it deliberately does **not**
keep Design-2's ADT↔enum reflection.

### 4.1 Threat model (why, in principle order)

1. **Security (#1) — two distinct sink classes:**
   - **Breakout / tag-escape** (`</style><script>…`, attribute-quote escape).
     This is the escalation class. It is closed **unconditionally at the DOM
     serialization sink**: `strip_style_close` (fixpoint + case-insensitive,
     catching `</StYlE` and the `</sty</stylele` re-seam a single pass misses)
     on **every** `<style>` raw-body child, and `escape_attr` on inline
     `style="…"`. Upstream wires this floor only into a handful of Ipe.Ui
     feature injectors and leaves the general `Ipe.Html` `<style>` render path
     **verbatim** — a real breakout gap. We close it at the **one**
     `renderVNode` raw-body arm so every delivery path (Html.render, Live, Tui,
     Webview) inherits it. **One floor, not six.**
   - **Non-breakout, in-declaration** (`@import` remote-stylesheet fetch =
     data-exfil / CSP-bypass, still live in modern engines; `background:
     url(javascript:…)`, `expression()` in legacy/IE-mode/email/webview
     renderers). `strip_style_close` does **not** catch these — they never break
     out of the element. **Silently dropping this protection is a real security
     regression I will not accept under principle #1** when the port *already
     owns* the gate (`SafeCssValue`/`SafeCssPropertyName`/`SafeCssSelector`).
     Critically, a raw **selector** or **media query** (`Css.rule sel …`,
     `Css.media q …`) is *also* a free-string entry — `SafeCssSelector` already
     proves the `@import`-via-selector vector — so gating **only**
     `Css.property` is **insufficient**. All three entry points must gate.

2. **Correctness (#2):** byte-shape parity with the current `css.rs` render fold
   is achievable — the pure-Ipê fold mirrors `render_prop`/`render_rule`.

3. **Soundness (#3):** the leaf kernels are **primitive-typed** (`String ->
   Maybe String`, `String -> String`) — trivially schemable, **no compiled-ADT
   ↔ runtime-enum reflection**, so no exit-0-then-cargo-fail surface. A dropped
   declaration is a first-class, exhaustiveness-checked `CssDropped` variant.

4. **Efficiency (#4):** scan-once-at-construction + one final strip pass; no
   Design-2 double-scan-at-sink.

5. **Completeness (#5):** the full Css surface is expressible in pure Ipê; the
   escape hatch is gated; the typed builders (`px`/`rem`/`hex`/`rgb`/`color`)
   are total and structurally cannot carry injection.

6. **Readability (#6):** `css.rs` shrinks from reflection enums + builder/sink
   kernels to ~four primitive shims; policy stays single-sourced in
   `css_safety.rs`; render logic reads as ordinary Ipê.

### 4.2 Shape

- `Std/Css.ipe` is pure Ipê: ADTs `CssProp` (incl. a `CssDropped` variant),
  `CssRule` (`CssRule | CssMedia | CssKeyframes | CssRaw`), `Length`, `Color`,
  and the bounded keyword enums, each with a narrow `*Raw` escape hatch.
- **`CssProp` is exported as an OPAQUE type** (export the type, not the ctor —
  `Exposed::Type` + `Privacy::Private`, already supported). The only paths that
  build a `CssProp` are the gated smart constructors, so a rendered payload is
  **provably post-scan** — MAKE INVALID STATES UNREPRESENTABLE.
- Typed builders `px : Int -> Length`, `hex`/`rgb`/`color` **PARSE** numeric /
  enum inputs into ADTs whose `Display` can only emit `[0-9a-f#.%a-z-]`. A
  dangerous value is **unrepresentable** through them; they never touch the
  scanner.
- The only free-string entries — `Css.property k v`, `Css.rule sel …`,
  `Css.media q …`, and `Css.raw` — sanitize **at construction** through the
  leaf kernels:
  ```
  Css.safeValue    : String -> Maybe String   -- SafeCssValue::parse
  Css.safePropName : String -> Maybe String   -- SafeCssPropertyName::parse
  Css.safeSelector : String -> Maybe String   -- SafeCssSelector::parse
  Css.stripStyleClose : String -> String       -- breakout floor for CssRaw bodies
  ```
  `property k v` case-matches `(safePropName k, safeValue v)`: `Just/Just`
  builds `CssProp k2 v2`, anything else builds `CssDropped` (never a silent
  partial emit). `stylesheet`/`styles` fold in pure Ipê and finish through
  `stripStyleClose` as defence-in-depth.

**PARSE, DON'T VALIDATE:** `safeValue`/`safePropName`/`safeSelector` parse the
raw string **once** at the CSS-domain boundary and yield a proof-carrying
`String` or the explicit `CssDropped` state — no downstream re-check;
`strip_style_close` parses the `<style>` body once at the render sink.

### 4.3 Migration of the current Design-2 half-build

Retire `css.rs`'s reflection enums (`CssProp`/`CssRule`) and its
builder/sink kernels (`css_property_`, `css_rule_`, `css_stylesheet_`,
`css_styles_`) in favour of the four primitive leaf shims over the **unchanged**
`css_safety.rs` policy (add `pub` wrappers). Net kernel surface **shrinks**, and
the ADT-reflection drift burden (every ADT edit had to mirror the Rust enum)
disappears. Register the four leaf kernels in `sky_kernels` with their primitive
schemes.

---

## 5. Classification rule — compiled-source vs kernel

> **Discriminator = transparency.** A `VarHome::Kernel` value is opaque: Ipê
> cannot `case … of` it.
>
> - A module **MUST be compiled-source** iff any exported function
>   pattern-matches a data type the module itself defines.
> - A module **MAY stay a kernel** only if it is a pure boundary:
>   (a) an effect sink/source (`File`, `Http`, `Time`, `Random`, `System`,
>   `Io`, `Db`, `Task`), (b) an opaque primitive the Ipê side never destructures
>   (`Dict`, `Set`, `Bytes`, `Crypto`), or (c) a security choke-point that must
>   be single-sourced in Rust (`css_safety` validators, HTML escapers).
> - When **both** hold, **SPLIT**: pure-Ipê ADT + logic as compiled-source, plus
>   a **minimal leaf-kernel** boundary-sink surface.

Applied:

| Module | Verdict | Why |
|---|---|---|
| `Ipe.Css` | compiled-source **+** leaf security kernels | Defines + matches `CssProp`/`CssRule`/`Length`/`Color`; but the three free-string entries are a security choke-point → split. |
| `Ipe.Error` (#85) | compiled-source | Rich `Error` ADT that its own combinators case-match; replaces the `Error = String` minimal registration (#86) with no kernel-path golden flip. |
| `Maybe` / `Result` / `List` | compiled-source (canonical) | Combinators case-match their own ctors. Kernel-shadowed today; migrate opportunistically. |
| `Dict` / `Set` / `Bytes` / `Crypto` | kernel | Opaque primitives, never destructured in Ipê. |
| `File`/`Http`/`Time`/`Db`/`Task`/`System`/`Io`/`Random` | kernel | Effect boundaries. |
| `Ipe.Ui` / `Ipe.Html` | split (later) | Huge own ADTs (→ source) but carry render/escape kernels (→ leaf). |

---

## 6. Build tasks (ordered — SPIKE first)

The spike validates every seam — injection, resolve relaxation, both gate
exemptions, the annotation gate, and Std-homed-union ctor lowering — on a
~15-line surface, **before** authoring the 1500-line `Ipe.Css`.

1. **SPIKE — tiny compiled-source module with its own ADT.**
   - `crates/ipe/stdlib/Std/Palette.ipe` (~15 lines, fully annotated):
     `module Ipe.Palette exposing (Shade(..), toHex)` /
     `type Shade = Dark | Light` / `toHex : Shade -> String` case-matching
     `Shade`. Exercises the exact hard part impossible for a kernel: a
     Std-source module **defining and matching its own ctor**. Chosen to avoid
     reserved type names (`Length`/`Color`) so it validates injection
     independently of the reserved-name exemption.
   - `stdlib.rs`: add `COMPILED_STD_MODULES` + `compiled_std_source` +
     `is_compiled_source`; add the disjointness test vs `STDLIB_MODULE_QUALIFIERS`;
     extend `every_embedded_module_parses`.
   - `project.rs`: implement `inject_compiled_std_closure` (with the
     no-import short-circuit).
   - `lib.rs`: extract `compile_modules`; route `build` **and** `build_project`
     through it; wire the closure before `topological_order`.
   - `resolve.rs`: relax the `:244` continue with `&& !deps.contains_key(dep_path)`;
     thread `ModuleOrigin`; exempt `EmbeddedStdlib` from N0025; add the
     fail-closed unannotated-top-level gate.
   - `examples/spike-std-source/` (`sky.toml` + `src/Main.ipe` doing
     `import Ipe.Palette exposing (Shade(..), toHex)` and `toHex Dark`).
   - **GREEN gate:** `ipe build` exit-0 (no `IPE-N0020`/`N0025`), emits, then
     the emitted Cargo project `cargo build`s and **runs to the expected value**
     under `IPE_E2E=1`. Add this as an integration test.

2. **Fail-closed resolution + N0025 tightening.** Confirm: an unresolved
   `Ipe.*`/`Ipê.*` import that is neither injected nor a registered kernel
   qualifier is a clean `IPE-N`; a user file literally named `Ipe.Foo` is
   rejected by N0025 (`ModuleOrigin::User`). Regression-test a **mixed** import
   set (kernel `Ipe.Prelude` + source `Ipe.Palette` in one module) so the
   `:244` conjunct keeps kernels on the `continue` path.

3. **Std↔Std closure test.** A source module importing a second source module
   is closure-expanded (walk `extract_imports_from_source` on the embedded
   string) and ordered dep-first.

4. **Author `Std/Css.ipe`** — 19 ADTs (`CssProp` incl. `CssDropped`, `CssRule`
   + variants, `Length`, `Color`, keyword enums), typed builders
   (`px`/`rem`/`hex`/`rgb`/`color`, pure Ipê), render fold (pure Ipê), and the
   gated `property`/`rule`/`media`/`raw` entries. `CssProp` exported opaque
   (type without ctor).

5. **Runtime CSS migration.** Shrink `css.rs` to the four primitive leaf shims
   over unchanged `css_safety.rs` policy; retire the reflection enums + old
   builder/sink kernels; register the four leaf kernels in `sky_kernels`. Add
   the breakout-floor test: `styleNode [] attackerCss` rendered via
   `Ipe.Html.render` strips `</style>` at the render sink. Add the opaque-ctor
   test: `CssProp` is exposed **without** its ctor, and a raw string via
   `Css.property` is sanitized/dropped.

6. **Register `Ipe.Css`** in `COMPILED_STD_MODULES` (**not**
   `STDLIB_MODULE_QUALIFIERS`); an example importing `Ipe.Css` builds and
   `cargo`-compiles (doubles as the ordering regression: closure runs before
   topo).

7. **Unpark `Ipe.Error` (#85)** via the identical mechanism — a separate
   atomic commit to isolate the ~69-golden churn from the Css work.

8. **File** whole-program rank-based let-generalisation as an incremental
   follow-up (unblocks future un-annotated-polymorphic stdlib helpers only).

---

## 7. Rejected & why

- **Upstream "stdlib root last ⇒ user shadows stdlib" + presence-based
  silent-kernel fallthrough.** Rejected on **security #1**. A stray
  `Std/Auth.ipe` silently overriding the audited bcrypt+JWT implementation is a
  supply-chain hazard; a typo silently resolving to a phantom kernel is
  fail-open. We keep bundled stdlib authoritative (user `Ipe.`/`Ipê.` = hard
  error) and make resolution fail-closed (source XOR registered kernel XOR clean
  error).

- **Design-2 (keep `css.rs` `CssProp`/`CssRule` reflection enums + a
  `css_stylesheet_` sink kernel that reconstructs the Rust enum from a lowered
  Ipê ADT).** Security-*equivalent* to the chosen path but rejected on
  **completeness #5 + soundness #3 + readability #6**: it reintroduces exactly
  the compiled-ADT ↔ runtime-enum reflection the pure-Ipê north star exists to
  delete — fragile (every ADT edit must mirror `css.rs` or silently drift) and a
  large kernel surface. The primitive-kernel middle path gets Design-2's
  security with none of the reflection.

- **Pure Design-1 (pure-Ipê Css, breakout-floor only, no per-declaration
  gate).** Rejected on **security #1**: it silently drops the `@import` /
  `url(javascript:)` / `expression()` protection the port already owns. The
  floor catches breakout, not in-declaration exfil sinks (`background:
  url(http-exfil)` never breaks out). Not acceptable when the gate costs four
  thin primitive kernels.

- **Escape-hatch-gate-only (gate just `Css.property`).** The orchestrator's
  lean, rejected on **security #1**: a raw **selector** (`Css.rule sel …`) or
  **media query** (`Css.media q …`) is *also* a free-string entry —
  `SafeCssSelector` proves the `@import`-via-selector vector. Value-only gating
  misses it. All three free-string entries must gate.

- **Whole-program let-generalisation as a prerequisite.** Rejected as
  mis-scoped (§3): annotation-driven per-module generalisation (matching
  upstream) already handles the fully-annotated Css/Error. A fail-closed
  annotation gate makes the precondition machine-checked; rank-based
  generalisation is a later incremental item, never a soundness/security gate.

- **Bespoke "add all embedded stdlib as roots" (upstream-style).** Rejected on
  **efficiency #4** vs the transitive closure: injecting + parsing `Ipe.Css` on
  every build even when unused. The closure short-circuits when no import
  matches, so unused-Std builds pay nothing.
