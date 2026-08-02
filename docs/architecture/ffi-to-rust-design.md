# FFI-to-Rust: finishing the consolidated boundary

The boundary itself is specified in `docs/architecture/ffi-to-rust.md` — three
orthogonal axes (representation / binding path / provenance), one fail-closed
failure story, one capability/sandbox model. This document is the companion
*implementation state map and completion plan*: what of that specification is
live in the tree, what remains, how each remaining piece is designed, and the
test-first order in which to land it. Nothing here re-opens a settled spec
decision; where a work package must resolve a spec-level ambiguity, the spec
is amended in the same change.

Priority order for every decision: security > correctness > soundness >
efficiency > completeness > readability. FFI is arbitrary Rust — a
remote-code-execution surface on both the build and run sides — so the
standing rule of the spec's §7 gate is restated here as a per-package gate:
**no FFI surface increment merges without a security-soundness-guardian
review of its design and diff, its untrusted paths inside the build jail,
its capability contribution attributed, and its failures inside the typed-Err
story.**

## 1. Current state — done vs missing

Where the spec names a subsystem, this table names the code that implements
it today.

| Spec area | State | Where |
| --- | --- | --- |
| Kernel seam — FFI call lowers to `Call { callee: Kernel(Ffi(id)) }`, open `KernelId` registry, `Rust.` namespace trust boundary | shipped | `src/compiler/canon/src/resolve.rs:707` (non-`FfiInterface` module cannot claim a `Rust.`-home name) |
| Interface gate — `Ffi.binding` mintable only in `ModuleOrigin::FfiInterface`, driver-generated | shipped | `src/compiler/canon/src/resolve.rs:466,908,4256` |
| Inspector — post-macro-expansion rustdoc-JSON introspection in a jail | shipped | `tools/ipe-ffi-inspector/src/main.rs` |
| Pipeline — trust prompt → jailed fetch → jailed inspect → typed decode → gates → three emitters → pinned per-crate cache | shipped | `src/ipe-cli/src/ffi.rs`; `src/compiler/ffi/src/{pkginfo,call,instance,unify,interface,emit,bindings,naming}.rs` |
| Validated-identifier newtypes; saturating scalar coercion single source | shipped | `src/compiler/ffi/src/{pkginfo,num_coerce}.rs` |
| Function binding — `FnShape` sum, `Fallibility` bit, closed-set generics at concrete instantiations, `MODELLABLE_5` | shipped | `src/compiler/ffi/src/{call,instance}.rs`, seals in `src/compiler/ffi/tests/` |
| Transparent import — struct→record / enum→closed-union, affirmative-evidence classification, `#[non_exhaustive]`→opaque, both-direction glue, per-type decision recorded | shipped | `src/compiler/ffi/src/transparency.rs`; interface cutover in `interface.rs` |
| Define surface — `[[rust.define.struct/enum/closure]]`, async closures, closure→`run` handoff, trait-impl exposure, `#[define_in_ipe]` | shipped | `src/compiler/ffi/src/{wrapper,instance,bindings}.rs`, seals `define_*_seal.rs`, `closure_handoff_seal.rs` |
| Define-transparency unification — all-identity-carrier define types surface as records/unions through the import glue | shipped (fail-closed to covered seams) | `src/compiler/ffi/src/transparency.rs:261–296,552` |
| Panic boundary — `catch_unwind` in every sync wrapper, async join-error fold, redacting error funnel, opaque-getter `Clone` containment, `panic = "abort"` `compile_error!` fence | shipped | `src/compiler/ffi/src/bindings.rs:138,1131` |
| Asserted call — `Rust.Ffi.call`, exact-carrier rule, admitted-crate-only, two-checker discipline, `ffi-raw` capability, refusal diagnostics | shipped (initial fail-closed shape) | `src/compiler/ffi/src/asserted.rs`; `Capability::FfiRaw` in `capability_scan.rs:205,329` |
| Build jail — bubblewrap primary, proven-`unshare` fallback, refusal default, offline compile/inspect, argv-only, Windows + FreeBSD returning arms, subprocess seccomp | shipped | `src/compiler/sandbox/src/{build_jail,seccomp}.rs`, `tests/build_jail_{e2e,windows_e2e,freebsd_e2e}.rs` |
| Run jail — per-axis `JailForTarget`; Linux/x86-64 + macOS confine the full axis set, Windows partial (Job Object + AppContainer + ACL-volume fail-closed), stub elsewhere = empty set | shipped for those targets | `src/compiler/sandbox/src/run_jail.rs:826–971` (`platform_confined_axes`, `CONFINED_AXES` per-OS), `src/ipe-cli/src/ffi.rs:1329` (`jail_for_host`) |
| Tier-2 wrapper admission — source panic-scan, capability inference + fail-closed reconcile, differential confinement (exercise harness + reconciler + fail-closed gate), macOS native enforcement | shipped | `src/compiler/ffi/src/capability_scan.rs` (`reconcile_for:751`, `must_refuse_for:453`), `src/ipe-cli/src/audit_native.rs` |
| Runtime consent — native-bearing classification from capability inference, consent-scoped run, low-value-only override, `database` as a derived axis | shipped | `src/compiler/sandbox/src/run_jail.rs:798,891` |
| Coverage ledger — per-crate `coverage.md` naming every drop with its reason | shipped | `src/compiler/ffi/src/driver.rs:413,455,984` |

