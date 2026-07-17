# Phase 7 residuals — design for the three recorded-but-not-shipped gaps

This is a **design spec, not an implementation**. It closes the three items
`docs/architecture/salsa-incremental-compilation-2026-07-11.md` §14.6 and
§14.9 explicitly surveyed and recorded rather than forced into Phase 7's own
budget:

1. `ModelSchemaTag`/bincode hardening (H22/H24) — §14.6 items 1-2.
2. The proactive `event: reload` SSE frame — §14.6 item 3.
3. SIGTERM process-group propagation — §14.9 Bug 3's closing paragraph.

## Revision log

**2026-07-12 — revised in response to an independent adversarial review.**
Still a design spec; nothing below reflects code that has been written.
Seven findings, all addressed in this revision:

1. **Soundness — `model_schema_tag`/`hash_ty` silently swallowed
   `resolve_ident` failures.** Both functions are now `DResult`-returning
   and propagate `ctx.resolve_ident` failures via `?`, matching
   `emit_live_app_inner`'s existing fallible-gate pattern. See §1.3's
   hashing skeleton and the new decisions-ledger item 10 in §1.5.
2. **Soundness — enum-variant reordering among same-payload-shape variants
   was invisible to the hash despite being wire-format-relevant.** The
   hash now folds in each variant's NAME at its declaration position (not
   sorted, unlike record fields), so both a rename and a reorder change
   the tag. Decision 4 in §1.5 is rewritten to state the new, more honest
   trade-off. See §1.3 and the new Stage-A test in TDD step 1A.3.
3. **Accuracy — the "6 fixtures" blast-radius count was off by one.**
   Corrected to 5 throughout (§0, §1.4, §1.6, TDD step 1B.6); the sixth
   `rg` hit is a comment in `tests/golden/tui_entry_case_taskrun/Main.sky`,
   not an actual `Live.app` call.
4. **TDD/internal consistency — Stage B steps 1B.1-1B.3 claimed an
   unachievable "run full suite, no regression, commit" per step.**
   `SqliteStore::new`'s signature changes in 1B.1 but `choose_store` isn't
   updated until 1B.4, so `runtime` does not compile as a whole crate in
   between. 1B.1-1B.4 are now explicitly disclosed as one atomic commit,
   mirroring Problem 2's own step-2.1 disclosure for the identical
   whole-crate-compilation situation.
5. **Documentation — Problem 2's delivery guarantee was never stated as an
   explicit decision.** Added as decisions-ledger item 6 in §2.5:
   best-effort, at-most-once, the enumerate→push race is an accepted
   low-consequence gap.
6. **Soundness (most significant) — the SIGTERM forwarder was installed
   unconditionally for both `run()` and `spawn()`, silently changing the
   EMBEDDING HOST PROCESS's SIGTERM disposition when `spawn()` runs
   in-process.** Installation is now gated on `external_stop.is_none()` —
   `run()` only, never `spawn()`. See §3.3's revised design, decision 7 in
   §3.4 (rewritten), and the new negative-control test in §3.5.
7. **Verification gap — decision 6's claim that a second SIGTERM "falls
   through to the OS default" was unverified and is most likely false.**
   Per `signal-hook-registry`'s own documented behaviour (unregistering
   the last action for a signal does NOT restore the previous/default
   disposition — the signal is subsequently ignored, not delivered), the
   claim is corrected: the real escape hatch for a hung teardown is
   SIGKILL, not a second SIGTERM. See the revised decision 6 in §3.4 and
   the new double-SIGTERM proof test in §3.5.

## 0. Scope, method, and why this stays one document

All three problems are read from the actual current code (`runtime/src/
sky_runtime/live/store.rs`, `crates/sky_backend_rust/src/emit_live.rs`,
`crates/sky_watch/src/process.rs`, `crates/skyc/src/watch.rs`), not from the
2026-07-11 doc's summary alone — §1, §2, and §3 below each open with a
"verified against code" note that either confirms or corrects that summary.

**One document, three internally-separated sections.** Problem 3 shares
nothing with Problems 1/2 (different crates, different runtime, no data or
control-flow dependency) and could ship as its own patch in any order
relative to the other two. Problems 1 and 2 both touch `runtime/src/
sky_runtime/live/`, both reuse `crate::sky_runtime::telemetry::
production_from_env()`, and both are naturally read by the same reviewer in
one sitting given they're both closing the SAME §14.6 paragraph — splitting
them into separate files would just make a reviewer open three tabs to
cross-reference the shared context (H22/H23/H24's definitions, the
`SessionStore` trait, the four backend impls). Kept as one doc, three
`##`-level sections, each independently mergeable; the final TDD list groups
steps by problem and states the one real ordering constraint (all of
Problem 1's steps are sequential; Problems 2 and 3 have no internal ordering
constraint and can interleave with Problem 1 or each other freely).

**Verified-against-code corrections to the 2026-07-11 summary**, noted once
here so each section below doesn't have to re-state them:

- The doc's "140+-test golden-oracle SEAL" figure describes the WHOLE
  golden suite (433 fixtures under `tests/golden/`). A raw `rg -l
  "Live\.app\b|Live\.appRouted" tests/golden` returns 6 files, but one of
  them — `tests/golden/tui_entry_case_taskrun/Main.sky` — matches only
  inside a `--` COMMENT describing an unrelated bug repro (`-- \`case
  List.head argsList of Just "live" -> Live.app {...} |> Task.run; _\``);
  that fixture never actually constructs a `Live.app` call. The TRUE blast
  radius is **5** fixtures: `live_let_bound_routes`,
  `live_routed_empty_routes_wrong_ctor_notfound`, `live_param_routes`,
  `live_routed_empty_routes_int_notfound`, and
  `live_routed_empty_routes_ok`. Problem 1's re-baseline blast radius is
  5 fixtures, not 140+ (corrected from an earlier drafting pass's off-by-one
  — see the revision log at the top of this document).
- `IrType::Record` is `BTreeMap<Symbol, IrType>` (`crates/sky_ir/src/
  ir.rs:513`), and `Symbol` derives `Ord` over its raw `u32` intern id
  (`crates/sky_intern/src/lib.rs:6-7`) — **intern order, not field-name
  order**. This is a real, previously-unrecorded hazard for Problem 1: a
  structural hash that walks this map in its native key order is sensitive
  to WHICH FILE GOT PARSED FIRST, not to the Model's actual shape. §1.3
  below designs around it explicitly.
- `crates/sky_backend_rust/src/lib.rs:115-125`'s `RecordStruct.fields` is
  already documented as "sorted by field name" — this is the canonical,
  existing convention Problem 1 reuses rather than inventing a second one.
- `runtime/src/sky_runtime/live/store.rs` is edited by BOTH Problem 1 (the
  `schema_tag` field/column on `SqliteStore`/`PostgresStore`/`RedisStore`
  plus their `SessionStore` impls, §1.4) and Problem 2 (the new
  `live_sessions()` trait method and its four mechanically-identical
  bodies, §2.3) — same file, disjoint methods/fields, so this is a textual
  merge-conflict risk if the two problems are worked on in parallel
  branches, never a logic conflict (neither problem reads or depends on
  the other's addition). Whichever lands first, the other rebases; there
  is no ordering requirement beyond "don't edit the same lines at the same
  time."

## 1. Problem 1 — `ModelSchemaTag`/bincode hardening (H22/H24)

### 1.1 Verified current state

- `SqliteStore<Model, Msg>` and `PostgresStore<Model, Msg>` (`runtime/src/
  sky_runtime/live/store.rs:117-360`) persist a session's Model as
  `serde_json::to_string(&model)` into a `sky_sessions(sid TEXT PRIMARY KEY,
  blob TEXT NOT NULL, last_seen …)` table. `RedisStore` (`store.rs:376-466`)
  does the same into a `sky:sess:<sid>` string key with a native Redis TTL.
- H22 ("restore-time deserialize failure panics") already holds: `get()`
  uses `serde_json::from_str(&blob).ok()?` — a `?`-propagated `None`, never
  an `unwrap`/panic. Confirmed by reading every `get()` body; no change
  needed for H22 in isolation.
- H24 ("a structurally different but syntactically valid blob silently
  decodes wrong") does **not** hold: `serde_json::from_str::<Model>` fills
  a newly-added `Option`/defaulted field with its zero value and silently
  drops a removed field's JSON key — no tag, no version, no rejection.
- `crates/sky_backend_rust/src/emit_live.rs::emit_live_app_inner` already
  gates the Model type through `#91`'s `check_admissible_model` (the
  `ir_type_is_serde` predicate) before emitting `live_app(...)`/
  `live_app_routed(...)` — so by the time any new hashing code would run,
  `model_ty` is GUARANTEED to satisfy `Serialize + DeserializeOwned + Clone
  + PartialEq` already. Problem 1 never has to handle an inadmissible Model.
- `crates/skyc/src/cache.rs:244-250`'s `compiler_revision_hash()` (whole
  `current_exe()` byte hash, used ONLY as the on-disk build-cache epoch) is
  a tempting but, per §1.2 below, WRONG reuse target for the schema tag —
  see the decisions ledger for why.

### 1.2 The `compiler_revision` axis: redefined, not reused verbatim

