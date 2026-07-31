# The typed-`Url` subsystem

`Ipe.Url` is one subsystem with three coherent facets over a single value:

1. **the typed `Url`** — an opaque, parse-once absolute-URL value (shipped);
2. **the routing parser** — pure-data patterns that match a typed `Url` into
   typed route captures (shipped as `Ipe.Url.Parser`);
3. **the outbound-request boundary** — `Ipe.Http` request construction and the
   runtime SSRF guard, both consuming that same typed `Url` (planned).

The design invariant that unifies them: **there is exactly one parse of any URL
string.** A raw string becomes a `Url` once, at the seal. Everything downstream
— the router that matches on it, the request builder that carries it, the SSRF
guard that vets it — consumes the already-typed value and never re-parses a
string. No stringly re-entry, no second parser that can disagree with the first.

## The typed `Url` value

### What is shipped

`Url` is an opaque newtype over the `url` crate's `Url`, exposed by the
`Ipe.Url` stdlib module. Its one public constructor is the seal (type
signature, quoted from `src/stdlib/Ipe/Url.ipe`):

```
fromString : String -> Result Error Url
```

`fromString` parses with the `url` crate and **fails closed**: a relative
reference (`"/path"`), a scheme-less host (`"example.com"`), or unparseable
garbage all surface as a typed `Err`, never a silent accept. A `Url` value is
therefore **always an absolute, scheme-carrying URL** — and, for a hierarchical
scheme, host-carrying. That "always has a scheme, always (hierarchically) has a
host" property is a construction invariant, so downstream code never
re-encounters a scheme-confused or hostless URL. This is parse-don't-validate:
the invariant is proven by the type, established once.

Typed accessors consume a `Url` (never a `String`): `scheme`, `host : Maybe`,
`port : Maybe` (scheme default applied), `path`, `query : Maybe`,
`fragment : Maybe`, and the un-parse `toString`. The injection-safe builder
`buildQuery : List (String, String) -> String` percent-encodes every key and
value through the `url` crate's `form_urlencoded` serializer, closing the
query-injection footgun that raw string concatenation leaves open.

The crucial shared-parser fact: `reqwest::Url` is `pub use url::Url`, so the
`Url` type boundary and the runtime SSRF guard parse with the **identical**
parser. The type boundary and the guard cannot diverge on what counts as a
valid URL.

**Trust model.** `Url` guarantees *syntactic* validity — a well-formed absolute
URL with a scheme. It deliberately does **not** decide whether that URL is
*safe to fetch*: `http://169.254.169.254/` is a perfectly valid `Url`. Safety
is a separate runtime authority (the SSRF policy). `Url` is the syntactic parse
boundary that **feeds** that authority; it is not a substitute for it.

### What remains

Two facets build on this shipped core: the routing-combinator parser, and the
outbound-request adaptation together with its SSRF boundary.

## The routing-combinator parser

A routing parser turns a typed `Url` into a typed route value through
composable combinators, rather than stringly key lookups over path segments and
query keys. It consumes a `Url` — it does **not** re-parse a raw string — so it
inherits the seal's guarantees for free.

### Shape (compared to `elm/url`'s `Url.Parser`)

`Ipe.Url.Parser` follows `elm/url`'s `Url.Parser` path vocabulary so an author
who knows Elm reads it without translation. It diverges in one structural way:
a `Pattern` is **pure data**, not a function-carrying `Parser`, and the caller
applies the route constructor. The reason is a Rust-backend limit — a router
that threads a builder function through the parser (as `elm/url` does) cannot
lower, because the backend stores a function value only in a union payload,
never in a record field, never forwarded through a closure capture, and never as
a non-`Clone` element of a `List` walked by value (which is exactly what a
`oneOf` of builder-carrying alternatives would require). The full rationale is
in `docs/divergences-from-elm.md`.

Surface (shipped):

- `Pattern` — an opaque pure-data route pattern: an ordered list of segment
  matchers plus the query keys to read. Composes and lists freely.
- `s : String -> Pattern` — match one exact static path segment.
- `string : Pattern` / `int : Pattern` — capture one path segment as a `String`
  / `Int` (a non-integer segment fails `int`, fails closed).
