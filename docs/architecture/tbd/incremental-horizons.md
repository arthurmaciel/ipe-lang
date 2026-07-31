# Incremental compilation — horizons

An idea catalog for what incrementality could ultimately buy Ipê, beyond the
next phases. The grounded, phased implementation plan lives in
[`incremental-phases-design.md`](incremental-phases-design.md); this document
is its breadth-first companion — the horizons the phased plan can pull from,
each tagged **NEAR-TERM** / **STRETCH** / **MOONSHOT** with an honest
feasibility read against what exists.

## The base

What every idea below builds on (decision records:
`docs/adr/0032-salsa-incremental-compilation-phase1.md`,
`docs/adr/0034-language-server-salsa-second-consumer.md`):

- **In-process**: a salsa query graph spanning the whole front end and back
  end — `parse` → `imports` → `resolve_imports` → `canonicalize` →
  `module_interface` → `typecheck` / `infer_module_scoped` / `typed_interface`
  → `lower_program` → `emit_project` — keyed on `SourceFile`/`SourceRoot`
  inputs, with the emit→cargo boundary outside the graph
  (write-if-different + delete-orphans).
- **Cross-process**: a content-addressed `EmittedProject` cache
  (`src/ipe-cli/src/cache.rs`) keyed by `compute_project_key` (length-prefixed
  hash of entry path, target, driver, every module's path + trust origin +
  full source) under an **epoch directory** derived from the `ipe` binary's
  own bytes + the active `rustc` — stale entries are structurally
  *unreachable*, not detected ("refuse, don't guess" by construction).
- **Consumers**: the LSP and `ipe watch` read the same DB; neither owns a
  second analyzer.
- **The governing hazard**: under-invalidation. A stale build that looks
  correct is a correctness violation and outranks every efficiency gain. The
  SEAL (`ipe` exit 0 ⇒ emitted Rust `cargo build`s) must hold for cached
  outputs exactly as for fresh ones.

Every idea below is judged first on whether it can *preserve* those two
invariants (no under-invalidation, no SEAL bypass), and only then on speed.

---

## 1. Merkle project keys — subtree-granular cache addresses

**What.** `compute_project_key` is one flat hash: any edit to any module
misses the whole cache. Generalize it to a Merkle DAG mirroring the import
graph — each module's key = hash(source, trust origin, keys of its imports);
the project key is the root. Cache entries can then attach at *any* node:
per-module artifacts (interfaces, metadata, emitted Rust files) survive edits
to unrelated subtrees.

**Buys.** The cross-process cache goes from all-or-nothing to proportional:
an edit invalidates exactly its import-ancestors' entries. The compiled-source
stdlib becomes a permanently-warm subtree — its keys change only with the
epoch. This is also the substrate ideas 5–7 below address into.

**Feasibility.** High. It is the cross-process mirror of the dependency
structure salsa already tracks in-process; the per-module metadata seam in the
phased plan gives it natural attachment points. The interner-relocation
problem documented in `cache.rs` (embedded `Symbol`s are process-local) still
gates *which* artifacts can be persisted per-module — string-form interfaces
and emitted Rust text are safe today; IR is not.

**Main risk.** Key incompleteness — an input that affects a module's output
but is not folded into its node key (a build flag, an injected module, a
driver choice) re-creates the exact under-invalidation hazard the flat key was
built to kill. The mitigation is structural: one key-derivation function per
node kind, property-tested for "distinct inputs ⇒ distinct keys", never
ad-hoc key assembly at call sites.

**Tag: NEAR-TERM.** The single highest-leverage generalization of what
already exists.

## 2. Content-addressed definitions (Unison-style)

**What.** Go below modules: identify every declaration by the hash of its
alpha-renamed, canonicalized body (references to other decls appear as *their*
hashes, not names). The same function is never compiled twice — across edits,
sessions, machines, or renames. Names become metadata pointing at hashes;
a rename is a metadata edit that invalidates nothing.

**Buys.** The theoretical optimum for reuse: a whitespace refactor, a decl
reorder, a module split, a rename — all cache hits. Cross-machine sharing
becomes trivial (idea 5) because addresses are machine-independent by
construction. Structural dedup falls out: two textually different but
alpha-equivalent decls share one compiled artifact.

**Feasibility.** The hashing itself is tractable — canonicalized AST +
deterministic serialization + the interner-relocation discipline (serialize
symbols as strings) already understood. What is *hard* is everything
downstream: emitted Rust names would need to derive from hashes (or a
hash→name table), colliding with the SEAL's byte-exact golden pipeline and
with generated-code readability (principle 6 — a human reads the emitted
crate). Diagnostics, diffs, and git history all speak names. Unison solved
this by owning the entire codebase format and UI; Ipê deliberately keeps
plain-text files as the source of truth.