The design doc's compressed phrase is `ModelSchemaTag = H(compiler_revision,
structural_hash(Model type))`. Read literally against `cache.rs`'s existing
`compiler_revision_hash()` (whole-binary content hash), this would mean: any
`cargo install` of `skyc` — including one that changes nothing about Model
emission or the store's wire format, e.g. a diagnostic-message typo fix —
invalidates **every persisted session of every deployed Sky.Live app**
rebuilt with that `skyc`. H22's fail-soft path makes this SAFE (a rejected
tag just falls through to a fresh `init`, never a panic or wrong data), but
it defeats the actual point of a persistent store for production apps: a
routine point-release redeploy would silently drop every logged-in user's
session, every time, for reasons the Model shape has nothing to do with.
That is over-invalidation, and the project's own principle order ranks
under-invalidation as the higher-cost failure mode ONLY relative to
soundness gaps — here neither reading of `compiler_revision` opens a
soundness hole (H22 already guards the corrupt/mismatched case), so the
tie-break is between two SAFE choices, and efficiency/completeness (a store
that silently discards every session on every unrelated rebuild is a
availability regression, not a soundness one) favours the narrower one.

**Decision: `compiler_revision` in the schema tag is a hand-maintained wire-
format epoch constant, not a binary content hash.** This mirrors the
`KEY_TAG`/`EPOCH_TAG` domain-separation convention `cache.rs` already uses
(`b"skyc-build-cache-key-v1"` — "bumped whenever the key's ingredient set
changes shape, never for a value change within the same shape"). Concretely:
`pub const LIVE_MODEL_SCHEMA_WIRE_VERSION: &str =
"sky-live-model-schema-v1";`, declared **once**, in `runtime/src/
sky_runtime/live/store.rs` — a runtime-crate constant present in every
compiled Sky.Live binary automatically via the vendored runtime, needing NO
per-project backend emission at all. It is bumped only when the wire framing
itself changes (the tag-header length, the encoding of the body) — an
intentional, reviewed, rare edit, exactly like `KEY_TAG`. The Model's own
shape is covered by the OTHER half of the hash (§1.3), which changes
per-project and per-edit as it should.

### 1.3 `structural_hash(Model type)` — the mechanism

New function, new module: `crates/sky_backend_rust/src/emit_model_schema.rs`,
sibling to the existing `emit_model_gate.rs` (same crate, same need for
`ctx: &EmitCtx` to resolve `Symbol`s to names and to look up user-enum
variant shapes — `emit_model_gate.rs` already establishes this exact
pattern via `ctx.resolve_ident`/`ctx.enum_variant_payloads`).

**Fallibility: `ctx.resolve_ident` failures propagate, never get silently
dropped or defaulted.** `EmitCtx::resolve_ident` (`lib.rs:876`) is
`DResult<&str>`, and its own doc comment is explicit that a failure is an
internal invariant violation the lowerer is contracted never to trigger —
"surfaced as a [`Diagnostic::CompilerBug`] rather than silently emitting an
empty (and uncompilable) Rust identifier." That contract applies here with
full force: `model_schema_tag` is a collision-avoidance hash gating H24 (a
structurally-different Model must be REJECTED, not silently accepted with
a wrong or partial fingerprint), not a human-readable diagnostic message
where a best-effort fallback is low-stakes. Both `model_schema_tag` and its
recursive helper `hash_ty` are therefore `DResult`-returning and propagate
every `ctx.resolve_ident` call via `?` — exactly the pattern
`emit_live_app_inner` (`emit_live.rs:325`, already `DResult<Option<String>>`)
already uses for its own gate calls (`check_admissible_model`, etc.). A
resolve failure now surfaces as a compiler-bug diagnostic at the ONE call
site (§1.4's Stage B wiring, itself already inside a `DResult` function),
never as a hash silently computed over incomplete or defaulted input.

```rust
/// SHA-256 structural fingerprint of `model_ty`, folded with the wire-format
/// epoch constant. Two Models with the same field NAMES, same field ORDER
/// (by name — see below), same field TYPES (recursively), the same
/// nominal identity for every reachable user enum, and the same VARIANT
/// NAME at each declared enum position, hash IDENTICALLY, independent of
/// Symbol intern order, independent of which module was parsed first,
/// independent of Sky-source field-literal order.
///
/// # Errors
/// Propagates a [`Diagnostic::CompilerBug`] if `ctx.resolve_ident` fails
/// for any field/variant symbol reachable from `model_ty` — an internal
/// invariant violation (the lowerer is contracted to hand the backend only
/// resolvable symbols), never silently defaulted (see the fallibility note
/// above).
pub fn model_schema_tag(ctx: &EmitCtx, model_ty: &IrType) -> DResult<[u8; 32]>
```

**Field order: sorted by resolved NAME, never by `Symbol`.** `IrType::
Record` is a `BTreeMap<Symbol, IrType>`; iterating it directly walks
Symbol-id order, which depends on intern order (parse order), not on the
Model's shape (§0's corrected-summary note). The function instead does:
`fields.iter().map(|(sym, ty)| Ok((ctx.resolve_ident(*sym)?, ty))).collect
::<DResult<Vec<_>>>()?`, and **sorts by the resolved name string** before
hashing — the exact same canonicalisation `RecordStruct.fields` already
uses to dedupe/name the synthesised Rust struct (`lib.rs:118`), reused here
so the schema tag and the actual emitted struct field layout are always
derived from the SAME source of truth (no second, potentially drifting,
ordering convention).

**Enums get nominal identity, records don't — matching Sky's own type
system.** Sky records are structural (row-typed): a `{ x : Int, y : Int }`
from module A unifies with one from module B — so `structural_hash` never
folds a record's "name" in (records don't have one at the `IrType` level).
Sky ADTs (`IrType::Enum { home, name, args }`) are nominal — `type Color =
Red | Green` in module A is NOT the same type as an identically-shaped enum
in module B, even with the same variant shapes, and the type checker
already treats them as distinct. `structural_hash` folds in the resolved
`(home module path, type name)` for every `IrType::Enum` it walks, ON TOP OF
each variant's payload shape. This closes a real H24 gap a shape-only hash
would miss: renaming/retargeting a Model field from one enum to a
DIFFERENTLY-NAMED but byte-identical-shaped enum would otherwise round-trip
through bincode with the WRONG semantic meaning attached — exactly the
"restore passes the gate with nonsense" hazard H24's own wording warns
about.

**Variant NAMES ARE hashed, at their declared position — reversed from an
earlier drafting pass.** The original design deliberately omitted variant
names ("hashing names too would over-invalidate on a purely cosmetic
rename with zero actual risk"), reasoning about RENAME-tolerance only. That
missed a real, distinct hazard: bincode's default derive assigns each
variant's on-wire discriminant by DECLARATION INDEX, so REORDERING two
variants is wire-format-relevant even when neither variant's payload shape
nor name changes — e.g. `type Status = Pending | Active | Done` (three
zero-payload variants, a common Sky idiom) reordered to
`Active | Pending | Done` flips what index 0 decodes to, silently
corrupting every persisted row, while a shape-only hash (payload lists in
declaration order, no name attached) hashes byte-identically before and
after, since `Pending`'s and `Active`'s empty-payload contributions are
literally indistinguishable at the byte level and only their POSITION
carries the actual semantic difference — which a shape-only hash cannot
recover once the names that anchor "which position means what" are
dropped. Folding in each variant's name at its declaration position (never
sorted, unlike record fields — see below for why) fixes both hazards at
once: a rename changes the position's contribution (correctly stricter than
before), and a reorder changes which name's bytes land at which position
(correctly detected, where before it was invisible). See decision 4
(revised) in §1.5 for the honest restatement of this trade-off, and TDD
step 1A.3 for the regression test.

**Records sort by name (order-independent); enums do NOT (order is
wire-significant) — this is a deliberate, load-bearing asymmetry, not an
inconsistency.** A Sky record's emitted Rust struct is ALREADY
canonicalised to sorted-by-name field order regardless of Sky-source
declaration order (`RecordStruct.fields`, `lib.rs:118`) — so the actual
wire order for a record is deterministic and source-order-independent,
and sorting the hash's field order to match is correct. A Sky enum's
emitted Rust enum, by contrast, preserves Sky-source DECLARATION order for
its variants (bincode's derive keys the discriminant on that order) — so
the hash must walk `ctx.enum_variant_payloads`'s variants in that SAME
un-sorted declaration order for the hash to track the real wire format,
exactly as the reorder hazard above requires.

**`ctx.enum_variant_payloads`'s backing storage needs one small, additive
extension: retain each variant's `Symbol`, not just its payload shape.**
Verified against `EmitCtx::build` (`lib.rs:291-358`): `enum_variants:
BTreeMap<(ModPath, Symbol), Vec<Vec<IrType>>>` is populated by a loop that
already iterates `def.variants` in declaration order and has `variant.name`
in hand at `lib.rs:353` (it is inserted into the SIBLING `variant_fields`
map at that exact line) — but the loop discards it before pushing into
`enum_variants` (`all_fields.push(variant.fields.clone())`, `lib.rs:356`,
name not carried along). The fix is local to this one loop: change
`enum_variants`'s value type to `Vec<(Symbol, Vec<IrType>)>` and push
`(variant.name, variant.fields.clone())` instead. `enum_variant_payloads`'s
signature becomes `&self, home: &ModPath, sym: Symbol) -> &[(Symbol,
Vec<IrType>)]`; its ONE existing caller
(`emit_model_gate.rs:235`, `for payloads in
ctx.enum_variant_payloads(home, *name)`) adapts trivially to `for (_,
payloads) in ...` since it only ever needed the payload shapes. No new
traversal, no new map, no behavioural change to the existing admissibility
gate — the variant `Symbol` was always being computed one line above where
it used to get dropped.

**Exhaustive over every `IrType` variant, no catch-all.** Modelled directly
on `emit_live.rs::ir_type_display_name`'s existing exhaustive match over the
same enum (both files match `IrType`; keeping both exhaustive means a future
`IrType` variant is a compile error in TWO places, not one silently-passing
one). Every variant gets one fixed `u8` domain tag (`Int=1, Float=2, Str=3,
Bool=4, Char=5, Unit=6, Maybe=7, List=8, Result=9, Dict=10, Set=11,
Tuple=12, Record=13, Enum=14, Fun=15, …` through the full ~30-variant list)
hashed before the variant's own payload, using the SAME `update_len_prefixed`
framing `cache.rs` already established (`crates/skyc/src/cache.rs:129-137`)
to rule out delimiter-collision between sibling fields. Non-serde-admissible
variants (`Task`, `Cmd`, `Sub`, `Fun`, `Ui`, `Db`, …) still get an arm — they
can never actually reach this function on a well-typed program (the #91/#94
gate runs first and rejects them), but a total, panic-free match is cheaper
to write correctly than a partial one with a `Diagnostic::CompilerBug`
"unreachable" arm, and it means this file needs zero changes if the
admissibility gate's own rules ever loosen.

**Cycle safety: the same fuel-bounded recursion `emit_model_gate.rs`
already uses.** `leaf_of_bounded`'s `fuel: u32 = 64` precedent
(`emit_model_gate.rs:199-207`, "the type checker forbids infinite value
types, so this is belt-and-braces, never reached in practice") is reused
verbatim rather than inventing a new visited-set mechanism — `sky_types`'s
own occurs-check already makes a literally-infinite Model type
unrepresentable; the fuel bound only protects against a compiler bug in
THAT invariant, and a bound already proven adequate for the analogous
`emit_model_gate.rs` walk is the right one to reuse here, not a novel one.

**Hashing skeleton** (SHA-256, `sha2` — new dependency, `crates/
sky_backend_rust/Cargo.toml` currently has none; add `sha2 = "0.10"`,
matching the pin already used by `sky_watch`/`skyc`/`runtime`):

```rust
const WIRE_EPOCH: &str = "sky-live-model-schema-v1"; // == store.rs's LIVE_MODEL_SCHEMA_WIRE_VERSION
const TAG_RECORD: u8 = 13;
const TAG_ENUM: u8 = 14;
// … one const per IrType variant, exhaustively matched below.

fn hash_ty(ctx: &EmitCtx, ty: &IrType, h: &mut Sha256, fuel: u32) -> DResult<()> {
    if fuel == 0 { h.update([0xFFu8]); return Ok(()); } // belt-and-braces only; unreachable on well-typed input
    match ty {
        IrType::Record(fields) => {
            h.update([TAG_RECORD]);
            let mut named: Vec<(&str, &IrType)> = fields.iter()
                .map(|(s, t)| Ok::<_, Diagnostic>((ctx.resolve_ident(*s)?, t)))
                .collect::<DResult<Vec<_>>>()?;
            named.sort_by_key(|(n, _)| *n);           // canonical order — name, not Symbol
            update_len_prefixed(h, &(named.len() as u64).to_le_bytes());
            for (name, field_ty) in named {
                update_str(h, name);
                hash_ty(ctx, field_ty, h, fuel - 1)?;
            }
        }
        IrType::Enum { home, name, .. } => {
            h.update([TAG_ENUM]);
            update_str(h, ctx.resolve_ident(*name)?);
            update_str(h, &home.to_string());          // nominal identity
            let variants = ctx.enum_variant_payloads(home, *name);
            update_len_prefixed(h, &(variants.len() as u64).to_le_bytes());
            for (variant_sym, payload) in variants {
                // Declaration order preserved (never sorted, unlike record
                // fields above) and each variant's NAME folded in at its
                // own position: bincode assigns an enum's discriminant by
                // DECLARATION INDEX, so both a rename (name changes, index
                // fixed) and a reorder (index changes, name set fixed) are
                // wire-format-relevant and must both change the hash.
                update_str(h, ctx.resolve_ident(*variant_sym)?);
                update_len_prefixed(h, &(payload.len() as u64).to_le_bytes());
                for field_ty in payload { hash_ty(ctx, field_ty, h, fuel - 1)?; }
            }
        }
        // … one arm per remaining IrType variant, exhaustive, no `_ =>`.
    }
    Ok(())
}