What remains, in the spec's own dependency order:

| Missing piece | What it is | Blocking on |
| --- | --- | --- |
| **Async-breadth acceptance target** | The shim-free storefront example (`ipe rust add`-bound crates, used-set DCE) that *measures* admission breadth. It does not exist; `examples/ffi/` holds only the Iced and Bevy spikes, and the storefront app exists only as the pure-Ipê mirror example | nothing — pure inspector/generator work |
| **Async-breadth burn-down** | Widening the honest-drop set (trait-generic parameters, fallible typed identifiers, remaining ledger classes) until that target builds shim-free | the target above |
| **Define-transparency seam residual** | Glue for a transparent define type appearing in a closure signature or as another define's member (those defines stay opaque today, reason recorded) | nothing |
| **Asserted-call residuals** | `Rust.Ffi.unsafe` spelling (absent from the tree), lossless-widening in the inspected check, `Result`/borrow-taking targets, span-mapped attribution of rustc-caught assertion errors | nothing (each independently) |
| **Tier-2 axis re-open (per platform)** | New run-jail arms so more targets admit capability-bearing wrappers: FreeBSD (build-jail arm exists, run-jail arm does not — `CONFINED_AXES` is empty there), non-x86-64 Linux, and closing Windows's runtime-conditional gaps | per-arm OS work; each arm its own reviewed slice |
| **Per-platform admission CI matrix** | Driving every supported target through the Tier-2 exercise harness + reconciler at index admission, so a version is never admitted for a platform it was not observed on | the harness (shipped) + CI wiring |
| **Crate-coverage roadmap** | The spec §3 table (Iced → Axum/Hyper → Ratatui → Bevy → Slint/Dioxus/Gtk) driven to working examples, feeding admission gaps back as breadth items | everything above (consumes it) |
| **Stretch hardening** | Build-jail seccomp depth beyond subprocess-deny; private/ssh git sources behind a flag | unresolved design (ssh-agent scoping) |
| **User-facing honesty** | Explain pages: abort-only-target refusal, "the crate's enum grew" transparent-union evolution diagnostic | nothing |

Everything in the missing list is breadth, hardening, or diagnostics on top
of a complete boundary; no missing piece introduces a new *kind* of trusted
surface. That is the intended end-state of the spec's plan, and it is why
the remaining work parallelises well: the security-critical architecture is
already load-bearing and reviewed.

## 2. Design of the remaining pieces

