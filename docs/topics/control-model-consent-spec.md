# Control-model consent — implementation spec (issue #2460)

Make a package's **control model** (TEA / Server / Direct) a compiler-derived,
build-enforced consent signal on the SAME fail-closed footing the capability
axes already have. Disclosure (`ipe audit`) landed in PR #2484; this spec covers
the remaining #2460 scope: consumer-side **acceptance + build refusal** and the
**diagnostic** when an entry runs an unaccepted control model.

## 1. Ground-truth anchors (verified against HEAD `5fd0d6198`)

### Control model — the derived value (SSOT, already exists)
- `ControlModel` enum + `word()`: `src/ipe-cli/src/delivery.rs:118-139`
  (`Tea` / `Server` / `Direct`).
- Projection from the compiler-pinned shape: `Shape::control_model`
  `src/ipe-cli/src/delivery.rs:86-92`; `Shape::from_main`
  `src/ipe-cli/src/delivery.rs:99-108`.
- The compiler's own pin: `ipe_canon::shape_source::classify_main_shape`
  `src/compiler/canon/src/shape_source.rs:81`.

### Control-model disclosure (already exists — PR #2484)
- `ControlModelDisclosure` (`Entry(ControlModel)` | `NotApplicable`, deliberately
  NO `unknown` variant): `src/ipe-cli/src/audit.rs:175-192`.
- `derive_disclosure` (fail-closed derivation; unread/unparseable runnable entry
  → `reject`; `main`-less module → `NotApplicable`):
  `src/ipe-cli/src/audit.rs:1076-1142`.
- Disclosed in JSON verdict (`controlModel`) at `src/ipe-cli/src/audit.rs:411,424`
  and human summary `disclosure_summary` `src/ipe-cli/src/audit.rs:1146-1157`.
- Audit orchestration: `audit_gate` `src/ipe-cli/src/audit.rs:341` calls
  `capability_consistency` (L354) then `derive_disclosure` (L360).

### Capability consent — the fail-closed mechanism to MIRROR
- Manifest schema `capabilities = { declares = […], accepts = […] }`:
  parse `read_capabilities_record` `src/ipe-cli/src/package_manifest.rs:374-395`;
  fields `ProjectManifest.capabilities` (declares) + `.capabilities_accept`
  (accepts) `src/ipe-cli/src/project.rs:74-91`; render `render_capabilities`
  `src/ipe-cli/src/package_manifest.rs:1432-1449`.
- Self-truth check (declared vs inferred): `capability_consistency`
  `src/ipe-cli/src/audit.rs:1000`.
- **Consumer build gate** (the shape to mirror): `web_consent::gate`
  `src/ipe-cli/src/web_consent.rs:105-142` and `native_ffi_consent::gate`
  `src/ipe-cli/src/native_ffi_consent.rs:136-167` — each takes
  `(inferred, granted=capabilities_accept, provenance)` and returns
  `Err(refusal)` for an inferred-but-ungranted axis. Wired into the build/run
  path at `src/ipe-cli/src/driver/commands.rs:606` (`gate_web_consent`, def L2794)
  and `:611` (`gate_native_ffi_consent`, def ~L2850), and again in `watch` at
  `:1903`. `resolve_for_run` supplies `resolved.inferred`.
- Error channel is typed: `CliError` (`refusal(..)` builds a `CliError`), never a
  bare `String`.

### Gap this spec closes
`ControlModel` is DISCLOSED but never CONSENTED. There is no manifest field for a
consumer to accept a control model and no build gate that refuses an unaccepted
one. `capabilities_accept` covers effect axes only.

## 2. Derivation (unchanged — reuse the SSOT)

Control model = `Shape::from_main(classify_main_shape(entry)).control_model()`.
No re-parse of a string, no second inspection of `main`. The consent gate reads
the SAME derived `ControlModel` the disclosure reads. A runnable entry whose
model cannot be derived is ALREADY a fail-closed audit rejection
(`derive_disclosure`); the gate never sees an "unknown".

## 3. Manifest surface — the accept field

Extend the `capabilities` record with an OPTIONAL control-model accept list,
parsed by the same closed-vocabulary discipline as `declares`/`accepts`:

```
capabilities =
    { declares = [ Network ]
    , accepts = [ ]
    , acceptsControl = [ Direct ]   -- consumer consents to a Direct entry
    }
```

- New field on `ManifestFields`/`ProjectManifest`:
  `control_models_accept: BTreeSet<ControlModel>` (typed, closed set — parse
  through a `ControlModel::from_word`-style reader that REJECTS any token outside
  `{ tea, server, direct }`; unknown token → `CliError` reject, mirroring
  `reject_unknown_capability`).
- `read_capabilities_record` gains an `"acceptsControl"` arm; the "expected
  `declares` or `accepts`" message extends to name `acceptsControl`.
- `render_capabilities` renders the field when non-empty (round-trips for
  `migrate`/`init`; empty set omitted, so existing manifests are unchanged).
- `ControlModel` needs `Ord`/`PartialOrd` for `BTreeSet` (add derive in
  `delivery.rs`) and a `from_word` inverse of `word()`.

