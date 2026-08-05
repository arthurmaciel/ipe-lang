# The `Ipe.<Module>.Unsafe` escape convention + an inferred `unsafe` capability

Status: design proposal, no implementation yet. Every fenced block below is
**illustrative of the proposed surface** — the convention does not exist yet, so
none of these are runnable; they show intended shapes and names, not shipped API
or verified commands. Issue references use bare numbers (the tracker link belongs
in the commit/PR, not in this doc).

## The problem: the type is a trust boundary, the escape hatch has none

Ipê already makes the **type** a trust boundary. `parse-don't-validate` is
codified as a set of reserved, opaque, validating-parse-only security-tier types:
`SqlFragment`, `Secret`, `Path`, `Regex`, `Url`, the `PubSub` topic handle, and
the JS-widget `CustomElement`. Each is reserved in canon
(`RESERVED_BUILTIN_TYPES`, `src/compiler/canon/src/resolve.rs`), un-shadowable by
user code (IPE-N0026), and reachable only through a constructor that parses
(`Url.fromString`, `Sql.column`, `Regex.compile`). A value of one of these types
is a *proof* that a check ran; there is no un-parsed way to obtain one.

What has **no** boundary today is the other direction — the **escape hatch**: the
`raw` / injection path that deliberately *bypasses* a parse and constructs the
sink input from an unchecked `String`. Every real system needs a few of these,
and they already exist, scattered and inconsistently marked:

- `db.rs::db_exec_raw` — verbatim SQL, no parameterisation. Its doc-comment
  already names it `unsafeExecRaw` and calls it "the raw-SQL injection surface".
- A proposed `Html.raw : String -> Html` (tracked issue 666) — needed for
  string-built HTML and inline `<script>`, but an unbounded XSS surface.
- The proposed raw-JS seam (tracked issue 333) — the `data-ipe-eval`/`new Function`
  path the JS-ports design explicitly forbids at the front door but must express
  *somewhere* for a third-party widget.

These are the anti-`SqlFragment`: places where the parse is skipped on purpose.
Today nothing structural marks them. A reviewer must *know* `db_exec_raw` is
dangerous; the compiler cannot tell you a program contains a raw-HTML sink. The
security posture is "remember the escape hatch" — the exact failure mode the
`SqlFragment` design was built to eliminate on the constructor side.

This document proposes a single language-wide convention that gives the escape
hatch a boundary as strong as the one the type gives the constructor — **and makes
that boundary machine-readable**, disclosed by `ipe capabilities` exactly like
`native-ffi` is today.

## Precedence: secure by construction before you mark unsafe

Marking a function `unsafe*` is a *warning label*, not a fix. By the fundamental
rule **fix the structure, not the symptom** — and Security first — this
convention applies **only to irreducible hatches**. Before any `*Raw` / escape
function is relocated into `Ipe.<M>.Unsafe`, apply this gate:

1. **Can the raw input be parsed/validated into the safe type?** If yes, route it
   through the validator so the function is secure by construction
   (make-invalid-states-unrepresentable) — it is then *not* an escape hatch. A
   fixable `*Raw` MUST be fixed, never merely labeled: a hazard a validator pass
   would have removed is not one to relieve with a name.
2. **Can a narrower, safer API replace the raw one?** Prefer it (e.g. a scoped
   consume over a raw reveal).
3. **Only if the bypass is genuinely irreducible** — arbitrary input, no
   validation can guarantee safety, and a legitimate verbatim need exists — does
   it get `Ipe.<M>.Unsafe.unsafe*` plus the `unsafe` capability.

Per-function audit (the front half of this work):

- **`Ui.*Raw` (grid tracks / animate / transition) → SECURE, not Unsafe.** Route
  the raw CSS string through `CssSafety`'s validator, or replace it with a typed
  API; a validated CSS value has no injection hatch left to mark. [rule 1]
