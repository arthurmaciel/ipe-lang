# `Ipe.Html` round-trip: raw injection, inline `<script>`, and render-out

Status: this is **largely confirmation, not new design**. The escape hatch it asks
for already ships. This doc records what EXISTS, separates the two round-trip
directions on security grounds, names the sanctioned inline-`<script>` mechanism,
and specifies the migration for string-built views. Illustrative Ipê is marked as
such; every cited symbol was verified against the tree.

The core convention this doc rides on is specified once in
[`unsafe-escape-convention-design.md`](./unsafe-escape-convention-design.md) — the
`Ipe.<Module>.Unsafe` submodule rule, the `unsafe*` prefix, and the import-derived
`unsafe` capability. This doc does not restate it; it confirms `Ipe.Html` conforms.

## The two directions are not one problem

`Html ↔ String` is two independent crossings with opposite security posture:

- **OUT — `render : Html -> String`.** Serialising a *typed* tree to a string.
  The tree was built from escaped constructors, so the serialiser is the XSS
  barrier, not a hole. **Safe. Ships on the plain `Ipe.Html` surface.**
- **IN — `raw : String -> Html`.** Injecting an *un-typed* string as markup,
  bypassing every escape. This is the anti-parse: a value that looks like `Html`
  without any proof one ran. **Dangerous. Lives only behind the `unsafe` hatch.**

Conflating them ("no string round-trip") over-restricts: server-side rendering
(SSR pages, HTML email) is a legitimate, safe OUT need. The rule is directional —
**rendering out is safe; parsing a string back in is the guarded direction.**

## Decision 1 — the core (a)-vs-(b) choice: (a), and it already ships

The choice is **(a) a security-reviewed escape hatch**, resolved exactly as
`unsafe-escape-convention-design.md` predicts. `Ipe.Html.Unsafe.unsafeRaw` is
**IMPLEMENTED, not proposed**:

- `src/stdlib/Ipe/Html/Unsafe.ipe` — `module Ipe.Html.Unsafe exposing (unsafeRaw)`,
  `unsafeRaw : String -> Html msg`, a point-free `Ffi.kernel "Html_unsafeRaw"`.
- Kernel `HtmlRawNode` (`("Html", "unsafeRaw")`) → runtime
  `ipe_runtime::ui::helpers::html_raw_node_`, emitting the `HRaw(String)` node the
  render sink splices verbatim.
- The plain-surface `Html.raw` and `Html.unsafeRaw` spellings are **removed** —
  both reject with `IPE-N0005` (no such member). Only
  `import Ipe.Html.Unsafe exposing (unsafeRaw)` resolves. Proven by
  `security_html_raw_unmarked_is_rejected`,
  `security_html_unsafe_raw_off_plain_html_is_rejected`, and
  `security_html_unsafe_raw_compiles` in `src/ipe-cli/tests/negative_suite.rs`.
- Importing the submodule discloses the `unsafe` capability program-wide —
  import-derived, tested by `importing_an_unsafe_submodule_discloses_unsafe` and a
  real-module case over `Ipe.Html.Unsafe` in
  `src/compiler/lower/src/capabilities.rs`.

Option (b) — ban string-built HTML outright — is rejected for the reason the
convention doc gives: a ban exiles the inevitable raw sink to a hand-edited
emitted file with *no* disclosure, strictly worse for Security. Allow-and-disclose
keeps it inside the auditable perimeter. So for genuinely-arbitrary trusted markup,
`unsafeRaw` is the answer and it exists. For the string-built uses, the *port* is
still option (b) in spirit — most are not arbitrary markup and should become direct
`Html` composition (see Decision 4); `unsafeRaw` is the residue, not the default.

**PROPOSED-NEW on the raw-IN surface: nothing.** This issue closes as "confirm
`unsafeRaw` covers raw-HTML injection + document the migration."

## Decision 2 — inline `<script>`: a raw HTML sink is not a safe script sink

An inline `<script>` body has its own escaping model, distinct from HTML text: the
serialiser emits `<script>` and `<style>` children **verbatim** (`raw_text`), so
HTML-escaping does *not* run inside them (`render_into_ctx` /
`raw_body = tag == "script" || tag == "style"`, `src/runtime/rust/src/html.rs`).
The pitfalls are therefore script-specific, not HTML-generic:

- **`</script>` breakout** — any `</script` substring in the body (even inside a
  JS string or JSON) closes the element early and drops the tail into markup.
- **JSON-in-script** — embedding config as JSON is common; the JSON must be
  breakout-safe, not merely HTML-escaped (HTML-escaping would corrupt the JS).

A single `unsafeRaw` sink does **not** solve this: it hands the whole burden to the
caller with no structural help for the common case. Ipê's asymmetry is deliberate
and already codified at the sink: **`<style>` bodies are close-tag-neutralised**
(twice — at `styleNode` construction and again via `strip_style_close` at the
sink), because they are reachable from `Ipe.Css` values; **`<script>` bodies are
NOT stripped**, because the only way to produce one is an author-owned hatch. That
asymmetry is the design telling us script content must be minted through a *typed,
breakout-safe* path, not a raw string.

### Sanctioned mechanism (tiered, prefer typed)

**Common case — embed config / bootstrap data → a typed, breakout-safe construct,
NOT a raw string.** The shipped exemplar is
`Ipe.Web.Head.Unsafe.unsafeJsonLd : String -> Html msg`
(`src/stdlib/Ipe/Web/Head/Unsafe.ipe`), which wraps the body as
`Html.script [ Attr.attribute "type" "application/ld+json" ] [ Html.text body ]`.
Its own doc states the discipline: **build the JSON from typed data (an encoder
over your own record types via `Ipe.Json.Encode`, see
[`codec-and-store-design.md`](./codec-and-store-design.md)) so the string is
structurally trusted; never splice raw request input.**

That exemplar is honest but incomplete: it is named `unsafe*` and lives in an
`.Unsafe` submodule precisely because it does *not yet* neutralise `</script>`.
The **PROPOSED-NEW** refinement, in the same spirit as the `<style>` path, is a
**safe** typed data-embed that needs no `unsafe`:

> **PROPOSED-NEW** (illustrative signature, not shipped):
> `Ipe.Html.jsonData : String -> Value -> Html msg`
> — emits `<script type="…" id="…">…</script>` whose body is a `Value` serialised
> by `Ipe.Json.Encode` with `<`/`>`/`&`/U+2028/U+2029 escaped to their `\uXXXX`
> JS-string forms. `</script>` breakout is then **structurally impossible** (no
> literal `<` survives), so this ships on the **plain `Ipe.Html` surface** and
> discloses **no** capability. The reader recovers the data JS-side with
> `JSON.parse(document.getElementById(id).textContent)` (or, preferably, the
> typed `sync`/transport channel of the JS-interop design, below).

This makes the safe path the terse one and reserves the hatch for the rare
arbitrary case, exactly the asymmetry Decision 1 relies on.

**Arbitrary trusted script body → the hatch.** When the body is genuinely
arbitrary trusted JS (a third-party analytics snippet), the sanctioned surface is
`Html.script [...] [ Ipe.Html.Unsafe.unsafeRaw body ]` — the `unsafe` capability
fires on the `Ipe.Html.Unsafe` import, the `unsafe*` name marks the call, and the
caller owns the `</script>` invariant. Reserve this for irreducibly-arbitrary
markup; never for embed-config, which the typed path covers.

**Prefer a safe channel over any script body — the JS-interop discipline.** For
passing typed *data* from Ipê to JS, the JS-interop design
([`web-js-ports-design.md`](./web-js-ports-design.md)) supplies a typed transport
(`Js.subscribe` / `sync`) that removes the `data-ipe-eval` / `new Function` seam
entirely. A bootstrap `<script>` that only carries data is usually better expressed
as that transport — no script body, no breakout surface at all. `jsonData` is the
fallback for the genuine first-paint / no-JS-yet case; a raw script body is the
last resort.

## Decision 3 — render-OUT: legitimate and already shipped, on the safe surface

`render : Html -> String` (and `toString`, its alias) **exist on the plain
`Ipe.Html` surface** (`src/stdlib/Ipe/Html.ipe`), kernels `HtmlRender` /
`HtmlToString` → runtime `html_render_`. `renderStatic : (model -> Html msg) ->
model -> Task Error ()` renders a view once to a static string as a `Task`. These
are the SSR / HTML-email need and they are **safe**: the serialiser is the escape
barrier (`escape_text` on every non-raw `HText`; unsafe tag names dropped;
`<script>`/`<style>` bodies handled as above). Rendering OUT never re-introduces
untyped input — the input is already a typed `Html` tree — so it is correctly
capability-free and needs no `.Unsafe` home. The dangerous direction is only IN.

