# The Ipê Language Server (LSP) — Design Spec

Status: design-only (no code). Authoritative synthesis of the LSP design panel.
Scope: a JSON-RPC-over-stdio language server giving every LSP-compliant editor
(VS Code, JetBrains, Neovim, Helix, Emacs, Zed) rich Ipê support, with a
headline feature — **editor-agnostic TEA-scaffolding** delivered through
standard-LSP snippet completions, code actions, and lint quick-fixes.

Related specs: `incremental-compilation-and-watch.md` (the salsa query layer this
server consumes), `roadmap.md` §C.2 (flat namespace + auto-import), the reference
Haskell server at `../sky/src/Sky/Lsp/{Server,Index,Diag}.hs`.

Naming note: the `sky`→`ipe` project rename has landed — compiler crates are
`ipe_*` under `src/compiler/`, the driver crate `ipe` is `src/ipe-cli`, and
the LSP crates are `ipe_lsp_*` under `src/lsp/`. Crate names in this spec
reflect that layout.

---

## Executive summary

The LSP is **not an analyzer**. It is a second *consumer* of the one salsa query
graph — exactly like `ipe watch` — sitting on top of the existing compiler crate
DAG (`ipe_parse → ipe_canon → ipe_types → ipe_lower → ipe_ir`). It never
re-implements parsing, name resolution, or type inference; every diagnostic,
hover type, completion type, rename identity, and — critically — every
scaffolding insert is produced or verified by the *same* checker `ipe` runs. A
divergent second analyzer that could disagree with `ipe` is the cardinal defect
and is foreclosed structurally.

- **Framework:** `lsp-server` (rust-analyzer's synchronous crate) + a
  single-writer main loop that owns the salsa `Db`, with reads dispatched to a
  worker pool over `snapshot()`s. Chosen because salsa cancellation is
  synchronous unwinding gated on `&mut` writes and composes with a sync loop; the
  async `&self` model of `tower-lsp` fights it.
- **Build order vs salsa:** ship a **pre-salsa v0** behind a stable analysis
  trait, backed by non-incremental whole-project recompute against the *same*
  compiler crates (slower, never divergent). Swap the salsa backend in later with
  **zero handler changes**. The LSP does not block on the salsa layer landing.
- **TEA-scaffolding catalogue (the headline):**
  - Snippets (context-free creation): `tea-app`, `tea-tui`, `tea-webview`,
    `tea-worker`, `msg`, `update-arm`, `sub`, `cmd-perform`, `handler`, `route`.
  - Code actions (program-reading transforms): *Scaffold TEA app for this
    module*, *Add Msg variant + matching update arm*, *Add a subscription*,
    *Convert `main = Task.run …` to a TEA worker*.
  - Lints + quick-fix (compiler-sourced): *Msg variant with no update arm* (the
    exhaustiveness diagnostic), *update arm not returning `(Model, Cmd)`* (a type
    error), *growing imperative `main`* (a stylistic hint).
- **Editor-agnostic by construction:** snippet completions and code actions are
  base-LSP-spec features every client renders with no per-editor plugin.
- **Soundness guarantees, designed first:** (1) one type-checker by construction;
  (2) every synthesized edit passes a full parse+canon+typecheck+exhaustiveness
  round-trip before it is offered/applied — a scaffold that breaks the build is
  unrepresentable; (3) resilient parser + total query paths + `catch_unwind` so
  half-typed buffers never crash the server; (4) exhaustive walker `match`es make
  a missing walker arm for a new AST node a *compile error*.

**Build-order verdict re: salsa:** do NOT block on salsa. Ship v0 on the batch
backend behind the analysis trait; features are gated by *verification-cost
class*, not by "does it compile against the trait" — cheap O(single-read)
features (diagnostics, hover, formatting, snippets, symbols, go-to-def) ship in
v0; features that need O(full-recheck-per-invocation) (speculatively-verified
code actions, auto-import) or O(whole-program) (find-refs, workspace symbol
index) are built early behind the trait but *enabled* when the salsa backend
delivers sub-100 ms verification.

---

## Principles (ordering is literal)

`security > correctness > soundness > efficiency > completeness > readability`,
plus the two fundamental rules: **PARSE, DON'T VALIDATE** and **MAKE INVALID
STATES UNREPRESENTABLE**. LSP-specific corollaries, each foreclosed below:

- (a) One compiler, one type-checker, one formatter — never a divergent second
  analyzer.
- (b) Every code action / quick-fix / scaffold yields parse-clean, type-clean,
  `ipe fmt`-clean Ipê. An insert that breaks the build is a defect.
- (c) No panic on malformed/partial in-editor buffers — the server eats
  half-typed code constantly and must degrade gracefully, never crash.

---

## Q1 — Architecture

**Decision.** The LSP is a new thin server crate over a shared salsa query layer
that wraps the existing compiler crates; it holds no language logic of its own.
*Rationale:* the reuse and anti-divergence mandate is only structural if the LSP
literally has nothing to type-check *with* except the compiler's own queries.

**Decision (framework).** `lsp-server` + synchronous single-writer main loop +
snapshot readers. *Rationale:* salsa's synchronous cancellation composes with a
loop that owns the `Db`; `tower-lsp`'s async handlers fight it — the reason
rust-analyzer (the reference salsa consumer) declined `tower-lsp`.

**Decision (build order).** Ship a pre-salsa v0 behind an analysis trait; swap
salsa in later without touching handlers. *Rationale:* v0 reuses the same
compiler crates, so it is slower but never divergent; the LSP must not block on
the (large, longer-horizon) salsa effort.

### Crate topology

```
ipe_parse ─┐
ipe_canon ─┤
ipe_types ─┼─ (existing compiler DAG) ── ipe_db   ◄── shared salsa query layer
ipe_lower ─┤                              ▲      ▲
ipe_ir   ──┘                              │      │
                            ┌─────────────┘      └──────────────┐
                       ipe watch                             ipe_lsp  (NEW binary: `ipe lsp`)
                       (other consumer)                      ├── ipe_lsp_server   — main loop, JSON-RPC, VFS, cancellation, offset mapping
                                                             ├── ipe_lsp_features — hover/def/refs/completion/semtok/symbols/rename/format handlers
                                                             └── ipe_lsp_tea      — TEA snippet catalog + code-action generators + lint→quickfix
```

- **`ipe_db`** — the salsa database crate mandated by
  `incremental-compilation-and-watch.md`. It is **shared**, not LSP-private: that
  document names the LSP the *primary* salsa consumer and states the query layer
  feeds both the LSP and `ipe watch`. Inputs (`source_text(FileId)`,
  `file_set()`, `project_config()`, `codegen_flags()`,
  `ffi_package_interface(PackageId)` — a reserved seam, FFI is parked),
  `compiler_revision()`, `toolchain_fingerprint()`) and derived queries (`parse`,
  `resolve_imports`, `module_interface`, `canonicalize`, `typecheck`, `lower`)
  are exactly as locked there. The LSP consumes the front half (parse →
  typecheck); it stops at `lower`/`program_metadata` for whole-program lints and
  **never drives emit/cargo**.