- **`Secret.reveal` → prefer a scoped consume** `Secret.use : Secret -> (String
  -> a) -> a` (the raw value never escapes the closure), falling back to
  `Ipe.Secret.Unsafe.unsafeReveal` only where a scoped form cannot express the
  need. [rule 2, then 3]
- **`Db.unsafeExecRaw` / `Db.unsafeFragment`, `Html.unsafeRaw`, `Js.unsafeEval`
  → Unsafe (irreducible).** No validator makes arbitrary SQL / HTML / JS safe;
  the secure norms (parameterised queries, escaped `Html` construction, no eval)
  are already the ordinary API, and these are the genuine verbatim escapes. For
  `Html`, additionally offer a secure `Html.sanitize : String -> Html` where a
  sanitiser exists, so `unsafeRaw` is reserved for truly-trusted input.
- **Audit every remaining `*Raw` / `reveal` / `*FromString`** against the three
  hatch shapes (mint-by-assertion, demote-to-raw, inject-into-a-sink). The
  `*FromString` constructors that return `Maybe` / `Result` are safe parses and
  stay; `Secret.fromString` (promotion *into* the protected type) is safe and
  stays.

## The convention: one submodule, one prefix, belt and suspenders

> Every parse-bypassing / unsafe escape in the standard library lives in a
> per-module submodule **`Ipe.<Module>.Unsafe`** and is named **`unsafe*`**.

Illustrative signatures (not shipped):

```
Ipe.Html.Unsafe.unsafeRaw       : String -> Html msg
Ipe.Db.Unsafe.unsafeFragment    : String -> SqlFragment
Ipe.Db.Unsafe.unsafeExecRaw     : Db -> String -> Task Error Int
Ipe.Js.Unsafe.unsafeEval        : String -> Cmd msg      -- the raw-JS seam
```

The redundancy between the path and the name is **deliberate**, and it is two
independent signals with two independent audiences:

- **(a) The import path `Ipe.<M>.Unsafe` is the MACHINE signal.** Any module that
  imports an `.Unsafe` submodule is inferred to carry a new capability — call it
  `unsafe` — disclosed by `ipe capabilities` and checked against the package
  manifest exactly like `native-ffi`. A dependency's raw sinks become visible
  *before you run it*, from its code alone, with nothing to declare. This is the
  contribution the naming alone cannot make: a grep for `unsafeRaw` is not a
  build artifact; a capability is.

- **(b) The method name `unsafe*` is the HUMAN signal**, at the call site and in
  review. `H.unsafeRaw userInput` reads as dangerous even to someone who never
  saw the import line. This defends against exactly the case where (a)'s path
  signal is hidden: `import Ipe.Html.Unsafe as H` aliases the path away, so the
  call site shows only `H.unsafeRaw` — the name is the last line of defence when
  the alias erased the path. (The capability inference still fires on the *import*
  regardless of the alias, so (a) is never actually defeated by an alias — but the
  reader of a diff hunk who does not see the import header still gets the warning.)

Belt (the capability, machine-checked, alias-proof) and suspenders (the name,
human-legible, alias-proof at the call site). Neither alone is sufficient; both
together degrade gracefully.

## Decision 1 — Is `Ipe.<M>.Unsafe` expressible today?

**Yes, with small additive changes. No new module-system concept is required.**

Three facts from the current machinery establish this:

1. **The reserved-namespace gate keys on the FIRST segment only.** IPE-N0025
   (`resolve.rs::canonicalise_module_in_project`) rejects a *user* module whose
   first path segment is `Ipe`, but an `EmbeddedStdlib`-origin module is exempt —
   it is the one legitimate definer of an `Ipe.…` home, vouched by the
   unforgeable `ModuleOrigin` tag the build driver constructs (never derivable
   from module text). A shipped `Ipe.Html.Unsafe` is `EmbeddedStdlib`, so it
   passes the gate; a *hostile* user file literally named `Ipe.Html.Unsafe` is
   discovered as `User` origin and stays rejected. The provenance discipline that
   protects `Ipe.Auth` today protects `Ipe.Html.Unsafe` for free.

