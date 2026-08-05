# Securing the `unsafe` axis: a secure-before-mark audit + slice plan

Companion to `unsafe-escape-convention-design.md`, which defines the mechanics
(the `Ipe.<M>.Unsafe` submodule, the `unsafe*` name prefix, the import-derived
`unsafe` capability, and why the type is a trust boundary but the escape hatch
had none). That document answers *how* the axis works. This one answers *what
goes where*: it enumerates every trust-bypass sink in the stdlib and runtime,
classifies each against the governing rule below, and orders the work.

Read that document first. Nothing here restates its convention or its capability
model.

## The governing rule (secure before you mark)

Marking a function `unsafe*` is a warning label, not a fix. By **fix the
structure, not the symptom** with **Security** first, a hatch is relocated into
`Ipe.<M>.Unsafe` **only** when its safety cannot be mechanically guaranteed.

The decision, applied to every sink below:

1. **Can the raw input be parsed/validated into the safe value at construction
   or at the sink?** If a sanitiser (`CssSafety`'s `SafeCssValue` /
   `SafeCssPropertyName` / `SafeCssMediaQuery` / `strip_style_close`, the HTML
   escaper, `SafeAttrName`, `sanitise_url_attr`, `valid_sql_ident`) makes the
   value safe, route it through that sanitiser. The function is then secure by
   construction and is **not** an escape hatch. It keeps a terse safe name; it
   does **not** move to `.Unsafe`. A fixable hatch MUST be fixed, never merely
   labelled.
2. **Can a narrower, safer API replace the raw one?** Prefer it (a scoped
   consume over a raw reveal).
3. **Only if the bypass is genuinely irreducible** — arbitrary input, no
   validator can guarantee safety, and a legitimate verbatim need exists — does
   it move to `Ipe.<M>.Unsafe.unsafe*` with a doc-comment stating the invariant
   the caller must uphold, disclosed program-wide by the `unsafe` capability.

The security-defence carve-out (ADR-0057) is a hard floor across all of this:
`CssSafety`, the HTML escaper, the URL/attribute-name validators, the SQL
identifier validator, and crypto stay **native** (never lowered to `.ipe`), and
no reclassification below weakens any XSS / CSS-injection / secret barrier. The
render-sink escaping remains intact — several sinks in the table are `already
secure` precisely *because* that native sink already gates them.

## What the audit found (the headline)

The single most important finding reframes the existing convention doc's own
per-function notes into verified ground truth:

> **Every `Ui.*Raw` sink and every `Css.*` raw builder already routes its
> user-supplied string through the native `CssSafety` sanitiser and drops
> fail-closed on a breakout.** They are already secured by construction, at the
> render sink or at the smart constructor. None of them is a true escape hatch,
> and none moves to `.Unsafe`. The `Raw` suffix on their names is a
> *misnomer* — it advertises danger that the sanitiser has already removed.

Concretely, `build_style_string` (`src/runtime/rust/src/ui/render.rs`) gates
`AttrStyle`, `AttrGridTracks`, `AttrTransition`, `AttrAnimation`,
`AttrFontFamily`, `AttrFontDecoration`, `AttrBgImage`, `AttrBgGradient`,
`AttrOverflow`, `AttrBorderStyle` through `SafeCssValue::parse` /
`SafeCssPropertyName::parse`, dropping any declaration that fails. `AttrAttribute`
passes through to the HTML sink where `SafeAttrName` + `sanitise_url_attr` gate
it. In the compiled-source `Css.ipe`, `colorRaw` / `lengthRaw`(via `prop`) /
`rawProp` / `property` / `defineVar` route through `safeValue` / `prop`, and
`raw` / `keyframes` apply `stripStyleClose` + `containsDangerousCssConstruct`
(the `<style>`-breakout floor + `@import`/`expression(` drop). The live
style-injection pass (`web/style_inject.rs`) re-validates every marker payload
through the same `CssSafety` policy — defence in depth.