Each subsection: what it does, how it emits, how it is typed, how a value
crosses safely. All emission stays concrete and monomorphised — auto-binding
of Rust crates with no hand-written shims; the generator emits real nominal
types and wrappers that rustc checks in the same `cargo` build as the bound
crate, never `dyn Any`, never an erased fallback.

### 2.1 The storefront acceptance target

A tracked example project (`examples/ffi/storefront/`) that re-expresses the
storefront app's service layer over `ipe rust add`-bound crates — an async
HTTP client, a serialisation crate, a database driver — instead of stdlib
kernels. It is a *measurement instrument*: `ipe build` must reach exit 0 with
zero entries in `coverage.md` that block the used set, and the emitted crate
must contain only used-set bindings (dead-code elimination keyed on the
`wrapper_ref_name` SSOT). Every admission gap it surfaces becomes a named
drop-class work item; a drop that blocks the used set is a root-cause item,
never a shim. The example doubles as the E2E regression net for all later
breadth work: once green, it stays green.

Typing and emission are entirely the existing machinery — the target adds no
new surface, which is what makes it the right first package: it converts
"async-breadth" from a vague direction into an enumerated, testable list of
drop classes.

### 2.2 Breadth burn-down (drop-class by drop-class)

Each admission class (e.g. trait-generic parameters at modellable bounds,
fallible typed identifiers, multi-borrow readers) lands as its own slice:

- **Typed at decode.** The class is admitted by extending the closed decode
  sums (`FnShape` / `ArgTypeRef` / `Carrier`) — never by loosening a gate.
  The new arm's invalid states remain unrepresentable; anything outside the
  new arm still over-drops with its reason.
- **Emitted concretely.** The generator renders the same sentinel-bracketed
  wrapper shape as every existing binding: owned-only crossing (strip `&`,
  re-borrow at the call site), `catch_unwind` body, typed `Err` via the
  funnel, saturating coercions only through `num_coerce.rs`.
- **Proven by seal.** Each class gets a seal test beside the existing
  `*_seal.rs` suite proving the emitted project builds and behaves, plus a
  coverage-ledger assertion that the class no longer appears as a drop.

### 2.3 Define-transparency seam residual

Today a define type that is *named by other generated Rust* — a closure
signature's parameter or return, another define's field or variant payload —
keeps the opaque-nominal surface even when all its members are identity
carriers, because the conversion glue does not yet cover those seams. The
residual work is to route those two seams through the same record⇄struct /
union⇄enum glue the import path uses: the generated closure adapter converts
at entry/exit, and a define member converts inside the containing type's
constructor forwarder. The fail-closed classification in `transparency.rs`
already records *why* each such type stays opaque; the change flips those
reasons to covered one seam at a time, never by widening the classifier
ahead of the glue. No new trust surface: every emitted line remains a total
function of decode-validated data.

### 2.4 Asserted-call residuals

Four independent residuals, each preserving the shipped invariants (parsed
path, exact-carrier, admitted-crate-only, `Result Error T`, born inside the
panic boundary, `ffi-raw` attribution unforgeable):

- **`Rust.Ffi.unsafe`.** A distinct spelling required for `unsafe fn`
  targets — modeled on Rust's own `unsafe`: explicit and localized, not a
  type-system off-switch. The shim wraps the call in an `unsafe` block; the
  marker's presence is a *second* disclosed capability fact folded into
  `ffi-raw` attribution. Raw-pointer and lifetime-carrying signatures stay
  refused even under the marker (no sound owned crossing exists for them).
- **Lossless widening in the inspected check.** The compile-time
  assertion-vs-inspection check admits identity *or lossless widening*
  (`u8 → Int`-class only); anything needing a clamp stays refused at the
  assertion site. The shim still performs no coercion — widening happens in
  the checker's acceptance predicate, not in emitted code, so the
  rustc-checked shape remains exact.
