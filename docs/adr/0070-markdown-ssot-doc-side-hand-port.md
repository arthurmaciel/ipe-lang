Status: Accepted
Date: 2026-09-15

# 0070. Doc-side Markdown renders through the `Ipe.Markdown` parse tree

## Context

`Ipe.Markdown` is the language's Markdown authority: a pure-Ipê parser that
turns Markdown source into a `Block`/`Span` tree and renders it to `Ipe.Ui`
elements, emitting no raw HTML and routing every link `href` / image `src`
through a scheme allowlist. It is safe by construction.

The `ipe doc` HTML site renders doc-comment and guide Markdown through its own
separate Rust renderers — a hand-rolled comment renderer and a general
document renderer. Two independent Markdown implementations means two parse
behaviours that can drift, and two security surfaces (text escaping, URL-scheme
gating) that must each be got right independently. The Markdown-to-HTML step is
an injection boundary: any text byte emitted unescaped, or any `javascript:` /
`vbscript:` / dangerous `data:` URL that reaches an `href`, is a cross-site
scripting hole. Under the principle order (Security first, then Correctness),
one audited rendering path is safer than several.

The obstacle to a literal single source of truth is that the doc site is Rust
tooling in the `ipe` binary: it cannot run Ipê at doc-time (no toolchain on a
reader's machine) and must not introduce a build-time bootstrap cycle. So the
`Ipe.Markdown` *parser* cannot simply be invoked from the doc renderer.

Two structural obstacles rule out compiling `Ipe.Markdown`'s parser and
committing the emitted Rust:

- The per-module emitter produces Rust that still references sibling modules
  and resolves its `String.*` / `List.*` calls through the runtime crate. A
  committed emitted parser would re-couple the doc tooling to the runtime — a
  dependency the doc tooling deliberately does not carry.
- Emitted Rust routinely indexes and unwraps in ways the workspace clippy
  deny-set forbids; admitting it into the doc rendering trusted surface would
  require a lint escape hatch there, a strictly worse security posture than
  hand-written lint-clean code.

## Decision

Establish `Ipe.Markdown`'s `Block` / `Span` / `HeadingLevel` tree as the single
Markdown parse model, and render doc-site Markdown through it:

- **Hand-port the parser** (`parseBlocks` / `parseSpans` and the ADTs) into a
  std-only leaf module of the doc tooling, written with `Result` and iterators —
  no indexing, unwrap, or panic — so it is deny-set-clean by construction and
  adds no runtime dependency.
- **A semantic-parity gate** keeps `Ipe.Markdown` authoritative: for a shared
  corpus, the ported parser's tree must equal a tree snapshotted from an actual
  `ipe` run of `parseBlocks` / `parseSpans`, regenerated and diffed in CI. Any
  drift reddens the build. This is what preserves the single source of truth for
  parse behaviour despite the second implementation.
- **A doc-side `Block`→HTML walker** renders the shared tree. It is the sole
  security-critical component and must: HTML-escape every text byte by default
  (`&` `<` `>` `"` `'`); route every `href` / `src` through one shared
  `is_safe_href` scheme allowlist (`http` / `https` / `mailto` and scheme-less
  relative; reject `javascript:` / `vbscript:` / dangerous `data:`) enforced at
  BOTH parse and emit (defend in depth), failing closed to escaped plain text on
  rejection; carry an explicit exhaustive arm per `Block` (8), `Span` (7), and
  `HeadingLevel` (6) constructor so a new constructor forces a compile error;
  and bound blockquote nesting depth so deeply-nested input cannot exhaust the
  stack.
- **One `is_safe_href` allowlist SSOT** lives in the leaf module; the app-side
  and doc-side paths both gate URLs through the same allowlist rather than each
  carrying its own.

Rejected alternatives:

- *Compile-and-commit the parser.* Rejected on the two structural obstacles in
  Context: it re-couples doc tooling to the runtime and needs a clippy escape
  hatch in the security-critical surface. Retained only as a documented fallback
  should a runtime-free inline-lowering emit mode ever be built.
- *A curated kernel shim* (emit the parser slice plus a dozen hand-written
  duplicate `String` / `List` kernels). Rejected: it substitutes a dozen
  duplicated kernels — each its own drift risk — for one parser, more surface
  for the same outcome.

## Consequences

- The parse logic now has two implementations. This is a deliberate cost: it
  keeps compiler-emitted Rust out of the doc rendering surface and needs no lint
  escape hatch. The parity gate is the invariant that must hold — if it is ever
  weakened or removed, the two parsers may silently diverge.
- The two doc-side incumbent renderers are retired only behind a
  byte-equivalence gate over the real corpus, and only after the ported parity
  gate and a refusal suite (URL-scheme rejection, raw-markup literalisation,
  full escape-set) are green. A rendered page must never lose escaping or gain
  an executable URL during the migration.
- The single `is_safe_href` allowlist must remain the only home for URL-scheme
  policy. A second copy anywhere reintroduces the drift the SSOT removes.
- The heading-level offset a doc page applies (a body heading nesting under the
  page chrome) lives in the doc-side caller, never in the shared parser, so the
  parser stays a faithful `H1`..`H6` model for every consumer.

## Conventions

ADRs describe Ipê on its own terms. Do not reference any prior or external
implementation, parity with another system, or project ancestry — state each
decision as a standalone Ipê decision.