Default (empty `acceptsControl`) is the STRICT default — see §4.

## 4. The fail-closed rule (the core of #2460)

New module `src/ipe-cli/src/control_model_consent.rs`, sibling of
`web_consent`/`native_ffi_consent`, same signature shape:

```rust
pub fn gate(
    derived: ControlModel,                 // the compiler-pinned entry model
    accepted: &BTreeSet<ControlModel>,     // manifest capabilities.acceptsControl
    entry_module: &str,                    // for the diagnostic's provenance
) -> Result<(), CliError>
```

Fail-closed rule — **which models require explicit acceptance**:
- `Tea` and `Server` are the managed models: the runtime drives the loop, effects
  flow only through the capability axes already gated. They are the LOW-power
  default and are accepted implicitly (an empty `acceptsControl` admits them).
- `Direct` (`Shape::Script`, a plain `Task Error ()`) is the ELEVATED model: the
  program drives itself to completion outside the managed loop. It MUST appear in
  `acceptsControl` or the build is REFUSED.

Rationale (Security #1, fail-closed by construction): "absent proof the input is
safe, take the conservative branch." A consumer pulling a dependency that has
silently become a self-driving `Direct` program is exactly the disclosure the
issue targets ("a dependency crosses into Direct state"). The elevated model is
the one that must be affirmatively consented; the managed models are the safe
default. This keeps every EXISTING managed package building unchanged (no empty
`acceptsControl` breaks) while making the elevated transition a hard stop.

`gate` returns `Ok(())` when `derived` is managed (`Tea`/`Server`) OR when
`accepted.contains(&derived)`. Otherwise `Err(refusal(...))`.

Defence in depth (mirrors the capability gate's two boundaries): enforce at
- (a) **build/run boundary** — `gate_control_model_consent` called alongside
  `gate_web_consent` at `commands.rs:606`-region and the `watch` site `:1903`,
  deriving the model from the resolved entry;
- (b) **audit boundary** — `audit_gate` (after `derive_disclosure`) runs the same
  `gate` against the package's OWN `control_models_accept`, so `ipe audit` refuses
  a package whose disclosed elevated model its own manifest does not accept. Two
  independent gates, one shared derivation.

## 5. The diagnostic

`refusal(derived, entry_module)` builds a typed `CliError` (a
`CliError::UsageOwned`/dedicated variant matching how `web_consent::refusal`
builds one — reuse the existing refusal-frame helper so the phrasing is one
SSOT). Message teaches, names the model, the module, and the fix:

> `entry` runs the **Direct** control model — a self-driving `Task Error ()`
> program that runs to completion outside the managed update/view loop. A
> consumer must consent to this elevated control model. Add `Direct` to
> `capabilities.acceptsControl` in `package.ipe` to accept it, or switch the
> entry to a managed (`tea`/`server`) shape.

When a dependency (not the app's own entry) introduces the Direct model, the
provenance names the disclosing module (same `entry_module` mechanism the web/ffi
gates use to name the crossing site).

## 6. TDD steps — refusal tests FIRST

Order: red (refusal) → green (gate) → wire → disclosure round-trip.

1. **`delivery.rs`**: `control_model_from_word_round_trips` +
   `from_word_rejects_unknown` (closed vocabulary). RED first.
2. **`package_manifest.rs`**:
   - `reject_unknown_control_model` — `acceptsControl = [ Telepathy ]` → rejected
     (mirror `reject_unknown_capability`). **REFUSAL, up front.**
   - `reads_accepts_control` — `acceptsControl = [ Direct ]` parses to the typed
     set.
   - `render_round_trips_accepts_control` — non-empty renders + re-parses; empty
     omitted (existing manifests unchanged).
3. **`control_model_consent.rs`** (the fail-closed core):
   - `direct_entry_without_accept_is_refused` — `gate(Direct, &{}, "Main")` →
     `Err`. **THE REFUSAL a regression/attacker walks in on.**
   - `direct_entry_with_accept_proceeds` — `gate(Direct, &{Direct}, _)` → `Ok`.
   - `managed_models_need_no_accept` — `gate(Tea, &{}, _)` and
     `gate(Server, &{}, _)` → `Ok` (safe default admits managed).
   - `refusal_names_model_and_module` — message contains `Direct` and the module,
     and is > 40 chars (a lesson, not a slap).
4. **audit**: `audit_refuses_unaccepted_direct_package` — a package whose entry is
   `Direct` with empty `acceptsControl` → audit rejects (boundary (b)).
5. **build wiring**: an integration test (or `commands` unit) proving a Direct
   entry app without `acceptsControl` fails the build gate before emit, and one
   proving acceptance lets it through.

Every acceptance path fails closed at ipe-time (the gate runs before emit/cargo),
never open at cargo-time — the SEAL holds because refusal precedes emission.

## 7. Non-goals (explicit)
- LSP-hover / `ipe doc` control-model surfaces (spec'd for a follow-up lane in
  PR #2484's body) are out of scope here.
- No change to the `ControlModel` derivation or the disclosure surface — both
  landed and are reused verbatim.