The genuinely irreducible hatches are a small, specific set: verbatim SQL,
verbatim HTML/script bodies, untyped column access, and the (planned) raw-JS
seam. These already carry an `unsafe*` name instinct in the codebase
(`Db.unsafeExecRaw`, `Db.unsafeQuery`, `Db.unsafeGetString`,
`Html.unsafeRaw`, `Web.Head.unsafeJsonLd`) but are **not yet** housed in a
`.Unsafe` submodule — so the machine signal (the capability) does not fire. The
work is mostly *relocation of already-named hatches* plus *renaming the
misnamed-safe `*Raw`* — not new sanitiser design.

## The classification table

Every trust-bypass sink in the stdlib + runtime. `SECURE` = already secured by a
sanitiser (rule 1) — keep terse, do **not** move to `.Unsafe`; `→UNSAFE` =
irreducible (rule 3) — move to `Ipe.<M>.Unsafe.unsafe*`; `RENAME` = already
secure but the `Raw`/`unsafe` name overstates the (removed) risk.

| Sink (surface name) | Module / impl | Guarantee it touches | Current protection | Verdict | How / why |
|---|---|---|---|---|---|
| `Ui.style` (`AttrStyle`) | `ui/render.rs:146` | CSS-injection | `SafeCssPropertyName` + `SafeCssValue`, drop-on-fail | **SECURE** | Both key and value scanned at the render sink; fail-closed. Already safe — keep. |
| `Ui.gridTracksRaw` (`AttrGridTracks`) | `ui/render.rs:324`; `Ui/Grid.ipe` | CSS-injection | `SafeCssValue` per axis, drop-on-fail | **SECURE + RENAME** | Value gated at sink. `Raw` misnames a scanned value; retire the `Raw` suffix (see slice B). |
| `Ui.transitionRaw` (`AttrTransition`) | `ui/render.rs:319`; `Ui/Transition.ipe` | CSS-injection | `SafeCssValue`, plus `sink_safe_declaration_list` in live inject | **SECURE + RENAME** | Shorthand scanned at sink and re-scanned in `style_inject`. Retire `Raw`. |
| `Ui.animateRaw` (`AttrAnimation`) | `ui/render.rs:337`; `Ui/Animation.ipe` | CSS-injection | `SafeCssValue` on shorthand; `sink_safe_keyframes_body` on the `@keyframes` body in live inject | **SECURE + RENAME** | Both the `animation:` shorthand and the keyframes body are gated fail-closed. Retire `Raw`. |
| `Ui.htmlAttribute` (`AttrAttribute`) | `ui/render.rs:390` | attr-injection / XSS | `SafeAttrName` + `sanitise_url_attr` at HTML sink | **SECURE** | On*-handlers, `srcdoc`, dangerous URL schemes dropped at the sink. Keep. |
| `Ui.name` (`AttrAttribute "name"`) | `ui/helpers.rs:776` | attr-injection | fixed key + HTML sink escaping | **SECURE** | Fixed key; value escaped. Keep. |
| `Css.property` / `rawProp` / `defineVar` | `Css.ipe` | CSS-injection | `prop` → `safePropName` + `safeValue` | **SECURE** | Smart constructor parses both name and value; failure → `CssDropped`. Keep (`rawProp` is a convenience splitter, still gated by `prop`). |
| `Css.colorRaw` | `Css.ipe:511` | CSS-injection | `safeValue`, drop → `ColorTransparent` | **SECURE** | Scanned at construction; opaque `Color`. Keep. |
| `Css.lengthRaw` / `calc` / `minmax` | `Css.ipe:449` | CSS-injection | `LenRaw` re-scanned by `prop` at declaration time | **SECURE** | Every `Length` flows into a declaration through `prop`; scanned there. Keep. |
| `Css.raw` (`CssRaw`) | `Css.ipe:1577` | `<style>` breakout / CSS SSRF | `stripStyleClose` + `containsDangerousCssConstruct` | **SECURE** | Close-tag-neutralised + `@import`/`expression(` dropped; the render sink strips again. A trusted-input hatch whose floor is mechanical — keep, do not relocate. |
| `Css.keyframes` (`CssKeyframes`) | `Css.ipe:1541` | `<style>` breakout / CSS SSRF | `safeSelector` on name + per-frame `stripStyleClose` + joined dangerous-construct scan | **SECURE** | Same floor as `raw`. Keep. |
| `Db.exec` / `Db.queryDecode` / `Sql.*` (`GuardedSql`) | `db.rs`; `Sql` kernels | SQL-injection | parameterised binds; `valid_sql_ident` on every identifier | **SECURE** | The safe default: values are binds, identifiers validated. Keep terse. |
| `Sql.column` | `db.rs:2068` | SQL-injection | `valid_sql_ident`; invalid → poisoned fragment | **SECURE** | Parse-don't-validate on the one identifier path. Keep. |
| `Db.unsafeExecRaw` (`db_exec_raw`) | `db.rs:860` | SQL-injection | none — verbatim SQL text | **→UNSAFE** | No validator makes arbitrary SQL safe; parameterised `exec` is the norm. Move to `Ipe.Db.Unsafe.unsafeExecRaw`. |
| `Db.unsafeQuery` (`db_query_params`) | `db.rs:949` | SQL-injection | binds parameterised, but the **query text** is a verbatim `String` | **→UNSAFE** | The SQL string is caller-authored verbatim; only the binds are safe. Move to `Ipe.Db.Unsafe.unsafeQuery`. |
| `Db.unsafeGetString` / `GetInt` / `GetBool` / `GetField` | `db.rs:1018` | type-safety (untyped column read) | none — string-keyed, decoder-bypassing | **→UNSAFE** | Bypasses the typed `queryDecode` row codec: a string column key with no decode proof. Not injection, but an untyped-access bypass; move to `Ipe.Db.Unsafe.unsafeGet*`. (No SQL is issued, so no injection axis — the invariant is "the caller asserts the column's type".) |
| raw `SqlFragment`-from-string (planned) | — | SQL-injection | none — mints reserved `SqlFragment` without `valid_sql_ident` | **→UNSAFE** | The anti-`Sql.column`: same reserved return type, parse deliberately skipped. New member `Ipe.Db.Unsafe.unsafeFragment`. |
| `Html.unsafeRaw` (`HtmlRawNode` → `HRaw`) | `ui/helpers.rs:557` | XSS | none — verbatim HTML text | **→UNSAFE** | No escaper runs; the whole point is verbatim markup. Already `unsafe*`-named; relocate to `Ipe.Html.Unsafe.unsafeRaw`. Pair with a secure `Html.sanitize : String -> Html msg` where a sanitiser exists, so `unsafeRaw` is reserved for trusted input. |
| `Html.text` (`HtmlTextNode`) | `ui/helpers.rs:548` | XSS | escaped at render (`escape_text`) | **SECURE** | The safe default; escaped by construction. Keep. |
| `Html.styleNode` | `ui/helpers.rs:570` | `<style>` breakout | `strip_style_close` at construction + sink | **SECURE** | Baked-in floor; keep. |
| `Web.Head.unsafeJsonLd` | `Web/Head.ipe:154` | XSS (verbatim `<script>` body) | none — CDATA-like verbatim | **→UNSAFE** | Raw JSON into a scripting context; no escaper. Already `unsafe*`-named; relocate to `Ipe.Web.Head.Unsafe.unsafeJsonLd`. |
| `Secret.reveal` (`secret_reveal`) | `secret.rs:150` | secret-leak | the single greppable un-parse; caller decides where it lands | **→UNSAFE** (after a scoped alternative) | Prefer a scoped `Secret.use : Secret -> (String -> a) -> a` where the raw value never escapes the closure (rule 2); `Ipe.Secret.Unsafe.unsafeReveal` remains for the residual cases a scoped form can't express. Runtime `secret_reveal` stays native + greppable. |
| `Secret.fromString` (`secret_from_string`) | `secret.rs` | — (promotion *into* the protected type) | seals a string into `Secret` | **SECURE** | Promotion, not bypass. Keep. |
| `Secret.redacted` | kernels | secret-leak | returns the masked form | **SECURE** | Safe by design. Keep. |
| `*.fromString` parses (`Path`/`Url`/`Regex`) | stdlib | injection / traversal | validating parse returning `Maybe`/`Result` | **SECURE** | Parse-don't-validate seals; not bypasses. Keep. |
| `Rust.` FFI crossing (`Callee::Ffi`) | `lower.rs:6869` | opaque effect / type-safety | discloses `native-ffi` (+ `ffi-raw` when asserted) | **already disclosed** | Handled by the existing FFI capability axis, not this one. `unsafe` is its sibling; no move. |
| `Process.run` (`process_run`) | `system.rs:302` | command-injection | direct argv, **no shell** | **SECURE** | Args are a literal `argv` vector; no shell metachar interpretation. Gated by `subprocess` capability. Keep. |
| `Html.voidNode` / `Html.node` (runtime-tag) | `ui/helpers.rs` | tag/attr-injection | tag flows through the HTML render sink; attrs gated by `SafeAttrName` | **SECURE** | The sink escapes; a hostile tag name yields inert escaped text, not a new element. Keep. |

