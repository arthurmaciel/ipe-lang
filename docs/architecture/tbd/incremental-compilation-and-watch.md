# Incremental compilation and the developer loop for Ipê

> **Status:** authoritative design spec (design-only — no implementation
> commitment beyond the locked decisions below). Supersedes the memory note
> `incremental-compilation-salsa.md` for scope purposes.
> **Implementation status:** Phase 1 (salsa db + `SourceFile`/`SourceRoot`
> inputs + `parse`/`imports` tracked queries on the one-shot `ipe` path)
> landed 2026-07-11 — see
> `docs/architecture/salsa-incremental-compilation-2026-07-11.md`.
>
> **Principles order (hard):** security > correctness > soundness > efficiency
> > completeness > readability. Two fundamental rules govern every decision:
> **PARSE, DON'T VALIDATE** and **MAKE INVALID STATES UNREPRESENTABLE**.
> The paramount hazard is incremental **under-invalidation** — a stale build
> that looks correct is a correctness violation and outranks any efficiency
> gain. Hot-reload must never open a dynamic-code / `eval` hole; Ipe.Live's
> no-`data-sky-eval` + strict-CSP (no `unsafe-eval`) posture is a hard
> invariant at every reload level.

---

## Executive summary (Q1–Q4 verdicts)

1. **Q1 — WHERE incremental fits.** The whole pure compiler DAG
   (`sky_parse → sky_canon → sky_types → sky_lower → sky_ir → sky_backend_rust`)
   becomes a salsa query graph, cut durably at **`sky_ir`**. The emit→cargo
   boundary is *outside* salsa, bridged by deterministic + write-if-different +
   delete-orphans reconciliation. **FFI introspection stays a coarse
   content-hash cache outside the graph**, regenerated only by explicit
   `ipe add`/`ipe install`; its typed `.ipei`/`kernel.json` interface feeds
   *in* as a per-package salsa input.
2. **Q2 — `ipe watch`.** Strict allowlist scope; bounded debounce; salsa
   minimal-recompute; deterministic emit reconciled to disk; **last-good binary
   stays alive on any red build**; readiness-gated restart; localhost-only, no
   network control channel; every stage timeout-bounded.
3. **Q3 — Hot-reload.** Ship **L0** for v1 (rebuild + restart + SSE reconnect +
   persisted-session restore), packaged as **L0+** (proactive `event: reload`
   frame + readiness-gated handoff + sqlite dev-store default). L1/L2 are
   categorically unreachable on the Rust-AOT path (no stable dylib ABI) and are
   deferred to the future **interpreter tier** — swapping typed IR server-side,
   wire protocol unchanged.
4. **Q4 — Does watch alone feel immediate?** **YES — conditionally.** It buys
   the *illusion of continuity* (you never lose your place), not literal
   sub-100ms instantaneity; raw latency is ~1–few seconds bounded by cargo
   compile+link. The illusion holds for Ipe.Live web (and Ipe.Tui) **when** (a)
   the dev session store is persistent (sqlite default), (b) the cargo relink
   long-pole is tamed (fast linker + runtime/app crate split), and (c) the
   browser reconnects to a restored Model. It degrades for Ipe.Webview, CLI /
   long-running jobs, and rich transient client-only state — the cases the
   interpreter tier later makes instant universally.

---

## 0. Framing — the invariants everything is derived from

Five invariants form the floor. A violation of any is a release blocker.

- **INV-1 (No under-invalidation).** Every memoized artifact is a *pure
  function of its complete, tracked input set*. There are no hidden inputs. A
  query that reads state outside the salsa input graph is the bug — forbidden
  by construction. Enforced structurally: the DAG crates hold **no
  `std::fs`/`std::env` capability on the query path**, so "forgot to register an
  input" is a design-time-visible error, not a silent staleness bug.
- **INV-2 (No eval hole).** The browser wire protocol carries *typed data only*
  (VNode patches + a closed set of typed SSE frame kinds). No running app ever
  loads or evaluates code arriving over its network surface.
  `data-sky-eval` stays forbidden; strict CSP (no `unsafe-eval`) is preserved
  unchanged by every reload level.
- **INV-3 (Last-good liveness).** A failing rebuild never kills the running
  binary.
- **INV-4 (Confined watch).** The watcher observes only a typed, canonicalised,
  project-root-confined path set. No symlink escape; bounded event intake.
- **INV-5 (Owned artifacts).** The emitted Rust project is ipe-owned. Disk↔salsa
  reconciliation is content-hash based; mtime is never trusted for correctness
  (only as a cargo-facing optimisation).

The Trusted Computing Base for incrementality is deliberately tiny: (a) salsa's
red-green algorithm, (b) the *completeness* of input registration, (c) the
content-hash addressing of every cache. If those three hold, staleness is
unrepresentable.

---

## Q1 — WHERE incremental compilation fits (LOCKED)

**Decision.** The pure compiler pipeline is a salsa query graph spanning
`sky_parse → sky_canon → sky_types → sky_lower → sky_ir → sky_backend_rust`,
with the durable incremental cut-point at **`sky_ir`**. The
`ipe → emitted Rust project → cargo` step is *outside* salsa. The **FFI
subsystem's generation stays a coarse content-hash cache outside the graph**;
its typed interface (`.ipei` + `kernel.json`) enters as a per-package salsa
**input**.

**Rationale (one line).** Salsa owns the pure, keystroke-frequency DAG; cargo
owns machine-code with its own incremental engine; the effectful, network-
adjacent, minutes-long FFI introspector must never sit on the hot loop, so it is
firewalled to an explicit user-gated cache and only its verified interface
crosses in.

### Why `sky_ir` is the cut-point

`sky_ir` is the shared consumer point: the AOT path continues
`sky_ir → sky_backend_rust`; the future interpreter tier (Q3) consumes `sky_ir`
directly. Anchoring durable incrementality here means hot-reload and production
build share one incremental front-end, and L1/L2 later become a *backend swap*,
not a rewrite.

### FFI participation — the two-tier split

| Concern | Placement | Trigger |
|---|---|---|
| Introspection (`PkgInfo` JSON → `.ipei` + `kernel.json` + `_bindings.rs`) | **Outside salsa** — coarse content-addressed cache under `~/.cache/ipe/ffi/rust/` | Explicit `ipe add` / `ipe install` only; **never** an ordinary source save |
| Typed interface (`.ipei`, `kernel.json`) | **Inside salsa** — per-package `ffi_package_interface(PackageId)` input | Set when a hash-verified interface change is observed |
| `_bindings.rs` | Just an emitted source file | Flows through emit→cargo like any generated `.rs` |

**Why introspection stays out of the automatic recompute graph** (security-
first): it invokes crate introspection over registry-pulled code, may touch the
network, is toolchain-dependent, and at Stripe-SDK scale (76k symbols) takes
15+ min. Memoising an impure, effectful computation inside salsa is a soundness
liability (salsa cannot know when the crate/toolchain changed underneath it) and
would make a file-save able to trigger untrusted-code execution. It is therefore
an explicit, user-gated step behind the `ipe add`/`install` command — matching
the ffi-port-spec fail-closed / RCE-sandbox posture.

