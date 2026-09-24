Status: Living (consolidated)
Date: 2026-09-18
Archive: misc/docs/archive/ADR/

# 0006. Dev-loop & program-as-data

How the inner edit→see-it loop is made fast without ever letting the dev preview
diverge from what ships. The lever is to compile nothing for the common edit:
the program is partitioned into static parts the runtime holds as patchable data
and dynamic parts that stay compiled logic, run through one read-from-data
routine that is identical in dev and production.

## Decisions

### The dev-loop speed lever is compiling nothing

The inner-loop latency floor is set by how much an edit forces to recompile, and
the dominant edit class is view/appearance. Measured against the alternatives —
native incremental recompile (already near its floor), a core/view crate split
(net-negative on the dominant class), alternative codegen backends (delta too
small), a separate view-host runtime (removes only the relink) — only a
data-patch tier both beats the recompile floor by roughly an order of magnitude
*and* speeds the dominant edit class. Dev-loop effort is invested there; the
incremental recompile path remains the correct fallback for edits that introduce
genuinely new compiled logic. Reopening a shelved lever *for dev-loop speed*
requires overturning that measurement, not an intuition.

### Program-as-data: partition into patchable data vs compiled logic

Every program is partitioned into static parts held as patchable data and dynamic
parts that stay compiled, under one dev-equals-production mechanism: **emit
compiled code that reads a data-table entry — production bakes the entry, dev
patches it over the live socket, and the same compiled read/apply/render routine
runs in both; only the table contents differ.** Because the interpreter *is* the
compiled read-from-data routine, dev equals production by construction. The
partition is realised part by part — view structure and appearance as templates
with typed holes (`web/template.rs`, `literal_table.rs` — a crate-root module
re-exported as `web::literal_table`), simple update arms
as transition descriptions (`web/transition.rs`, `transition_classify.rs`),
subscriptions and effect wiring as descriptions (`web/sub_desc.rs`,
`web/cmd_wiring.rs`), session-scoped init as a datum (`web/init_datum.rs`) —
while anything not provably static stays compiled.

The classifier is **biased toward compiled**: a part is patchable data only when
the compiler can *prove* it carries no model-derived logic. A misclassification
that recompiles is merely slow; one that hot-swaps a logic change is a
correctness bug, so the unprovable case always falls back to recompile. Handlers
are never serialised as closures — a static handler is a template constant
carrying an opaque id; a model-dependent handler is a hole filled by the server's
per-render handler map.

Every data-driven part carries a conformance test proving that interpreting the
data equals running the baked specialisation; the read/apply routines are bounded
by construction. Relaxing the compiled-bias or shipping a data-driven part
without its conformance test is the exact failure this structure prevents.

### Hot-swap the static tier; recompile the dynamic tier, made fast

An appearance or provably-static structural edit is handed to the running program
as a new table, applied, and re-rendered at the program's **current** Model
through the existing diff-and-push path — no compile, no restart, no state reset.
The classifier is **emit-diff** (`hot_classify.rs`): it re-runs only the
front-end and compares the new emitted output against the previous; the edit is
static-only iff the sole difference is table contents. Because the emitted output
is the source of truth, a logic change can never be misclassified as appearance.
The set of hoistable literals is a single declarative registry keyed by kernel,
self-enforcing by an exhaustive match (a new kernel does not compile until
classified) and registry-driven tests proving each entry renders byte-identically
to its direct emit and refuses a Model-dependent value.

A change to control flow, a Model-dependent value, or a new handler is a new
program: it recompiles. The recompile is kept cheap by incremental compilation
and a **blue-green swap** in the dev watcher — a persistent front proxy holds the
port, the rebuilt binary starts behind it and is cut over on a readiness signal,
and the running Model is handed to the new process, so a logic rebuild neither
drops the browser connection nor loses your place. This apparatus is **dev-only**
and may be aggressive precisely because it never ships; the shipped program is
the ordinary compiled binary, exposing drain/readiness hooks for its deployment
platform to orchestrate.

### Model-state preservation across rebuilds, with fail-closed resets

`ipe watch` persists the running Model to a server-side dev session store so a
rebuild preserves live state. `web::additive::reconstruct` splices the checkpoint
onto the new binary's `init` only on a *proven additive superset* (a gained
field) and only if the merged object decodes strictly; any other change (removal,
rename, retype) forces a clean `init`. Two escape hatches sit strictly on top,
both fail-closed: `ipe watch --reset-state` gates every returning session to a
fresh `init` for the binary's lifetime (an unrecognised env value leaves the
additive algorithm in place), and the debugger's "reset to init"
(`RecordBuffer::reset_to_init`, the `/_ipe/debug/reset` endpoint) restarts the
recorded history and live session without restarting the server. The debugger
reset endpoint is CSRF-protected and gated behind the `debugger` feature so it
never appears in a release build; a reset fires no `update` and no `Cmd`.

The same server-held model-as-data is what makes time-travel (replaying recorded
messages through `update`) and live-state inspection fall out of the partition
rather than being built as separate machinery.