- `top : Pattern` — match the root; the identity of `slash`.
- `slash : Pattern -> Pattern -> Pattern` — sequence two path patterns
  (`elm/url`'s `</>`).
- `withQuery : Pattern -> Query -> Pattern` (`elm/url`'s `<?>`) and
  `query : String -> Query` — attach and name a query key.
- `parse : Pattern -> Url -> Maybe Captures` — match a pattern against a typed
  `Url`, yielding the ordered `Captures` on a total match, `Nothing` otherwise.
- `Captures` + `firstString` / `firstInt` / `firstQuery` — the ordered captures
  and the common single-capture readers; the caller applies its route
  constructor in a `case` chain over `parse` results.

The design keeps invalid states unrepresentable: `parse` returns `Maybe
Captures` (a total match or none — never a partial/ambiguous match), and there
is no wildcard that silently swallows an unrecognised path. An unmatched `Url`
is an explicit `Nothing` the caller must handle. A caller expresses "first
matching alternative wins" as an ordinary `case` chain over `parse` results.

### Backend lowering

`Ipe.Url.Parser` is a **compiled-source Ipê module** with **no new kernels and
no stored function values**: `Pattern` / `Captures` are pure-data records and
unions, and every combinator is an ordinary Ipê function over them. Matching
consumes the parsed `Url`'s components through the **already-shipped** `Url`
accessors (`path` / `query`) — the split into path segments and query pairs
happens once, over the typed value. Because the whole module is pure data plus
pure functions, it lowers cleanly with no anti-drift kernel-registration cost.

The parser is the syntactic router; it never fetches. It therefore has **no**
security boundary of its own beyond totality — a mismatched route surfaces as
`Nothing`, and there is no path by which route-matching reaches the network.

## The outbound-request adaptation

Today `Ipe.Http` carries the request target as a raw `String`:
`HttpRequest.url : String` and `withUrl : String -> HttpRequest -> HttpRequest`.
An unparseable or scheme-confused string can reach the request builder and is
only caught at the runtime boundary.

The adaptation makes the typed `Url` the primary request target:

- The request builder accepts a typed `Url`. The primary constructor path takes
  a `Url` (built through the seal), so an unparseable target cannot be assembled
  into a request in the first place — the parse failure is a typed `Err` at
  `fromString`, upstream of any builder.
- There is **no unmarked stringly default** as the primary path. A convenience
  that accepts a raw string is a *marked* parse-at-the-boundary helper (it runs
  `fromString` and returns `Result`), never a silent accept that defers the
  failure.
- The `Url` handed to the builder is the same value the runtime receives; the
  runtime does not re-parse a fresh string to reconstruct it. The request DTO
  carries the typed value (or its single canonical serialization) to the
  boundary; downstream code does not re-validate.

This closes the audit's "URL validated at runtime, not in the type" gap: the
syntactic validity that the runtime once discovered by re-parsing is now a
property of the type the builder already required.

## The SSRF boundary (security-critical)

The Http-request-URL path is an **SSRF boundary**. Security is the highest
principle; this facet is designed fail-closed by construction.

### What the typed `Url` does and does not remove

The typed `Url` is the **syntactic** boundary — "this is a well-formed absolute
URL with a scheme and a host". It is *not* a substitute for the runtime SSRF
policy, and the runtime guard is **kept, not removed**. The reason is
structural: **redirect targets are untrusted and only knowable at runtime.** A
first-hop `Url` that is perfectly safe can redirect to
`http://169.254.169.254/`; only the runtime, following the redirect, sees that
target. The typed `Url` feeds the policy; it cannot replace it.

### The policy (fail-closed)

The runtime SSRF guard (`ipe_runtime::ssrf`, applied by
`http_client::ssrf_apply`) enforces, when the deny-private guard is active:

- **Scheme allowlist.** Only `http`/`https` reach the request surface
  (`ws`/`wss` for the WebSocket surface); everything else (`ftp`, `file`, …) is
  rejected.
- **Private/loopback/link-local deny.** A host resolving into any disallowed
  range is blocked: loopback, RFC-1918 private, link-local (incl. AWS IMDS
  `169.254.169.254`), unique-local, unspecified, this-network `0.0.0.0/8`,
  CGNAT `100.64.0.0/10`, IETF-protocol `192.0.0.0/24`, benchmarking, reserved
  `240.0.0.0/4`/broadcast, and the v4-mapped / NAT64 / 6to4 IPv6 encodings whose
  embedded IPv4 falls in any of those ranges.
- **DNS-rebinding pin.** The vetted IP is the IP reqwest connects to: the guard
  pins reqwest's resolver to the checked address (and installs a vetting
  `dns_resolver` for every hop), closing the validate-then-discard TOCTOU where
  a name could re-resolve to a rebind target between check and connect.
- **Per-hop redirect re-validation.** Every redirect target is re-checked by
  the same guard before it is followed; too many hops fail closed. The redirect
  fail-closed floor is the URL-literal-plus-scheme re-check **together with** the
  vetting `dns_resolver`: reqwest's redirect-policy callback receives no client
  builder, so a redirect hop is not re-pinned via the per-hop URL check alone —
  the rebind window on a redirect hop is closed at connect by the vetting
  resolver, and the per-hop URL check is the scheme/literal guard on top.

The default is the safe outcome: when `IPE_HTTP_DENY_PRIVATE` is unset the guard
is tied to the production gate — **on in production**, off only in explicit dev
— and an out-of-policy target is a typed `Err`, never a permissive fallthrough.

### No parse/fetch confusion — the two boundaries share one parser

The soundness hazard for a typed-URL SSRF design is **parse/fetch confusion**: a
validated `Url` that differs from the value actually fetched, so the check
vetted a *different* URL than the request hit. This design forecloses it:

- **One canonical parser.** The `Url` type boundary and the runtime guard both
  parse with the `url` crate (`reqwest::Url` *is* `url::Url`). The syntactic
  value the type carries and the value the guard validates are byte-identical —
  there is no second parser to disagree.
- **The validated value is the fetched value.** The typed `Url` handed to the
  builder is the value carried to the runtime and the value the guard vets; the
  runtime does not re-derive a target from a re-parsed string. The `Url` cannot
  be forged past the check because its only constructor is the seal.
- **Runtime re-validation survives at the one place it must.** The syntactic
  type cannot see redirect targets — only the runtime can — so the per-hop
  runtime re-check is retained. The type is the *feed*, the runtime guard is the
  *authority*; neither is dropped in favour of the other.

## Dependency-ordered implementation plan

Ordered by what unblocks what. Each item lands behind the full gate and THE
SEAL (`ipe build` exit 0 ⇒ emitted Rust `cargo build`s).

**Shipped:**

1. **Routing parser — pure-data patterns.** `Ipe.Url.Parser`: a compiled-source
   Ipê module exposing the pure-data `Pattern` / `Captures` types and the
   combinator surface (`s` / `int` / `string` / `slash` / `top` / `withQuery` /
   `query` / `parse` / `firstString` / `firstInt` / `firstQuery`), realised over
   the shipped `Url` accessors with no new kernels. Independent of the Http
   facet.

**Available now (the `Url` core is shipped; no blockers):**

2. **Http request target — typed `Url` in.** Adapt `Ipe.Http` request
   construction so the primary path takes a typed `Url`, with a *marked*
   parse-at-the-boundary helper for raw strings and no unmarked stringly
   default. Independent of the routing parser. **Mandatory
   security-soundness-guardian review of the adapted boundary before merge.**
3. **SSRF boundary consumes the typed `Url`.** Ensure the value the guard vets
   is the value the builder carried (no runtime re-parse to reconstruct the
   target), while retaining the per-hop runtime re-validation for redirect
   targets. Rides on item 2; reviewed as one boundary change with it.

Items 1 and 2/3 are independent and may proceed in parallel. Items 2 and 3 are
one boundary and land together under a single guardian review.

**No external blocker.** The `Url` core is shipped, so none of the three items
is blocked on another subsystem; the only ordering is the internal 2-before-3
coupling (they are the same boundary).

## Risks & open questions

- **Parser state exposure (resolved).** `elm/url` threads a continuation type
  through the parser. Ipê could not lower that function-carrying `Parser` (a
  builder function reaches a record field, a closure capture, or a
  by-value-walked list — all rejected), so `Ipe.Url.Parser` uses a pure-data
  `Pattern` and the caller applies the route constructor over `parse`'s
  `Captures`. No author-visible state type is leaked; see
  `docs/divergences-from-elm.md`.
- **Kernel vs compiled-source for the path/query split (resolved).** The whole
  module is compiled-source over the existing `Url` accessors — no kernel was
  needed, so no anti-drift registration cost.
- **Marked-stringly ergonomics.** The raw-string request helper must be
  *obviously* a parse boundary (returns `Result`), not a drop-in that tempts
  authors back to stringly targets. *Open:* naming and surface so the typed path
  is the path of least resistance.
- **`buildQuery` ↔ query-parser round-trip.** The injection-safe builder and the
  routing query-parser should agree on encoding, so a query built by one and
  parsed by the other round-trips. *Open:* a shared encoding contract (or a
  round-trip test asserting it) between the two.
- **Scheme set consistency (mandatory narrowing).** The request surface accepts
  `http`/`https`; the typed `Url` legally carries any absolute scheme (`file:`,
  `ftp:` are valid `Url` values by design). The *marked* helper and the builder
  therefore **must** reject non-`http(s)` schemes at the type/API layer, so the
  runtime scheme allowlist is defence-in-depth rather than the first (and, when
  the guard is off in dev, the *only*) line. Narrowing early is a requirement,
  not an option: a builder-constructed request fails closed at the API layer
  even with the runtime guard disabled.

## Soundness review

Verdict: **approve with conditions.** The design mirrors the shipped ground
truth, and the four soundness properties hold as designed:

- **Fail-closed SSRF boundary.** All guard work is gated on the deny-private
  decision; when the toggle is unset the guard is on in production. Scheme
  allowlist and private-deny are enforced at the entry and re-enforced per hop.
  An out-of-policy target is a typed `Err`, never a permissive fallthrough.
- **No parse/fetch confusion.** The type boundary and the runtime guard parse
  with one canonical parser (`reqwest::Url` is `url::Url`), so the validated and
  fetched values are byte-identical on the first hop. Redirect targets are
  runtime-only, so the per-hop runtime re-validation is genuinely required and
  kept — not defence-in-depth. The redirect fail-closed floor is the per-hop URL
  scheme/literal check **together with** the vetting resolver that re-vets each
  hop's address at connect (the redirect-policy callback alone cannot re-pin).
- **Unforgeable `Url`.** The type is an opaque newtype whose sole constructor is
  the parse seal, with no serde/`From` forge path; a `Url` cannot be
  manufactured past the check.
- **Router totality.** `parse` returns `Maybe`, `oneOf []` matches nothing, and
  there is no silent wildcard, so a mismatched route is an explicit `Nothing`.
  The router consumes the typed `Url`, never re-parses a string, and never
  reaches the network — it has no security boundary of its own.

Conditions folded into this design:

1. **Narrow the scheme at the API layer (mandatory).** The marked raw-string
   helper and the builder must reject non-`http(s)` (and `ws`/`wss` for the
   WebSocket surface) schemes at the type/API layer, so a builder-constructed
   request fails closed even with the runtime guard off in dev. Captured in the
   scheme-set risk above as a requirement, not an option.
2. **Typed error channel.** The marked parse-at-the-boundary helper returns
   `Result Error _` — never `Result String _` and never a sentinel.
3. **Redirect-floor accuracy.** The redirect re-validation is described as the
   per-hop URL scheme/literal check plus the vetting resolver together, not the
   URL re-check alone.
4. **One boundary, one review.** The request-target adaptation and the SSRF
   consumption land together under a single binding security-soundness-guardian
   review before merge.

No block-level defect: no design path lets a non-`http(s)` scheme, a
private/loopback/link-local host, or a rebind reach an outbound fetch while the
guard is on; condition 1 closes the guard-off (dev) gap as defence-in-depth. A
`buildQuery` ↔ query-parser encoding divergence is a correctness (not security)
concern and is required to be covered by a round-trip test.