**FFI cache key = the full provenance tuple (this IS the address):**

```
key = H( crate_name,
         exact_locked_version,
         source_checksum_from_lockfile,
         resolved_feature_set,
         rustc/toolchain_fingerprint,   # explicit, separate axis — NOT folded into introspector version
         introspector_tool_version )
```

Content-addressing makes under-invalidation *unrepresentable*: a `cargo update`,
a feature-flag flip, or a `rustup` bump yields a *different address*, so a stale
entry is an unreachable miss, not a wrong hit (MAKE-INVALID-STATES-
UNREPRESENTABLE). The `rustc`/toolchain axis is separate because `Cargo.lock`
does not encode the compiler version, yet rustdoc-derived output depends on it.

**Defence in depth — `VerifiedFfiInterface`** (PARSE, DON'T VALIDATE). A usable
FFI interface value is *only constructible* through a provenance-hash-verified
constructor. There is no code path that feeds an `.ipei` into `kernel_types()`
without hash verification. Content-addressing makes a stale entry unreachable;
the typed constructor makes a stale interface unusable even if somehow reached —
two independent guarantees at near-zero cost.

**Watch-time miss policy (refuse, don't guess).** During `ipe watch`, a
content-address miss on the FFI cache **hard-refuses** with
`FFI cache stale for crate X — run 'ipe install'`. It never silently
regenerates, because regeneration is the effectful minutes-long step we
deliberately keep off the hot loop.

**FFI is currently PARKED** (compiler completion precedes the FFI consumer /
M4 kernel registry). The salsa design **reserves the `ffi_package_interface`
input seam now** (cheap forward-compat) without building it. Minimum forward
contract: `kernel_types()` unions the static kernel table with a set of
per-package interface inputs; adding FFI later flips inputs on, with no
query-graph redesign.

---

## Q1b — The salsa query graph (LOCKED)

### Inputs (the only things the driver may `set_input`; queries never read the world)

| Input | Shape | Durability | Notes |
|---|---|---|---|
| `source_text(FileId)` | `Arc<str>`, one per `.ipe` file | low | Byte-equal re-save is a no-op at the boundary |
| `file_set()` | `Arc<BTreeSet<FileId>>` | low | In-scope source set from the scope walk |
| `project_config()` | typed `ProjectConfig` (parsed `sky.toml`), **field-granular** | high | Editing `[log].level` must not invalidate codegen; editing `entry` must |
| `codegen_flags()` | typed (`IPE_DCE`, `IPE_SOLVER_BUDGET`, budget factor, env-prefix) | high | Build-affecting env parsed to typed inputs — closes the hidden-env hole |
| `ffi_package_interface(PackageId)` | `Arc<VerifiedFfiInterface>` | high | Per-package (Q1); reserved seam while FFI is parked |
| `compiler_revision()` | content hash seeded from the `ipe` binary's own build hash | high | Bumps invalidate everything on a compiler upgrade |
| `toolchain_fingerprint()` | rustc/toolchain identity | high | Affects emit + FFI. Source is not a watched file → re-derived per revision + **hard-refuse on mid-session change** (see durability rule) |

**Durability rule (a subtle under-invalidation trap).** High durability lets
salsa skip revalidation cheaply — but that is *only sound* if the driver
genuinely observes the input's source and calls `set_input` on change. **A
high-durability input MUST have its source in the watch allowlist.** Getting
this backwards silently under-invalidates.

**Toolchain-fingerprint observability (closing the rule's own loophole — GAP-2).**
`toolchain_fingerprint()` is high-durability but its source — the active rustc /
rustup state (`rustc -vV`, plus any `rust-toolchain.toml` / `rustup override`) —
is **not a watched `.ipe` file**, so a mid-session `rustup update` or directory
override would leave every high-durability emit memo (and the FFI cache, H1)
validated against the *old* fingerprint: a stale build that compiles. A
high-durability input whose source cannot be cheaply watched must not silently
persist across a change. `ipe watch` therefore treats the toolchain as a
**refuse-don't-guess** input, identical in spirit to the FFI-cache-miss policy:

- **Re-derive the fingerprint at the start of every revision** (a `rustc -vV` +
  rustup-state read is milliseconds; it runs in the driver, never on a salsa
  query path, preserving INV-1).
- **If it differs from the session's pinned fingerprint, HARD-REFUSE** the
  rebuild with `toolchain changed (was <A>, now <B>) — restart 'ipe watch'` and
  keep the last-good binary alive (INV-3). The session never silently re-uses a
  cache keyed on the old fingerprint, and never silently rebuilds the world
  mid-session under a compiler the user swapped underneath it.

Re-deriving-and-refusing is chosen over set-input-and-recompute deliberately: a
toolchain swap invalidates *everything* (emit + the entire FFI cache), so a full
restart is both the honest cost and the simplest sound state — there is no
partial-recompute worth preserving, and refusing keeps the "restart clears all
doubt" posture. (`compiler_revision()` — the `ipe` binary's own build hash —
cannot change under a live watch because the running watcher *is* that binary;
it is re-read only at process start.)

**`project_config` field-granularity mechanism (the projection firewall).** A
single `ProjectConfig` struct input is *sound but over-invalidating*: any
`sky.toml` edit bumps the one input, so a naive DAG would re-run everything that
read it — editing `[log].level` (a pure runtime concern) would needlessly
re-emit codegen. The doc's efficiency claim ("editing `[log].level` must not
invalidate codegen; editing `entry` must") is realised by **interposing thin
per-field derived queries between the single struct input and its consumers**:
`config_entry() = project_config().entry`, `config_log_level() =
project_config().log.level`, one projection per build-relevant field. Consumers
depend on the **specific field query they use**, never on `project_config()`
directly. When `[log].level` changes, the input bumps and *every* projection
re-validates, but only `config_log_level()`'s **output value changes**;
`config_entry()` returns a byte-identical value, so salsa back-dates it (red-
green: an unchanged derived output does not propagate) and every codegen query
downstream of `config_entry()` early-cuts. This gets field-granular invalidation
with a single input and no bespoke diffing — the projection queries ARE the
field-granularity mechanism. (This is the same firewall shape as
`module_interface`: interpose a projection whose output changes less often than
its input, and let salsa's value-equality early-cut do the rest.)

### Derived queries (per-module granularity + firewalls)

| Query | Depends on | Firewall / role |
|---|---|---|
| `parse(FileId)` | `source_text(self)` only | Edit in A never re-parses B |
| `imports(FileId)` | `parse(self)` | Dependents key on this, not full AST |
| **`resolve_imports(ModuleId)`** | `imports(self)` + `file_set()` | **MODULE-RESOLUTION EDGE** — maps each import *name* to a resolved `ModuleId`/`FileId` (or `Unresolved`) as a pure function of the in-scope file set; this is the query that makes add/delete/rename/shadow of a module re-canonicalise its importers. See resolution obligation below. |
| `program_modules()` | `file_set()` | The `ModuleId` set the whole-program steps quantify over — makes the "ALL" in `lower(ALL)` an *explicit* `file_set()`-derived dependency, not an implicit one |
| **`module_interface(ModuleId)`** | `parse(self)` | **PRIMARY FIREWALL** — see completeness obligation below |
| `canonicalize(ModuleId)` | `parse(self)` + `resolve_imports(self)` + `module_interface(deps)` | `deps` is exactly the resolved set from `resolve_imports(self)`; importers don't re-canon on a dep *body* change, but DO re-canon when resolution changes (add/delete/rename/shadow) |
| `kernel_types()` | static kernel table + `ffi_package_interface(*)` | Early-cuts on unchanged FFI packages |
| `typecheck(ModuleId)` | `canonicalize(self)`, `resolve_imports(self)`, `module_interface(deps)`, `kernel_types()` | HM + exhaustiveness + region types, per module; carries the resolution edge so a newly-satisfied / broken import re-typechecks the importer |
| `lower(ModuleId)` | `typecheck(self)` | Per-module IR (mirrors `.ipe/lowered/`) |
| **`program_metadata()`** | `program_modules()` + `lower(ALL of program_modules())` (full lowered-IR set — **never** firewalled behind interfaces) | Whole-program DCE reachability + monomorphisation table + `_fieldIndex` orderings. The explicit `program_modules()` dep means an *added* module cannot be silently excluded from DCE/mono |
| `program_ir_module(ModuleId)` | `lower(self)` + `program_metadata()` | Post-DCE/mono IR; early-cuts when metadata byte-identical |
| `emit_rust_file(RustFileId)` | `program_ir_module(owner)` + `program_metadata()` | Text of one `.rs`; body edit → only that file changes |
| `emit_manifest()` | `emit_rust_file(ALL)` + `program_modules()` + `project_config()` | `Map<PathBuf,(ContentHash,Arc<str>)>` — the *complete* intended project; the `program_modules()` dep makes the emitted `mod` list a pure function of `file_set()` so a prune (deleted module) propagates; top-level driver request |

**`resolve_imports` module-resolution obligation (the missing-edge under-invalidation
gate).** Import statements name *modules*, not files. The map from an import name
to a concrete `ModuleId`/`FileId` is a **pure function of `file_set()`** (which
files are in scope) plus the importer's own `imports(self)` — and it is a *derived
query*, so salsa tracks the `file_set()` read. This closes a whole family of
stale-build bugs that a `parse`/`module_interface` DAG alone misses, because those
queries key only on file *contents*, never on the *set* of files:

- **Add** a `.ipe` that satisfies a previously-`Unresolved` import → `resolve_imports`
  changes from `Unresolved` to a `ModuleId` → `canonicalize`/`typecheck` of the
  importer re-run (previously: importer stayed red-or-stale because no query it
  depended on observed the new file).
- **Delete** an imported module → resolution flips to `Unresolved` → importer
  re-typechecks and correctly goes red (previously: importer compiled against a
  vanished module).
- **Rename** a module, or **add a new file that shadows** an existing module name →
  resolution retargets → every importer re-canonicalises against the new target.

Because `resolve_imports` reads `file_set()`, adding/removing a file bumps the
`file_set()` input and salsa re-validates every `resolve_imports` that read it —
so no importer can be silently skipped. Resolution is **refuse-don't-guess**: an
ambiguous name (two files claim one module id) resolves to a typed `Ambiguous`
value that `canonicalize` turns into a hard error, never a silent pick. The value
is a closed enum (`Resolved(ModuleId) | Unresolved | Ambiguous`) — MAKE-INVALID-
STATES-UNREPRESENTABLE — so "resolved to nothing" and "resolved to two things"
are distinct, handled states, not a `None` that reads as "no imports".

**`module_interface` completeness obligation (release gate, not "signatures
only").** Because Ipê performs **type-directed lowering** — codegen of module A
depends on the *resolved types* in module B, not merely B's names — the interface
summary MUST be a **sound over-approximation of every cross-module observable
the monomorphiser / generics emitter can see**: exported types, constructor
arities, **full resolved value signatures**, fixity, re-exports, and
parametric-record-alias shapes. Counter-example that a name-level interface
would miss: flip B's export from `Html msg` to `String` — A's names resolve
identically, but A's emitted Rust must change. **Rule: when in doubt, include it
in the hash.** An interface that omits one type-directed-lowering-visible fact is
a stale-build (under-invalidation) bug that name-level resolution cannot catch.
This obligation is a foregrounded release gate.

**Global DCE/mono — the over-vs-under-invalidation resolution.** The
whole-program step is split: a small `program_metadata()` depends on
`program_modules()` (the `file_set()`-derived `ModuleId` set) and the *full*
lowered-IR set over that set, and **re-runs every build** (it must — a body edit
that promotes a dead function to live has to re-run DCE; firewalling metadata
behind interfaces would under-invalidate). The explicit `program_modules()`
dependency is load-bearing: without it, an added module could lower fine yet be
absent from the reachability/mono quantification, silently excluded from the
program (an under-invalidation of the *set*, not the *contents*). Its output is
usually byte-identical, so downstream `program_ir_module` / `emit_rust_file`
early-cut and only the edited module re-emits. Soundness beats the efficiency
temptation to firewall metadata.

> **OPEN DECISION 1 — call-graph-shape firewall for `program_metadata`.**
> A proposed optimisation keys reachability on a `call_graph_shape` +
> `call_site_type_args` summary so pure body edits (no changed call edges, no
> changed call-site type-args) early-cut `program_metadata` too — valuable at
> Stripe-SDK scale (76k symbols) where "re-run every build" is non-trivial. The
> risk: the shape summary is *itself* an under-invalidation surface (omit one
> edge that flips reachability → stale code compiled). **Recommendation:** ship
> the conservative "re-run every build" as the sound v1 floor; treat the
> call-graph-shape firewall as a separately-audited **post-v1** escalation.
> User to confirm whether v1 must include the optimisation or may defer it.
>
> **LOCKED (user, 2026-07-02): DEFER.** v1 ships the conservative "re-run
> whole-program reachability every build" floor (sound by construction). The
> call-graph-shape firewall is a separately-audited post-v1 escalation, not a
> v1 requirement.

### Making under-invalidation unrepresentable — the emit→cargo bridge

Salsa cannot see into cargo. The bridge is a three-part protocol so the on-disk
project is a **pure function of `emit_manifest()`**, never an accretion:

1. **Deterministic emit.** `emit_rust_file` is a pure function of salsa inputs:
   fields sorted by `_fieldIndex`, sorted mono table, stable ordering
   everywhere, no wall-clock / no map-iteration nondeterminism. Byte-identical
   inputs ⇒ byte-identical `.rs`. This is simultaneously a soundness property and
   what stops cargo from rebuilding spuriously.
2. **Content-gated, atomic write.** Compare bytes to disk; write (and thus bump
   mtime) **only if content differs**. Cargo keys incrementality on mtime, so
   rewriting identical bytes would force a needless Rust rebuild. Writes are
   atomic (tmp + rename) so cargo never sees a half-written file.
3. **Manifest-driven prune.** `emit_manifest` is authoritative. Anything on disk
   under the emit root not in the manifest is **deleted**. This makes an
   orphan/stale `.rs` (a deleted module, a DCE'd binding) unrepresentable — the
   classic "removed source but old codegen lingers and still compiles" bug is
   structurally impossible (INV-5).

**Transactional emission.** Compute the full `emit_manifest` in memory; reconcile
to disk only if **all** Ipê-side stages (parse→emit) succeeded. On a Ipê-side
failure, disk is left at the last consistent state and the running binary is
untouched. If Ipê succeeds but cargo fails, sources are consistent (they
type-checked) and the last-good binary keeps serving.

### Cross-session persistence

> **OPEN DECISION 2 — persist salsa memo state to disk?**
> - **Option A (conservative):** in-memory salsa db only, per-`ipe watch`
>   session; cold start re-derives from source (correct by construction). A
>   missed-input bug is cleared by any restart.
> - **Option B (reference parity / cold-start speed):** persist per-module
>   lowered IR to `.ipe/lowered/` (mirrors the Haskell `sky watch`), each
>   entry **content-addressed by its complete input set**, and gate the entire
>   cache directory behind a **version-epoch prefix** so a compiler upgrade
>   abandons the whole prior directory wholesale (never trusts per-entry keys
>   across compiler versions). Entries are advisory: hash miss → recompute,
>   corrupt entry → discard.
>
>   **Cache-key completeness (ties GAP-1 — persisted-cache is where an
>   incomplete key survives a restart, so the address must be whole-project, not
>   leaf-body).** The content-address of a persisted artifact MUST include, in
>   addition to the artifact's own byte content:
>   - the **`file_set()`-derived module identity** (the resolved `ModuleId` set +
>     each module's resolved import targets from `resolve_imports`) — otherwise a
>     module *deletion* or *rename* would leave a stale on-disk entry that a
>     cold start rehydrates, re-introducing the vanished module (the exact
>     set-vs-contents under-invalidation GAP-1 closes in-memory, now also on
>     disk);
>   - the **emitted `mod` list** (the set of `.rs` files the manifest declares) —
>     so a prune propagates to the crate root, not just to leaf `.rs` bodies;
>   - the **app-crate `Cargo.toml`** content hash — so an `ipe add` dependency
>     change (which alters `[dependencies]` but need not touch any `.ipe` body)
>     invalidates the cached crate rather than reusing a build against the old
>     dependency set.
>   In short: the persisted address is over the *whole intended project*
>   (`emit_manifest`'s complete key surface), never merely the leaf source
>   bytes — the same completeness obligation as `module_interface`, applied to
>   the on-disk cache.
>
> **Recommendation:** Option B with the version-epoch directory gate — it gives
> cold-start speed and reference parity while confining the "durable stale entry"
> risk to a whole-directory wipe on upgrade. Adopt Option A only if the team
> wants the strictest "restart clears all doubt" posture for v1. User to pick.
>
> **LOCKED (user, 2026-07-02): OPTION B.** Persist per-module lowered IR to
> `.ipe/lowered/`, content-addressed over the whole-project key surface
> (module identity + resolved import targets + emitted `mod` list + app-crate
> `Cargo.toml` hash), behind a version-epoch directory prefix (a compiler
> upgrade wipes the whole prior directory). Entries advisory: hash miss →
> recompute, corrupt → discard.

---

## Q2 — `ipe watch` (LOCKED)

**Decision.** `ipe watch` is a dev-only command that watches a confined
allowlist, debounces events into one salsa revision, recomputes the minimal
subgraph, reconciles emitted Rust to disk, runs a timeout-bounded incremental
cargo build, and on green performs a readiness-gated restart — **keeping the
last-good binary alive on any failing rebuild**.

**Rationale (one line).** A sound, confined, last-good-preserving loop that
composes salsa's minimal recompute with cargo's incremental engine and the live
runtime's existing SSE reconnect.

> **Note:** `ipe` today stops at emit and never invokes cargo (only an
> `IPE_E2E`-gated test does). `ipe watch` (and an integrated `ipe build`) must
> add the cargo-build + run orchestration that does not exist yet. `ipe watch`
> composes an integrated `ipe build` cargo step rather than owning a private
> divergent driver.

### Watch scope (strict allowlist, INV-4)

**Included:**
- `sky.toml`
- entry-point's directory, recursive `.ipe` walk
- `tests/` if present
- `~/.cache/ipe/ffi/rust/*.ipei` **+ `kernel.json`** — *interface files only*, read-only

**Excluded** (generated dirs — watching them would self-trigger a rebuild loop):
`sky-out/` and the emit root, the rest of the project-local `.ipe/` dir
(`.ipe/lowered/`, `.ipe/source.hash`), `target/`, `.git/`, `node_modules/`,
`dist/`.

**Why watch the FFI interface files (H13).** An `ipe add` run in a *second
terminal* changes `.ipei`/`kernel.json` on disk; a running `ipe watch` that
didn't observe them would build the whole session against a stale FFI interface —
a soundness bug. Watch only the *interface* files (not the whole
`~/.cache/ipe/ffi/` tree, and since introspection never runs during a watch rebuild
there is no self-trigger). A detected change is accepted **only** through the
hash-verified `VerifiedFfiInterface` constructor — watching adds observation, not
trust.

**Watch directories, not inodes** — editors save via tmp-write + rename, so
inode-level watches miss the new file. **Canonicalise every discovered path and
confine to the project root**; a `WatchedPath` is only constructible from an
in-root canonical path, so symlinks resolving outside the root are refused
(path-traversal foreclosure). Bound watched-file count + total bytes (DoS guard).

### Debounce + coalescing

Watch the directory; coalesce FS events over a **quiescence window (~80–120 ms)**,
resetting on each event, with a **hard latency cap (~400–500 ms)** so a continuous
trickle still eventually fires. Dedup by canonical path; drop excluded-dir events
at the source; bound the event queue (INV-4). This absorbs editor save-storms,
format-on-save, and rename pairs. On a partially-written file, the atomic
rename semantics + byte-equal input drop mean a syntactically-broken transient is
either never observed or handled as a normal red build (INV-3).

### Minimal-recompute path

On a settled batch: for each changed `.ipe`, `set_source_text` to the new content
(byte-equal changes are dropped at the input boundary — mtime-only touches don't
propagate); `set_project_config` on a `sky.toml` change. Salsa's red-green walk +
the `module_interface` firewall recompute only the dirtied subgraph — body-only
edits stop at the firewall and only the edited module re-lowers and re-emits.
Then request `emit_manifest()`, reconcile files (content-gated write +
delete-orphans), and invoke one cargo build against a **warm shared target dir +
sccache**. **Single-flight:** if a new change lands mid-build, cancel the salsa
computation (rust-analyzer-style cancellation on new input) and coalesce to the
latest state; never run overlapping cargo builds.

Target: warm salsa recompute for a single-body edit well under 100 ms; total
wall-clock dominated by cargo (see Q4), low single-digit seconds — matching or
beating the Haskell 1–3 s warm rebuild.

### Build-error policy — the explicit state machine (INV-3)

The running process is modelled as a state machine so invalid states (e.g. "old
killed but new failed to bind port") are unrepresentable. **On the v1 fixed port,
two processes cannot hold the port at once** (Q3), so the honest order is
**stop-old-before-spawn-new**, with an explicit, bounded down-window — NOT the
make-before-break shape a shared port would allow. The last-good binary artifact
stays on disk throughout, so a post-kill readiness failure recovers by
**respawning last-good** (this is what actually makes H16 hold — the old *process*
is gone, but the old *artifact* is not):

```
                 ┌────────────────────────────────────────────────────────┐
                 ▼                                                          │
        ┌──────────────┐   red build      ┌────────────────────┐           │
        │ RunningGood  │─────────────────▶│ RebuildFailed      │           │
        │ (last-good   │◀─────────────────│ (still serving old) │           │
        │  proc alive) │   next green     └────────────────────┘           │
        └──────┬───────┘                                                    │
               │ green build w/ changed binary hash                        │
               ▼                                                            │
        ┌───────────────────────┐                                          │
        │ StopOld               │  SIGTERM old → await port release         │
        │ (graceful, bounded)   │  (bounded grace → SIGKILL)                │
        └──────┬────────────────┘                                          │
               │ port released                                             │
               ▼                                                            │
        ┌───────────────────────┐   ◀── DOWN-WINDOW starts here            │
        │ SpawnNew              │  bind port, run init                      │
        │ (no old proc alive)   │                                          │
        └──────┬────────────────┘                                          │
               │ await /_sky/readyz                                        │
        ┌──────┴───────────────────────────┐                              │
        │ readiness OK                       │ readiness FAIL / timeout    │
        ▼                                    ▼                             │
  ┌──────────────┐              ┌──────────────────────────┐              │
  │ RunningGood  │  DOWN-WINDOW │ RespawnLastGood          │              │
  │ (new proc)   │  ends here   │ (re-exec last-good        │──────────────┘
  └──────────────┘              │  artifact from disk,      │  readiness OK
                                │  report new binary broken)│  → RunningGood
                                └──────────────────────────┘  (last-good)
```

**Down-window is explicit and bounded.** Between `StopOld` completing (port
released) and `SpawnNew` passing readiness, the port is unbound and the browser's
`EventSource` sees a closed connection — this is the sub-second reconnect gap the
"Reconnect honesty" note below quantifies. It is a *deliberate* v1 cost of the
fixed-port floor, not a bug. `SO_REUSEPORT` / ephemeral-port-behind-proxy (Q3)
is the mechanism that collapses this window to zero by re-enabling make-before-
break; it is optional polish for v1 and, if adopted, would restore the
old-alive-while-new-spawns shape.

- **Any red build** (parse / canon / type / lower / emit error **or** cargo
  build failure) → the previously-running binary **stays alive**; the diagnostic
  is printed; the live process is untouched. The next green build kills +
  respawns.
- **Distinguish "Ipê lowering succeeded" from "cargo build succeeded"** and swap
  the binary **only on the latter** — the analog of the Haskell "printed success
  before `go build` ran" bug.
- **`LastGoodBinary` is only constructible** for a build+process that passed its
  readiness probe (`/_sky/readyz` for Ipe.Live; alive + optional health for
  CLI). A failed build never produces one (parse-don't-validate at the process
  boundary). It captures **the on-disk artifact path + its content hash** (not
  merely a handle to the live process), so after `StopOld` kills the old process
  the artifact is still re-execable — that is precisely what `RespawnLastGood`
  re-launches when a new binary fails readiness. A green build producing a
  **byte-identical** binary (comment/test-only change) → no restart, no
  observable churn.
- **cargo incremental-cache corruption** (a hard cargo error inconsistent with
  the source delta) → clean-rebuild the emitted crate, **still keeping the
  last-good binary alive** until the clean build goes green.

### Restart / reconnect

On green build with a changed binary hash, the **fixed-port v1 order is
stop-before-spawn**: **SIGTERM old → await port release (bounded grace → SIGKILL)
→ spawn new → await readiness**. If the new binary **fails readiness** (crash on
boot, port-bind race, wedged init), the watcher **re-execs the last-good artifact
from disk** (`RespawnLastGood`) and reports the new binary as broken — so the user
is never stranded even though the old *process* was already killed (H16 holds via
the on-disk artifact, not a live spare process). The browser's existing Ipe.Live
SSE machinery auto-reconnects (immediate `hello` handshake, 8 s hello watchdog,
35 s heartbeat, exponential backoff, `__skyEventQueue` replay). `init` does not
re-run (per-session), so with a persistent store the user lands on their restored
Model.

> **Reconnect honesty (fixed-port caveat).** True zero-gap make-before-break is
> **not achievable on a fixed port** without `SO_REUSEPORT` or a front proxy —
> two processes cannot bind one port. The v1 floor is therefore the
> stop→spawn→readiness order above with an **explicit, bounded down-window** (the
> port is unbound from port-release until the new process passes readiness),
> accepting a sub-second reconnect. On a clean SIGTERM the TCP connection closes
> and `EventSource` fires `onerror` and reconnects at ~500 ms base backoff; the
> 8 s hello watchdog only bites on a *silent-but-open* wedged proxy, which a
> normal restart is not. `SO_REUSEPORT` / ephemeral-port-behind-proxy collapses
> the down-window to zero and restores the old-alive-while-new-spawns shape; it is
> promoted from "impossible" to **optional v1 polish** — the diagram's fixed-port
> path is the required floor, the make-before-break path is the polish that
> mechanism unlocks. See Q3/Q4 for the `event: reload` frame.

### Timeout / hang bounding (AGENTS.md §3)

Every rebuild+cargo cycle is **timeout-bounded** with a hard ceiling; the child
server has a readiness/heartbeat max-wait so a wedged child can't poison the
parent; a wedged cargo is killed on ceiling breach; watch surfaces "this rebuild
exceeded budget" rather than hanging silently. The child process is killed when
the watcher exits. Prefer event-driven monitoring over polling wait-loops.

### Security posture

- Reads only allowlisted `.ipe`/`.ipei`/`.toml`; **never executes** watched
  files. The emitted project is built by cargo (no eval).
- **No network-facing control port.** Any future hot-reload signalling channel
  (Q3) is loopback-only + token-authed, and **absent** (not merely disabled)
  under the production gate — wired only when `productionFromEnv() == dev`.
- **watch must NOT auto-run the FFI inspector** as a side effect of a file save
  (ties Q1 — introspection is `ipe add`/`install` only; net-denied,
  no-untrusted-code-exec posture from ffi-port-spec §A extends to watch).
- The spawned binary runs at the user's own privilege — identical attack surface
  to running the app manually. `ipe watch` is not a new attack surface.
- **Emit-time injection foreclosure (H10).** A source-derived string
  concatenated unescaped into emitted Rust is a build-time code-injection vector,
  amplified by watch frequency. Emit **only through typed IR** via an
  escaped-literal emitter; **never** string-concat user data into `.rs`
  (MAKE-INVALID-STATES-UNREPRESENTABLE; security is top of the stack).

### First-run vs warm-run UX

Cold `ipe watch` pays a full cargo build of the emitted project + runtime
(potentially minutes; competes for the shared ~15 GB cargo target — see disk
hygiene). The user sees an explicit "cold build (first run)" indicator distinct
from warm 1–3 s rebuilds, so "watch is slow" is never misattributed to the salsa
layer. **ENOSPC is a first-class watch failure mode** (AGENTS.md §6): a
near-full-disk build dies at the file-copy/link step and masquerades as a codegen
regression; watch checks free space before a cold build and surfaces ENOSPC
distinctly.

---

## Q3 — Hot-reload spectrum (LOCKED: v1 = L0+; L1/L2 = interpreter tier)

**Decision.** Ship **L0+** for v1. L1 and L2 are categorically unreachable on the
Rust-AOT path and are deferred to the future single-`ipe`-binary **interpreter
tier**, where a hot-swap is *typed IR evaluated server-side*, wire protocol
unchanged.

**Rationale (one line).** L0+ is the only fully sound, eval-hole-free reload
level under Rust-AOT (no stable dylib ABI), and its session-restore continuity
already crosses the perceptual threshold; in-process code swap would require
either an unsound dylib (UB from mismatched layouts) or an as-yet-unbuilt
interpreter.

### The spectrum weighed against the Rust-AOT constraint

| Level | What it is | AOT feasibility | Verdict |
|---|---|---|---|
| **L0** | rebuild + process restart + SSE auto-reconnect + persisted-session restore | Native (just rebuild the binary) | **v1** |
| **L0+** | L0 + proactive `event: reload` frame + readiness-gated handoff + sqlite dev-store default | Native | **v1 (recommended packaging)** |
| **L1** | view-only hot-swap without restart, preserve Model in-process | **Impossible on AOT** — no stable dylib ABI; dlopen of a locally-built cdylib is ABI-fragile across rustc versions + monomorphisations, and the Model struct's *shape* changes between builds | Interpreter tier |
| **L2** | state-preserving TEA hotpatch (swap `update`+`view`, migrate Model) | Impossible on AOT (L1 blockers + Model migration) | Interpreter tier |

**We do NOT recommend AOT in-process code hot-swap at any level.** A dlopen-based
swap is a soundness minefield (ABI instability, reinterpreting an old Model under
new code) and adds a dynamic-code-loading surface — it violates
security > correctness > soundness.

### L0+ packaging (all sound, all AOT-native)

- **Proactive `event: reload` SSE frame** on graceful shutdown so the browser
  reconnects fast instead of waiting on watchdogs. It is a **signal only** — a
  token in a *closed set* of typed frame kinds (hello / heartbeat / patch /
  reload); an unknown/malformed frame is **dropped, not interpreted**
  (parse-don't-validate on the wire, INV-2). The emitter is **absent** under the
  production gate. *(Classified as minor polish, not load-bearing — a clean
  SIGTERM already triggers `EventSource` reconnect; the frame mainly avoids the
  wedged-proxy watchdog path.)*
- **Readiness-gated handoff** (Q2) — the old binary serves until the new one
  passes `/_sky/readyz`.
- **sqlite dev session-store default** — see below; this is the load-bearing
  L0+ element.

### The load-bearing L0 hinge — session store (H: `MemoryStore` dies on restart)

`init` is per-session-not-per-reload, but the pinned dev default store is
`memory`, which is **lost on restart**: a watch-triggered restart wipes all
sessions, `init` re-runs fresh, and the user loses form contents / current page /
scroll on every save — destroying the very illusion L0 rests on. **Decision:
`ipe watch` defaults the Ipe.Live dev session store to `sqlite`** (file-backed,
survives restart), distinct from the app's production `[live].store`; if the app
explicitly configures `memory`, watch warns. This single change converts L0 from
"fast reload that resets my screen" into "I edited, the screen updated where I
left it" — i.e. it *is* the perceptual difference between L0 and L1 for the
common case.

**Model restore is total AND schema-gated (soundness — GAP-4).** If an edit
changed the Model *type*, the serialized blob from the old process is
schema-mismatched. The trap is that **deserialize success is not semantic
correctness**: a permissive format (serde-JSON with `#[serde(default)]` fields, or
two structurally-similar Model types sharing a byte layout) can deserialize an
*old, semantically-wrong* blob into the *new* Model type without error — passing
a "did it deserialize?" gate with nonsense. This bites even L0 (a plain restart
after a Model change), not just the L2 interpreter path. Two independent
guarantees close it:

1. **Schema tag checked BEFORE deserialize.** Every persisted dev-store blob
   carries a header `ModelSchemaTag = H(compiler_revision, structural_hash(Model
   type))` — the structural hash covers field names, field order (`_fieldIndex`),
   and each field's resolved type, recursively. On restore the watcher compares
   the stored tag to the *current* build's tag **first**; a mismatch **rejects the
   blob outright** (drop session → fresh `init`) and the raw bytes are never
   handed to the deserializer. So a changed Model type can never even *attempt* a
   cross-type decode — the wrong-shape blob is unreachable, not merely
   caught-if-it-throws.
2. **Deserialize failure is still total.** For a matching tag, a decode error
   (corruption, truncation) is *also* handled as drop session → fresh `init`, so a
   damaged same-schema blob can never panic the new process.

A `RestoredModel` is only constructible when **the schema tag matches AND the
blob deserialized into the current type**; otherwise `init`. Tag-check-first makes
the common "I changed the Model" case a clean, cheap reject; the deserialize gate
is defence-in-depth for same-schema corruption.

**Pinned Rust dev-store serialization format.** The old Go runtime used `gob`,
which is Go-only and has no place in the Rust backend; the Rust dev-store format
is pinned here as **length-prefixed `[ ModelSchemaTag header ][ bincode body ]`**:
a fixed, self-describing header (magic + format-version + the 32-byte
`ModelSchemaTag`) followed by a `bincode`-encoded Model body. `bincode` is chosen
over serde-JSON precisely because it is **non-self-describing and non-permissive**
— it will not silently fill defaulted/missing fields, so it cannot quietly accept
a shape it wasn't written for; combined with the mandatory tag pre-check, a
mismatched blob is rejected at the header before the body is ever parsed. The
header's format-version lets a future format change reject old blobs wholesale
(same version-epoch discipline as the on-disk lowered-IR cache).

### The interpreter tier as the sound L1/L2 enabler (later)

The single `ipe` binary is planned to host compiler + interpreter. In interpreter
mode `ipe` is the long-lived host; the app runs as evaluated `sky_ir`, and the
Model lives as an interpreter value (not a compiled Rust struct). This changes
the economics:

- **L1 (view hot-swap):** salsa recomputes the changed module's typed IR; the
  interpreter atomically swaps the evaluator's function-table entry for `view`
  (and any changed pure fn). The next SSE render tick evaluates the new `view`
  IR against the *unchanged* Model; DOM patches flow over the existing SSE
  connection. **No restart, no dylib, no eval-of-text.**
- **L2 (update+view swap + Model migration):** swap `update` too. Migration
  exploits that the compiler knows *both* old and new Model types (salsa still
  holds the previous typed IR): synthesise a **structural migration** — fields
  present in both with compatible types carry over; new fields take
  `init`-derived defaults; removed fields drop. A `MigratedModel` is **only
  constructible when every field maps soundly**; otherwise `MigrationInfeasible`
  → clean fall-back to L0 fresh `init` (never a blind cast). Additionally,
  because a structurally-sound migration can still be *semantically* wrong (a
  field keeps its type but changes meaning), **L2 auto-migration is opt-in and
  always offers an explicit "reset session" escape hatch**.

**Security at L1/L2 (INV-2, hard).** "Push a new view over SSE" means push
**data (a VNode tree / patch), never code.** The new `view` executes
**server-side** (interpreter); only the resulting VNode diff crosses to the
browser — byte-format-identical to an ordinary SSE patch. The hot-swap API
accepts a typed `IrModule` — **never `String`/bytes-of-code**; there is no
function anywhere that takes source/JS text and executes it client-side.
`__skyReviveScripts` is NOT repurposed as a reload channel. The IR-swap control
channel is loopback-only + token-authed + **absent** in production. CSP posture
and the `data-sky-eval` ban are untouched at every level.

> **OPEN DECISION 3 — does hot-reload justify building the interpreter tier?**
> Genuine panel fork.
> - **Position A (minimalist):** the interpreter is justified by *other* roadmap
>   goals (REPL, WASM/portability); hot-reload is a beneficiary once it exists,
>   not the justification. L0+ with sqlite + crate-split relink already crosses
>   the perceptual threshold, so L1/L2's marginal gain (residual relink latency +
>   transient client-only state) does not put a second execution backend on the
>   near-critical path.
> - **Position B (moat):** state-preserving hot reload with a *type-checking
>   guarantee* (which React fast-refresh cannot offer) is a genuine DX moat and
>   belongs on the roadmap's critical path; the `sky_ir` cut-point already makes
>   the interpreter reuse the whole salsa front-end.
> Both agree: L0+ ships first; L1/L2 are interpreter-tier and eval-free. The
> fork is *priority/justification* only. **Additional gate (both positions):**
> L1/L2 via interpreter is gated on a **differential-conformance invariant
> (H12)** — interpreter output ≡ AOT output across the example sweep, as a
> release gate — so the hot-reload substrate can never "lie" (dev/prod
> divergence). User to set the roadmap priority.
>
> **LOCKED (user, 2026-07-02): POSITION A (minimalist).** The interpreter tier
> is justified by other roadmap goals (REPL, WASM/portability); interpreter-tier
> hot-reload (L1/L2) is a beneficiary once it exists, NOT on the near-critical
> path. L0+ ships first and already crosses the perceptual "immediate" threshold.
> The H12 differential-conformance gate (interpreter ≡ AOT across the sweep)
> still applies if/when L1/L2 is built.

---

## Q4 — Does `ipe watch` ALONE feel immediate? (LOCKED: YES, conditionally)

**Decision.** **YES** — `ipe watch` alone (L0+) produces the *illusion of
immediate update* for Ipe.Live web apps, making L1/L2 a later refinement rather
than a v1 requirement — **under the conditions below**. It buys the **illusion of
continuity** (you never lose your place), **not** literal sub-100 ms
instantaneity.

**Rationale (one line).** Raw latency is ~1–few seconds bounded by cargo
codegen+link (not the salsa front-end); what reads as "immediate" is the
session-store restore + per-session `init` landing the user back on their exact
Model after a brief reconnect flash — continuity is the emotionally salient
component of immediacy.

### Latency budget (single body-edit, Ipe.Live)

| Term | Estimate | Notes |
|---|---|---|
| Salsa recompute (edited module, stops at firewall) | tens of ms | Not the gate |
| Write-if-different reconcile | single-digit ms | Not the gate |
| **cargo incremental build + LINK** | **~1–several s** | **DOMINANT — outside salsa; the real gate** |
| Readiness-gated restart | ms–~1 s | runtime init, store open |
| SSE reconnect + Model restore | sub-second | browser auto-reconnects; `init` does not re-run |

### The conditions under which watch-alone feels immediate

1. **Persistent dev session store** (sqlite default). With `memory`, restart
   wipes Model → fresh `init` → screen resets → illusion broken. **This is the
   make-or-break condition.**
2. **The cargo relink long-pole is tamed** — all sound, AOT-side, no code swap:
   - **Fast linker (mold / lld)** — the single biggest win; link dominates
     incremental Rust.
   - **Emitted-project crate split:** a stable pre-compiled **`ipe_rt`** runtime
     crate (warm in the shared cargo cache, genuinely app-independent) + a thin
     **`app`** crate holding all emitted user code (incl. DCE/mono output). A
     body edit recompiles+relinks only the small crate. This shrinks the
     *compilation unit that actually changes* — the sound AOT approximation of
     hot-reload. (Honest bound: the win is capped at keeping the runtime out of
     the recompile.)
   - Dev profile: `opt-level=0`, `debug=line-tables-only`, `incremental=true`,
     tuned `codegen-units`, no LTO; shared target dir + sccache.
3. **The browser reconnects to a restored Model** — a sub-grace-period restart
   (under the ~500 ms banner grace) avoids painting the "Reconnecting…" banner
   entirely; the proactive `event: reload` frame helps here. "No banner flash" is
   part of the immediate-illusion bar.

### Where L0 visibly breaks the illusion (the honest boundary)

The continuity illusion is TRUE for **Ipe.Live web** (and **Ipe.Tui** — redraw
survives restart fine) with a persisted session and a fast rebuild, and degrades
progressively for:

- **(a) Model-type change** forcing fresh `init` (deserialize mismatch → safe L0
  degradation, but the screen resets).
- **(b) In-flight user input** lost across restart.
- **(c) Rich transient, non-persisted client-only state** — open dropdown,
  scroll, focus (the `diff` focus-preservation is for patches, not restarts).
- **(d) Large Model** where deserialize+re-render is slow, or **rebuild > ~2–3 s**
  (flicker becomes a visible reload).
- **(e) Non-web shapes:** **Ipe.Webview** restart re-opens a native window
  (jarring); **CLI / long-running jobs** lose in-progress work.

These are the cases the interpreter-tier L1/L2 later makes instant *universally* —
a v2 argument, and the honest boundary of the L0+ "yes."

### Escape hatch already in the runtime

A large fraction of the perceived hot-reload need is met by **L0 + pub/sub
resync**: post-restart, the server can `Cmd.publish` / `Sub.subscribeTopic` fresh
state to all live sessions (AGENTS.md notes reload-as-resync is "a missing
broadcast"). This stops being sufficient exactly at cases (b)–(e) above.

### Measurement / regression gate (AGENTS.md "spotted = filed")

The illusion bar gets a concrete regression test: an e2e that measures
save→repaint under a fixed budget on a reference Ipe.Live example, so "instant
enough" is enforced, not asserted. A budget breach files a task.

---

## Consolidated hazard ledger

Soundness class = never under-invalidate; security class = no eval/injection
hole. Each hazard is foreclosed by one of the two fundamental rules.

| # | Hazard | Class | Foreclosure |
|---|---|---|---|
| H1 | FFI cache keyed too coarsely (`name@version`) reuses stale bindings after toolchain/feature/lockfile change | under-invalidation | Cache **address = full provenance tuple** incl. explicit rustc/toolchain axis; stale entry has a different address → unreachable miss |
| H2 | A query reads a hidden input (env var, `sky.toml` field, clock, FS) → stale result | under-invalidation | All external data **parsed once into typed salsa inputs**; DAG crates have no `std::fs`/`std::env` on the query path |
| H3 | `module_interface` firewall omits a type-directed-lowering-visible fact → downstream emits stale Rust | under-invalidation | Interface = **sound over-approximation of all cross-module observables incl. resolved types**; "when in doubt, include it" — release gate |
| H4 | On-disk lowered-IR entry reused when inputs changed | under-invalidation | Entry **content-addressed by complete input set**; **version-epoch directory** wiped on compiler upgrade |
| H5 | ipe trusts a hand-edited emitted `.rs` (mtime newer) | drift | Emitted project is **ipe-owned**; reconcile by content hash, regenerate on mismatch (INV-5) |
| H6 | Global DCE/mono firewalled behind interfaces → dead-fn-promoted-to-live not re-emitted | under-invalidation | `program_metadata()` depends on **full lowered-IR set**, re-runs every build; downstream early-cuts on byte-identical metadata |
| H7 | Orphan/stale emitted `.rs` lingers and still compiles | invalid state | `emit_manifest` authoritative; **delete anything not in it** |
| H8 | Cargo spurious rebuild from identical-byte rewrite | efficiency (not soundness) | Content-gated write; touch mtime only on real content change |
| H9 | "Success" reported before cargo built | false green | Separate "lowering succeeded" from "cargo succeeded"; swap binary only on the latter |
| H10 | Source-derived string concatenated unescaped into emitted Rust → build-time code injection (watch-frequency amplified) | **security (injection)** | Emit **only through typed IR** via escaped-literal emitter; never string-concat user data into `.rs` |
| H11 | Hot-reload ships code/JS to the browser for eval | **security (eval hole)** | Wire protocol is **data-only typed patches**; new `view` runs server/interpreter-side; `data-sky-eval` ban + CSP no-`unsafe-eval` preserved at every level |
| H12 | Interpreter (L1/L2) and AOT backend diverge → dev hot-reload shows behaviour the shipped binary won't reproduce | correctness (dev/prod divergence) | **Differential-conformance release gate**: interpreter output ≡ AOT output over the example sweep; interpreter honours the same Task-everywhere effect boundary — no dev-only kernels |
| H13 | Cross-process `ipe add` changes `.ipei`/`kernel.json` while `ipe watch` runs → session builds against stale FFI interface | under-invalidation | Watch the FFI **interface files read-only**; accept change only through hash-verified `VerifiedFfiInterface` |
| H14 | On-disk lowered-IR cache poisoned across a compiler upgrade by an incomplete per-entry key | under-invalidation, survives restart | **Version-epoch directory prefix** — compiler upgrade abandons the whole prior cache dir; entries advisory (miss→recompute) |
| H15 | Half-swapped live process ("old killed, new failed to bind port") | invalid state | Explicit process state machine; fixed-port order **SIGTERM-old → await-port-release → spawn-new → await-readiness**; on readiness FAIL **`RespawnLastGood` re-execs the on-disk last-good artifact**; `LastGoodBinary` captures artifact path+hash (not a live-process handle) so recovery survives the old process already being dead; down-window explicit + bounded |
| H16 | Transient typo strands user with no app | correctness/UX | Keep last-good binary alive on any failed rebuild (INV-3) |
| H17 | Unsafe dylib ABI for in-process AOT swap | soundness (UB) | Rejected; L1/L2 deferred to sound interpreter-IR swap |
| H18 | Watcher follows a symlink out of root / unbounded event intake | security (traversal / DoS) | `WatchedPath` only from in-root canonical paths; bounded coalescing queue (INV-4) |
| H19 | watch auto-runs the FFI inspector on a hostile crate as a side effect of a save | security (RCE) | Introspection is `ipe add`/`install` only; net-denied; never salsa-demand-triggered |
| H20 | cargo incremental-cache corruption returns a stale/broken binary | correctness (outside salsa) | Clean-rebuild emitted crate on inconsistent hard-error; last-good binary stays alive |
| H21 | L2 blind-casts persisted Model bytes to a changed Model type | soundness (invalid state) | Migration only via compiler-witnessed structural compatibility; else `MigrationInfeasible` → L0 fresh `init`; opt-in + reset escape hatch |
| H22 | Restore-time Model deserialize failure panics the new process | soundness | `RestoredModel` only constructible from a **schema-tag-matched AND valid-into-current-type** blob; else drop session → fresh `init` (total) |
| H23 | Dev-only reload/hot-swap endpoint exposed in production | security | Channel **absent** (not disabled) under the production gate — wired only when `productionFromEnv() == dev` |
| H24 | Permissive serialization silently deserializes an old semantically-wrong Model blob into the new type → restore passes the gate with nonsense (bites L0, not just L2) | soundness (invalid state that looks valid) | Blob carries `ModelSchemaTag = H(compiler_revision, structural_hash(Model))`; restore **rejects on tag mismatch BEFORE deserialize**; pinned non-self-describing format (**length-prefixed schema-tag header + `bincode` body**, replacing Go `gob`) cannot silently fill missing/defaulted fields |

---

## Open decisions for the user

1. **`program_metadata` call-graph-shape firewall** — v1 conservative "re-run
   every build" (recommended) vs the audited post-v1 shape-firewall optimisation
   for Stripe-scale. (Q1b)
2. **Cross-session salsa persistence** — in-memory-only (Option A) vs on-disk
   version-epoch-gated `.ipe/lowered/` (Option B, recommended). (Q1b)
3. **Interpreter-tier priority** — is state-preserving hot reload a
   near-critical-path DX moat (Position B) or a later beneficiary of an
   independently-justified interpreter (Position A)? L0+ ships first either way.
   (Q3)

---

## Relationship to existing docs

- FFI placement and the introspection sandbox posture: `ffi-port-spec.md`,
  `ffi-design.md`.
- Live runtime SSE reconnect, session stores, no-`data-sky-eval`/CSP invariant:
  `ui-live-tui-webview-spec.md`.
- Example sweep as the equivalence oracle (feeds the H12 differential-conformance
  gate): `examples-sweep-port.md`, `e2e-and-oracle-caching.md`.
- Roadmap sequencing (interpreter tier, single `ipe` binary): `roadmap.md`.
