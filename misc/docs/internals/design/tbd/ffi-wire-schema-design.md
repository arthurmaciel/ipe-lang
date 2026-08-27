# Single-owner `ipe-ffi-wire` schema

Status: design proposal, no implementation yet. Names and field maps below
describe the intended crate; nothing here is wired in today.

## 1. The problem

The FFI inspection document — the JSON the `ipe-ffi-inspector` binary emits and
the `ipe_ffi` crate decodes — is one wire contract that lives in **two**
hand-maintained struct families in two crates:

- **Producer side**, `tools/ipe-ffi-inspector/src/main.rs` (a single
  21,472-line file): `#[derive(Serialize)]` structs `PkgInfo`, `Function`
  (24 emitted fields), `Param`, `Generic`, `Call`, `Receiver`, `TransitiveDep`,
  `ForeignTypeDecl`, `TypeMember`, `TypeVariantDecl`, plus the manual
  `Serialize for TypeRef`.
- **Consumer side**, `src/compiler/ffi/src/{pkginfo,transparency,call}.rs`:
  `#[derive(Deserialize)]` twins `WirePkgInfo`, `WireFunction` (33 fields —
  more than the producer, because the flag soup the producer omits when default
  is spelled out here), `WireParam`, `WireGeneric`, `WireStructField`,
  `WireEnumVariant`, `WireTransitiveDep`, `WireForeignType`, `WireTypeMember`,
  `WireTypeVariant`, `WireCall`, `WireReceiver`.

The two families are two halves of one contract that must agree
**field-for-field, camelCase-key-for-key**. But the contract is written down
nowhere; it is the *intersection* of two independently-edited struct sets. Add
`enumFieldCount` to the producer and forget the consumer, or rename a `#[serde(
rename = "…")]` key on one side only, and the field silently decodes to its
`#[serde(default)]` — a **dropped field on the most security-sensitive
subsystem in the compiler**. FFI is the blocking security gate (ADR 0040 / 0044
/ 0046); a silently-defaulted wire field there is exactly the failure mode the
whole parse-don't-validate decode boundary exists to prevent, reintroduced one
level up, at the wire itself.

Three properties make the drift real rather than theoretical:

- **No schema version.** Neither side carries a `schemaVersion` field. A stale
  inspector on `PATH` (the binary is resolved "beside the `ipe` binary or on
  PATH", `src/ipe-cli/src/ffi.rs`, so it can be an *older separately-built*
  build than the `ipe` that spawns it) produces JSON a newer consumer decodes
  with missing fields silently defaulted, and vice versa.
- **`default`-everywhere is load-bearing but blind.** Every consumer field is
  `#[serde(default)]` for forward compatibility (unknown keys ignored, absent
  keys defaulted). That is correct for *genuine* forward compat, but it is also
  precisely what turns a *drift* (a key that should be present and is not) into
  a silent zero/empty/false instead of a decode error.
- **The families already disagree in arity.** 33 vs 24 fields is not a bug —
  the producer omits defaults via `skip_serializing_if`, the consumer names
  them all — but it means no human can eyeball the two lists for agreement, and
  no test asserts it.

## 2. The decision: a shared `ipe-ffi-wire` types crate

Extract the wire contract into **one new library crate,
`src/compiler/ffi/wire/` (crate name `ipe_ffi_wire`)**, holding the serde types
that *are* the contract. Both sides depend on it:

- the **inspector** depends on `ipe_ffi_wire` and constructs its values, then
  `serde_json::to_string`s them;
- **`ipe_ffi`** depends on `ipe_ffi_wire`, `serde_json::from_str`s into them,
  then runs the existing validating `TryFrom<Wire…>` conversions into the
  domain layer (`FnInfo`, `PkgInfo`, `FnShape`, `ForeignTypeCatalog`, `Call`).

The wire crate owns **only** the serde-facing types and their camelCase key
mapping. It owns **no domain logic and no validation** — those stay in
`ipe_ffi`, which is the security boundary. The wire crate is a dumb,
serde-derived data schema; making it dumb is what lets the inspector (a
`unwrap`/`panic`-permissive CLI tool, per its `Cargo.toml` lint config) depend
on it without dragging the strict `ipe_ffi` gate into the tool, and without the
gate depending on the tool.

### 2.1 Crate-vs-generation: recommend the shared crate

Two shapes could give the contract a single owner. **Recommend the shared crate.**