pub fn model_schema_tag(ctx: &EmitCtx, model_ty: &IrType) -> DResult<[u8; 32]> {
    let mut h = Sha256::new();
    update_str(&mut h, WIRE_EPOCH);
    hash_ty(ctx, model_ty, &mut h, 64)?;
    Ok(h.finalize().into())
}
```

`enum_variant_payloads`'s return type in this sketch is written as
`&[(Symbol, Vec<IrType>)]` per the accessor extension described above —
`variants` iterates as `(variant_sym, payload)` pairs, not bare payload
lists.

### 1.4 Three-stage rollout

**Stage A — compute only, entirely inert.** Ships `emit_model_schema.rs`
(above) and its unit tests. Touches NO emission path (`emit_live.rs` is not
called), NO generated `main.rs`, NO golden fixture. Zero blast radius —
mergeable on its own with nothing downstream depending on it yet.

**Stage B — emission + a companion schema-tag COLUMN, JSON body unchanged.**
This is the "clearly separate, reviewable step" the task calls for, and it
is scoped NARROWER than the design doc's literal `[header][bincode body]`
framing on purpose (see decisions ledger): Stage B closes H24 (reject a
mismatched blob before ever handing it to the deserializer) WITHOUT yet
touching the SERIALIZATION FORMAT, decoupling the security-critical
"reject on mismatch" property from the higher-blast-radius "switch encoding"
change.

- `emit_live_app_inner` (`emit_live.rs`) calls `let schema_tag =
  model_schema_tag(ctx, model_ty)?;` right next to the existing #91/#94
  gate calls (same function, same scope, `model_ty` already in hand,
  `emit_live_app_inner` already `DResult`-returning so the `?` is a
  zero-friction addition — a `ctx.resolve_ident` failure now surfaces as
  the SAME `Diagnostic::CompilerBug` propagation path #91/#94's own gate
  calls already use, never a silently-defaulted hash), formats the 32
  bytes as a Rust byte-array literal, and emits one new top-level `const`
  in generated `main.rs`:
  ```rust
  /// Sky.Live Model schema-compatibility tag (H24). Computed at compile time
  /// from the Model type's structural shape; the session store rejects a
  /// persisted blob whose tag does not match BEFORE deserializing it.
  const SKY_LIVE_MODEL_SCHEMA_TAG: [u8; 32] = [0x3a, 0x91, /* … 32 total */];
  ```
  and passes it as one new trailing argument to `live_app(...)`/
  `live_app_routed(...)`.
- `sky_runtime::live::live_app`/`live_app_routed` (`runtime/src/
  sky_runtime/live/mod.rs:1157`/`1203`) gain one new parameter,
  `schema_tag: [u8; 32]`, threaded straight into `store::choose_store::
  <Model, Msg>(&store_kind, &store_path, live_ttl(), schema_tag)`.
- `choose_store`'s signature gains `schema_tag: [u8; 32]`, forwarded ONLY to
  `SqliteStore::new`/`PostgresStore::new`/`RedisStore::new` — `MemoryStore`
  ignores it (never round-trips through bytes), matching the EXISTING
  precedent in `choose_store`'s own doc comment ("The `Model: Serialize`
  bound is for the persistent backends; memory needs none, but a single
  signature keeps the codegen call uniform").
  **Compile-atomicity note (mirrors Problem 2's step-2.1 disclosure):**
  `choose_store` (`store.rs`) calls `SqliteStore::new(path, ttl).await` /
  `PostgresStore::new(path, ttl).await` today, in the SAME FILE as the
  three constructors whose signatures this bullet and the two below it
  change. Changing `SqliteStore::new`'s signature (the TDD step that adds
  its `schema_tag` parameter) without also updating `choose_store`'s call
  site in the same commit means `store.rs` — and therefore the whole
  `runtime` crate — does NOT COMPILE, not merely "regresses a test"; this
  is the identical whole-crate-compilation situation Problem 2's step 2.1
  discloses for its own trait-method addition. The TDD list below (1B.1
  through 1B.4) is therefore written as ONE atomic commit: each
  numbered step documents its own write-test → confirm-fails →
  implement narrative, but `cargo test -p sky-runtime-rust` across the
  whole crate does not go green again until 1B.4's threading lands, so
  1B.1-1B.3 are not independently commit-able intermediate states (see
  the TDD section's restated note at the top of 1B.1).
- `SqliteStore`/`PostgresStore` gain a `schema_tag: [u8; 32]` field and a
  companion `schema_tag TEXT NOT NULL` column on `sky_sessions` (added to
  the existing `CREATE TABLE IF NOT EXISTS` — a pre-existing on-disk table
  from before Stage B simply never gets this column via `IF NOT EXISTS`;
  see the decisions ledger for why that's fine and no `ALTER TABLE` is
  needed). `get()` reads the row's `schema_tag` column FIRST; a value that
  doesn't hex-match the live process's `self.schema_tag` is treated
  IDENTICALLY to "no row" (same `None` return the cold-miss path already
  takes) — REJECTED BEFORE `serde_json::from_str` ever runs. `set()` always
  writes the CURRENT tag alongside the (still-JSON) blob.
- `RedisStore` stores a Redis HASH per session (`HSET sky:sess:<sid> blob
  <json> tag <hex>` + `EXPIRE sky:sess:<sid> ttl`) instead of a bare string
  value — one key, one TTL, no risk of the tag and blob fields drifting out
  of sync via two separately-expiring keys. `get()` reads both fields with
  one `HGETALL`/`HMGET`, rejects on tag mismatch before touching the blob.
- **Golden re-baseline**: the 5 `Live.app`/`Live.appRouted` fixtures under
  `tests/golden/` (§0) now emit one extra `const` line and one extra call
  argument — re-run the golden harness, accept the new expected output for
  exactly those 5, re-run the FULL 433-fixture suite to confirm nothing
  else moved (proving the change is scoped to Live cfgs only, per
  `emit_live_app_inner`'s own gate).

**Stage C — the wire-format swap (JSON → bincode), highest blast radius,
ships last, entirely runtime-side.** Zero additional backend/emission
change (the `SKY_LIVE_MODEL_SCHEMA_TAG` const and the threading from Stage B
are already in place and unchanged) — Stage C touches only `store.rs`.

- Blob encoding becomes `base64_encode(schema_tag_bytes(32) ++
  bincode::serialize(&model)?)`, stored as a plain `String` in the SAME
  `blob TEXT` column Stage B already writes to — **no SQL schema change at
  all**. This is the one design choice that meaningfully de-risks "the
  highest-blast-radius piece" the task flagged: Postgres `TEXT` cannot hold
  arbitrary binary safely (NUL bytes, invalid UTF-8), so raw bytes are never
  written to any column directly; base64-wrapping the whole
  tag-plus-bincode payload sidesteps that entirely, for both backends,
  uniformly, with zero `ALTER TABLE`.
- Tag comparison moves from "compare the companion column" (Stage B) to
  "compare the leading 32 bytes of the decoded blob" (Stage C) — same
  reject-before-deserialize guarantee, now self-contained in one column; the
  Stage-B `schema_tag` column is simply stopped-reading (left in place,
  unused — dropping it is optional disk hygiene, never required for
  correctness, since an unread column costs nothing and SQLite's `ALTER …
  DROP COLUMN` needs 3.35+ which is not worth forcing as a hard dependency
  here).
- **No explicit migration step for pre-Stage-C rows.** A row written under
  Stage B's JSON body is NOT valid base64-of-(32-byte-tag+bincode) — `get()`
  will either fail base64 decode outright, or succeed but produce a leading
  32 "bytes" that (astronomically likely) don't match the live tag — either
  way it falls through the EXACT SAME fail-soft path H22 already guarantees
  ("corrupt/unrecognized blob → drop session → fresh `init`, never a
  panic"). Old sessions age out harmlessly on first touch after a Stage-C
  deploy; no migration tooling, no downtime, no data-format detection code
  needed.
- `RedisStore`'s HASH `blob` field switches to the same base64 encoding; the
  companion `tag` HASH field is retired the same way (stop writing it, stop
  reading it — the tag now lives inside `blob`).

### 1.5 Decisions ledger — Problem 1

1. **`compiler_revision` redefined as a hand-maintained wire-epoch constant,
   not `cache.rs`'s whole-binary content hash** — avoids over-invalidating
   every deployed session on every unrelated `skyc` rebuild; matches the
   existing `KEY_TAG`/`EPOCH_TAG` domain-separation convention already in
   this codebase (§1.2).
2. **Field order canonicalised by resolved NAME, never by raw `Symbol`** —
   `IrType::Record`'s `BTreeMap<Symbol, _>` sorts by intern id, which is
   parse-order-dependent, not shape-dependent; reusing the SAME
   name-sorted convention `RecordStruct.fields` already establishes avoids
   a second, potentially-drifting canonicalisation (§1.3, §0).
3. **Enums fold in nominal identity (home + name); records don't** —
   mirrors Sky's own type system (records structural, ADTs nominal); closes
   a same-shape-different-meaning H24 gap a pure structural hash would miss
   (§1.3).
4. **Variant NAMES ARE now hashed, at their declared position — reversed
   from an earlier drafting pass.** bincode's default derive tags enum
   variants by DECLARATION INDEX, so BOTH a rename (same position,
   different name) AND a reorder (same name set, different position) are
   wire-format-relevant; the original "hash payload shapes only, skip
   names, tolerate renames" design missed the reorder case entirely — a
   shape-only hash cannot distinguish two same-shaped variants (e.g. two
   zero-payload variants) swapping position, since their byte
   contributions are identical regardless of which name occupies which
   slot (§1.3's worked `Pending | Active | Done` example). The revised,
   honest trade-off: a purely cosmetic rename now ALSO changes the tag —
   a strictly more conservative choice, never an unsound one, since the
   cost is bounded by H22's existing fail-soft floor (a rejected tag just
   starts that one session fresh at `init`, the same safe path a genuine
   shape change already takes) — in exchange for catching the reorder
   hazard, which a rename-tolerant hash cannot afford to miss without
   leaving a real H24 gap open (§1.3).
5. **Exhaustive `IrType` match, no catch-all** — mirrors
   `ir_type_display_name`'s existing exhaustive match; a future `IrType`
   variant becomes a compile error in this file too (§1.3, CLAUDE.md §8's
   "New AST nodes require explicit walker arms" non-regression rule).
6. **Fuel-bounded recursion reusing `emit_model_gate.rs`'s existing `64`**
   — no new cycle-safety mechanism; the type checker already makes
   infinite Model types unrepresentable, so this is the SAME belt-and-
   braces precedent, not a new invariant to maintain (§1.3).
7. **Three stages, the middle one narrower than the design doc's literal
   wording** — Stage B closes H24 via a companion tag COLUMN while the
   body stays JSON, decoupling "reject on mismatch" (security-critical,
   cheap, low blast radius) from "switch to bincode" (an independent
   efficiency/self-describing-ness change with a genuinely higher blast
   radius); Stage C can therefore ship later, or even be paused, without
   leaving H24 open (§1.4).
8. **Stage C blob stays base64-in-a-TEXT-column; no `ALTER TABLE`, ever** —
   Postgres `TEXT` cannot hold arbitrary bytes safely; wrapping the whole
   tag+bincode payload in base64 keeps ONE encoding strategy across
   SQLite/Postgres/Redis and needs zero DDL migration on any backend
   (§1.4).
9. **No explicit data migration for pre-Stage-C rows** — an old JSON row
   fails the new tag check (or base64 decode) and takes the SAME fail-soft
   "drop session, fresh `init`" path H22 already guarantees; the migration
   IS the fail-soft path, not a new mechanism (§1.4).
10. **`model_schema_tag`/`hash_ty` are `DResult`-returning and propagate
    every `ctx.resolve_ident` failure via `?`, never `.ok()`/
    `.unwrap_or("")`** — a resolve failure here is the SAME internal-
    invariant-violation contract `resolve_ident`'s own doc comment states
    (`lib.rs:876`); silently dropping a field from the hash
    (`.filter_map(...).ok()...`) or collapsing a failed enum-name
    resolution to `""` (`.unwrap_or("")`) would undermine H24's entire
    point — detecting a structurally-different Model before trusting it —
    rather than merely degrading a diagnostic message. This is NOT the
    same situation as `emit_model_gate.rs::blame`'s existing
    `.unwrap_or("")` (`emit_model_gate.rs:185`), which only feeds a
    human-readable error MESSAGE (low stakes, a slightly worse string on
    an already-failing path) rather than a collision-avoidance hash (high
    stakes, the failure mode is "wrongly accepts a mismatched Model")
    (§1.3).

### 1.6 Proof-test inventory — Problem 1

| Test | Stage | Asserts |
|---|---|---|
| `record_field_rename_changes_the_hash` | A | Renaming one Model field changes `model_schema_tag` |
| `record_field_reorder_by_intern_order_is_hash_stable` | A | Two `IrType::Record`s with the SAME field-name/type set, built from Interners that assign DIFFERENT raw `Symbol` ids to those names, hash IDENTICALLY — the regression proof for the BTreeMap-intern-order hazard (§0, §1.3) |
| `identical_shape_different_enum_name_differs` | A | Two structurally-identical but differently-NAMED single-payload enums hash DIFFERENTLY |
| `enum_variant_reorder_among_same_shape_variants_changes_the_hash` | A | `Pending \| Active \| Done` (three zero-payload variants) vs. `Active \| Pending \| Done` (same three variants, first two swapped) hash DIFFERENTLY — the regression proof for the bincode-discriminant-reorder hazard (§1.3, finding 2 of the 2026-07-12 review) |
| `deeply_nested_type_never_hangs` | A | A type nested past the fuel bound still returns (no infinite recursion, no panic) |
| `sqlite_store_rejects_a_row_written_by_a_different_schema_tag` | B | Two `SqliteStore`s over the same path, different `schema_tag` — `get()` after the tag change returns `None`, not `Some(Cold(stale_shape))` |
| `sqlite_store_accepts_a_row_written_by_the_same_schema_tag` | B | Same tag both times — `get()` still returns `Some(Cold(model))` (the gate isn't "always reject") |
| `postgres_store_rejects_a_row_written_by_a_different_schema_tag` (`SKY_TEST_PG_URL`-gated) | B | Same as sqlite, over Postgres |
| `redis_store_rejects_a_row_written_by_a_different_schema_tag` (`SKY_TEST_REDIS_URL`-gated) | B | Same as sqlite, over the HASH-per-session Redis shape |
| Golden re-baseline (5 `Live.app`/`Live.appRouted` fixtures) | B | Emitted `main.rs` gains exactly the new const + arg; remaining 428 fixtures byte-identical |
| `sqlite_store_new_format_round_trips_model_through_bincode` | C | `set()` then `get()` through the base64(tag+bincode) path returns the original Model |
| `sqlite_store_old_json_row_is_rejected_not_crashed` | C | A raw pre-Stage-C JSON row (seeded directly, bypassing `set()`) is rejected cleanly by `get()`, never panics |
| `postgres_store_*` / `redis_store_*` mirrors of the two above | C | Same properties, Postgres/Redis |

## 2. Problem 2 — the proactive `event: reload` SSE frame

### 2.1 Verified current state

- `SessionStore<Model, Msg>` (`store.rs:32-42`) has exactly 4 methods
  (`get`/`set`/`delete`/`sweep`); no enumeration capability of any kind.
  Confirmed by reading the full trait definition.
- All four backends' LIVE (in-process) session cache has the IDENTICAL
  shape: `RwLock<SessionMap<Model, Msg>>` where `SessionMap<Model, Msg> =
  HashMap<String, (SessionHandle<Model, Msg>, Instant)>` (`store.rs:49`,
  reused verbatim by `MemoryStore.sessions`, `SqliteStore.mem_cache`,
  `PostgresStore.mem_cache`, `RedisStore.mem_cache`). This means the new
  trait method's four bodies are, genuinely, mechanically identical — one
  `RwLock::read()` + `.values().map(|(h, _)| h.clone()).collect()` per
  backend, differing only in the field name.
- `SessionEntry<Model, Msg>::sse_tx: Option<SseTx>` (`mod.rs:305`) is set
  when a browser attaches (`mod.rs:1455`); `SseTx = mpsc::Sender<SsePatch>`
  and `SsePatch(pub String)` (`sse.rs:5-7`); `sse::frame(event, data)`
  (`sse.rs:41`) builds the raw SSE text. Pushing a new frame kind is
  already a one-line `tx.send(SsePatch(sse::frame("reload", "{}"))).await`
  — the SAME mechanism the existing `"hello"`/`"heartbeat"`/`"patch"`
  frames already use (`mod.rs:1477-1508`). No new SSE machinery needed.
- `live_shutdown_signal()` (`mod.rs:1060`) is currently a **zero-argument**
  async fn, called as `.with_graceful_shutdown(live_shutdown_signal())`
  (`mod.rs:1890`) from inside the SAME generic scope where `store` (the
  `Arc<dyn SessionStore<Model, Msg>>` built by `choose_store` at
  `mod.rs:1178`/`1237`) is already a live local binding. Threading `store`
  through is a pure, additive, internal signature change — not
  user-observable.
- **H23 (Go-era hazard ledger, `docs/architecture/incremental-compilation-
  and-watch.md:870`): "Dev-only reload/hot-swap endpoint exposed in
  production — security — channel ABSENT (not disabled) under the
  production gate."** This applies directly and is a hard requirement, not
  an optional nicety — see §2.4.
- `crate::sky_runtime::telemetry::production_from_env()`
  (`telemetry.rs:143`) is the Rust port's existing equivalent of Go's
  `productionFromEnv()`, already used by the console/metrics production
  gates — the correct, established check to reuse here.

### 2.2 `SessionStore` trait addition

```rust
#[async_trait]
pub trait SessionStore<Model, Msg>: Send + Sync {
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>>;
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>);
    async fn delete(&self, sid: &str);
    async fn sweep(&self) {}

    /// Every session handle THIS PROCESS currently holds live (i.e. has an
    /// in-memory driver + possibly an open SSE connection). Deliberately
    /// scoped to the LOCAL mem-cache, never the full persisted table: a
    /// "Cold" row on disk (another replica's session, or one this process
    /// simply hasn't touched yet) has no SSE connection in THIS process to
    /// push anything to, so it is out of scope for what this method is for.
    /// Returns handles directly (not bare sids) — the caller (§2.3) needs
    /// each handle's `sse_tx` and would otherwise have to re-`get()` every
    /// id, opening a TOCTOU-ish gap where a session evicted between the
    /// enumerate and the re-fetch is silently skipped OR (worse) touches
    /// its TTL a second time for no reason.
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>>;
}
```

No default body (unlike `sweep`) — every backend has an opinion (all four
happen to share the SAME mem-cache field, but the trait doesn't assume
that structurally; a future fifth backend without an in-process cache would
have to make an explicit, reviewed choice, not silently inherit a
possibly-wrong default).

### 2.3 Four backend impls — mechanically parallel, one per storage shape

**`MemoryStore`** (`store.rs:65-96`):
```rust
async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
    self.sessions
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(h, _)| h.clone())
        .collect()
}
```

**`SqliteStore`** (`store.rs:143-239`) — identical body, over `mem_cache`
instead of `sessions`; the persisted `sky_sessions` table is untouched (a
`Cold` row there has no live handle to return):
```rust
async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
    self.mem_cache
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(h, _)| h.clone())
        .collect()
}
```

**`PostgresStore`** (`store.rs:273-360`) — byte-for-byte the same body as
`SqliteStore`'s, over its own `mem_cache` field (same field name, same
type — the two structs are already structurally twins per the existing
`store.rs` comments).

**`RedisStore`** (`store.rs:376-466`) — same body again, over its
`mem_cache` field; Redis's own server-side data is irrelevant here for the
same reason Postgres's `sky_sessions` table is (a locally-live handle is
what has an SSE connection, and the reload push only ever targets THIS
process's open connections).

### 2.4 Wiring — `live_shutdown_signal`, and the H23 production gate

New small, independently-testable helper (`mod.rs`, near `live_shutdown_
signal`):

```rust
/// Push a bounded `event: reload` frame to every session THIS PROCESS is
/// currently serving over SSE, so a connected browser skips its own
/// reconnect-wait and refetches immediately instead of waiting out
/// `SKY_LIVE_RETRY_BASE_MS`'s backoff ladder. Dev-mode only — see H23
/// (`docs/architecture/incremental-compilation-and-watch.md:870`): a
/// production deployment must have NO reachable path that pushes this
/// frame, so the call is gated at its ONE call site (`live_shutdown_
/// signal`, below), never inside this helper — a caller that reaches this
/// function has already decided dev-mode applies.
async fn push_reload_to_live_sessions<Model, Msg>(store: &Arc<dyn SessionStore<Model, Msg>>)
where
    Model: Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    for handle in store.live_sessions().await {
        let tx = handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sse_tx
            .clone();
        if let Some(tx) = tx {
            let _ = tx.send(SsePatch(sse::frame("reload", "{}"))).await;
        }
    }
}
```

`live_shutdown_signal` gains one parameter — `store: Arc<dyn
SessionStore<Model, Msg>>` — threaded from its ONE call site
(`mod.rs:1890`, `.with_graceful_shutdown(live_shutdown_signal(store
.clone()))`, mirrored at both `live_app` and `live_app_routed`'s shared
inner scope where `store` already exists). Inside the function, right after
`observability::mark_draining()` (`mod.rs:1070`, i.e. once the shutdown is
committed and orchestrators have been told to stop routing new traffic) and
BEFORE the bounded grace-timer drain begins:

```rust
if !crate::sky_runtime::telemetry::production_from_env() {
    push_reload_to_live_sessions(&store).await;
}
```

**On "absent, not disabled" (H23).** The Go-era wording was written for a
ROUTE (`/_sky/console`, `/_sky/metrics`) — for those, "absent" means the
route is never added to the router, so an attacker cannot even PROBE for
it. This new capability is not a route at all: it is a server-INITIATED
push onto an ALREADY-OPEN, already-authenticated SSE connection, fired
exclusively from inside the shutdown sequence — there is no
attacker-controllable input that reaches it (no request body, no query
param, no header selects it). Its risk model is therefore narrower than
the routes H23's literal wording was written against. Even so, the
recorded requirement ("wired only when dev") costs nothing to honour
exactly as stated: the `if !production_from_env()` guard at the single call
site means the code path is unreachable in production with the SAME
one-`if` shape every other production gate in this file already uses
(`observability`'s dev-console mount, the metrics-auth gate) — consistent
with, not a special case of, this codebase's existing gating convention.

### 2.5 Decisions ledger — Problem 2

1. **`live_sessions()` returns handles, not bare sids** — avoids a
   re-`get()` round trip that would double-touch TTL bookkeeping and open
   a narrow eviction race between enumerate and use (§2.2).
2. **Scoped to the LOCAL mem-cache only, never the persisted table** — a
   `Cold` row has no SSE connection in this process; only locally-live
   sessions are meaningful targets for a push (§2.2).
3. **All four backends share one mechanical body** — every backend already
   carries the identical `RwLock<SessionMap<Model, Msg>>` shape for its
   in-process cache; the new method is a direct consequence of that
   existing symmetry, not new design (§2.3).
4. **The H23 gate lives at the ONE call site, not inside the push
   helper** — keeps the helper itself simple and unconditionally testable
   in isolation (§2.6's `push_reload_to_live_sessions_sends_one_frame_per_
   live_session` doesn't need an env var), while still making the
   production code path genuinely unreachable via the same `if`-gate
   convention already used elsewhere in this file (§2.4).
5. **`event: reload`'s payload is the empty object `"{}"`, matching the
   existing `heartbeat` frame's shape** — no new client-JS payload
   contract to design; the reload frame only needs to be a DISTINCT EVENT
   NAME the client's `EventSource` listener switches on (client-JS wiring
   is out of scope for this backend/runtime spec).
6. **Delivery guarantee: best-effort, at-most-once, never retried** — the
   code sketch's `let _ = tx.send(...).await` (§2.4) discards the
   `Result`: a full/closed channel silently drops that one session's
   reload frame rather than blocking the shutdown sequence or retrying.
   This is an explicit, accepted trade-off, not an oversight — a dropped
   `reload` frame has a low-consequence fallback (the browser's own
   `EventSource` reconnect-on-drop logic, `SKY_LIVE_RETRY_*`'s existing
   backoff ladder, already fires independently the moment the SSE
   connection itself closes during shutdown), so the correctness floor
   was already covered before this feature existed; `push_reload_to_live_
   sessions` only ever shaves latency off that existing recovery path, it
   is never the ONLY mechanism a client relies on to notice a restart.
   The enumerate (`live_sessions()`) → push (`tx.send`) window is
   similarly a narrow, accepted race: a session that disconnects between
   the two steps just doesn't receive a frame it can't act on anyway
   (§2.4).

### 2.6 Proof-test inventory — Problem 2

| Test | Asserts |
|---|---|
| `memory_store_live_sessions_lists_only_locally_cached_handles` | `live_sessions()` on `MemoryStore` returns exactly the set/not-yet-deleted sids |
| `sqlite_store_live_sessions_excludes_cold_rows` (`feature = "db"`) | A row inserted directly into `sky_sessions` (bypassing `set()`, simulating a cross-replica session) is NOT returned by `live_sessions()`; a `set()`-created one is |
| `postgres_store_live_sessions_excludes_cold_rows` (`SKY_TEST_PG_URL`-gated) | Same property, Postgres |
| `redis_store_live_sessions_excludes_cold_rows` (`SKY_TEST_REDIS_URL`-gated) | Same property, Redis |
| `push_reload_to_live_sessions_sends_one_frame_per_live_session` | Two live sessions with bound `sse_tx` channels each receive exactly one `sse::frame("reload", "{}")`; a session with `sse_tx = None` is skipped without panicking |
| `live_shutdown_signal_skips_the_reload_push_in_production` | With `ENV=production`, the reload push helper is never invoked (asserted via a channel that would receive the frame in dev but stays empty in prod) |

## 3. Problem 3 — SIGTERM process-group propagation

### 3.1 Verified current state

- `crates/skyc/src/lib.rs:1`, `crates/skyc/src/main.rs:1`, and
  `crates/sky_watch/src/lib.rs:1` are ALL `#![forbid(unsafe_code)]`. The
  workspace's ONE sanctioned `unsafe` block
  (`runtime/src/sky_runtime/live/console_proxy.rs:161`, `PR_SET_PDEATHSIG`)
  lives in the `runtime` crate, which is NOT under a blanket
  `forbid(unsafe_code)` (`runtime/src/lib.rs:20`'s own comment: it uses
  named-exception `deny` lints instead) — confirming `PRINCIPLES.md`'s
  framing that this is the ONE exception, and it lives somewhere `skyc`/
  `sky_watch` structurally cannot reach without opening a second one.