2. **Dotted stdlib submodules already register and resolve.** `Ipe.Html.Attributes`
   and `Ipe.Html.Events` are live kernel-qualifier submodules
   (`src/compiler/canon/src/env.rs`), and `Ipe.Db.Sql` is a resolved qualifier
   (`target_gate.rs`). `Ipe.<M>.Unsafe` is the identical shape: either a
   kernel-qualifier entry (for `unsafeRaw`/`unsafeFragment`, which are thin
   wrappers over an existing runtime kernel) or a `CompiledStdModule` /
   parse-fixture `StdModule` entry in `src/stdlib/src/lib.rs`, whichever the
   member's implementation needs. The disjointness invariant
   (`compiled_vs_kernel_qualifier_disjoint`) applies unchanged.

3. **The reserved-type interplay is already correct.** `unsafeFragment` *returns*
   the reserved `SqlFragment`; `unsafeRaw` *returns* `Html`. The `.Unsafe`
   submodule does not need to declare or shadow any reserved type — it produces
   an already-reserved type *without* running the parse. That is precisely the
   point: the escape hatch mints a security-tier value by assertion instead of by
   proof, and the capability is what discloses that an assertion was used.

**Changes needed** (all additive, none touching user-facing behaviour on the
default path):

- **Canon:** none for namespace hosting — the `EmbeddedStdlib` exemption and the
  dotted-submodule qualifier machinery already cover `Ipe.<M>.Unsafe`. One new
  piece of bookkeeping: canon must record, per module, whether any import's path
  ends in the `Unsafe` segment, and thread that fact to lowering so the capability
  scan can see it (see Decision 2).
- **Kernels (`ipe_kernels`):** add the `Unsafe` capability variant to the closed
  `Capability` vocabulary (`src/compiler/kernels/src/capability.rs`) — one enum
  arm, one `as_str`/`FromStr` pair (`"unsafe"`), one `ALL` entry. The exhaustive
  `all_lists_every_variant_once` test bumps 9 to 10.
- **Lower (`program_capabilities_scan`, `lower.rs`):** insert `Unsafe` into the
  set when the module imported an `.Unsafe` submodule — structurally the same
  one-liner as the existing `if usage.ffi { insert(NativeFfi) }` /
  `if usage.ffi_asserted { insert(FfiRaw) }` block.
- **CLI / manifest (ADR 0044):** `ipe capabilities` prints `unsafe`; `ipe add`
  is loud on it (same treatment `native-ffi` gets); the `[capabilities] declared`
  manifest gate accepts and checks it.

Nothing in the *module system*, import resolution, or the N0025/N0026 gates needs
a new concept. The convention rides entirely on existing seams.

## Decision 2 — The `unsafe` capability: how it is inferred and where it slots

### Import-derived, not kernel-tagged

Today's seven runtime capabilities (`network`, `filesystem`, `database`, `env`,
`subprocess`, `clock`, `random`) are **kernel-tagged**: each stdlib kernel carries
a `Capability`, and `program_capabilities_scan` unions the tags of every reachable
kernel. The two FFI capabilities (`native-ffi`, `ffi-raw`) are different — they
are **crossing-derived**: `program_capabilities_scan` sets them from IR flags
(`usage.ffi`, `usage.ffi_asserted`) that mark a `Rust.` crossing, not from a
per-kernel tag.

`unsafe` follows the **crossing-derived / import-derived** model, and this is the
correct classification: "unsafe" is not a *resource axis* the OS jail can isolate
(unlike network or filesystem) — it is a *provenance* fact, "this program contains
a value minted by assertion rather than by parse", exactly as `native-ffi` means
"this program contains an effect the compiler could not see through". Both are
disclosure signals, not sandbox axes. This mirrors the existing split cleanly:
`native-ffi`/`ffi-raw` sit in the vocabulary as non-jail-enforced disclosure
capabilities, and `unsafe` joins them.

