Status: Accepted

# 0034. Language server as a second consumer of the salsa query graph

## Context

The `ipe lsp` subcommand serves editor features (diagnostics, hover, completion,
go-to-definition, find-references, rename, semantic tokens, folding, inlay hints,
signature help, document symbols, code actions) over JSON-RPC-on-stdio. The
danger a language server invites is a *second analyzer*: a fast, approximate
front-end that computes types, name resolution, and diagnostics differently from
the compiler, and therefore disagrees with it. A hover that shows a type the
build does not, or a completion that offers a name the checker rejects, is a
correctness violation — the server would be lying about the program.

The compiler already exposes its front end as a memoised salsa query graph
(`parse`, `imports`, `resolve_imports`, `canonicalize`, `module_interface`,
`typecheck`, `lower`) keyed on `SourceFile`/`SourceRoot` inputs, with a durable
cut-point at the whole-program IR. A body-only edit re-runs only the stages
downstream of the edited module. The server needs keystroke-fast, always-correct
answers; the query graph already computes exactly those answers, incrementally.

## Decision

The language server is a **second consumer of the one salsa database**, never a
divergent second analyzer. Every editor feature is served from the same memoised
queries the compiler runs: completion and hover read `canonicalize` and
`typecheck`; navigation reads `resolve_imports`/`canonicalize`; signature help
reads `typecheck`. The server owns no parser, name resolver, or type inferencer
of its own.

Structure: a thin transport crate (`ipe_lsp_server`) built on **`lsp-server`**
(a minimal JSON-RPC framing loop) rather than `tower-lsp` — the async actor model
of `tower-lsp` buys nothing over a synchronous request loop against a salsa DB,
and a synchronous loop keeps cancellation and single-flight straightforward. A
feature crate (`ipe_lsp_features`) holds one module per capability, each a pure
function from `(db, request)` to an LSP response.

Boundaries that make lying unrepresentable:

- **The transport is the only place that touches stdio/JSON.** JSON-RPC params
  are parsed once into typed request structs at the server boundary; feature
  handlers hold no `std::io`/`std::fs`. Positions cross the UTF-16 `Position` ↔
  byte `Span` boundary through **one** `PositionEncoding`-aware converter, never
  re-derived ad hoc.
- **Edits are verified, not asserted.** A `rename` produces a `WorkspaceEdit`
  built from resolved definition + reference spans out of the query graph; there
  is no path from an unresolved symbol to a surfaced edit.
- **Graceful degradation, never a wrong answer.** A program that does not
  type-check still yields completion — the provider falls back to kind-only
  items (no type annotation) rather than inventing types or returning nothing.
- **Diagnostics reuse the compiler's teaching corpus.** Every diagnostic already
  carries its `Code` and an `explain_page`; the server surfaces those verbatim
  and does not author a parallel corpus.

`typecheck` is currently a **coarse whole-program query** (one solve per program,
memoised across no-op edits), not per-module. This is correct — it can never show
a stale type — but a settled edit on a large project re-solves the whole program.
Per-module `typecheck(ModuleId)` granularity is a separate, tracked `ipe_types`
refinement that lands later with zero handler changes.

The **type-directed completion** differentiator (offer only candidates
compatible with the type the context expects at the cursor) is deliberately *not*
shipped in this decision. It requires net-new solver work — an `ExpectedTypes`
sidecar recorded during constraint generation and an `expected_type_at` query,
plus scratch-arena speculative unification isolated from the memoised solve. That
foundation is tracked as remaining work, not part of this accepted architecture.

## Consequences

- The server can never disagree with the build about parsing, name resolution, or
  types, because it runs the build's own queries. This is the load-bearing
  invariant: any new feature must be sourced from a query, never from a private
  re-analysis.
- Editor latency rides on salsa incrementality. Keystroke responsiveness for
  hover/navigation is already good (parse/canon are per-module firewalled);
  settled-edit diagnostic latency on large projects is bounded by the coarse
  `typecheck` until per-module granularity lands.
- Feature-handler `match`es over compiler AST/IR carry no wildcard arm, so a new
  variant is a compile error in the server until every handler gets its arm —
  the server cannot silently mishandle a new language construct.
- The `lsp-server` choice foregoes a ready-made async capability router; each
  capability is dispatched explicitly in the main loop. This is a deliberate
  trade for a synchronous, cancellation-friendly path against the DB.
- The unshipped type-directed completion and the coarse-vs-per-module `typecheck`
  refinement are the two open follow-ons; both extend this architecture without
  altering it.