- **`ipe_lsp_features`** handlers are **pure functions from `(analysis snapshot,
  position) → LSP payload`**. Zero parsing/typing/resolution logic. This is what
  makes "the LSP cannot disagree with `ipe`" structurally true: it has nothing
  to disagree *with*.
- **Capability rule (INV-1 discipline).** `ipe_db` and the feature handlers
  hold no `std::fs`/`std::env`/`std::io` capability on the query path.
  Filesystem/stdio access lives only in `ipe_lsp_server`. A query that reads the
  world is a compile-time-visible design error, not a latent staleness bug.

### The analysis trait (the backend-swap seam)

Every handler is written against a single trait — the seam that lets v0 (batch)
and v1 (salsa) coexist without touching a single capability:

```
trait ProgramView {
    fn parse(&self, f: FileId) -> Arc<ParseResult>;          // green tree + diagnostics; never Err
    fn resolve(&self, m: ModuleId) -> Arc<Resolution>;
    fn typecheck(&self, m: ModuleId) -> Arc<TypeckResult>;   // types, region-types, diagnostics
    fn diagnostics(&self, f: FileId) -> Vec<Diagnostic>;     // union parse+canon+type+exhaustiveness
    fn index(&self) -> Arc<SymbolIndex>;                     // defs/refs/symbols
    // hover_at / defs_for / refs_to / completions_at / sem_tokens / doc_symbols … all derived
}
```

- **v0 backend (`BatchView`):** calls the existing compiler front-end crates as a
  batch, debounced, on whole-file settle; memoizes the last result per module in
  a coarse cache keyed by content hash. Correct and simple. Critically, it uses
  the *identical* parser/canonicaliser/type-checker — so **v0 cannot diverge from
  `ipe` either**; it is only slower.
- **v1 backend (`SalsaView`):** the same trait over `ipe_db`. Red-green gives
  keystroke-frequency incrementality; the `module_interface` firewall means a
  body edit re-checks only the edited module. **No handler changes** — the trait
  signatures are stable from day one.

### Salsa integration — how incrementality drives responsiveness