- `stop_gracefully` (`crates/sky_watch/src/process.rs:353-370`) never
  actually sends SIGTERM today — it polls `try_wait()` until `grace`
  elapses, THEN escalates to the SAFE, portable `Child::kill()` (SIGKILL).
  Its own doc comment already explains why: a true SIGTERM needs a raw
  `kill(2)`, which would be a second `unsafe` site next to
  `console_proxy`'s sole sanctioned one.
- `run_inner`'s main loop (`crates/skyc/src/watch.rs:650-848`) is a single
  blocking `evt_rx.recv()` over `OrchestratorEvent` (`FsBatch` /
  `CompileDone` / `CargoDone` / `Shutdown`). The EXISTING `external_stop`
  wiring (`watch.rs:623-630`) is a template already proven in this exact
  file: a small forwarder thread blocks on an independent receiver and, on
  success, sends `OrchestratorEvent::Shutdown` into the SAME `evt_tx` the
  main loop already drains. `OrchestratorEvent::Shutdown => break` (`watch
  .rs:846`) then runs the FULL orderly teardown (`drop(watcher)` →
  kill any in-flight `cargo build` → `supervisor.shutdown(...)` →
  join the compile worker → join the coalesce thread → `Ok(())`).
- **The actual gap**: `run()` (`watch.rs:495-497`, the CLI-facing entry
  with `external_stop = None`) installs NO signal handler of any kind
  today. A bare OS `SIGTERM` delivered to the `skyc` PID with its DEFAULT
  disposition TERMINATES THE PROCESS IMMEDIATELY — this is a hard kernel
  kill, not a Rust panic/unwind, so it runs NONE of `run_inner`'s teardown
  code (no `drop(watcher)`, no `supervisor.shutdown()`, no reaping the
  supervised child). The supervised child — a literal `Child` handle held
  by `skyc`, spawned via `apply_green` — is orphaned, re-parented to
  `init`/systemd, and keeps running (and keeps holding its port) forever.
  This is a DIFFERENT failure mode from Bug 3 (§14.9, already fixed): Bug 3
  was "the EMBEDDER forgot to call `stop()`" (closed by `WatchHandle`'s
  `Drop`); this is "the OS never gave `skyc`'s own Rust code a CHANCE to
  run ANY cleanup code at all," which no amount of `Drop`/`WatchHandle`
  logic can fix — the process is dead before any of it executes.
  Interactive Ctrl-C is a DIFFERENT, already-fine case: it delivers SIGINT
  to the whole foreground PROCESS GROUP (parent AND child both die
  independently, abruptly, but TOGETHER — no orphan). A supervisor sending
  SIGTERM to only the `skyc` PID (systemd's default; most container
  orchestrators) has no such symmetry.