Concretely: the signal is the **import of an `Ipe.<M>.Unsafe` submodule**, not the
call of a specific function. Import-based (not call-based) inference is a
deliberate choice — it is *conservative* (fail-loud): a module that imports
`Ipe.Html.Unsafe` but has a dead-code path to `unsafeRaw` still discloses
`unsafe`, because the import is itself the reviewable act of reaching for the
escape hatch. It also matches the honest-superset rule the audit already uses (a
library exposes the capabilities of its whole API, not just its entry-reachable
subset).

### One axis or per-domain?

**Recommendation: ONE axis, `unsafe`, refined by a per-domain *tag* in the
verbose report — not a combinatorial `unsafe-html` / `unsafe-sql` vocabulary.**

Rationale:

- **The closed vocabulary must stay small and coarse.** `capabilities.md` is
  explicit that the vocabulary is "closed and coarse for now"; `network` is *any*
  network. A `unsafe-<domain>` axis per stdlib module that ever grows an escape
  hatch is an *open* vocabulary — it grows every time a new `.Unsafe` submodule
  ships, and a manifest gate over an open set cannot be exhaustively reasoned
  about. `native-ffi` did not split into `native-ffi-net` / `native-ffi-fs`; it
  stayed one axis and let the *declared native capabilities* carry the detail.
- **The domain is already recoverable, for free, from the import set.** The
  program's disclosed imports already name *which* `.Unsafe` modules it uses.
  `ipe capabilities --verbose` can therefore print `unsafe (via Ipe.Html.Unsafe,
  Ipe.Db.Unsafe)` — the domain as a *sub-line under the single axis*, derived,
  not a distinct capability. You get per-domain visibility without per-domain
  vocabulary growth.
- **Consent is monotone and simple.** `ipe add` asks "do you consent to this
  package using an unsafe escape?" once, and the verbose breakdown tells the
  auditor which sinks. Splitting the axis would force N consent questions for N
  domains with no security gain — the danger of a raw sink is uniform (assertion
  replaced proof), even when the sink differs.

So: **the axis is `unsafe`; the domain is disclosed as a derived detail, never as
its own capability.** If a future need for *enforced* per-domain gating arises
(e.g. "this deployment forbids raw HTML but permits raw SQL"), that is a manifest
*policy* over the already-disclosed import set, layered on top — not a change to
the capability vocabulary.

### Slot in the model + outputs

- **`Capability` enum:** append `Unsafe` after `FfiRaw` (declaration order is the
  report order via the derived `Ord`). Wire name `"unsafe"`.
- **`ipe capabilities`:** prints `unsafe` on its own line when any `.Unsafe`
  submodule is imported; `--verbose` appends the `via …` domain breakdown.
- **`ipe add` (ADR 0044):** loud on `unsafe`, same as `native-ffi` — installing is
  informed consent to it.
- **`[capabilities] declared` manifest:** `unsafe` is a legal declared token; an
  undeclared-but-inferred `unsafe` is the same compile-time honesty error a
  hidden `network` would be — a raw sink cannot hide.

## Decision 3 — Naming: `unsafeRaw` vs `raw` inside `Ipe.<M>.Unsafe`

**Recommendation: keep `unsafe` IN the method name — `Ipe.Html.Unsafe.unsafeRaw`,
not `Ipe.Html.Unsafe.raw`.** The redundancy is a feature, not a smell.

The strongest argument is the **alias-erasure** case. A reader auditing a diff
sees a hunk, not the file header (illustrative):

```
-    node = div [] [ text summary ]
+    node = H.unsafeRaw summary        -- the danger is legible here
```

With `import Ipe.Html.Unsafe as H`, the path signal is *gone* from the call site.
If the member were named `raw`, the hunk would read `H.raw summary` —
indistinguishable from a benign `H.raw` on some innocent `H`. Naming it
`unsafeRaw` makes the call site self-incriminating *independent of the import*,
which is the whole point of a review-time signal. The two signals must be
independently robust; an alias defeats the path-at-call-site but not the name.