**Buckets:** SECURE (already safe, keep as-is): **~19** sinks. SECURE + RENAME
(safe, retire the misleading `Raw` suffix): **3** (`Ui.gridTracksRaw`,
`transitionRaw`, `animateRaw`). →UNSAFE (irreducible, relocate to `.Unsafe`):
**7** — `Db.unsafeExecRaw`, `Db.unsafeQuery`, the `Db.unsafeGet*` family (one
slice), `Db.unsafeFragment` (new), `Html.unsafeRaw`, `Web.Head.unsafeJsonLd`,
`Secret.reveal`→`unsafeReveal` (behind a new scoped `Secret.use`). The raw-JS
seam (`Ipe.Js.Unsafe.unsafeEval`) is a *future* member of the same rule, not an
existing sink.

## Reserved-type / home implications

- No reserved-type list changes. Every `→UNSAFE` member either **(a)** returns
  an already-reserved security-tier type without running its parse
  (`unsafeFragment` → `SqlFragment`, `unsafeReveal` un-seals `Secret`) or **(b)**
  constructs a sink input from an unchecked `String` (`unsafeRaw` → `Html`,
  `unsafeExecRaw` → a `Task`). In neither case does `.Unsafe` *declare* a
  reserved type, so IPE-N0026 is untouched. The submodule only *produces*
  reserved values by assertion — which is exactly what the `unsafe` capability
  exists to disclose.