**Option A — shared `ipe_ffi_wire` crate (recommended).** One serde struct set,
compiled into both binaries. The build graph already supports it:
`tools/ipe-ffi-inspector` is a workspace member (`Cargo.toml` `members`), so a
sibling library crate is an ordinary `path` dependency both sides pick up with
zero new tooling. Producer and consumer are then the *same Rust types* — a
field cannot exist on one side and not the other, and a `rename` is written
once. Drift at the type level becomes **unrepresentable**, the same way the
domain newtypes make an injection-bearing version unrepresentable past decode.
This mirrors ADR 0044's manifest decision verbatim: "the capability vocabulary
is re-exported from the compiler's kernel registry so the manifest's declared
set and the compiler's inferred set are the *same* type, never two drifting
string lists." The wire schema is the same move for the inspection document.

**Option B — generate the consumer from the producer's serde types (rejected).**
Emit the `Wire…` structs from the inspector's `Serialize` types via a build
script or a checked-in codegen step. Rejected: it adds a codegen toolchain and
a generated-file freshness gate for no gain a shared crate does not already
give; the "single source" it offers is *the inspector*, which is the untrusted,
lint-relaxed tool — the wrong crate to make canonical for a security boundary;
and a subprocess wire contract does not need code generation when both processes
are Rust in one workspace. Generation earns its keep across a *language*
boundary (the historical Haskell generator ↔ Rust inspector split that some of
the in-code comments still reference); it does not here, where both ends are
Rust.

Serialize and Deserialize on **the same struct** is the mechanism that makes
Option A a *single* owner rather than a shared-but-still-two-derives crate: each
wire type derives **both** `Serialize` and `Deserialize`, the inspector uses the
former, `ipe_ffi` the latter, and the round-trip test (§3.3) exercises both on
one type so a `rename`/`skip` that breaks one direction fails a test.

### 2.2 The subprocess-boundary versioning story

The contract crosses a **process** boundary (spawn inspector, read its stdout),
not a linked-API boundary — the inspector binary may be an *older or newer*
separately-built artifact than the `ipe` that spawns it. A shared crate makes
the two agree *when built together*; it does **not** by itself catch a
version-skewed binary found on `PATH`. Close that with one field the shared
crate owns:

- add `schemaVersion: u32` to `PkgInfo`/`WirePkgInfo` (a single `const
  WIRE_SCHEMA_VERSION` in `ipe_ffi_wire`, emitted by the inspector, checked by
  `ipe_ffi` at decode);
