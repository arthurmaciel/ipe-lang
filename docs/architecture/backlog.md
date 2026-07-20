# Backlog

Durable open-work items. One line each; newest at the bottom.

- MUST-FIX REVIEW — direct IR→WASM backend (`ipe_backend_wasm`, spec in `docs/architecture/tbd/wasm-backend.md`): rejected as unbuildable-as-scoped. Headline blocker: the vendored rustc-origin `runtime.wasm` (linear memory) cannot interoperate with the proposed WASM-GC app for any heap-carrying, generic, or higher-order kernel — so runtime-reuse (the plan's cost-bounding thesis) does not exist and the 4-5-month MVP is mis-costed. Needs a single coherent app↔runtime memory story proven on one non-flat value before re-review.
