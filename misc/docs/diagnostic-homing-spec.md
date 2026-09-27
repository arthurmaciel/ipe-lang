# Spec — every diagnostic is homed to the source that produced it

Crates: `ipe-cli` (`driver/build_pipeline.rs`, `watch.rs`), `ipe_lsp_server`,
`ipe_diagnostics`, `ipe_lower` (L0102). Principles: Correctness (2), Ease of
use (5), SSOT. Lane tier: high.

## Class-closing property

A `Diagnostic` carries a `SourceId` (module + file) from the stage that made
it, not a span re-mapped afterwards. Every consumer (CLI render, watch, LSP
publish) routes by that id through one function; a diagnostic with no
resolvable home is a typed `Unhomed` bucket rendered at the entry, never
silently dropped or pinned to the wrong file.

## Why the instances exist (origin/main 983fdc404)

- `build_pipeline.rs:1039` `source_for_span_in_linked` and `:1093`
  `attribute_post_link_error` re-derive the file from a span after linking —
  a span from a stdlib or dependency module lands in the entry file.
- Warnings (not errors) skip the homing path; the LSP publishes them on the
  wrong URI or not at all (#2930). Fix on `lane/2930-homed-warnings` and
  `integration/lsp-loose` — unmerged.
- L0102 span points at the enclosing decl, not the offending expression
  (#2941 R1). Fix on `lane/2941-l0102-span` — applies cleanly to main.
- Test-stdlib lint diagnostics are homed to the user file (#2922). Fix on
  `lane/2922-test-stdlib-lint` — unmerged.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2930 warnings homed | real; fix stranded | no homed-warnings code on main |
| #2935 post-link attribution by span | real | `build_pipeline.rs:1039`, `:1093` |
| #2922 test-stdlib lint homing | real; fix stranded lane/2922 | — |
| #2941 R1 L0102 span | real; lane/2941-l0102-span applies clean | `git apply --check` |

## Implementation plan

1. Land `lane/2941-l0102-span` (clean).
2. `Diagnostic { source: SourceId, span, … }`; each stage fills `source` at
   construction; `attribute_post_link_error` and
   `source_for_span_in_linked` become a lookup by `SourceId` (delete the span
   heuristics).
3. Rebase lane/2930 and lane/2922 onto (2): warnings and lints use the same
   field.
4. One `route_diagnostics(Vec<Diagnostic>) -> BTreeMap<SourceId, Vec<_>>` used
   by CLI, watch, LSP.

## Prove the refusals

- type error inside a dependency module → reported in that module's file;
- warning in an imported module → LSP publishes on the imported URI;
- diagnostic with a synthetic span → `Unhomed`, rendered, not dropped;
- L0102 caret under the offending expression (snapshot).

Lane tier: high.
