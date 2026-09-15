# Control-model consent — implementation spec (issue #2460)

Make a package's **control model** (TEA / Server / Direct) a compiler-derived,
audit-enforced consent signal on the SAME fail-closed footing the capability
axes already have. Disclosure (`ipe audit`) landed in PR #2484; this spec covers
the remaining #2460 scope: consumer-side **acceptance + audit refusal** and the
**diagnostic** when a package's entry runs an unaccepted control model.

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

Fail-closed rule — **`accept`-style, opt-in-authoritative** (mirrors the
capability `accept` axis, not the coarse `declares`/crossing gates):
- An **empty** `acceptsControl` leaves the control model *unconstrained*: the
  model is disclosed but not gated. A clean package — including a legitimate
  self-driving `Direct` script (`main : Task ()`) — certifies unchanged. This is
  the decisive property: `ipe audit` must keep certifying every existing clean
  package, and a `direct` program is a first-class, legitimate shape, not a
  defect.
- A **non-empty** `acceptsControl` is *authoritative*: the author has opted into
  control-model consent, so the program's actual derived model MUST appear in the
  set. A derived model absent from a non-empty set is a fail-closed refusal — the
  declared acceptance is stale (the entry's control model drifted out from under a
  pinned accept-set). This is the drift the issue targets, caught without turning
  every script into a refusal.

Rationale (Correctness #2 bounds Security #1 here): gating a package's OWN direct
entry unconditionally would refuse every legitimate script/CLI package and break
the "clean package certifies" contract — a Correctness regression bought for no
security gain, since an author running/publishing their own program is not a
supply-chain event. The `accept`-axis semantics (empty = not-in-use, non-empty =
must-cover-the-truth) is the fail-closed shape that fits: the refusal is real and
tested (a non-empty accept that omits the derived model), and the clean path stays
green.

`gate` returns `Ok(())` when `accepted` is empty OR contains `derived`; otherwise
`Err(refusal(...))`.

### Enforcement point — the audit boundary (scoping correction)

The consent is enforced at the **audit boundary**, in `audit_gate` as the LAST
check — after `derive_disclosure` AND after the native-bearing fail-closed checks
(binding regeneration, provenance, Tier-2), so it is the final gate on an
otherwise-certifiable package and never preempts a native-surface refusal: a
package whose runnable entry is disclosed as the elevated
`Direct` model, with an `acceptsControl` set that does not contain it, is a
fail-closed audit rejection (`IPE-S0004`). A managed model or a library
(`NotApplicable`) needs no acceptance and passes silently.

It is DELIBERATELY not enforced on the plain `ipe build`/`ipe run` path for an
app's OWN entry. A top-level `Script`/`Direct` program (a plain `Task Error ()`
CLI tool or a static-site generator) is a first-class, legitimate shape the
author chose for themselves — refusing to build it absent an `acceptsControl`
entry would break every existing script app and the examples sweep, sacrificing
Correctness (principle #2) for no security gain: an author running their own
program is not a supply-chain event. The consumer boundary the issue targets ("a
dependency crosses into Direct state") is the AUDIT of a package to be
consumed/published — which is exactly where the gate sits, on the same disclosure
the audit already surfaces. `ipe audit` is the point at which a package's
self-driving model is certified as safe-to-consume; the gate makes that
certification fail-closed.

`ipe audit` already runs `capability_consistency` (the capability half of the
same "declared truth == inferred truth" check) at this boundary; the
control-model gate is its sibling on the control-model axis, sharing the one
`derive_disclosure` SSOT so disclosure and enforcement can never disagree.

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

Order: red (refusal) → green (gate) → wire into `audit_gate` → disclosure
round-trip. The refusal tests (steps 2 and 4) are written before the gate is
enforced, so the fail-closed path is pinned first.

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
   - `a_nonempty_accept_omitting_the_derived_model_is_refused_naming_the_module` —
     `gate(Direct, &{Tea}, "Dep.Runner")` → `Err`. **THE REFUSAL a control-model
     drift walks in on.**
   - `a_nonempty_accept_covering_the_derived_model_proceeds` —
     `gate(Direct, &{Direct}, _)` and a covering superset → `Ok`.
   - `an_empty_accept_set_leaves_every_model_unconstrained` — `gate(_, &{}, _)` →
     `Ok` for Tea/Server/Direct (clean packages certify).
   - `a_managed_model_omitted_from_a_nonempty_accept_is_also_refused` — the rule is
     uniform once opted in.
   - `the_refusal_is_a_lesson_not_a_slap` — message > 40 chars, names the model,
     module, and `acceptsControl`.
4. **audit boundary** (`audit.rs`):
   - `audit_refuses_a_direct_entry_the_manifest_does_not_accept` — a package whose
     entry discloses `Direct` with an empty `acceptsControl` → the consent gate
     rejects (`IPE-S0004`). **THE REFUSAL.**
   - `audit_accepts_a_direct_entry_the_manifest_accepts` — the same entry with
     `acceptsControl = [ Direct ]` passes.
   - `audit_never_gates_a_managed_tea_entry` — a `Web.tea` entry passes with an
     empty accept set (managed is the safe default).

Every acceptance path fails closed at ipe-time (the audit rejects before it
certifies the package), never open at cargo-time — a package whose self-driving
model the consumer has not accepted is turned back at the audit, not after emit.

## 7. Non-goals (explicit)
- LSP-hover / `ipe doc` control-model surfaces (spec'd for a follow-up lane in
  PR #2484's body) are out of scope here.
- No change to the `ControlModel` derivation or the disclosure surface — both
  landed and are reused verbatim.