- on mismatch, fail **loudly** through the existing `Diagnostic::WireMalformed`
  channel ("inspector schema vN, this ipe expects vM — rebuild/reinstall
  `ipe-ffi-inspector`"), never a silent default.

This upgrades the current *silent* skew failure into a *named, fail-closed*
one — consistent with the FFI decode boundary's whole posture and with ADR
0046's "the untrusted build can never forge a clean result" reasoning: a
version-skewed inspector can no longer forge a *complete* document.

## 3. The migration

### 3.1 Field-by-field map (behaviour-preserving)

The move is mechanical: for each producer struct, its consumer twin becomes the
same `ipe_ffi_wire` type. Concretely (producer → consumer twin → unified type):

| Inspector (`main.rs`) | Consumer (`ffi/src`) | Unified `ipe_ffi_wire` type |
| --- | --- | --- |
| `PkgInfo` | `WirePkgInfo` (`pkginfo.rs`) | `WirePkgInfo` |
| `Function` (24) | `WireFunction` (33) | `WireFunction` |
| `Param` | `WireParam` (`pkginfo.rs`) | `WireParam` |
| `Generic` | `WireGeneric` (`pkginfo.rs`) | `WireGeneric` |
| `Call` | `WireCall` (`call.rs`) | `WireCall` |
| `Receiver` | `WireReceiver` (`call.rs`) | `WireReceiver` |
| `TransitiveDep` | `WireTransitiveDep` (`pkginfo.rs`) | `WireTransitiveDep` |
| `TypeRef` (manual `Serialize`) | (decoded in `call.rs`) | `WireTypeRef` (both derives) |
| `ForeignTypeDecl` | `WireForeignType` (`transparency.rs`) | `WireForeignType` |
| `TypeMember` | `WireTypeMember` (`transparency.rs`) | `WireTypeMember` |
| `TypeVariantDecl` | `WireTypeVariant` (`transparency.rs`) | `WireTypeVariant` |
| `Function.structFields` | `WireStructField` (`pkginfo.rs`) | `WireStructField` |
| `Function.enumVariants` | `WireEnumVariant` (`pkginfo.rs`) | `WireEnumVariant` |

The two arity numbers reconcile because the unified type names **every** field
and lets `skip_serializing_if` govern emission: the producer's 24 emitted fields
are the non-default subset of the 33 the consumer names; a single struct with
`#[serde(default, skip_serializing_if = …)]` on each optional field emits
exactly the producer's shape and decodes exactly the consumer's shape. The
manual `Serialize for TypeRef` (a tagged-union-by-key encoding) moves into the
wire crate beside a matching `Deserialize`, so the union's key discipline
(`param`/`prim`/`ctor`/`closure`/`serdeValue`/`serdeValueRef`) is written once.

Fields that are **producer-only and never read** (`Function.exported`,
`Generic.mono_resolved` which is `#[serde(skip)]`) stay producer-internal: they
either move to the wire type with `skip_serializing_if`/`skip` preserved, or —
cleaner — stay as inspector-local working fields that are *projected into* the
wire type at emit. The migration must audit each such field and record which
bucket it lands in, because an "unused" field is exactly where a silent
contract gap hides.

### 3.2 How a schema change is made afterwards — in one place

After the extraction, adding a wire field is: edit the one `ipe_ffi_wire`
struct, add the `skip_serializing_if` if optional, set the inspector to populate
it, and consume it in the `TryFrom` in `ipe_ffi`. The **compiler** now forces
the field to exist on both sides — a producer that populates a field the type
does not declare fails to compile; a consumer that reads one that is not
declared fails to compile. The round-trip test (next) closes the last gap: a
field present in the type but *not actually emitted* by the inspector (or not
consumed) is caught by asserting the decoded value.

### 3.3 The round-trip guard

Add `src/compiler/ffi/tests/wire_round_trip_seal.rs` (a SEAL test, matching the
existing `*_seal.rs` family in `ffi/tests`): construct a maximal
`ipe_ffi_wire::WirePkgInfo` populating **every** field with a distinctive
non-default value (every flag `true`, every optional string non-empty, one of
each `WireTypeRef` variant, a `Generic`, a `ForeignTypeDecl`, a
`TransitiveDep`), `serde_json::to_string` it (the inspector's exact path),
`serde_json::from_str` it back, and assert structural equality field-by-field.
This guards **both** serde directions on one value: a `rename` that disagrees
between `Serialize` and `Deserialize`, a `skip_serializing_if` that drops a
non-default field, or a field the round trip cannot preserve, all fail here.

A second, thinner guard asserts the **key set**: serialize the maximal value to
`serde_json::Value` and assert the emitted key set equals a checked-in expected
set. This catches a *renamed* key even when both directions agree with each
other but disagree with the historical wire (the golden-JSON fixtures the
`ipe add` E2E path and the `*_seal.rs` decode fixtures already depend on).
Because the schema now has a version (§2.2), the key-set test also pins
`WIRE_SCHEMA_VERSION`, so bumping the schema is a deliberate, reviewed edit to
the expected set — not a silent drift.

## 4. How this sequences with the FFI implementation issues

The wire schema is **foundational** to the three open FFI-boundary issues: each
of them edits, extends, or reasons about the inspection document, and each is
strictly easier and safer on **one** schema than on the twins. The wire crate
should therefore land **under** them — before, as their base — not alongside.

- **PR 396 — implement the consolidated FFI-to-Rust boundary spec.** This is
  the umbrella "one boundary" work; a boundary with a *doubly-owned* wire
  contract is not consolidated. `ipe_ffi_wire` is the literal artifact that
  makes "one boundary" true at the wire. Land the wire crate first; issue 396
  then extends *one* schema.
- **Issue 333 — JS interop: one typed boundary discipline (transports, reserved
  types, package sharing).** The same "one typed boundary, no drifting twin
  string lists" discipline issue 333 wants for the JS/transport boundary is
  exactly what `ipe_ffi_wire` establishes for the native-FFI boundary. It is
  the reference implementation of the discipline and should precede issue 333
  so issue 333 inherits the pattern (a shared wire-types crate) rather than
  reinventing it.
- **Issue 651 — deep FFI binding-generation for translated `[rust.dependencies]`
  (wire into `ipe install`).** This *generates* bindings by driving the
  inspector across a converted project's crate set; it consumes the inspection
  document heavily and will want to add fields (per-crate provenance, install
  wiring). Adding those to a single schema with a version and a round-trip
  guard is safe; adding them to the twins is the drift hazard multiplied by the
  number of crates a converted project pulls in. Issue 651 must build on the
  schema, not the twins.

### 4.1 R2 — the 21k-line inspector file — as a compounding factor

`tools/ipe-ffi-inspector/src/main.rs` is a single 21,472-line file. It compounds
the twin problem two ways: the producer structs are buried at the top of a file
no reviewer reads whole, so producer-side drift is *invisible in review*; and
any of issue 396 / 333 / 651 touching the producer must navigate the monolith.
The disentanglement note's item **R2** (carve the inspector) and item **SND2**
(give the wire schema one owner) are the two halves here.

Carving the inspector is **not a prerequisite** for the schema landing:
extracting the *struct definitions* (a few hundred lines at the file head) into
`ipe_ffi_wire` is a localized cut that does not require carving the 21k-line
*walker*. In fact the schema landing **helps** the carve — pulling the schema
types out of `main.rs` removes the producer's most drift-sensitive lines from
the monolith first, shrinking what R2 must later split and giving the walker a
clean typed target to build into. So: schema first, inspector carve (R2) second,
and the first landing makes the second smaller.

## 5. The two landings

**First landing — extract `ipe_ffi_wire`, point both sides at it, add the
guards (mechanical, behaviour-preserving).** New crate `src/compiler/ffi/wire`;
move the serde types (both derives + the `TypeRef` union encoding) into it; add
`schemaVersion` + `WIRE_SCHEMA_VERSION` + the loud-mismatch decode check;
replace the inspector's local structs and `ipe_ffi`'s `Wire…` structs with
`use ipe_ffi_wire::…`; add `wire_round_trip_seal.rs` + the key-set guard. No
behaviour change: the emitted JSON is byte-identical (modulo the new
`schemaVersion` key), the golden fixtures re-bless once, the domain `TryFrom`s
are untouched. Independently landable and green.

**Second landing (optional) — carve the 21k-line inspector (R2).** With the
schema already out of `main.rs`, split the remaining walker into modules
(rustdoc traversal, accessor synthesis, cross-crate index, argv/CLI, emit).
Purely a producer-internal refactor against the now-stable `ipe_ffi_wire`
target; the wire contract does not move. Independently landable and green;
sequence *after* the first landing and, ideally, before issue 396 / 651 add
producer code so they land into the carved structure.

Each landing is independently landable, independently green, and leaves the wire
contract owned in exactly one crate.

## Affected issues

- **PR 396** — foundation. Consolidated FFI boundary spec; `ipe_ffi_wire` is
  the artifact that makes "one boundary" true at the wire. Land under it.
- **Issue 333** — pattern source. JS-interop "one typed boundary, no drifting
  twins" is the same discipline; issue 333 should inherit the shared-wire-crate
  pattern.
- **Issue 651** — heavy consumer. Deep FFI binding-generation drives the
  inspector across many crates; must extend the single schema, not the twins.
- **Issue 292** — adjacent, non-conflicting. Tier-2 per-platform sandbox matrix
  reads the *decoded domain* `PkgInfo`, not the raw wire; a single wire schema
  makes the document it confines against unambiguous, but no code overlap.
- **Issue 661** — adjacent. `Ipe.Cache` codegen SEAL violation is a domain/emit
  bug, not a wire-schema one; unaffected, but both live under the FFI/codegen
  SEAL discipline this schema strengthens.
- **Issue 641** — unaffected. `Db.open` stdlib gap; no FFI-wire interaction.
- **Issues 665 / 666 / 672 / 667-family** — unaffected. Stdlib/kernel-lowering
  (Task retry, Html raw/render, Random) bugs; no inspection-document interaction.
- **Issues 671 / 674** — adjacent, non-conflicting. Seccomp baseline + sandbox
  regression tests are runtime-jail (ADR 0040/0046), downstream of the decoded
  document; no wire-schema overlap.
- **Issues 663 / 664 / 397 / 541** — unaffected (deferred). Ipe.Codec,
  Ipe.Analytics, Ipe.Parser, is_json partition test; stdlib, no FFI-wire
  interaction.
- **Issue 470** — adjacent. Hosted ipe-index repo (ADR 0044) carries capability
  sets, not inspection documents; no overlap, but shares the "one typed
  contract" ethos.
- **Issue 240** — orthogonal. Git-history artifact pruning; process-only.
- **Issues 561 / 294** — orthogonal (deferred). Diagnostics quality bar +
  readability audit; the new schema-mismatch diagnostic should meet the 561 bar
  and the new crate should meet the 294 naming bar, but no code conflict.
- **Issues 473 / 317 / 284 / 139** — unaffected (deferred). Playground, WASM
  backend, `ipe lint`; no FFI-inspection-wire interaction.