- **`didChange` → `set_source_text(FileId, text)`** on the main thread. Salsa
  marks dependents dirty but recomputes nothing eagerly. Byte-equal re-set is an
  input-boundary no-op (auto-save storms don't churn).
- **Reads are pulls on a `snapshot()`** dispatched to a blocking pool. Hover =
  `typecheck(module).region_types[region_at(pos)]` — a memoized read, sub-ms
  warm. A hover right after a diagnostics pass reuses `typecheck(module)`.
- **Diagnostics are demand-driven and debounced** (~80–150 ms quiescence). On a
  settled edit the loop demands `typecheck` for open documents and their
  importers (the `resolve_imports` reverse edge names who to refresh), then maps
  the compiler's own `Diagnostic` values to `publishDiagnostics`. The
  `module_interface` firewall keeps a body-only edit sub-100 ms warm.

### Document sync + cancellation

- **Sync kind: Incremental** (`TextDocumentSyncKind::INCREMENTAL`). Open buffers
  are held in a `ropey` rope; range edits apply in O(log n); the rope also gives
  cheap **UTF-16 ↔ byte-offset** conversion (LSP positions are UTF-16 code units;
  the compiler's `Span` is bytes — a correctness footgun that lands *every* edit
  in the wrong place if wrong). This conversion is a single named,
  property-tested module (hazard L-I).
- **VFS overlay (the LSP ↔ watch reconciliation point).** A `Vfs` layer resolves
  each `FileId` to the open editor buffer if present, else disk bytes; `didClose`
  reverts to disk. Both the LSP and `ipe watch` feed the *same* salsa inputs,
  differing only in who sets them and whether unsaved buffers shadow disk.
- **Cancellation.** Only the main loop mutates the `Db`, applying
  `didOpen`/`didChange`/`didClose` in receipt order (single writer). A write bumps
  the salsa revision, which cancels in-flight read snapshots via salsa's
  `Cancelled` unwind; the worker unwinds cleanly and the handler returns LSP
  `ContentModified (-32801)`. The client re-requests against the new state. A
  stale hover can never be delivered against text the user already changed
  (correctness > efficiency). `$/cancelRequest` cooperatively drops the pull. A
  per-request latency budget (≈ the reference server's 3 s) returns a friendly
  "request exceeded budget" rather than hanging.

---

## Q2 — Core capabilities and priority

**Decision.** Prioritise by value-per-unit-soundness-risk and by
verification-cost class, not by protocol completeness. *Rationale:* a capability
that could disagree with `ipe` or crash on partial input is gated before any
ergonomic feature; a capability whose interactive latency is structurally bad on
the v0 backend is built early but *enabled* on salsa.

| Prio | Capability | Source (one type-checker) | Cost class | Ships in |
|---|---|---|---|---|
| P0 | Document sync + no-crash-on-partial | resilient parser | foundation | v0 |
| P0 | **Diagnostics** | `parse`/`canonicalize`/`typecheck` **verbatim** | single read | v0 |
| P1 | Hover (types) | region-types from `typecheck`; kernel sigs from `kernel_types()` | single read | v0 |
| P1 | Go-to-definition / declaration | `resolve_imports` + canonical binding sites | single read | v0 |
| P1 | Document symbols | `parse` top-level bindings/types | single read | v0 |
| P1 | Formatting | delegate to the `Format` crate (`ipe fmt`), one full-doc `TextEdit` | single read | v0 |
| P2 | Completion (scope + type-directed) | resolved scope + region types + symbol table | read + index | v0 (basic) / salsa (full) |
| P2 | Semantic tokens | exhaustive AST walker (Q5) | single read | v0 |
| P2 | Find references | `collect_references` walker over the resolved program | whole-program | salsa |
| P2 | Completion auto-import (Q4) | canonicaliser export index (salsa query) | whole-program | salsa |
| P3 | TEA code actions / quick-fixes (Q3) | speculatively verified WorkspaceEdit | full recheck/invoke | salsa (v0: apply-time-verified — OPEN-2) |
| P3 | Rename | `prepareRename` gate + refs walker + speculative typecheck | whole-program | salsa |
| P3 | Inlay hints | region-types (inferred `let`/param types) | single read | salsa |
| P3 | Signature help | `typecheck` callee signatures | single read | salsa |

Formatting reuses the exact `Format` crate `ipe fmt` uses; the result must be
idempotent (a second pass is byte-identical, an existing project invariant).
Never a second formatter (hazard L-K). Formatting degrades gracefully on a
non-parsing buffer (returns no edits rather than mangling — principle c).

---

## Q3 — TEA scaffolding (the headline)

**Decision.** Deliver TEA scaffolding through three standard-LSP mechanisms,
selected by how much semantic context the insertion needs: **snippet
completions** for context-free creation, **code actions** for program-reading
transforms, **lint diagnostics with quick-fixes** for detected drift.
*Rationale:* all three are base-LSP-spec features every client renders with no
per-editor plugin, so one server-side implementation reaches every editor.

### Why this is the editor-agnostic path

`CompletionItem` with `insertTextFormat = Snippet` (`$1`/`${2:default}`
tabstops), `CodeAction` returning a `WorkspaceEdit`, and diagnostics carrying
quick-fixes are all in the base LSP spec. Every mainstream client — VS Code,
JetBrains LSP, Neovim built-in LSP, Helix, Emacs eglot/lsp-mode, Zed — consumes
them with **zero per-editor plugin code** beyond registering the `ipe lsp`
binary. The alternative — per-editor snippet engines (VS Code `.code-snippets`,
UltiSnips, YASnippet) or a bespoke extension — would need N per-editor
implementations and would drift. Putting scaffolding *in the server* is the whole
reason the headline reaches every LSP editor. Snippet support is a client
capability (`completionItem.snippetSupport`); when a client does not advertise
it, the server falls back to offering the same scaffolds as **code actions**
(plain-text `WorkspaceEdit`s), which every client supports.

### Mechanism selection rule

- **No context / user typing a trigger →** snippet completion.
- **Needs to read the typed program to produce a correct edit →** code action.
- **A detectable defect/opportunity the checker already sees →** lint diagnostic +
  quick-fix.

### (a) Snippet completions — context-free creation at the cursor

Fixed, reviewed catalog. Each snippet's skeleton, with its default tabstop values
filled, is parse-clean and `ipe fmt`-clean **by construction**, enforced by a
golden test (Q5). Snippets never claim the *filled* result type-checks — a bad
fill lights up from the same checker's next diagnostics pass.

**Two type-safety classes (explicit classification).** The catalog splits by
whether the expansion is a *self-contained program* or a *fragment referencing
free names*:

- **Self-contained / type-clean** — the expansion is (or completes) a whole
  type-checking program on its own. `tea-app`, `tea-tui`, `tea-webview`,
  `tea-worker` expand to a full TEA skeleton (`Model`, `Msg`, `init`, `update`,
  `view`, `subscriptions`, wiring) whose default fill parses, formats, **and**
  type-checks standalone. `msg`, `route`, `handler` are self-contained
  declarations that type-check on their own.
- **Fragment / skeleton-parse+fmt-clean only** — the expansion references free
  names bound in the surrounding module and is *not* type-clean in isolation:
  `update-arm` (`${1:Variant} -> ( model, Cmd.none )` references `model`),
  `cmd-perform` (`Cmd.perform (${1:task}) ${2:ResultMsg}` references `task` /
  `ResultMsg`), `sub` (`Sub.batch [ … ]` references whatever it wires). These are
  guaranteed only *skeleton parse-clean + fmt-clean*; their type-correctness is
  adjudicated by the one live checker's next diagnostics pass once the fragment is
  applied into its enclosing context, exactly like any hand-typed edit. The golden
  test (Q5) asserts each class at its own bar — full type-check for the
  self-contained set, parse+fmt only for the fragment set — so no fragment is ever
  over-claimed as a type guarantee.

| Trigger | Expands to (default tabstops shown) |
|---|---|
| `tea-app` | full `Live.app` skeleton — `type Msg`, `Model` alias, `init`, `update` (with `case msg of` scaffold), `view`, `subscriptions`, and `Live.app { … }` wiring |
| `tea-tui` | as `tea-app` but `main = Tui.app cfg \| Task.run` |
| `tea-webview` | as `tea-app` but `main = Webview.app cfg \| Task.run`, `window` cfg |
| `tea-worker` | `main = Task.run scheduledWork` Ipe.Cli worker shape |
| `msg` | `type Msg = ${1:Variant}` |
| `update-arm` | `${1:Variant} -> ( model, Cmd.none )` |
| `sub` | `subscriptions model = Sub.none` / `Sub.batch [ … ]` |
| `cmd-perform` | `Cmd.perform (${1:task}) ${2:ResultMsg}` |
| `handler` | `${1:name} : Handler` + `${1:name} req = Task.succeed (Server.text "${2}")` |
| `route` | `route "${1:/path}" ${2:Page}` |

Before/after for `tea-app` (empty line → accepted with defaults):

```elm
-- before: (cursor on an empty line, having typed `tea-app`)

-- after:
type alias Model = { count : Int }

type Msg = Increment | Decrement

init : () -> ( Model, Cmd Msg )
init _ = ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment -> ( model, Cmd.none )
        Decrement -> ( model, Cmd.none )

view : Model -> Element Msg
view model = Ui.text "hello"

subscriptions : Model -> Sub Msg
subscriptions _ = Sub.none

main =
    Live.app { init = init, update = update, view = view, subscriptions = subscriptions }
```

Note: linked tabstops keep a *name* in sync as the user types it once; they do
**not** enforce the variant↔arm exhaustiveness invariant. That invariant belongs
to the code action (b) and the exhaustiveness lint (c) — an over-trusted snippet
is a quiet path to a scaffold that violates guarantee (b).

### (b) Code actions — program-reading transforms

These read `resolve`/`typecheck` outputs and emit a `WorkspaceEdit` built from
structured (typed-IR / AST) insertion, never string concatenation into the
buffer. Each is speculatively verified before it is surfaced/applied (Q5).

**Scaffold TEA app for this module** — offered on a module with no `main`.
Inserts a coherent `Live.app` / `Tui.app` / `Webview.app` skeleton (offered as
separate action titles per app shape from the matrix). Because it reads the
module, it seeds `Model`/`Msg` from existing decls rather than duplicating them.
For the Ipe.Ui-heavy case the AGENTS.md guidance to split State/Update/View
applies: the action offers a *single-module* skeleton by default and a
*multi-module split* variant for larger apps.

**Add Msg variant + matching update arm** — the flagship. Invoked on a `Msg` ADT
or an `update` case. One atomic `WorkspaceEdit` appends `| NewVariant` to the
`Msg` decl *and* inserts `NewVariant -> ( model, Cmd.none )` into `case msg of`.

```elm
-- before:
type Msg = Increment | Decrement
update msg model =
    case msg of
        Increment -> ( { model | count = model.count + 1 }, Cmd.none )
        Decrement -> ( { model | count = model.count - 1 }, Cmd.none )

-- after "Add Msg variant + matching update arm" (variant name is a placeholder):
type Msg = Increment | Decrement | Reset
update msg model =
    case msg of
        Increment -> ( { model | count = model.count + 1 }, Cmd.none )
        Decrement -> ( { model | count = model.count - 1 }, Cmd.none )
        Reset     -> ( model, Cmd.none )
```

Adding the variant without its arm would make `update` non-exhaustive (a compile
error), so the action always emits *both* edits — the post-edit program
type-checks by construction. This is "make invalid states unrepresentable" at the
tooling level.

**Add a subscription** — extends/creates `subscriptions`, wiring a
`Sub.subscribeTopic`/`Sub.every` (and, if needed, the receiving Msg variant +
update arm), adding the `Ipe.Sub` import if absent.

```elm
-- before:
subscriptions _ = Sub.none

-- after:
subscriptions model =
    Sub.batch [ Time.every 1000 Tick ]
```

**Convert `main = Task.run …` to a TEA worker** — ties the Task-everywhere
design. Offered when `main` is a growing `Task.run` chain. Lifts the imperative
body into an `init`/`update`/`subscriptions` worker shape, preserving the effect
as the initial `Cmd`.

```elm
-- before:
main =
    Task.run
        (File.readFile "in.txt"
            |> Task.andThen process
            |> Task.andThen (File.writeFile "out.txt"))

-- after (Ipe.Cli TEA worker):
type Msg = Loaded (Result Error String) | Wrote (Result Error ())
init _ = ( {}, Cmd.perform (File.readFile "in.txt") Loaded )
update msg model =
    case msg of
        Loaded (Ok s)  -> ( model, Cmd.perform (File.writeFile "out.txt" (process s)) Wrote )
        Loaded (Err e) -> ( model, reportError e )
        Wrote _        -> ( model, Cmd.none )
main = Cli.app { init = init, update = update, subscriptions = \_ -> Sub.none } |> Task.run
```

### (c) Lints / hints — compiler-sourced, with quick-fixes

Wherever the compiler already emits the relevant `Diagnostic`, the LSP surfaces
it directly — the lint lives in the *compiler's* diagnostic set (one analyzer),
not a divergent LSP-only linter.

| Lint | Source | Quick-fix |
|---|---|---|
| **Msg variant with no update arm** | the exhaustiveness checker's own non-exhaustive-`case` diagnostic | "Add missing arm(s)" — the same generator as action (b) |
| **update arm not returning `(Model, Cmd msg)`** | a type error the checker already produces | wrap the return as `( _, Cmd.none )` |
| **growing imperative `main`** | LSP-local syntactic shape check (severity Hint) | offer "Convert to TEA worker" (action b) |

The first two are the compiler's diagnostics wearing a quick-fix — no new
analysis. Only the third is LSP-originated; it is a *stylistic, build-irrelevant*
Hint that makes no semantic claim `ipe` adjudicates, individually toggleable,
and clearly quarantined (Q5-G1). Even this stylistic lint's quick-fix synthesizes
an edit, so its fix still passes the speculative-verify gate before it is offered
— an LSP-originated judgment can never introduce a build-breaking fix.

---

## Q4 — Auto-import

**Decision.** Ship auto-import against the *current* namespace via
`completionItem/resolve` `additionalTextEdits` plus an unresolved-name quick-fix,
resolving through the canonicaliser's own export index (a salsa query, never an
LSP-maintained parallel list); ambiguity yields a disambiguation list, never a
silent pick. *Rationale:* the mechanism is namespace-shape-agnostic — it reads
whatever resolution the canonicaliser produces — so it does **not** block on the
C.2 flat-namespace redesign.

### Mechanism (two entry points, one resolution source)

1. **Completion + lazy resolve.** `completions_at` offers in-scope *and*
   not-yet-imported symbols drawn from the canonicaliser's export index. The
   import edit is attached lazily via `completionItem/resolve`: on selection the
   resolved item carries `additionalTextEdits` inserting `import Ipe.Money
   exposing (allocate)` (or extending an existing `exposing (…)` list, or
   adopting an existing alias) at the correct sorted position, deduped, run
   through the `Format` crate so it is fmt-clean and idempotent with format-on-
   save. Not-yet-imported items sort below in-scope names.
2. **Quick-fix on the unresolved-name diagnostic.** The canonicaliser already
   emits an unresolved-name / did-you-mean diagnostic (AGENTS.md §3.1). The LSP
   attaches "Import `X` from `Ipe.Y`" quick-fixes — one per candidate module,
   using the same shared import-insertion helper as the completion path.

Both paths run the resulting edit through the speculative-verify gate (Q5), so an
auto-import can never introduce an ambiguous or cyclic import that breaks the
build.

### Dependency on the flat-namespace redesign (roadmap C.2)

The **mechanism** is independent of C.2 and ships now. C.2 changes only two
things, both non-mechanical:

- It *raises the value* of auto-import (nothing imported by default → used
  constantly).
- It *raises collision frequency* (a flat pool means bare-name collisions become
  routine, not rare). This **promotes the refuse-don't-guess disambiguation list
  to a day-one priority requirement**, not a polish item — it is the one part of
  auto-import C.2 actually affects, and it is a UX-priority change, not a
  mechanism change.

---

## Q5 — Reuse and soundness guarantees

**Decision.** Design the four guarantees first; every capability is built to
satisfy them. *Rationale:* under the principle order, a fast-but-divergent or
crash-prone or build-breaking feature is worse than a missing one.

### G1 — One type-checker (forecloses the divergent-analyzer hazard)

Structural, not disciplinary. `ipe_lsp_features` depends on the compiler crates
and reads `parse`/`resolve`/`typecheck`/`module_interface` via the trait; it has
**no `parse`/`resolve`/`infer` of its own**. Diagnostics published to the editor
are the compiler's `Diagnostic` values verbatim. Because there is nothing else to
consult, the LSP cannot produce a diagnostic `ipe` would not, or miss one it
would. Enforcement: a reviewer greps `ipe_lsp_features` for a solver/parser call
— finding one is the bug. The tempting shortcut — a fast approximate IDE parser
that "usually agrees" — is rejected outright; salsa incrementality *is* the
responsiveness mechanism, so there is no motive for a lying-but-fast shadow.

The **sole** sanctioned LSP-originated analysis is the quarantined stylistic
advisory-lint set (the growing-imperative-`main` hint): Hint severity,
build-irrelevant, individually toggleable, making no semantic claim. Every
*semantic* lint (type, exhaustiveness, reachability) must be sourced from a
compiler diagnostic. Even the stylistic lint's quick-fix passes the G2 gate.

### G2 — Every synthesized edit yields parse-clean, type-clean, fmt-clean Ipê

Two provenance channels, not one:

1. **Compiler-sourced fixes** (from `ipe_diagnostics`'s existing
   `Suggestion { span, replacement, applicability }` on `HelpLine::Suggest`)
   inherit the compiler's `Applicability` confidence model. `MachineApplicable` →
   offered as a preferred quick-fix; `HasPlaceholders` → rendered as a
   snippet-format edit so the user must fill blanks; `MaybeIncorrect` → offered
   non-preferred. This reuses the compiler's own confidence rather than
   re-deciding safety in the LSP.
2. **LSP-synthesized edits** (TEA scaffolds, auto-import inserts, rename) have no
   compiler provenance, so they pass a verification gate. The edit is built from
   structured typed-IR/AST insertion (never string concatenation of
   program-derived data — mirrors the emit-time injection foreclosure), applied
   to an in-memory/scratch-overlay copy, and run through the *same*
   `parse → canonicalize → typecheck` queries (plus exhaustiveness for the
   variant/arm action), then through `ipe fmt`.

**The verification scope MUST equal the `WorkspaceEdit`'s blast radius, not one
module.** A `WorkspaceEdit` is multi-file by nature: rename touches every file
holding a reference; the *multi-module split* scaffold variant writes several new
modules; an auto-import that extends an `exposing (…)` list changes that module's
`module_interface`. A gate that round-trips only the edited/invocation module can
pass while a *downstream importer* breaks — a stale reference after a rename, a
now-cyclic import, a `module_interface` change that violates a consumer's
expectations. So the `VerifiedEdit` constructor re-checks the full closure: **every
file the edit's `changes`/`documentChanges` touch, PLUS every importer of any
module whose `module_interface` the edit alters**, reached via the
`resolve_imports` reverse edge the spec already names for diagnostics refresh
(§"Diagnostics are demand-driven"). Body-only edits whose `module_interface` is
unchanged (e.g. *Add update arm*) collapse to the single edited module by that
same firewall — the importer set is empty — so the common case stays cheap; the
scope only widens when the interface actually moves.

The gate is expressed as a type: a `WorkspaceEdit` is derivable **only** from an
`Ok(VerifiedEdit)`, whose sole constructor runs the round-trip over that full
closure and returns `Err`
(never panics in release; debug-build panic is a test tripwire only) if any stage
is unclean in *any* touched-or-importing module. So "offer an action that breaks
the build" — including "breaks a build the user isn't looking at" — is
unrepresentable: there is no code path from a rejected edit to a surfaced
`WorkspaceEdit`. The overlay recompute is the constructor's engine; the type is
the foreclosure.

**fmt must be parse/type-preserving (load-bearing, tested).** The round-trip
type-checks the structured artifact and then runs `ipe fmt` — but the bytes
actually written to the buffer are the *post-fmt* ones, while the stage that
proved type-cleanliness ran on the *pre-fmt* artifact. The gate is only sound if
formatting cannot change what parses or what type-checks. This holds because
`ipe fmt` is a whitespace-only, idempotent reprinter (an existing project
invariant — see Q2), so the post-fmt artifact parses to the same AST and
type-checks identically. That property is load-bearing, so it is **asserted by
test**: for every `VerifiedEdit`, the post-fmt artifact must parse to the same
canonical AST and produce the same `typecheck` result as the checked pre-fmt
artifact (and a second fmt pass is byte-identical — idempotence, per L-K). A
formatter change that ever perturbed parse/type structure would trip this test
rather than silently let an unverified byte-image reach the buffer.

**Verification timing tracks backend cost (OPEN-2 residual noted below):**

- **salsa backend:** verify-on-offer — unsound actions are hidden, never shown
  (the incremental overlay recompute is cheap).
- **v0 batch backend:** verify-on-apply — offering is heuristic/structural; on
  selection the post-image is re-checked and the action *refuses with a message*
  if unclean. This holds the hard guarantee (never emit an unclean edit) while
  keeping v0 responsive; the only cost is a rare offered-then-refused action, a UX
  blemish erased once salsa lands.

Rename (highest risk) additionally: `prepareRename` refuses non-project-owned
targets (keywords, kernel/FFI names, module-prefixed bindings that collide with
Go/Rust reserved-name rewriting); the edit is built from the *same*
`collect_references` walker find-refs uses (so refs and rename cannot disagree);
and it passes the G2 gate before applying.

### G3 — No crash on partial/malformed buffers (PARSE, DON'T VALIDATE)

Three layers:

1. **Resilient parser — an external precondition, not an assumed property.** This
   layer *wants* `ipe_parse` to produce a best-effort green tree with typed error
   nodes plus a diagnostic list for half-typed input, and to **never**
   `panic!`/`unwrap` — a partial parse as a first-class value, not an exception.
   But note the reference Haskell parser is **not** resilient in this sense: it
   hard-fails to a single `ModuleError` on the first syntax error rather than
   recovering a partial tree. The LSP's *no-crash* guarantee does **not** depend
   on parser resilience — layers 2 and 3 (total query paths + handler
   `catch_unwind`) hold it unconditionally, so a hard-failing parse degrades to
   "diagnostics only, no hover/completion" rather than a crash. What *does* depend
   on resilience is the **quality** of partial results: recoverable scope names in
   completion, hover over a syntactically-broken neighbour, symbols for the part
   that parsed. Therefore resilience is stated here as an **explicit precondition
   the Rust `ipe_parse` MUST satisfy** (best-effort green tree + typed error nodes,
   never `panic!`/`unwrap`), backed by a **fuzz gate** (random/truncated/mutated
   buffers → no panic, bounded time, non-empty tree) — a contract the parser crate
   owes the LSP, not a property the LSP assumes it already has.
2. **Total query paths.** Every handler returns `Option`/`Result` and degrades to
   graceful partial results — hover over an error node returns "no info", not a
   panic; completion still offers recoverable scope names; diagnostics show what
   parsed. No `.unwrap()` on AST navigation or document lookups; position→region
   mapping clamps to buffer bounds.
3. **Handler-boundary `catch_unwind`** (defense in depth). An unexpected panic
   becomes an internal-error response for *that request* and a logged
   `CompilerBug` line — never a killed server. Salsa's `Cancelled` unwind is
   caught and distinguished (→ `ContentModified`), never treated as a crash.

### G4 — Walker-arm exhaustiveness as a compile error

The reference server's fragility was catch-all `_ -> []` arms silently dropping
new AST nodes from semantic tokens / references. In Rust the walkers
(`sem_tokens`, `collect_references`, `expr_idents`, `expr_all_refs`,
`refs_in_expr`, `collect_sem_tokens`) `match` on the AST/IR enums with **no
wildcard arm**. Adding a new AST variant produces a non-exhaustive-match *compile
error* in `ipe_lsp_features` until every walker gets its arm — the type system
enforces the AGENTS.md "new AST node requires explicit walker arms" rule, strictly
stronger than the grep-audited `_ -> []` convention. Belt-and-braces:
`#![deny(clippy::wildcard_enum_match_arm)]` on the walker modules, a CI grep for
`_ =>` in AST matches, and a golden snapshot test of the semantic-token stream +
reference set over the example corpus (to catch a compiling-but-wrong arm).

### G5 — Security posture

The LSP speaks JSON-RPC over stdio, in-process with the editor — no network
control channel (it inherits `incremental-compilation-and-watch.md`'s
loopback/no-network posture trivially). It executes no project code: parse/canon/
type are pure, and completion/hover never touch FFI introspection (FFI enters
only as a reserved salsa *input* on the explicit `ipe add` path; an LSP-time cache
miss hard-refuses, never regenerates — INV from the incremental spec). Structured
edits never splice unescaped source-derived strings.

---

## OPEN DECISIONS

- **OPEN-1 — Shared vs private salsa cache across processes.** If the LSP session
  and a terminal `ipe watch` both run against one on-disk content-addressed
  `.ipe/lowered/` cache (locked as Option B in the incremental spec), the
  concurrency/locking model and the cross-process staleness hazard (incremental
  spec H13) must be pinned. Provisional stance: the LSP runs its own in-memory
  salsa db and shares only the *read-only* content-addressed lowered-IR artifacts;
  it does not write the shared cache. To confirm against the incremental spec's
  cache ownership rules.
- **OPEN-2 — v0 scope for heavy code actions.** Reconciled to "build behind the
  trait early, gate enablement by verification-cost class." Residual fork: whether
  v0 *ships* the TEA code actions in apply-time-verified form (accept the rare
  offered-then-refused blemish) or defers them entirely to the salsa backend.
  Provisional stance: ship the two cheapest program-reading actions
  (*Add Msg variant + arm*, *Add subscription*) apply-time-verified in v0; defer
  *Scaffold TEA app* and *Convert to worker* to salsa. To validate against
  measured v0 recompute latency on a representative project.
- **OPEN-3 — Framework escape hatch.** `lsp-server` is locked as primary.
  `tower-lsp` is recorded as an acceptable v0-only alternative *iff* the
  single-writer + snapshot-reader loop is built explicitly from day one (not
  leaning on per-request async tasks). Kept open only as a recorded fallback; no
  action unless `lsp-server` proves unworkable.
- **OPEN-4 — Crate/name timing. RESOLVED:** the `sky`→`ipe` rename landed
  before the LSP crates were created; they are `ipe_lsp_*` from their first
  commit. Residual `sky`-era runtime names the LSP still touches (`sky.toml`)
  follow the codebase-wide rename schedule.

---

## Build order (phased)

- **Phase 0 — spine (v0 backend).** `ipe_lsp` crates; `lsp-server` main loop;
  `initialize` capability negotiation; `ropey` VFS + incremental sync + the
  property-tested UTF-16↔byte mapper (built and tested first — everything depends
  on it); resilient-parse reliance + per-handler `catch_unwind` + latency budget;
  the `ProgramView` trait with the `BatchView` backend. Ship **diagnostics** (P0)
  — highest value, zero new analysis, live immediately.
- **Phase 1 — the alive editor (P1).** Hover, go-to-def, document symbols,
  formatting, basic completion, semantic tokens (exhaustive walkers, G4). Entirely
  off existing compiler outputs; genuinely useful before salsa exists.
- **Phase 2 — salsa cut-in.** Implement `ProgramView` on `ipe_db`; swap the
  backend with **no handler changes**. Enable find-references + the workspace
  symbol index + full type-directed completion (the O(whole-program) features).
- **Phase 3 — the headline.** TEA snippet catalog (golden-tested), then the
  code actions (each behind the G2 `VerifiedEdit` gate, verify-on-offer under
  salsa), then the lint→quick-fixes (the exhaustiveness-derived "missing update
  arm" first). Rename lands here (shares the refs infrastructure + the gate).
- **Phase 4 — auto-import.** Completion-resolve `additionalTextEdits` +
  unresolved-name quick-fix on the shared import-insertion helper, with the
  disambiguation list as a P-level requirement.

**Verdict re: salsa:** the LSP does not block on the salsa layer. It ships a
useful v0 on the batch backend behind a stable trait (Phases 0–1), then is made
incrementally fast by substituting the salsa backend (Phase 2) — a backend swap,
not a rewrite, because the trait signatures were stable from day one. This is the
same "backend swap not rewrite" property the incremental spec buys at the
`ipe_ir` cut-point, applied to the IDE query layer. Features are gated by
verification-cost class, not by "compiles against the trait": the speculatively-
verified transforms (Q3 code actions, Q4 auto-import) are *enabled* when salsa
delivers sub-100 ms verification.

---

## Hazard ledger (LSP-specific)

| # | Hazard | Class | Foreclosure |
|---|---|---|---|
| L-A | Fast approximate 2nd analyzer disagrees with `ipe` | divergent analyzer | Salsa incrementality *is* responsiveness → no motive; features read only compiler queries; grep for a solver call = bug |
| L-B | Scaffold/quick-fix/auto-import/rename doesn't type-check | non-type-checking edit | `VerifiedEdit` type: `WorkspaceEdit` derivable only from an `Ok` that passed the full round-trip; compiler-sourced fixes gated by `Applicability` |
| L-C | Server panics on half-typed buffer | crash-on-partial | Resilient total parser + total query paths + handler `catch_unwind`; `Cancelled` distinguished from panic; fuzz-tested |
| L-D | LSP keeps a private symbol/type index that drifts | divergent analyzer | The "index" IS derived salsa queries (`resolve_imports`, `module_interface`, export index), never a hand-maintained store |
| L-E | Auto-import picks the wrong/ambiguous module | wrong edit | Reuse canonicaliser resolution; ambiguity → disambiguation list (refuse-don't-guess), never a silent pick |
| L-F | Rename breaks the build (missed ref / reserved-name collision) | non-type-checking edit | `prepareRename` gate + shared `collect_references` walker + G2 gate + refuse kernel/FFI/reserved targets |
| L-G | VFS (open buffer) vs disk (watch) divergence | correctness | VFS overlay: open buffer shadows disk while open, reverts on `didClose`; both consumers feed the same salsa inputs |
| L-H | Stale read delivered against already-changed text | correctness | Salsa cancellation: a write cancels in-flight snapshots → `ContentModified`; client re-requests |
| L-I | UTF-16 (LSP) vs byte (compiler) offset mismatch | correctness | Single centralized `ropey`-backed offset conversion; property-tested |
| L-J | New AST variant silently skipped by a walker | coverage gap | Exhaustive `match`, no wildcard → new variant is a compile error until an arm exists; `deny(wildcard_enum_match_arm)` + CI grep + golden snapshot |
| L-K | Formatting via a 2nd formatter drifts from `ipe fmt` | divergence | Delegate to the `Format` crate; assert idempotence (2nd pass byte-identical) |
| L-L | v0 (pre-salsa) diverges from `ipe` | divergent analyzer | v0 calls the *same* front-end crates non-incrementally → slower, never different; same `ProgramView` trait |
| L-M | Over-trusted snippet tabstop-linking taken as an exhaustiveness guarantee | non-type-checking edit | Snippets guarantee only skeleton parse/fmt-cleanliness; the variant↔arm invariant lives in code action (b) + the exhaustiveness lint (c) |
| L-N | LSP executes project code / touches FFI introspection off the `ipe add` path | security | Parse/canon/type are pure; FFI enters only as a reserved salsa input; cache miss hard-refuses, never regenerates |