**Main risk.** Hash-equivalence claiming more than it proves: two decls with
equal canonical hashes must have equal *observable* semantics under every
target, driver, and production flag — anything positional or context-dependent
in lowering (concrete `any`-carrier choice is per-position) breaks the "one
hash, one artifact" promise and ships a silent wrong build. Under-invalidation
in its sharpest form.

**Tag: MOONSHOT** as a codebase model. **STRETCH** as a *cache keying
discipline only* — per-decl hashes as internal cache addresses (names stay
the user-facing truth, files stay the storage) is the realistic slice, and
composes with idea 1 as its finest granularity level.

## 3. Per-declaration incrementality through the pipeline

**What.** Today's queries are per-module (`canonicalize`, `typecheck_module`,
`typed_interface`) or whole-program (`typecheck`, `lower_program`,
`emit_project`). Push tracked-query granularity to the declaration: edit one
function body in a 200-decl module and re-canon/re-check/re-lower/re-emit only
that decl (plus anything whose *interface view* of it changed).

**Buys.** Keystroke-scale latency independent of module size — the number
that dominates LSP feel on real projects. Firewalling: a body-only edit whose
decl signature is unchanged stops propagating at the interface boundary, so
downstream modules re-run nothing.

**Feasibility.** Medium. Salsa supports it (tracked structs per decl); the
`typed_interface` firewall already proves the pattern one level up. The
frontier question is where invalidation genuinely stops: canon is
decl-local given resolved imports; inference is decl-local only outside
mutually-recursive groups (the unit is the SCC of the intra-module call
graph, not the decl); emit is decl-local only if emitted Rust files are
decl-stable (ordering, shared type definitions, and the `any`-carrier
positions create cross-decl coupling in one file).

**Main risk.** The SCC subtlety: treating decls as independent when they are
mutually recursive re-infers with a stale principal type for the partner —
under-invalidation inside one module, invisible to per-module tests. The
invalidation unit must be *computed* (SCC), never assumed (decl).

**Tag: NEAR-TERM** for canon + inference-by-SCC (this is the phased plan's
natural next granularity step); **STRETCH** for per-decl emit.

## 4. Incremental type inference — constraint-solution reuse

**What.** Beyond re-running the solver on smaller units (idea 3): reuse the
*solution* itself across edits. Keep the solved constraint store; on a body
edit, retract only the constraints generated by the edited decl/SCC and
re-solve the delta, preserving principal types elsewhere.

**Buys.** Near-zero settled-edit latency even inside huge SCCs; the
`ExpectedTypes`/`expected_type_at` machinery for type-directed completion
gets a warm store to speculate against.

**Feasibility.** Low-medium. HM-style inference is order-sensitive;
retraction from a unification store means either truth maintenance
(dependency-tracking every substitution — a research-grade solver rewrite) or
level/region tricks that only work for careful constraint shapes. The honest
engineering observation: with idea 3's SCC granularity plus interface
firewalls, solves are already small — the marginal win of intra-solve reuse
is modest, and it attacks the invariant Ipê can least afford to wobble
(principal-type stability *is* what the LSP promises to never lie about).

**Main risk.** A retained substitution that the retracted constraints
justified — types that remain solved but are no longer derivable. This is
under-invalidation inside the type system itself: the checker accepts a
program it should reject. Highest soundness stakes of any idea here.

**Tag: MOONSHOT.** Recommend *not* pulling this into the phased plan;
idea 3 captures most of the value at a fraction of the risk.

## 5. Shared build caches — CAS across machines and CI

**What.** The epoch-prefixed content-addressed store, remoted: a shared
CAS (per-team, or public for stdlib/registry packages) that `ipe build`
consults before compiling. With idea 1's Merkle keys, CI warms the cache once
and every machine with the same epoch gets subtree-granular hits; with a
future package registry, packages ship *as* their cache entries — typed
interfaces + emitted Rust — and "installing" is populating the CAS.

**Buys.** Cold-start builds at clone time approach zero for unchanged deps;
CI lanes stop recompiling the world; the examples sweep (idea 6) becomes
shareable work.

**Feasibility.** Medium-high mechanically (the local store's key discipline
transfers; content addressing makes entries immutable and dedupable). The
epoch design needs one refinement: the epoch hashes *this machine's* `ipe`
binary bytes, so two machines with identical toolchains but separately-built
binaries never share addresses — a shared cache needs a portable epoch
(compiler version + source hash + rustc fingerprint) with the same
refuse-don't-guess property.