Secondary arguments:

- **It matches the reserved-type precedent's spirit.** `Url.fromString` names the
  parse it performs; `unsafeRaw` names the parse it *skips*. The verb carries the
  safety semantics at every call, not just at the import.
- **It is already the de-facto convention in the codebase.** `db.rs` documents its
  raw exec as `unsafeExecRaw` today. Adopting `unsafe*` codifies an existing
  instinct rather than inventing a term.
- **Grep-ability.** A search for a leading `unsafe` followed by a capital finds
  every escape-hatch *call* in a codebase in one query — a lint rule and a
  code-review anchor both key off the name, and the capability keys off the path.
  Two orthogonal audit surfaces.

The one cost — verbosity (`Unsafe.unsafeRaw` says "unsafe" twice) — is precisely
the intended cost: an escape hatch *should* be slightly awkward to reach for. The
safe path (`Html.text`, `Sql.column`) stays terse; the dangerous path pays a
small friction tax. That asymmetry is the design working.

## Decision 4 — Relationship to Sky: allow-and-disclose vs ban

Sky **bans** the raw seam outright: `data-sky-eval` is forbidden, there is no
`Html.raw`, and the client's `new Function` path is disallowed (carried into Ipê's
JS-ports design as "the front door is closed").

Ipê's out-of-the-box stance is deliberately different: **allow, but loudly
disclose.** Justification:

- **A ban is not portable across the real workload.** Inline `<script>` for a
  third-party analytics snippet, a legacy string-built HTML fragment, a raw SQL
  DDL migration, a browser-only widget SDK — these are legitimate, common needs. A
  hard ban pushes them *outside* the language (a hand-edited emitted file, an
  out-of-band shell step), where they have *no* disclosure at all. That is
  strictly worse for security: the escape happens anyway, invisibly.
- **Disclose-don't-ban keeps the escape inside the auditable perimeter.** Under
  this convention the raw sink is (1) confined to a named `.Unsafe` submodule,
  (2) marked at every call by `unsafe*`, and (3) surfaced as a capability in
  `ipe capabilities` and the package gate. The program is *more* auditable than
  under a ban that merely relocates the danger.
- **It composes with the existing consent model.** `native-ffi` is not banned —
  it is disclosed and consented to at `ipe add`. `unsafe` is the same shape of
  decision for the same reason: Ipê's thesis is *verify behaviour, not
  reputation*, and you cannot verify a behaviour you have driven out of the
  language.