No change proposed. This closes the "issue mentions `render`" thread: OUT was never
the hazard; the "no string round-trip" framing conflated it with IN.

## Decision 4 — migration of the string-built view

The raw-string uses and the two inline `<script>` bodies port by *kind*,
cheapest-and-safest first:

| Source pattern | Ports to | Surface / capability |
|---|---|---|
| String concat building element markup (most uses) | Direct `Html` composition (`div`, `text`, typed attrs) | Plain `Ipe.Html`; none. The bulk are not arbitrary markup — they become escaped trees. |
| Embed config/bootstrap JSON in a `<script>` | `Ipe.Html.jsonData` (**PROPOSED-NEW**) over an `Ipe.Json.Encode` `Value` | Plain `Ipe.Html`; none. Breakout-safe by construction. |
| JSON-LD `<script type="application/ld+json">` | `Ipe.Web.Head.Unsafe.unsafeJsonLd` (**exists**) | `Ipe.Web.Head.Unsafe`; `unsafe`. |
| Arbitrary trusted markup / arbitrary trusted script body | `Ipe.Html.Unsafe.unsafeRaw` (**exists**), inside `Html.script [...] [...]` for a script | `Ipe.Html.Unsafe`; `unsafe`. |

Goal: the two `<script>` bodies land on the **safe** `jsonData` path if they carry
data (the common reality), and only irreducibly-arbitrary residue reaches
`unsafeRaw`. A view that ends up importing `Ipe.Html.Unsafe` has made that reach
**disclosed**, not hidden.

**Capability disclosure in the manifest.** Per the convention doc, importing any
`Ipe.Html.Unsafe` (or `Ipe.Web.Head.Unsafe`) sets the import-derived `unsafe`
capability. `ipe capabilities` prints `unsafe` (with `--verbose` naming the
`via Ipe.Html.Unsafe` domain); `ipe add` is loud on it; an undeclared-but-inferred
`unsafe` is the same compile-time honesty error a hidden `network` would be. A view
ported entirely to direct composition + `jsonData` discloses **nothing** — the
desired end state.

**The diagnostic a user gets for a raw sink.** Reaching for `Html.raw` or
`Html.unsafeRaw` on the plain surface gets **IPE-N0005** (no such member) — the
name simply does not resolve there. The user must write the disclosing import
`import Ipe.Html.Unsafe exposing (unsafeRaw)` to reach it at all, which is the point:
the escape cannot be taken without the act that discloses it. (A friendlier,
suggestion-carrying render of this — "did you mean `Ipe.Html.Unsafe.unsafeRaw`? it
discloses the `unsafe` capability" — is the diagnostics-quality concern tracked
elsewhere, not this doc's surface change.)

## Security-review gate

`unsafeRaw`, `unsafeJsonLd`, and the render sink already shipped through the
language-boundary review. The **PROPOSED-NEW** `Ipe.Html.jsonData` is a new
language-boundary surface (it decides an escaping contract for a scripting context)
and MUST pass **security-soundness-guardian** review before merge — specifically:
that its `<`/`>`/`&`/U+2028/U+2029 → `\uXXXX` escaping makes `</script>` breakout
unrepresentable for *all* `Value` inputs, and that it introduces no capability
(it must not, or it is not the safe path it claims to be). This doc is the spec
that review checks against.

## Open ambiguity for the user

`Ipe.Html.jsonData` is the one genuinely-new surface proposed here — everything
else is confirmation. Decide whether to:

1. **Ship `jsonData`** as the safe typed data-embed (recommended: it makes the
   common inline-`<script>` case capability-free and breakout-safe, and lets the
   `unsafeJsonLd` exemplar be re-expressed over it — the guarded default the
   `<style>` path already models), or
2. **Defer it** and route all inline-`<script>` embeds through the existing
   `Ipe.Web.Head.Unsafe.unsafeJsonLd` + `unsafeRaw` hatches, accepting that
   data-embed inherits the `unsafe` capability until a typed sink lands.

Recommendation: (1). It closes the exact gap Decision 2 identifies — a raw sink is
not a safe script sink — and aligns the script path with the already-shipped
double-neutralised `<style>` path.