### 3.2 `#![forbid(unsafe_code)]` is per-crate — no exception, no wrapper crate needed

This is the explicit thing the task asked to verify, and the answer is: **no
exception is needed, and no workspace-edge wrapper crate is needed.**
`#![forbid(unsafe_code)]` is a lint attribute scoped to the crate it is
written in — it rejects an `unsafe` block appearing in THAT crate's own
source files. It has no effect on, and no visibility into, a DEPENDENCY
crate's own source — each crate is compiled as an independent unit with its
own lint configuration. A crate can depend on another crate that uses
`unsafe` internally (as `runtime` already does, transitively, via `libc`,
`sqlx`, `tokio`, etc. — none of which are workspace members and none of
which `runtime`'s OWN `forbid` — it doesn't have one — would touch anyway)
without ever writing `unsafe` itself.

`signal-hook` (the crate chosen below) has a **completely safe public
API**: `Signals::new([SIGTERM])` and its `.forever()` iterator are ordinary
safe Rust from a CALLER's point of view. The `unsafe` `sigaction(2)` FFI
call this needs happens inside `signal-hook`'s own internal registry
(`signal-hook-registry`, a separate crate `signal-hook` itself depends on)
— code `skyc`/`sky_watch` never write, never see, and are not
`#![forbid(unsafe_code)]`-responsible for. Adding `signal-hook` as a
dependency of `crates/sky_watch/Cargo.toml` is therefore fully compatible
with `sky_watch`'s existing `#![forbid(unsafe_code)]` with ZERO exception,
ZERO wrapper crate, and ZERO change to either crate's forbid/deny posture.

### 3.3 Design — a safe SIGTERM listener feeding the existing event channel

New module, `crates/sky_watch/src/signal.rs` (the salsa-agnostic crate —
this primitive knows nothing about `SkyDatabase`/`OrchestratorEvent`,
matching §14.1's own two-crate split rationale: "a confined path allowlist,
a debounce coalescer, a process supervisor" already live here as generic,
independently-testable primitives; a generic "run this closure on SIGTERM"
primitive belongs alongside them, not inside `skyc`'s salsa-aware
orchestrator):