So Ipê is portable **and** auditable where Sky is neither-or-safe-by-absence:
Sky's safety comes from the feature not existing; Ipê's comes from the feature
being unforgeably visible. (Nothing here prevents a *deployment* from re-imposing
Sky's ban as policy: a manifest that declares zero `unsafe` and a CI gate that
rejects any inferred `unsafe` reproduces Sky's stance exactly, as an opt-in.)

## Decision 5 — db.rs / SQL specifically

The db seam is the worked example that ties the convention to `GuardedSql`.

- **The safe default eliminates hand-written SQL.** With a bidirectional record↔JSON
  codec (tracked issue 663) plus a generated `Db.Store`, the common path is a
  typed record ↔ row codec: the developer writes a decoder/encoder, and CRUD is
  generated. Structured predicates go through the `SqlFragment` builders
  (`Sql.column`, parameterised binds) — the `GuardedSql` surface, where every
  identifier is validated (`valid_sql_ident`) and every value is a bind, never
  interpolated. Under this the *typical* app never writes raw SQL at all.
- **Raw SQL becomes the rare, marked escape.** The two current raw entry points —
  the `db_exec_raw` verbatim-statement path and a hypothetical
  "build a `SqlFragment` from an unchecked string" — move under `Ipe.Db.Unsafe`:
  - `Ipe.Db.Unsafe.unsafeExecRaw : Db -> String -> Task Error Int` (rename of the
    already-so-documented `db_exec_raw`).
  - `Ipe.Db.Unsafe.unsafeFragment : String -> SqlFragment` — the one way to mint a
    `SqlFragment` *without* the `valid_sql_ident` parse. This is the
    anti-`Sql.column`: same reserved return type, parse deliberately skipped,
    disclosed by `unsafe`.
- **The reserved type stays reserved; only its provenance splits.** `SqlFragment`
  remains un-shadowable (IPE-N0026). Now there are exactly two ways to obtain one:
  the *guarded* constructors in `Ipe.Db.Sql` (proof), and `unsafeFragment` in
  `Ipe.Db.Unsafe` (assertion). The type no longer *implies* a parse ran — the
  **capability** now tells you which provenance the program used. That is the
  convention closing the one gap the reserved type could not: a `SqlFragment` was
  always trustworthy *by type*, but the type could not distinguish "validated" from
  "asserted". `unsafe` supplies exactly that missing bit, disclosed program-wide.

## Decision 6 — Migration

| Existing / planned escape | Moves to | Notes |
|---|---|---|
| `db.rs::db_exec_raw` (doc'd `unsafeExecRaw`) | `Ipe.Db.Unsafe.unsafeExecRaw` | Rename + relocate qualifier; behaviour identical; now discloses `unsafe`. |
| raw `SqlFragment`-from-string | `Ipe.Db.Unsafe.unsafeFragment` | New member; anti-`Sql.column`; returns reserved `SqlFragment`. |
| `Html.raw : String -> Html` (issue 666) | `Ipe.Html.Unsafe.unsafeRaw` | The convention is the *answer* to that issue's open naming question. |
| raw-JS / `data-ipe-eval` seam (issue 333) | `Ipe.Js.Unsafe.unsafeEval` (or `unsafeElement`) | The one place the forbidden front-door seam is expressible, behind disclosure. |
| Any future stdlib parse-bypass | `Ipe.<M>.Unsafe.unsafe*` | The rule is general; new escapes have a fixed home + prefix. |

**Reserved-type interplay across the migration:** every `.Unsafe` member either
(a) returns an already-reserved security-tier type without running its parse
(`unsafeFragment`→`SqlFragment`, `unsafeRaw`→`Html`), or (b) constructs a sink
input from an unchecked `String`. In neither case does `.Unsafe` *declare* a
reserved type, so IPE-N0026 is untouched; the submodule only *produces* reserved
values by assertion, which is exactly what the `unsafe` capability exists to
disclose. No reserved-type list changes.

## Affected issues

Cross-reference of every open issue against this convention. The final column is a
one-line annotation suitable for appending to the issue body.

| Issue | Relationship | One-line annotation |
|---|---|---|
| 666 | **FIXES** | Resolves the open naming/placement question: `Html.raw` ships as `Ipe.Html.Unsafe.unsafeRaw`, disclosed by the new import-derived `unsafe` capability (see docs/architecture/tbd/unsafe-escape-convention-design.md). |
| 333 | **COORDINATES** | The forbidden `data-ipe-eval`/`new Function` seam gets its one sanctioned home as `Ipe.Js.Unsafe.unsafeEval`, behind the `unsafe` capability — the escape the "closed front door" still needs, made auditable. |
| 663 | **COORDINATES** | The record↔JSON codec + `Db.Store` is the safe default that makes hand-written SQL disappear; this convention defines the *rare escape* (`Ipe.Db.Unsafe`) for when raw SQL is still needed. |
| 641 | **COORDINATES** | `Db.open <driver> <dsn>` to an arbitrary external DB is an unaudited-connection escape; if it bypasses the configured-DB safety it belongs behind `Ipe.Db.Unsafe` + `unsafe`. |
| 661 | **INDEPENDENT** | Cache codegen SEAL bug; unrelated, but any raw-cache-key escape it later needs should follow `Ipe.Cache.Unsafe.unsafe*`. |
| 396 | **COORDINATES** | Consolidated FFI-to-Rust spec already discloses `native-ffi`/`ffi-raw`; `unsafe` is the sibling disclosure capability for parse-bypass sinks — same `program_capabilities_scan` insertion pattern. |
| 651 | **COORDINATES** | Converter-generated FFI bindings for translated `[rust.dependencies]` already surface `native-ffi`; any raw string-sink the converter emits must land in an `.Unsafe` submodule so `unsafe` is disclosed too. |
| 292 | **COORDINATES** | Per-platform sandbox matrix enforces resource axes; `unsafe` is a *disclosure* (non-jail) capability like `native-ffi`, so it is reported in the matrix but not OS-enforced — document it as such. |
| 139 | **SUBSUMES (partial)** | `ipe lint` gains a first-class rule for free: flag `unsafe*` call sites and mismatch between imported `.Unsafe` modules and declared `unsafe` — the name is the lint anchor, the capability the ground truth. |
| 561 | **COORDINATES** | Elm/Roc-bar diagnostics should give the `unsafe` capability/manifest-mismatch error a friendly, suggestion-carrying render (like IPE-N0025). |
| 541 | **COORDINATES** | The `is_secret`/`is_crypto`/`is_json` partition-test pattern extends naturally to a bidirectional `imports_unsafe_submodule` ⇔ `discloses unsafe capability` test. |
| 665 | **INDEPENDENT** | `Task.retryOn` lowering bug; unrelated to the escape convention. |
| 672 | **INDEPENDENT** | `Ipe.Random` source-vs-kernel drift; unrelated (Random has no parse-bypass sink). |
| 671 | **INDEPENDENT** | seccomp baseline denials; the OS jail is orthogonal to `unsafe`, which is a compile-time disclosure, not a runtime-enforced axis. |
| 674 | **INDEPENDENT** | Sandbox regression-test/dedup; no interaction. |
| 664 | **INDEPENDENT** | `Ipe.Analytics` is consent-gated at its own layer; if it ever needs a raw event blob, that blob follows `Ipe.Analytics.Unsafe.unsafe*`. |
| 397 | **INDEPENDENT** | `Ipe.Parser` combinator port; a parser is the *safe* path — no escape hatch (noted only to confirm no interference). |
| 470 | **INDEPENDENT** | Hosted ipe-index infra; consumes the capability set (incl. `unsafe`) as data but needs no design change here. |
| 473 | **INDEPENDENT** | Native playground; unrelated build/runtime work. |
| 317 | **INDEPENDENT** | In-browser playground; unrelated, though it should render `unsafe` in any capability display it shows. |
| 294 | **COORDINATES** | Readability/naming synthesis (P6) should adopt `unsafe*` + `Ipe.<M>.Unsafe` as the codified project-wide escape-naming rule. |
| 284 | **INDEPENDENT** | Direct WASM backend target; the capability scan runs pre-codegen, so `unsafe` inference is backend-agnostic — no interaction. |
| 240 | **INDEPENDENT** | Git-history pruning; no interaction. |

## Summary

The convention adds one closed-vocabulary capability (`unsafe`), one submodule
naming rule (`Ipe.<M>.Unsafe`), and one method-prefix rule (`unsafe*`), all riding
existing seams: the `EmbeddedStdlib` namespace exemption hosts the submodule, the
dotted-qualifier machinery resolves it, and the `native-ffi` crossing-derived
inference pattern is copied verbatim for `unsafe`. The type was already the trust
boundary; this gives the *escape hatch* an equally unforgeable boundary — visible
to a human at the call site (the name), to a reviewer in a diff (the name again,
alias-proof), and to the machine in `ipe capabilities` and the package gate (the
import-derived capability). Ipê's stance — allow-and-disclose, not ban — keeps the
inevitable raw sink inside the auditable perimeter instead of exiling it to an
invisible hand-edit.