**Main risk.** Security, squarely. A shared cache entry is code that will be
compiled and run: a poisoned entry is remote code execution delivered through
the build. Fail-closed posture: entries are signed by the producer, verified
before use, and — for anything crossing a trust boundary — *reproduced*
rather than trusted (compile locally, compare against the cache, alarm on
divergence). An unauthenticated shared cache is not a degraded mode; it is
off. This is the one idea where Security (principle 1) dominates the design
before a line is written.

**Tag: STRETCH** (team/CI, authenticated). **MOONSHOT** for a public
package-registry CAS.

## 6. The SEAL as a cache entry — verification memoization

**What.** The SEAL is proven by running `cargo build` on the emitted crate;
the examples sweep proves behavior by building and running examples. Both are
pure functions of (project key, epoch) — so cache the *verdicts*: a sealed
certificate stored at the same content address as the `EmittedProject` it
certifies. A sweep re-verifies only examples whose keys changed; CI skips
green work it has already proven for this exact compiler + toolchain + source.

**Buys.** The 39-example sweep and the golden byte-diff suite become
incremental: minutes → seconds for a leaf change. Combined with idea 5,
one CI run seals for everyone.

**Feasibility.** High — it is *only* new cache-entry kinds under the existing
epoch/key mechanism; the addressing discipline (stale = unreachable) is
already built and already carries exactly the right inputs (the epoch folds
in both `ipe` and `rustc`, which are precisely what a SEAL verdict depends
on).

**Main risk.** A verdict cached under an incomplete key certifies a build it
never saw — e.g. an environment input (`SKY_RUNTIME_DIR`-style dir contents,
a vendored runtime tree edit) that affects `cargo build` but not
`compute_project_key`. The runtime tree is deliberately outside today's key
(it does not affect `EmittedProject` *content*), but it *does* affect whether
the crate builds — so the SEAL-verdict key is a strict superset of the
artifact key. Getting that superset right is the whole design.

**Tag: NEAR-TERM.** Cheap, principled, and directly attacks the slowest
loop in the project (sweep latency). Recommended for the phased plan.

## 7. Hot code in a running app — the Elm-lineage answer

**What.** "Edit-and-continue" for a native-Rust target. Ranked by honesty:

1. **State-preserving restart (TEA superpower).** An Ipê app is `init` /
   `update` / `view` over a serializable model. Hot reload = compile a fresh,
   fully-verified binary; serialize the old process's model; start the new
   binary with `init := migrate(old_model)`; kill the old. The *code* is never
   patched — only state survives. Type-driven migration (old model type vs.
   new) tells the developer exactly when state cannot carry over, and resets
   only then. This is what "hot reload" in the Elm lineage actually means,
   and no dynamic-code hole opens: the new binary passed the full pipeline
   and the strict-CSP / no-`eval` posture is untouched.
2. **Instance swap on the wasm target.** The playground / `Ipe.Live` path
   already replaces whole artifacts; a wasm module instance swap with state
   handoff is the same restart pattern with faster process mechanics.
3. **Native function patching** (dylib-per-decl, jump-table indirection,
   `dlopen` swaps). Feasible in Rust only by compiling decls into separate
   dynamic objects behind stable ABIs — which forfeits monomorphization and
   cross-decl inlining (colliding with concrete-over-generic emission),
   introduces `unsafe` at every boundary (forbidden), and *is* dynamic code
   loading into a live process — the exact hole the no-`eval` invariant
   exists to keep shut.

**Buys.** Sub-second perceived reload with zero lost app state — the single
most magical developer-loop feature a compiled language can offer, and (1)
gets it without lying.

**Feasibility.** (1): high — it needs fast rebuilds (ideas 1/3/6 are the real
enablers), model serialization (a codegen concern, aligned with existing
encode/decode machinery), and a typed migration story. (2): medium, target-
scoped. (3): low, and disqualified on principle order before feasibility even
matters.

**Main risk.** For (1): silent state *mis*migration — an old model decoded
into a new type that happens to fit structurally but means something else.
Fail-closed: migrate only on provable compatibility, otherwise reset loudly.
For (3): the risk *is* the mechanism.

**Tag: NEAR-TERM** (state-preserving restart, as the designed meaning of
`ipe watch` hot reload); **STRETCH** (wasm swap); **rejected** (native
patching — recorded here so it is a decision, not an omission).

## 8. Speculative and compile-ahead work

**What.** Spend idle cores on work the developer *probably* wants next:
pre-warm downstream queries while a keystroke settles (the LSP touches
`parse`/`canon` instantly; speculatively run `typecheck`/`lower` before the
save); pre-build the emitted crate in the background so `ipe run` after a
green check is a link, not a build; in the playground, compile-ahead the
example variants a lesson links to.