- The `Ipe.<M>.Unsafe` home is expressible today with additive changes only
  (see the convention doc's Decision 1): the `EmbeddedStdlib` origin exemption
  hosts the submodule, and the dotted-submodule qualifier machinery
  (`Ipe.Html.Attributes`, `Ipe.Db.Sql` precedent) resolves it. A hostile user
  file literally named `Ipe.Db.Unsafe` stays rejected as `User` origin.
- **Re-reaching relocated members: the `Ffi.kernel` alias precedent.** A kernel
  that moves module home does not change its runtime function; only its
  resolved qualifier moves. `Path`, `Url`, `Html`, and `Ui` already resolve
  their compiled-source surface to native kernels through the `Ffi.kernel
  "Module_fn"` alias table. `Ipe.Db.Unsafe.unsafeExecRaw` resolves to the
  existing `db_exec_raw` runtime fn via the same mechanism — a qualifier entry
  pointing at an unchanged kernel. `Html.unsafeRaw` (already the surface name of
  `HtmlRawNode`) only needs its resolved qualifier changed from `Html` to
  `Html.Unsafe`; the `html_raw_node_` runtime fn is untouched.

## The implementation plan (ordered slices)

Each slice is atomic, guardian-gated, and either byte-identical in golden output
or a justified rebless (golden regen is cheap — never a cost factor). The
capability-plumbing slice (0) is a prerequisite for every relocation; the rename
slices are independent of it and of each other.

**Slice 0 — the `unsafe` capability + `.Unsafe` resolution (prerequisite,
ordered first).**
Additive, touches no user default path. From the convention doc's change list:
add the `Unsafe` arm to `Capability` (`capability.rs`: enum + `as_str`/`FromStr`
`"unsafe"` + `ALL`; bump `all_lists_every_variant_once` 9→10); thread an
`imports_unsafe_submodule` fact from canon to lowering; insert `Unsafe` in
`program_capabilities_scan` on that fact (mirrors the `usage.ffi` block); wire
`ipe capabilities` / `ipe add` / the `[capabilities] declared` manifest gate.
This slice ships no relocated member yet — it lands the machinery so the later
slices' relocations actually disclose. Guardian-gated on the capability-partition
test (the `imports_unsafe ⇔ discloses unsafe` bidirectional check).

**Slice A — `Html.unsafeRaw` → `Ipe.Html.Unsafe.unsafeRaw` (recommended FIRST
relocation).**
Smallest, highest-value, fully independent after slice 0. `HtmlRawNode` is
*already* surface-named `Html.unsafeRaw`; the change is purely its resolved
qualifier (`Html` → `Html.Unsafe`) via the `Ffi.kernel` alias precedent — no
runtime change, no new member. It is the single clearest XSS trust boundary, and
it validates the whole relocation pattern (qualifier move + capability
disclosure + golden re-bless of any example that used it) on the least code.
Optionally pair with a secure `Html.sanitize : String -> Html msg` in the same
slice if a sanitiser is available; otherwise file that as a follow-up so slice A
stays minimal.

**Slice B — retire the `Raw` misnomer on the three secured `Ui` builders
(independent, no dependency on slice 0).**
`Ui.gridTracksRaw` / `transitionRaw` / `animateRaw` are already secured by
`SafeCssValue`; rename their surface to drop `Raw` (e.g. `Ui.gridTracks`,
`Ui.transition`, `Ui.animate`) so the name no longer advertises a risk the
sanitiser removed. Update `Ui/Grid.ipe`, `Ui/Transition.ipe`, `Ui/Animation.ipe`
call sites and the kernel `d(…)` surface names. Pure rename; guardian-gated on
the golden re-bless. Independent of the capability work — parallelizable with
slice 0.

**Slice C — `Ipe.Db.Unsafe` (the SQL cluster; after slice 0).**
Relocate `Db.unsafeExecRaw`, `Db.unsafeQuery`, and the `Db.unsafeGet*` family
under `Ipe.Db.Unsafe`, and add the new `Ipe.Db.Unsafe.unsafeFragment` (the
anti-`Sql.column`). All ride existing runtime fns via the alias precedent. This
is the worked example the convention doc's Decision 5 anticipates. Ordered after
slice 0; internally the `unsafeGet*` family is one atomic sub-change.

**Slice D — `Web.Head.unsafeJsonLd` → `Ipe.Web.Head.Unsafe.unsafeJsonLd`
(after slice 0, independent of A/C).**
Already `unsafe*`-named; relocate the compiled-source member. Pure qualifier
move in `Web/Head.ipe`.

**Slice E — `Secret.use` + `Ipe.Secret.Unsafe.unsafeReveal` (after slice 0;
the only slice with new safe-API design).**
Add the scoped consume `Secret.use : Secret -> (String -> a) -> a` (rule 2), so
the common case never reveals into caller scope; relocate the residual
`Secret.reveal` → `Ipe.Secret.Unsafe.unsafeReveal`. The native `secret_reveal`
stays the single greppable un-parse; `Secret.use` is a thin scoped wrapper over
it. This slice carries the most design (the closure-scoped API) and should be
guardian-reviewed for the "raw value never escapes the closure" invariant.

**Independence / parallelism.** Slice 0 gates C, D, E, and the disclosure aspect
of A. Slice B is fully independent (pure rename of already-secure functions) and
can run in parallel with slice 0. A, C, D are mutually independent once slice 0
lands and can run as parallel lanes (distinct modules, distinct golden fixtures).
E is independent of A/C/D but carries new API design, so it should not share a
lane with a mechanical relocation. The future `Ipe.Js.Unsafe.unsafeEval` seam is
not in this plan's scope — it lands with the JS-ports work and simply follows the
same rule.

## Why this ordering serves the precedence

Security first: slice 0 makes the *machine* signal exist before any member
claims the `.Unsafe` home, so no relocation ever ships a hatch that silently
fails to disclose. Slice A moves the single clearest XSS boundary first, on the
least code, to de-risk the pattern. Slice B removes a *false* danger signal
(`Raw` on an already-scanned value) that dilutes the axis — a reader who greps
`Raw` and finds a dozen already-safe hits stops trusting the grep; retiring the
misnomer keeps the one-axis promise (grep `Unsafe`/`unsafe` → every *real* trust
boundary) honest. The remaining slices relocate the genuinely-irreducible set,
each atomic and golden-gated.