```rust
//! Task 24-continuation — a SAFE SIGTERM listener. Unix-only: SIGTERM has
//! no equivalent OS-level concept on Windows (Ctrl-C there is a distinct
//! console-event API this module deliberately does not attempt to unify
//! with — out of scope, matching this crate's existing unix-gated SIGKILL
//! escalation in `process.rs`).

#[cfg(unix)]
use std::thread::JoinHandle;

/// Spawn a dedicated OS thread that blocks for SIGTERM and, on receipt,
/// invokes `on_sigterm` exactly once, then exits. Uses ONLY safe APIs —
/// `signal-hook`'s public surface never requires the CALLER to write
/// `unsafe` (see the design doc's §3.2 for why this is compatible with
/// `#![forbid(unsafe_code)]`).
///
/// # Errors
/// If the OS refuses to register the handler (e.g. an already-exhausted
/// signal-handler slot — vanishingly rare in practice).
#[cfg(unix)]
pub fn install_sigterm_forwarder<F>(on_sigterm: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM])?;
    Ok(std::thread::spawn(move || {
        // Exactly one signal is enough — the caller's closure drives its own
        // full teardown. A second SIGTERM after this thread returns is NOT
        // guaranteed to terminate the process: `signal-hook-registry` does
        // not restore the pre-registration (default) disposition once the
        // last action for a signal is gone, so a second SIGTERM is most
        // likely silently IGNORED from that point on, not delivered as a
        // kernel kill (verified against `signal-hook-registry`'s own
        // documented `unregister` behaviour — see decision 6 in §3.4, and
        // the double-SIGTERM proof test in §3.5). The documented, TESTED
        // escape hatch for a hung teardown is therefore SIGKILL, never a
        // second SIGTERM.
        if signals.forever().next().is_some() {
            on_sigterm();
        }
    }))
}
```

`crates/sky_watch/Cargo.toml` gains `signal-hook = "0.3"` (no other
workspace crate currently depends on it or on `ctrlc` — confirmed by
`rg -n "signal-hook|ctrlc" Cargo.toml crates/*/Cargo.toml runtime/Cargo.toml`
this session, zero hits).

**Wiring, `crates/skyc/src/watch.rs::run_inner`** — inserted right after
`evt_tx`/`evt_rx` are created (`watch.rs:612`) and BEFORE the existing
`external_stop`-consuming block (`watch.rs:622-629`), so the new check can
still read `external_stop` (`Option::is_none(&self)` only borrows) ahead of
that block moving it. **Gated on `external_stop.is_none()` — installed for
`run()` only, NEVER for `spawn()`** (see the revised Scope note below;
this closes finding 6 of the 2026-07-12 review, the single most
significant correction in this revision):

```rust
if external_stop.is_none() {
    #[cfg(unix)]
    {
        let evt_tx = evt_tx.clone();
        // Errors are logged, never fatal — a platform where signal
        // registration fails degrades to "no SIGTERM-only-to-this-PID
        // handling" (the pre-existing behaviour), never a hard failure of
        // `ipe watch` itself.
        if let Err(e) = sky_watch::install_sigterm_forwarder(move || {
            let _ = evt_tx.send(OrchestratorEvent::Shutdown);
        }) {
            eprintln!("[ipe watch] warning: could not install SIGTERM handler: {e}");
        }
    }
}
```

This is the ENTIRE fix for the `run()` path. No new interruption mechanism
for `evt_rx.recv()` is needed — it is ALREADY the orchestrator's one
blocking wait-point, and it is ALREADY driven by `mpsc::Sender::send` from
other threads (the `FsBatch` forwarder, the `external_stop` forwarder); a
SIGTERM-to-`Shutdown` forwarder is a third instance of the exact same
pattern, so `evt_rx.recv()` unblocks the moment the signal arrives, and the
EXISTING `OrchestratorEvent::Shutdown => break` arm (`watch.rs:846`) runs
the FULL, already-correct, already-tested teardown sequence (§3.1) —
`drop(watcher)`, kill any in-flight `cargo build`, `supervisor.shutdown
(...)` (which itself SIGKILLs the supervised child within its own bounded
`graceful_stop` window), join both helper threads, return. A supervisor's
SIGTERM to only the `skyc` PID now runs this SAME orderly path instead of a
hard kernel kill that skips it entirely.

**Scope — gated to `run()`, NEVER installed for `spawn()` (reversed from
an earlier drafting pass).** `run()` (`pub fn run(opts) { run_inner(opts,
None) }`) is the ONLY caller that needs this: `spawn()` (`watch.rs:
448-469`) already has `WatchHandle`'s `Drop`-based teardown for the
"caller never signalled" case (Bug 3, already closed). An earlier draft of
this spec argued the forwarder should install UNCONDITIONALLY for both
callers, reasoning that `spawn()`'s own process would get
"graceful-on-SIGTERM behaviour as a strict improvement." **That reasoning
was wrong, and this is the review's most significant finding: `spawn()`
runs `run_inner` on a same-process background THREAD, not a subprocess**
(`watch.rs:448`'s `thread::spawn`) — it shares the EMBEDDING HOST's PID.
`signal-hook-registry` chains to a pre-existing handler only at FIRST
registration and does NOT restore the previous disposition once its own
actions are gone (confirmed against the crate's own documented behaviour —
see decision 6, revised, and finding 7's correction below). So for the
common case of a host that never installed its own SIGTERM handler (relying
on the OS default, "the process dies"), calling `sky_watch::spawn()` from
that host would SILENTLY AND PERMANENTLY change the WHOLE HOST PROCESS's
SIGTERM disposition from "the process dies" to "only sky_watch's own
teardown runs; the host process keeps running unless it independently
exits" — directly contradicting the "harmless, strict improvement" framing,
and directly reversing the ORIGINAL Bug-3 analysis (§14.9) that
deliberately scoped signal handling OUT of the `spawn()`/embedder path for
exactly this reason. `spawn()` is the documented embedder-facing API (this
spec and §14.9 both point external callers — an IDE integration, an LSP
host, the SkyDeploy control plane — at it), so installing a process-wide
signal handler as a side effect of calling it is a real landmine for the
FIRST genuine embedder, even though today's only caller
(`crates/skyc/tests/watch_integration.rs:147`, a test) doesn't happen to
hit it.

The forwarder is therefore installed IFF `external_stop.is_none()` — i.e.
`run()` only, using `external_stop` itself as the existing, free signal
that distinguishes the two callers (`None` for `run()`, `Some(stop_rx)`
for `spawn()`) — no new parameter or plumbing needed. This also loses
nothing real: as the original reasoning correctly noted, an embedder
running `ipe watch` as an in-process thread is not a target a
`kill -TERM <skyc-pid>` command can reach independently of its OWN
process's signal, so `spawn()` genuinely has no USE for this forwarder.
`spawn()` keeps relying exclusively on `WatchHandle::Drop` and its own
`stop_tx` channel, exactly as it did before this spec.

### 3.4 Decisions ledger — Problem 3

1. **No `unsafe`, no exception, no wrapper crate** — `forbid(unsafe_code)`
   is per-crate; `signal-hook`'s public API is 100% safe from the caller's
   side; its own internal `unsafe` lives in a dependency crate this
   workspace's forbid attributes never reach or need to reach (§3.2).
2. **`signal-hook` over `ctrlc`** — `ctrlc` is a thin, SIGINT-centric
   convenience wrapper; this problem is specifically about SIGTERM
   (Ctrl-C/SIGINT already works via process-group propagation, §3.1).
   `signal-hook` lets the exact signal be named explicitly and is the
   lower-level, more precise choice for a narrowly-scoped fix.
3. **The forwarder feeds the EXISTING `evt_tx`/`OrchestratorEvent::
   Shutdown` channel — zero new synchronisation primitives, zero new
   teardown logic** — a third instance of the SAME pattern
   `external_stop`'s forwarder already establishes (§14.8 ledger item 3,
   "generation-tagged events over a single unified channel"); the entire
   fix is "make one more thing able to send into a channel that already
   exists and is already correctly drained" (§3.3).
4. **`sky_watch::signal`, not `skyc::watch`** — the listener knows nothing
   about salsa/`SkyDatabase`; it belongs with the crate's other
   salsa-agnostic, independently-unit-testable primitives (`scope.rs`,
   `coalesce.rs`, `process.rs`), matching §14.1's own two-crate rationale
   (§3.3).
5. **Unix-only, explicit `#[cfg(unix)]`, no Windows equivalent attempted**
   — SIGTERM has no OS-level analogue on Windows; unifying with Windows's
   distinct console-event API is a separate, unscoped problem this spec
   does not attempt to solve, matching this crate's own existing
   unix-gated precedent (`process.rs`'s SIGKILL-escalation tests) (§3.3).
6. **A second SIGTERM during teardown is NOT specially handled, and does
   NOT reliably terminate the process — the escape hatch for a hung
   teardown is SIGKILL, not a second SIGTERM (corrected from an earlier
   drafting pass's unverified claim).** The original text asserted a
   second SIGTERM "falls through to the OS default" once the forwarder
   thread returns. Per `signal-hook-registry`'s own documented `unregister`
   behaviour (confirmed this session: once the LAST action for a signal is
   gone, the library does NOT restore the previous/default disposition —
   its own docs state the process will "effectively ignore [that] signal
   from now on, not terminate on [it]"), that claim is most likely FALSE: a
   second SIGTERM is more likely to be silently absorbed (ignored) than
   delivered as a kernel kill. This spec does not attempt to fix that —
   adding a restore-on-teardown or a second-signal watchdog is explicitly
   OUT of scope (the recorded gap is "install a handler so cleanup CAN
   run," not "add a new escalation layer on top of a teardown sequence
   that is ALREADY individually timeout-bounded at every step" —
   `SHUTDOWN_WAIT_BUDGET`, `graceful_stop`, `readiness`, all already
   exist) — but the OPERATOR-FACING claim about what a stuck `ipe watch`
   needs is corrected: SIGKILL, not a second SIGTERM. §3.5's new
   double-SIGTERM proof test records the actual observed behaviour so this
   is a tested fact, not a restated assumption (§3.3).
7. **Installed IFF `external_stop.is_none()` — `run()` only, NEVER
   `spawn()` (reversed from an earlier drafting pass's "install
   unconditionally" choice).** The original reasoning ("strictly improves
   `spawn()`'s own OS-process behaviour") missed that `spawn()` runs on a
   same-process background thread and shares the EMBEDDING HOST's PID —
   installing a process-wide SIGTERM handler as a side effect of an
   in-process embed call permanently changes the HOST's signal disposition
   (the single most significant finding of the 2026-07-12 review). Gating
   on `external_stop.is_none()` is free: `external_stop` is already the
   exact signal that distinguishes `run()` (`None`) from `spawn()`
   (`Some(stop_rx)`), so no new parameter or plumbing is needed — the fix
   is a one-`if` change at the existing insertion point (§3.3).

### 3.5 Proof-test inventory — Problem 3

| Test | Asserts |
|---|---|
| `sigterm_forwarder_invokes_callback_on_sigterm` (unix-gated) | `install_sigterm_forwarder` fires its closure after the test process signals itself via `kill -TERM $$` |
| `sigterm_forwarder_never_fires_without_a_signal` (unix-gated) | The closure is NOT invoked within a bounded poll window when no signal is sent (negative control) |
| `watch_shuts_down_the_supervised_child_on_sigterm_to_only_the_skyc_process` (`SKY_E2E=1`, unix-gated) | A real `ipe watch` subprocess, `kill -TERM <skyc-pid>` (the PID only, not its process group) — asserts BOTH the `skyc` process exits within a bounded wait AND the supervised child process is gone (`/proc/<pid>` check, same technique as the existing Bug-3 regression test) |
| `spawn_never_installs_a_sigterm_forwarder` (unix-gated) — regression test for finding 6 | Call `sky_watch::spawn(opts)` in-process, then deliver SIGTERM to the TEST process's own PID (the host `spawn()` is running inside); assert the test process is STILL ALIVE after a bounded wait (no exit) and that `WatchHandle::stop()` remains the only way the spawned watch loop actually shuts down — proves `spawn()` never touches the embedding host's SIGTERM disposition |
| `double_sigterm_after_forwarder_consumed_is_silently_absorbed_use_sigkill` (`SKY_E2E=1`, unix-gated) — proof test for finding 7 | A real `ipe watch` subprocess with a supervised child that ignores SIGTERM itself (so `supervisor.shutdown`'s graceful window is guaranteed to elapse and its own bounded SIGKILL escalation is what actually reaps the child) — send SIGTERM once (starts the documented graceful teardown), then, partway through the teardown's bounded wait (after the forwarder thread has already consumed the first signal and returned), send a SECOND SIGTERM to the SAME `skyc` PID; assert the `skyc` process's total wall-clock time to exit is NOT measurably shorter than a single-SIGTERM run's (i.e. the second signal has no observable escalating effect — it is absorbed, not delivered as an additional kill). The test's own doc comment states the operational conclusion in plain language: "a stuck `ipe watch` needs SIGKILL — a second SIGTERM is not a documented or relied-upon escape hatch." |

## 4. Combined TDD step list

Grouped by problem; ordered by the staging designed above. **Problem 1's
steps are strictly sequential** (Stage A before B before C — each stage's
tests depend on the previous stage's code existing). **Problems 2 and 3
have no internal ordering constraint against each other or against Problem
1** — any step below can interleave with any other group's steps; the only
hard rule is same-group ordering.

Every step: write the failing test, run it, confirm it fails for the
stated reason (not a typo/compile-error-elsewhere), implement the minimal
code to pass it, run it green, then run the surrounding crate's existing
test suite to confirm no regression, then commit.

### Problem 1 — Stage A (inert, `sky_backend_rust` only)

**1A.1 — Scaffold `emit_model_schema.rs` + first hash test.**
Add `sha2 = "0.10"` to `crates/sky_backend_rust/Cargo.toml`. Create
`crates/sky_backend_rust/src/emit_model_schema.rs` with a `model_schema_tag`
function stub of the FINAL fallible shape — `pub fn model_schema_tag(ctx:
&EmitCtx, model_ty: &IrType) -> DResult<[u8; 32]>` returning
`Ok([0u8; 32])` unconditionally (`todo!()`-free, so the crate compiles; see
§1.3's fallibility note — the signature is `DResult` from this very first
stub, never widened to fallible later, so no TDD step downstream has to
retrofit error propagation into already-written call sites). In its
`#[cfg(test)] mod tests`, add `record_field_rename_changes_the_hash`: build
two `IrType::Record`s by hand via a small `Interner` (mirror
`crates/sky_backend_rust/tests/golden.rs`'s `build_m0` pattern for
constructing minimal IR/`EmitCtx` fixtures), one `{x: Int, y: Int}`, one
`{x: Int, z: Int}` (renamed `y`→`z`), call `model_schema_tag` on both,
`.expect("hash of a well-typed test fixture must succeed")` each result,
`assert_ne!` the two `[u8; 32]`s. Run it — **confirm it fails** because
both calls return the same all-zero stub, not because of a compile error.
Implement the real record-hashing arm (name-sorted, per §1.3, propagating
`ctx.resolve_ident` via `?`). Run green. Declare `mod emit_model_schema;`
in `crates/sky_backend_rust/src/lib.rs`. Commit.

**1A.2 — The intern-order regression test.**
Add `record_field_reorder_by_intern_order_is_hash_stable`: build the SAME
`{x: Int, y: Int}` field-name/type SET twice, using two `Interner`s that
intern `"x"` and `"y"` in OPPOSITE ORDERS (so the two `IrType::Record`
`BTreeMap`s have their fields at DIFFERENT raw `Symbol` positions), call
`model_schema_tag` on both, `.expect(...)` each result, `assert_eq!` the
two `[u8; 32]`s. Run it — if the implementation
from 1A.1 iterates the `BTreeMap` directly instead of resolving-then-
sorting-by-name, **confirm this fails** (this is the regression proof for
the hazard in §0/§1.3). Fix the implementation to resolve+sort by name
before hashing if it doesn't already. Run green. Commit.

**1A.3 — Enum nominal-identity + variant-reorder tests.**
First, extend `enum_variant_payloads`'s backing storage (`EmitCtx::build`,
`lib.rs:291-358`) to retain each variant's `Symbol` alongside its payload
shape — change `enum_variants`'s value type from `Vec<Vec<IrType>>` to
`Vec<(Symbol, Vec<IrType>)>` and push `(variant.name, variant.fields.
clone())` at `lib.rs:356` instead of dropping the name; update
`enum_variant_payloads`'s return type to `&[(Symbol, Vec<IrType>)]` and its
ONE existing caller (`emit_model_gate.rs:235`) to destructure and ignore
the name it doesn't need (§1.3). Then add TWO tests:
`identical_shape_different_enum_name_differs` (construct two
single-variant, single-`Int`-payload user enums with DIFFERENT `home`/
`name` — e.g. `Mod.A::Wrapper` and `Mod.B::Box` — embed each as a Model
field, hash both, `assert_ne!`) and
`enum_variant_reorder_among_same_shape_variants_changes_the_hash`
(construct a three-zero-payload-variant enum `Pending | Active | Done` and
a second enum with the SAME `home`/`name` but variants declared as
`Active | Pending | Done` — first two swapped, same name SET, same shape
set — embed each as a Model field, hash both, `assert_ne!`; this is the
regression proof for finding 2 of the 2026-07-12 review: a shape-only hash
would hash these identically since a zero-payload variant's byte
contribution doesn't depend on which name occupies that position). **Confirm
both fail** against a stub that only hashes payload shape and ignores
variant names/position (the reorder test would ALSO fail against a
hypothetical name-hashing-but-sorted-by-name implementation — sorting
would defeat the whole point, since it would silently re-canonicalise the
reordered enum back to the same sorted sequence as the original; the test
therefore also serves as a regression guard against "sort variants by name
the same way record fields are sorted," a plausible but wrong fix). Implement
the `IrType::Enum` arm (name + home + each variant's own name folded in at
its declaration position, payload shapes, via the extended
`ctx.enum_variant_payloads`, propagating `ctx.resolve_ident` via `?`). Run
both green. Commit.

**1A.4 — Exhaustiveness + fuel-bound tests.**
Add `deeply_nested_type_never_hangs`: construct (or synthesize via a loop)
a `Maybe(Maybe(Maybe(...)))` chain past 64 levels deep, call
`model_schema_tag`, assert it RETURNS SOME `DResult` (either `Ok` or a
propagated `Err` — the property under test is termination, not success;
wrap in a bounded-time test harness, e.g. run on a thread with a `join`
timeout, or simply assert completion within the test's own default timeout
since the fuel bound guarantees O(64) work). Fill in the remaining `IrType`
match arms so the match is EXHAUSTIVE (delete any `_ =>` if one was used as
a placeholder in 1A.1-3).
`cargo build -p sky_backend_rust` — confirm a compile-time exhaustiveness
error if you temporarily comment out one arm (proving the match really is
exhaustive, not accidentally catch-all). Restore the arm. Run all Stage-A
tests green. Commit — **Stage A complete, mergeable on its own.**

### Problem 1 — Stage B (emission + companion tag column, JSON body unchanged)

**Compile-atomicity note for 1B.1-1B.4 (mirrors Problem 2's own step-2.1
disclosure).** `choose_store` calls `SqliteStore::new(path, ttl).await` /
`PostgresStore::new(path, ttl).await` in the SAME FILE (`store.rs`) whose
constructors 1B.1-1B.3 change the signature of. Changing
`SqliteStore::new`'s signature in 1B.1 without updating `choose_store`'s
call site (which only happens in 1B.4) means `store.rs` — and therefore
the whole `runtime` crate — does NOT COMPILE in between, not merely
"has a failing test." This is the identical whole-crate-compilation
situation Problem 2's step 2.1 discloses for its own trait-method
addition ("This will not compile until EVERY impl has a body … fold into
one commit if the trait addition forces all four to land atomically —
reasonable given Rust's whole-crate compilation"). 1B.1-1B.4 below are
therefore ONE atomic commit: each numbered step still documents its own
write-test → confirm-fails → implement narrative (useful for code review
and for bisecting WHICH change introduced a bug), but do not run `cargo
test -p sky-runtime-rust` expecting a green whole-crate build, and do not
`git commit`, until 1B.4's threading step lands. Only 1B.4's own paragraph
below ends with an actual "Commit."

**1B.1 — `SqliteStore` schema-tag column: reject on mismatch.**
In `runtime/src/sky_runtime/live/store.rs`, add
`sqlite_store_rejects_a_row_written_by_a_different_schema_tag` and
`sqlite_store_accepts_a_row_written_by_the_same_schema_tag` to the existing
`#[cfg(feature = "db")] mod tests`. Both require `SqliteStore::new` to
accept a new `schema_tag: [u8; 32]` parameter — **confirm both fail to
compile** first (parameter doesn't exist yet). Add the parameter, the
`schema_tag` struct field, the `schema_tag TEXT NOT NULL` column on the
`CREATE TABLE IF NOT EXISTS`, and the reject-before-deserialize check in
`get()` (compare the row's `schema_tag` hex against `self.schema_tag`
BEFORE `serde_json::from_str`). The pre-existing
`sqlite_store_checkpoint_survives_restart` test's call sites need updating
to pass a `schema_tag` too (don't change its assertions) — but per the
compile-atomicity note above, `store.rs` as a whole does not compile again
until `choose_store` is updated in 1B.4, so defer running the FULL test
module until then. Do not commit yet.

**1B.2 — `PostgresStore` mirror.**
Same two tests, same implementation shape, over `PostgresStore` (gated on
`SKY_TEST_PG_URL`, matching the existing precedent). Still part of the
1B.1-1B.4 atomic commit — do not commit yet.

**1B.3 — `RedisStore` mirror, HASH-shaped.**
Same two tests (gated on `SKY_TEST_REDIS_URL`) over `RedisStore`, but the
implementation switches from a bare string value to a Redis HASH
(`HSET sky:sess:<sid> blob <json> tag <hex>` + one `EXPIRE`) so the tag and
blob share one TTL. Still part of the 1B.1-1B.4 atomic commit — do not
commit yet.

**1B.4 — Thread `schema_tag` through `choose_store`/`live_app`/
`live_app_routed`.**
Add the `schema_tag: [u8; 32]` parameter to `choose_store` (forwarded to
the three persistent constructors, ignored by `MemoryStore`, matching the
existing "single signature keeps the codegen call uniform" comment) and to
`live_app`/`live_app_routed`'s public signatures in `mod.rs`. This closes
the compile gap opened by 1B.1-1B.3: `store.rs` — and the whole `runtime`
crate — compiles again as of this step. Now run the FULL `store.rs` test
module (`cargo test -p sky-runtime-rust --features db`) and confirm the
1B.1-1B.3 tests, plus the updated
`sqlite_store_checkpoint_survives_restart`, all pass; run the full
`runtime` crate test suite (`cargo test -p sky-runtime-rust --features
live,db,redis_store`) to confirm no other regression. Commit —
**the single atomic commit covering 1B.1 through 1B.4.**

**1B.5 — Emit the tag from `sky_backend_rust`.**
In `crates/sky_backend_rust/src/emit_live.rs`, add a unit test (in this
crate's own test module, not the 433-fixture golden harness) asserting
that emitting a small synthetic `live_app` cfg produces a string
CONTAINING a `const SKY_LIVE_MODEL_SCHEMA_TAG: [u8; 32] = [...]` line and
that the emitted `live_app(...)` call passes that identifier as its new
final argument. **Confirm it fails** (the const doesn't exist in the
output yet). Wire `let schema_tag = model_schema_tag(ctx, model_ty)?;`
into `emit_live_app_inner`, right next to the existing #91/#94 gate calls
(`emit_live_app_inner` is already `DResult`-returning, so the `?` needs no
signature change to the enclosing function — see §1.3's fallibility note),
and emit the const + threaded argument for both the routed and non-routed
branches. Run green. This step is independent of the runtime crate — no
compile-atomicity concern here, `sky_backend_rust` is its own crate.
Commit.

**1B.6 — Golden re-baseline.**
Run the golden harness across the 5 `Live.app`/`Live.appRouted` fixtures
under `tests/golden/` (§0 — `live_let_bound_routes`,
`live_routed_empty_routes_wrong_ctor_notfound`, `live_param_routes`,
`live_routed_empty_routes_int_notfound`,
`live_routed_empty_routes_ok`; `tui_entry_case_taskrun` is excluded —
its only match is inside a comment). Confirm exactly those 5 now differ
(new const line + new call argument) and the harness's accept/bless step
is used to record the new expected output for those 5 only. Re-run the
FULL 433-fixture suite; confirm the other 428 are byte-identical to before
this step (proof the change is scoped to Live cfgs, per
`emit_live_app_inner`'s own gate). Commit — **Stage B complete.**

### Problem 1 — Stage C (wire-format swap, runtime-only)

**1C.1 — Add `bincode`.**
Add `bincode = { version = "1", optional = true }` to `runtime/Cargo.toml`,
listed in the `db` and `redis_store` feature arrays (alongside `sqlx`/
`serde_json`, matching the existing pattern). Add a matching
`pub const BINCODE: CrateSpec = CrateSpec { name: "bincode", version: "1"
};` to `crates/sky_backend_rust/src/crate_specs.rs`, and reference it from
whichever `project.rs` manifest-surgery function already emits the `sqlx =
{...}` line (~`project.rs:592`) so a GENERATED project's own `Cargo.toml`
also declares `bincode` under the same feature condition. `cargo build -p
sky-runtime-rust --features db` — confirm it still builds (no test yet;
this step is pure dependency wiring). Commit.

**1C.2 — `SqliteStore` bincode round-trip.**
Add `sqlite_store_new_format_round_trips_model_through_bincode`: `set()` a
session, `get()` it back through a NEW `SqliteStore`, assert equality.
**Confirm it fails** if `set`/`get` still use `serde_json` (the test would
actually pass under the OLD format too, so instead assert something format-
SPECIFIC: that the raw `blob` column value, read directly via a raw SQL
query, is valid base64 whose decoded length is `32 + bincode::serialized_
size(&model)` — a property ONLY the new format satisfies). Implement:
`set()` writes `base64_encode(schema_tag_bytes ++ bincode::serialize(&model)
?)` into the SAME `blob` column (no schema change); `get()` decodes,
splits at byte 32, compares the tag (now inline, not the Stage-B companion
column — stop reading that column), then `bincode::deserialize`s the rest.
Run green. Commit.

**1C.3 — Old-row fail-soft test.**
Add `sqlite_store_old_json_row_is_rejected_not_crashed`: seed a raw
pre-Stage-C JSON row directly via SQL (bypassing `set()`), call `get()`,
assert `None` and, critically, assert the call does NOT panic (wrap in
`std::panic::catch_unwind` or simply rely on the test harness surfacing any
panic as a failure — either way, make the "no panic" assertion explicit in
the test's own doc comment). Run — should already pass given `get()`'s
existing `?`-propagated `None` shape, but write it FIRST per TDD discipline
and confirm it fails if you temporarily swap in an `.unwrap()` to prove the
test actually exercises the failure path. Restore the `?`. Commit.

**1C.4 — `PostgresStore` + `RedisStore` mirrors.**
Same two tests (`SKY_TEST_PG_URL`/`SKY_TEST_REDIS_URL`-gated) over
Postgres and Redis; Redis's HASH `tag` field is retired (stop writing it —
the tag now lives inside the `blob` field's leading 32 bytes, same as
Sqlite/Postgres). Commit — **Stage C complete. Problem 1 fully closed.**

### Problem 2 — `SessionStore.live_sessions()` + the reload frame

**2.1 — `MemoryStore::live_sessions()`.**
In `store.rs`'s existing `#[cfg(test)] mod tests`, add
`memory_store_live_sessions_lists_only_locally_cached_handles`: fresh
`MemoryStore`, assert `live_sessions()` is empty; `set()` two sids, assert
length 2; `delete()` one, assert length 1. **Confirm it fails to compile**
(method doesn't exist on the trait yet). Add `live_sessions` to the
`SessionStore` trait (no default body) and implement it on `MemoryStore`.
This will not compile until EVERY impl has a body — proceed directly to
2.2-2.4 before this compiles at all (or stub the other three with
`todo!()`-free placeholder bodies identical to `MemoryStore`'s, to get a
green compile sooner, then replace each with its own test in 2.2-2.4). Run
2.1's test green. Commit (or fold into 2.4 as one commit if the trait
addition forces all four to land atomically — reasonable given Rust's
whole-crate compilation).

**2.2 — `SqliteStore::live_sessions()`.**
Add `sqlite_store_live_sessions_excludes_cold_rows` (`feature = "db"`):
insert a row directly via raw SQL (simulating a cross-replica session, no
`mem_cache` entry), separately `set()` one normal session; assert
`live_sessions()` returns exactly the `set()`-created one. Implement the
identical-to-`MemoryStore` body over `mem_cache`. Run green. Commit.

**2.3 — `PostgresStore::live_sessions()`.**
Same test (`SKY_TEST_PG_URL`-gated), same body. Commit.

**2.4 — `RedisStore::live_sessions()`.**
Same test (`SKY_TEST_REDIS_URL`-gated), same body. Commit — trait addition
fully implemented across all four backends.

**2.5 — `push_reload_to_live_sessions` helper.**
In `mod.rs`, add `push_reload_to_live_sessions_sends_one_frame_per_live_
session`: build two fake `SessionEntry`s with bound `sse_tx` channels (one
`Some`, one `None`) inside a fake `SessionStore` (or reuse `MemoryStore`
directly, now that 2.1 gives it a working `live_sessions()`), call the new
helper, assert the `Some`-channel receiver gets exactly one
`SsePatch(sse::frame("reload", "{}"))` and the test doesn't panic on the
`None` one. **Confirm it fails to compile first** (helper doesn't exist).
Implement `push_reload_to_live_sessions` per §2.4. Run green. Commit.

**2.6 — H23 production gate + wiring into `live_shutdown_signal`.**
Add `live_shutdown_signal_skips_the_reload_push_in_production`: with
`ENV=production` set (via the test's own env-var scoping, restored after),
assert a bound `sse_tx` channel receives NOTHING from the reload path
within a bounded wait; with `ENV` unset/`dev`, assert it DOES. **Confirm
it fails** if `live_shutdown_signal` doesn't yet call the gated push (or
if the gate is missing/inverted). Thread `store: Arc<dyn
SessionStore<Model, Msg>>` through `live_shutdown_signal`'s signature and
its ONE call site (`.with_graceful_shutdown(live_shutdown_signal(store
.clone()))`, both `live_app` and `live_app_routed`), add the `if !
production_from_env() { push_reload_to_live_sessions(&store).await; }`
call right after `observability::mark_draining()`. Run green. Run the full
`runtime` crate `live` feature test suite for regressions. Commit —
**Problem 2 fully closed.**

### Problem 3 — SIGTERM forwarder

**3.1 — `sky_watch::signal::install_sigterm_forwarder`.**
Add `signal-hook = "0.3"` to `crates/sky_watch/Cargo.toml`. Create
`crates/sky_watch/src/signal.rs` with the function stub returning
`Ok(std::thread::spawn(|| {}))` (never actually listens yet). Add
`sigterm_forwarder_invokes_callback_on_sigterm` (unix-gated): call
`install_sigterm_forwarder(move || sent.store(true, Relaxed))`, shell out
`Command::new("kill").arg("-TERM").arg(std::process::id().to_string())
.status()`, poll (bounded, e.g. 2 s) for `sent.load(Relaxed)`. **Confirm it
fails** (the stub never sets the flag). Implement the real body per §3.3.
Run green. Add the negative control
`sigterm_forwarder_never_fires_without_a_signal` (assert `sent` stays
`false` after a bounded wait with no signal sent). Run green. Declare `pub
mod signal;` in `crates/sky_watch/src/lib.rs`. Commit — **mergeable on its
own, independent of everything else in this document.**

**3.2 — Wire into `run_inner`, gated on `external_stop.is_none()`.**
Add `watch_shuts_down_the_supervised_child_on_sigterm_to_only_the_skyc_
process` to `crates/skyc/tests/watch_integration.rs` (`SKY_E2E=1`,
unix-gated, mirroring the existing E2E test style and the Bug-3 regression
test's `/proc/<pid>/environ`-based child-liveness check): start a real
`ipe watch` subprocess (i.e. via `run()`, `external_stop = None`) against a
`Sky.Http.Server` fixture, confirm the supervised child is serving, `kill
-TERM <skyc-subprocess-pid>` (the PID only — explicitly NOT the process
group, to reproduce the systemd-style gap), assert BOTH the `skyc`
subprocess exits within a bounded wait AND the supervised child's PID is
no longer live. **Confirm it fails** pre-fix (the `skyc` subprocess dies
immediately via default SIGTERM disposition, but the supervised child is
still running — the test's second assertion fails). Add the forwarder
call to `run_inner` per §3.3's REVISED design — `if external_stop.is_none()
{ ... sky_watch::install_sigterm_forwarder(...) ... }`, inserted right
after `evt_tx`/`evt_rx` are created and BEFORE the existing
`external_stop`-consuming block moves it out. Run the new test green. Run
it again alongside the three existing `SKY_E2E=1` watch scenarios
(sequentially, per the project's own "never overlapping E2E watch runs
sharing a port" convention already established in §14.9) to confirm no
interference. Commit.

**3.3 — `spawn()` must never install the forwarder (regression test for
finding 6).**
Add `spawn_never_installs_a_sigterm_forwarder` (unix-gated, in
`crates/skyc/src/watch.rs`'s own `#[cfg(test)] mod tests` or
`crates/sky_watch`'s integration tests — wherever `sky_watch::spawn` is
already exercised): call `sky_watch::spawn(opts)` in-process against a
throwaway fixture, then deliver SIGTERM to the TEST PROCESS's OWN pid
(`std::process::id()` — the host `spawn()` is running inside, exactly the
scenario an embedder would hit), then assert the TEST PROCESS is STILL
ALIVE after a bounded wait (e.g. it can still execute further test
assertions — an actually-terminated process obviously can't) AND that the
spawned watch loop is still running (only `WatchHandle::stop()`, called
explicitly afterward, shuts it down). Without the `external_stop.is_none()`
gate from 3.2, this test would either hang (if the test harness's own
SIGTERM handling races with the newly-installed forwarder) or demonstrate
the host process's SIGTERM disposition has silently changed — **confirm
it would have failed against the UNCONDITIONAL-install design from an
earlier drafting pass** (temporarily remove the `if external_stop.is_none()`
guard to see the failure, then restore it). Run green with the guard in
place. Commit.

**3.4 — Double-SIGTERM proof test (corrects decision 6's claim, finding
7).**
Add `double_sigterm_after_forwarder_consumed_is_silently_absorbed_use_
sigkill` (`SKY_E2E=1`, unix-gated) per §3.5's test-inventory description:
start a real `ipe watch` subprocess whose supervised child ignores SIGTERM
(so `supervisor.shutdown`'s bounded grace window is guaranteed to run its
full course), send SIGTERM once, then send a SECOND SIGTERM partway
through the teardown's bounded wait (after the forwarder thread has
already consumed the first signal and returned). Measure and assert the
`skyc` process's total wall-clock exit time is NOT measurably shorter than
a comparable single-SIGTERM run — i.e. the second signal has no observable
escalating effect. Document the REAL observed result in the test's own doc
comment regardless of which way it comes out (the point of this test is to
replace an unverified claim with a measured fact): if the second SIGTERM
truly has zero effect, the comment states plainly "a stuck `ipe watch`
needs SIGKILL — do not rely on a second SIGTERM." Commit — **Problem 3
fully closed.**

---

## Research pointer (added 2026-07-13, per user)

**Problem 3 (SIGTERM process-group propagation) — mine the reference first.**
Before implementing, study how the reference Sky backend+runtime (`../sky`)
handles **killing the console sub-process** — it already solved child-process
teardown (spawn the `/_sky/console` mini-app, propagate termination, reap it
without orphaning). Grep `../sky/runtime-rust` + `../sky/src` for the console
spawn/kill path (`SpawnSkyConsole` / `MountSubApp` analog, process-group /
`setsid` / SIGTERM-forward logic). Mirror its proven teardown; combine with
this doc's §3.4 revision (forwarder gated to `run()`-only, never `spawn()`;
second SIGTERM → SIGKILL).