**Buys.** Perceived latency below actual latency; the settled-edit
diagnostic delay (the coarse-`typecheck` pain the LSP decision records)
partially masked without touching solver granularity.

**Feasibility.** Medium. Salsa's cancellation model fits (a real edit
invalidates the speculative revision and the work is simply discarded);
the discipline needed is that speculative results are *only* memoization
warmth — nothing user-visible is served from a speculation that outran its
inputs. Requires care with the parallel-revision story and with not
competing for the cores `cargo` needs.

**Main risk.** Complexity for a masking win, plus a subtle liveness hazard:
speculative work holding the DB write path hostage on every keystroke.
Low soundness risk (discarded work can't lie) — the cost is engineering
attention, which currently has higher-leverage targets.

**Tag: STRETCH.**

## 9. One reactive substrate — watch, LSP, playground, docs

**What.** Treat the salsa DB as a *reactive system*, not a request/response
one: consumers subscribe to queries; input changes push invalidations to
every subscriber. `ipe watch`, the LSP, the in-browser playground, and the
local docs site become four subscribers to one substrate — a file save in the
editor updates the browser preview and the terminal watch simultaneously,
with one compile.

**Buys.** Structural de-duplication (today watch and LSP are separate
consumers that could run separate DBs over the same files); the playground's
build+run path stops being its own pipeline; "reactive compiler" is itself a
differentiator consistent with the TEA worldview — the compiler as an
`update` function over source-change messages.

**Feasibility.** Medium — the DB's event callback hook is the seed of a
subscription layer; the hard part is process topology (one daemon owning the
DB, with LSP/watch/playground as clients) and the security perimeter of that
daemon (it accepts source text and serves artifacts — a local service with a
trust boundary, designed fail-closed like the playground sandbox).

**Main risk.** The daemon becomes a privileged long-lived process holding
compile state for everything — a single corruption or staleness bug now
poisons every consumer at once, and its IPC surface is attack surface. The
one-DB-many-consumers *invariant* is already won; the one-*process* topology
must earn its keep.

**Tag: STRETCH.**

## 10. Rebuild provenance — the teaching compiler explains itself

**What.** Every incremental system eventually faces "why did this rebuild?"
and (worse) "why did this *not* rebuild?". Expose the salsa event stream as a
first-class explanation: `ipe build --explain` prints the invalidation chain
(`Http.ipe changed → interface of Http changed → Api re-checked → emitted
api.rs unchanged → cargo untouched`) in the same progressive, kind-teacher
voice as diagnostics. The cache does the same for hits/misses (which key
component diverged).

**Buys.** Three things at once: a *debugging tool* for under-invalidation
(the paramount hazard becomes observable instead of silent — an incremental
run's explanation can be diffed against a from-scratch run's in CI as a
soundness oracle); a *trust* device (developers believe caches they can
interrogate); and a genuinely distinctive expression of the
compiler-as-teacher identity — no mainstream compiler teaches its own
incrementality.

**Feasibility.** High — the event callback exists, key derivation is
centralized, and the diagnostic-rendering machinery is built. The oracle
variant (explain-diff as an invalidation-completeness test) is the novel
part and is pure test infrastructure.

**Main risk.** Minimal soundness risk (read-only). The real risk is
explanation drift — an explain path that summarizes what the system *should*
have done rather than replaying what it did. It must render the recorded
event stream, never re-derive.

**Tag: NEAR-TERM.**

---

## What is genuinely novel and worth it for Ipê

Most incremental-compilation literature optimizes latency and treats
staleness as a bug class to test for. Ipê's principle order inverts the
frame, and the ideas worth owning are the ones only that frame produces:

1. **Verification memoization under refuse-don't-guess addressing (ideas
   1 + 6).** Caching *proofs* (SEAL verdicts, sweep results, golden diffs) at
   content addresses where staleness is unreachable-by-construction is a
   coherent, novel stance: the cache is part of the correctness argument, not
   a threat to it. It also attacks the project's actual bottleneck (sweep and
   CI latency), not a hypothetical one.
2. **State-preserving restart as the *definition* of hot reload (idea 7.1).**
   Declaring native code-patching rejected on principle, and building the
   TEA-native alternative that is both safer and better (typed model
   migration), is an Elm-lineage answer no Rust-targeting compiler currently
   gives.
3. **Incrementality that explains itself (idea 10).** The explain-diff
   soundness oracle turns the under-invalidation hazard from "hope the tests
   cover it" into an observable, CI-checkable property — and doubles as the
   most on-brand developer-facing feature in this document.

The phased plan should pull ideas 1, 3 (SCC granularity), 6, 7.1, and 10 into
its horizon; 2, 4, and 5-public stay recorded here as understood-and-deferred
rather than unexamined.