- **`Result`-returning and borrow-taking targets.** A target returning
  `Result<T, E>` folds `E` through the same typed-`Error` funnel as inspected
  bindings (one error channel, not two); a `&T`-taking parameter is admitted
  by the standard owned-crossing re-borrow. Both reuse existing generator
  arms — the work is extending the asserted signature parser and the
  pre-check, not new emission.
- **Span-mapped attribution.** When rustc is the checker of record, map the
  failing shim's sentinel region back to the assertion's source span so the
  user sees an ordinary Ipê diagnostic instead of a commented cargo error.
  Pure diagnostics plumbing; the comment-header attribution remains the
  fallback for unmappable output.

### 2.5 Tier-2 axis re-open and the admission matrix

Per-platform run-jail arms extend `CONFINED_AXES` for their target; the
single-sourcing discipline already prevents over-claim (`jail_for_host`
folds only what the compiled-in arm lists, and the stub arm lists nothing).
Each new arm is one reviewed slice: FreeBSD (`jail`/`capsicum` — the
build-jail arm's shell-free lessons carry over), then non-x86-64 Linux
(lifting the `target_arch` gate once seccomp filters are verified per-arch).
An arm ships only with an E2E proving each claimed axis fails closed under
the exercise harness, mirroring `run_jail_macos_e2e.rs`.

The admission CI matrix runs the shipped Tier-2 exercise harness +
differential-confinement reconciler on every supported platform at package
admission, so "admitted" always means "observed under confinement on that
platform, reconciled against the declaration". Platforms without a run-jail
arm remain refuse-gapped for capability-bearing wrappers — the matrix makes
that visible per-version rather than silently admitting from a Linux-only
observation.

### 2.6 Crate-coverage roadmap

Consumes all of the above in the spec's order — Iced and Bevy spikes already
exist under `examples/ffi/`; each subsequent crate class lands as a tracked
example whose gaps feed §2.2. This is deliberately last and open-ended: it
is the boundary's *user acceptance suite*, not new machinery.

## 3. Security model of the remaining work

The trust edges, restated for the remaining pieces (the full model is spec
§7; nothing below weakens it):

- **Trust edge 1 — crate acquisition and build.** All new breadth work sits
  *behind* the existing build jail: the inspector and every crate compile run
  network-unshared, filesystem-scoped, env-scrubbed, resource-capped;
  untrusted code runs only inside an explicit add/install. New drop-class
  admissions change what is *decoded*, never what is *executed* at
  inspection, so they add no build-time exposure. The storefront target adds
  real third-party crates to a tracked example — their versions are pinned
  by the cache's resolved-version key, and CI treats the example like any
  E2E (jailed add, warm-cache build).
- **Trust edge 2 — untrusted names into generated source.** Every new
  emitter arm consumes only decode-validated newtypes (`RustIdent`,
  `ModulePath`, `Carrier`); the injection kill-point stays at decode. Review
  obligation per package: demonstrate the new arm cannot render a string
  that did not pass a validating constructor.
- **Trust edge 3 — run-time foreign execution.** Every new wrapper body is
  born inside the panic boundary (`catch_unwind` / join-error fold / funnel
  redaction / abort fence). The capability story per piece: breadth and
  seam-residual work inherit the crate's native-bearing classification;
  `Rust.Ffi.unsafe` stays inside `ffi-raw`; new run-jail arms *extend
  containment* and therefore widen what may be admitted — which is exactly
  why each arm needs its own adversarial review and a fail-closed E2E per
  claimed axis before `CONFINED_AXES` lists it.
- **Trust edge 4 — the admission matrix.** The reconciler's verdicts become
  load-bearing for other people's machines; the matrix must fail closed on
  missing platforms (absence of an observation is a refusal for that
  platform, never a pass-through).
- **Reproducibility.** The per-crate cache stays pinned to resolved version
  + inspector toolchain channel; a bump is a cache miss forcing
  re-inspection. The asserted-call compile-time check reads only the pinned
  inspection, so it can never validate against a stale signature.

**The blocking gate, per package:** design reviewed by the
security-soundness-guardian before implementation starts *and* the diff
reviewed before merge; untrusted paths jailed; capability contribution
attributed; failures in the typed-Err story; the sandbox proof (E2E under
the jail) green. FFI is a language boundary — this review is mandatory for
every package below, including the "diagnostics-only" ones, because
attribution and explain text shape what users trust.

## 4. Scope and non-goals

**Ships first (minimal complete FFI):** the storefront acceptance target
green shim-free (storefront-target + breadth-burn-down + define-seams), and
asserted-call span attribution. That is a boundary a user can rely on for
real applications on Linux and macOS with full containment, Windows with
partial containment, honest refusal elsewhere.

**Ships later:** `Rust.Ffi.unsafe`, widening/`Result`/borrow asserted
targets, new run-jail arms, the admission matrix, the crate roadmap beyond
Iced/Bevy, seccomp depth, ssh git sources.

**Non-goals (permanent, from the spec):** no C-ABI FFI, no borrowed returns,
no erased generics, no `Ty::Any`, no hand-written shims, no runtime that
launders a panic into a value, no admission of an axis the platform jail
cannot contain, no second capability vocabulary.

## 5. Implementation plan — named work packages, test-first

Each package: the failing test first, the minimal change, the gate. Gates
common to every package: workspace build + clippy deny-set + full `ipe_ffi`
/ `ipe_sandbox` suites green; goldens byte-identical unless the package's
seal intentionally re-blesses; THE SEAL (`ipe` exit 0 ⇒ `cargo` builds)
proven by the package's seal test; **guardian design review before, diff
review after; sandbox-proof E2E where the package touches jail or admission
behaviour.** Packages are independently landable; the order below is
dependency plus risk-burn-down.

1. **storefront-target.** Failing test: an E2E (`SKY_E2E`-gated, like the
   existing seals) that adds the chosen crates, builds
   `examples/ffi/storefront`, and asserts exit 0 + an empty blocking-drop
   set in `coverage.md`. It fails today by enumerating real drops — the
   enumeration *is* the deliverable. Minimal change: the example project +
   the test harness; no compiler change. Gate: guardian review of the crate
   choices (each new third-party crate is new jailed-build surface + a
   pinned supply-chain fact).
2. **breadth-burn-down** (repeats per drop class). For each class from the
   storefront enumeration (ordered by how many storefront symbols it
   blocks): failing seal test asserting the class binds and behaves; minimal
   decode-sum + emitter-arm change; coverage-ledger assertion flips from
   named-drop to bound. Gate per class: seal green, no other ledger line
   changed (no silent drops), guardian review. The package exits when the
   storefront E2E is green shim-free with used-set DCE.
3. **define-seams.** Failing seals: a transparent define type in a closure
   signature, and as another define's member, each asserting the
   record/union surface and round-trip conversion behaviour. Minimal change:
   glue emission at the two seams, classifier flip per seam. Gate: existing
   `define_*_seal.rs` suite untouched-green (the fail-closed classifier must
   not widen ahead of the glue), guardian review.
4. **asserted-attribution.** Failing test: a deliberately wrong assertion
   against an over-dropped symbol produces a diagnostic carrying the
   assertion's source span, not raw cargo text. Minimal change:
   sentinel-region → span mapping in the emitted-build error path. Gate:
   negative-suite green; guardian review (attribution is a trust surface —
   a mis-mapped span blames the wrong code).
5. **asserted-breadth.** Per residual, in order `Result`-returning →
   borrow-taking → lossless widening: failing seal + negative tests (the
   refusal boundary must move exactly one notch — e.g. widening admits
   `u8 → Int` and still refuses anything clamping); minimal
   parser/pre-check/emitter change. Gate: exact-carrier property preserved
   (no `num_coerce` call reachable from `asserted.rs`), guardian review.
6. **ffi-unsafe.** Failing tests: an `unsafe fn` target unmarked is refused
   with the spelling suggested; marked, it binds; raw-pointer and lifetime
   signatures stay refused marked or not. Minimal change: the spelling in
   resolve + the `unsafe`-block shim arm + attribution fold. Gate: guardian
   review is the long pole — this widens what `ffi-raw` can reach; the
   review owns the refusal boundary.
7. **freebsd-run-arm.** Failing test: `run_jail_freebsd_e2e.rs` asserting
   each claimed axis fails closed under the exercise harness (mirror of the
   macOS E2E). Minimal change: the arm + its `CONFINED_AXES` entry,
   single-sourced as today. Gate: per-axis fail-closed E2E green on a
   FreeBSD runner, guardian review; the axis list may not exceed what the
   E2E proves.
8. **admission-matrix.** Failing test: an admission run for a platform with
   no observation must refuse (fail-closed matrix semantics), expressed as a
   reconciler unit test before any CI wiring. Minimal change: matrix wiring
   driving the shipped harness per platform. Gate: sandbox-proof runs on
   every wired platform; guardian review of the refusal semantics.
9. **honest-diagnostics.** Failing tests: negative-suite entries asserting
   the abort-only-target refusal and the transparent-union "the crate's
   enum grew" re-add diagnostic name their explain pages. Minimal change:
   diagnostics + explain text. Gate: jargon gate + guardian review of the
   security claims the prose makes.
10. **crate-roadmap** (repeats per crate class). storefront-target-style: a
    red example E2E, gaps filed as breadth-burn-down-style classes, green
    when shim-free. Gate as storefront-target. Open-ended by design.

Golden/SEAL/E2E implications: storefront-target, breadth-burn-down,
define-seams, asserted-breadth and ffi-unsafe add seal tests and may add new
goldens (new emission arms are new golden surface — bless once, then
byte-stable); no existing golden changes except where a seal intentionally
re-blesses an emitter's output shape. freebsd-run-arm and admission-matrix
are sandbox/E2E only and need platform CI capacity, so they can trail the
rest without blocking them.

## 6. Risks and cost

- **The RCE surface itself.** Every package increases what attacker-authored
  Rust can reach. Mitigation is structural (jail + capability + typed
  decode) plus procedural (the per-package guardian gate); the residual risk
  is review throughput — the plan's long pole, accepted deliberately. The
  worst credible failure is an emitter arm that renders un-validated text;
  the per-package trust-edge-2 demonstration exists to kill exactly that.
- **Auto-binding complexity growth.** Each breadth class enlarges the decode
  sums and emitter arms; the cost is borne once per class and fenced by
  seals. The counter-pressure is the over-drop rule: when a class's sound
  admission is unclear, it stays dropped with a recorded reason — breadth
  is never bought with soundness.
- **Crate-version and ABI pinning.** Source-level binding removes ABI risk
  (rustc checks every seam in one build), but version drift remains: the
  storefront example pins real crates whose majors will move. The
  cache-pin-forces-reinspection rule keeps correctness; the maintenance
  cost (periodic re-add + coverage re-check) is accepted and visible in CI.
- **Platform sandbox differences.** Per-axis honesty (`JailForTarget`)
  prevents over-claiming, but each new arm is bespoke OS work with its own
  failure modes (Windows's ACL-volume runtime conditions are the shipped
  precedent). Refuse-gapping a platform is always the safe fallback — the
  cost is user-visible narrowness, not a security hole.
- **Reference divergence.** The subsystem's ancestor generator/inspector
  design (the Sky Rust backend) is behind this tree — transparency, the
  define surface, Tier-2 admission and the asserted call are Ipê-native
  extensions with no upstream counterpart. Comparisons remain useful for
  the inspector's rustdoc-JSON handling only; the divergence ledger records
  the split, and no package should port upstream behaviour uncritically.
- **Acceptance-target flakiness.** A real-crates E2E inherits network and
  crates.io availability at add time; mitigated by the warm cache (CI
  re-adds only on pin change) and by keeping the jailed add a separate,
  retryable step from the build assertion.
