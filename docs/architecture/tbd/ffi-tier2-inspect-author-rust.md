# FFI Tier 2 — inspecting author-supplied Rust wrapper crates

Status: design (tbd). Specifies the second FFI tier: instead of *declaring* a Rust
type in the manifest (Tier 1, `[rust.provide.*]`), the package author *writes* a
normal Rust wrapper crate, and Ipê binds it by running the **same** inspect →
sandbox → generate pipeline it already runs on a crates.io dependency — plus two
provenance-specific gates (source panic-scan and capability inference).

Fenced code blocks are illustrative unless the prose says a fixture ran.

---

## 0. Why a second tier

Tier 1 (`[rust.provide.*]`, see `ffi-rust-type-creation-and-coverage.md`) is
**structured, declarative shims**: the author declares a shape (struct fields,
enum variants, closure signature) and the driver *generates* the Rust from
decode-validated newtypes. That makes it **safe by construction** — nothing but
the generated, carrier-typed code can exist — but it is:

- **cumbersome**: every field/variant/signature is enumerated in `ipe.toml`;
- **bounded**: only the closed carrier set and three forms are expressible; a
  crate whose API needs a real hand-written `impl Trait`, generics, builders, or
  glue logic is out of reach (e.g. a Bevy `#[derive(Component)]` type).

Tier 2 restores the ergonomics of hand-written wrappers (the historical
`skyshop-rs/wrappers/` pattern) **without** re-opening the trust hole that
pattern had, by reusing machinery that did not exist when hand-wrappers were
last used: the crate inspector, the RCE build sandbox, the capability model, and
`tools/panic-scan`.

The two tiers are complementary, not competing:

| | Tier 1 `provide.*` | Tier 2 wrapper crate |
|---|---|---|
| Author writes | a manifest declaration | normal Rust |
| Expressiveness | closed forms only | arbitrary Rust |
| Guarantee kind | **impossible by construction** | **checked and attributed** |
| Best for | the simple 80% | complex / imperative APIs |

---

## 1. The invariant that must survive (non-negotiable)

The keystone from Tier 1 holds unchanged: a `Ffi.binding "<wrapper>" args` node —
the only bridge from Ipê into Rust — is minted **only** inside a
`ModuleOrigin::FfiInterface` module the *driver* vouches for; user `.ipe` can
never mint a `ForeignCall`. Tier 2 does **not** hand the user a "write Rust in
`.ipe`" construct. The author's Rust lives in a separate **crate**, and Ipê binds
to its *public symbols* exactly as it binds a crates.io crate — through the two
typed decode boundaries (`PkgInfo` decode, `Call` decode), with the same
over-drop discipline. The author never names a `wrapper_ref_name`; the driver
generates the interface from the inspected symbols.

So Tier 2 is **not** "arbitrary Rust injected into emitted code". It is "a
dependency the author happens to have written, inspected and bound like any
other dependency, held to a higher provenance bar."

---

## 2. Author surface

A wrapper crate is declared in the package's `ipe.toml`, pointing at a local
crate the author owns:

```toml
# ipe.toml (illustrative)
[rust.wrapper]
path = "wrappers"                     # a normal Cargo crate at ./wrappers
# The public symbols to bind. Each must present an FFI-carrier-compatible
# signature; anything else over-drops with a diagnostic (never emit-and-fail).
expose = ["make_engine", "engine_step", "Engine"]
# Capabilities the wrapper exercises — consented at `ipe add`, ENFORCED by the
# sandbox at build and run (§5). Inference proposes this set; the author confirms.
capabilities = ["network"]
```

```rust
// wrappers/src/lib.rs (illustrative — normal, idiomatic Rust)
pub struct Engine { /* … */ }

pub fn make_engine(seed: i64) -> Engine { /* … */ }
pub fn engine_step(e: &Engine, input: String) -> Result<String, String> { /* … */ }
```

The author writes ordinary Rust — no per-field manifest ceremony, full access to
generics/builders/`impl Trait` internally — and lists what to expose.

---

## 3. The pipeline (reuses four existing subsystems + two new gates)

For each wrapper crate, at `ipe add` / `ipe install` (never silently at build):

1. **Inspect** — run the existing inspector over the wrapper crate's public
   symbols → the same typed `PkgInfo`. A local path crate is just a crate;
   nothing about the inspector changes. Signatures that are not
   carrier-compatible (borrowed returns, unsupported types) **over-drop** with a
   diagnostic, exactly as for a crates.io crate.
2. **Sandbox build** — the RCE sandbox (`ipe_sandbox`) builds the wrapper and its
   dependency tree. Same threat model as a crate's `build.rs` / proc-macros; no
   new sandbox needed.
3. **Generate bindings** — the generator emits `_bindings.rs` that *calls into*
   the wrapper's symbols under the **owned-only, closed-carrier, over-drop**
   discipline. Our generated bindings are therefore **SEAL-safe by
   construction**, even though the crate body is author-written: we only ever
   bind a carrier-compatible signature, and everything crosses the boundary as an
   owned value or an opaque `'static` handle.
4. **Source panic-scan** *(new gate — provenance)* — run `tools/panic-scan` on
   the wrapper's `.rs` source. A hit is a **user error**: author Rust in an Ipê
   package is held to Ipê's no-authored-abrupt-failure bar. This is the key
   asymmetry versus a crates.io dependency (§4).
5. **Capability inference + enforcement** *(new gate)* — infer the capability set
   the wrapper needs, reconcile with the manifest declaration, surface it under
   informed consent, and hand it to the sandbox to enforce (§5).
6. **Emit** — the interface admits the bound symbols as opaque nominals +
   forwarders (the same `interface.rs` path Tier 1 forwarders use). The wrapper
   crate becomes a path dependency of the emitted app crate.

Steps 1-3 and 6 already exist; steps 4-5 are the Tier 2 additions.

---

## 4. The guarantee, stated honestly

The guarantee **shifts in kind** between a crates.io dependency, Tier 1, and
Tier 2 — and the difference is the whole point, so it must be explicit:

- **crates.io dependency** — we do **not** source-scan it for panics (that would
  reject the ecosystem). We bind its symbols and sandbox its *behavior*; its
  internal panics are the documented "outside the guarantee" boundary.
- **Tier 1 `provide.*`** — **impossible by construction**: the Rust is generated
  from typed decode; no abrupt-failure or capability escape can exist because we
  wrote every line.
- **Tier 2 wrapper** — **checked and attributed**: the author wrote it, so we
  *verify* it (source panic-scan → user error; capability inference + sandbox
  enforcement; owned-only bindings; sandboxed build), and any violation is a
  *detected, attributed* user error rather than an unrepresentable state.

Tier 2 is strictly weaker than Tier 1 (detected vs unrepresentable) and strictly
stronger than a raw crates.io dependency (source-scanned + capability-inferred vs
behavior-sandboxed only). That ordering is the model's spine and belongs in the
README's precise wording.

**SEAL for Tier 2.** `ipe` exit 0 ⇒ the emitted app crate — our bindings *plus*
the wrapper as a path dep — cargo-builds. If the wrapper crate does not itself
compile, the sandboxed inspect/build (step 2) catches it and Ipê refuses **before**
exit 0, as a surfaced author build error — SEAL is preserved, never breached.

---

## 5. Capability inference — the hard part

**Status: landed** (`ipe_ffi::capability_scan` + the `install_wrapper` gate).
The static inference and the fail-closed reconcile ship; the runtime-enforcement
half is the one honest caveat, spelled out at the end of this section.

For Tier 1, capabilities are trivial (a struct ctor has none; a closure re-enters
the Ipê evaluator carrying the *caller's* capabilities). For arbitrary wrapper
Rust, the crate may touch `std::net`, `std::fs`, `std::process`, threads, env, or
FFI. A three-layer defence, weakest-to-strongest:

1. **Static inference (proposes).** Reuse the `panic-scan` token-scanner
   infrastructure to also flag capability-bearing paths (`std::net::*`,
   `std::fs::*`, `std::process::*`, `std::env::*`, raw `libc`/`extern`), mapping
   them to the capability taxonomy (`network`, `filesystem`, `process`,
   `environment`, …; see `package-coordination-and-capabilities-design.md`). This
   is *imprecise* (macros, re-exports, indirect calls) and is used only to
   **propose** a set and to flag an under-declared manifest.
2. **Declaration (consents).** The manifest `capabilities = [...]` is the
   author's claim, shown to the user at `ipe add` under informed consent — like
   every other native `Rust.` surface.
3. **Sandbox enforcement (guarantees).** The RCE sandbox enforces the *declared*
   capability set at build **and** the runtime capability scope enforces it at
   run: a syscall outside the declared set fails closed. This is the load-bearing
   layer — even if static inference misses a capability, the runtime sandbox
   contains it. Inference exists to make the declaration *honest and reviewed*,
   not to be the enforcement.

The security posture: **declaration + fail-closed enforcement**, with static
inference as an honesty check on the declaration. This mirrors the capability
model's existing stance and does not trust the wrapper's self-report alone.

### 5.4 The enforcement reality this release ships (fail-closed, not fail-open)

Layer 3's "runtime capability scope enforces it at run" presumes a sandbox
around the *emitted app* at `ipe run`. **That runtime jail does not exist yet.**
The emitted app — including the author's wrapper Rust — runs with the invoking
user's full ambient authority. The build/inspect jail (§3.2) denies network for
the wrapper *build*, but that does not constrain the *running* app.

Therefore a wrapper capability on a runtime-enforced axis (`network`,
`filesystem`, `database`, `env`, `subprocess`, `native-ffi`) is **infeasible to
enforce today**, and the rule "if a sound fail-closed enforcement is infeasible,
refuse the wrapper rather than admit it unenforced" applies with full force. The
shipped gate (`ipe_ffi::capability_scan::reconcile`, wired into `install_wrapper`)
is therefore:

- A wrapper that **declares** OR is **inferred** to reach any runtime-enforced
  axis is **hard-refused at install** — it cannot be installed in this release.
  Its runtime effects would be uncontained; admitting it would make the
  `capabilities` manifest field a false claim.
- Opaque constructs the scan cannot see past — an `extern` block / `#[link]` /
  `libc::` (native FFI), an `include!` / `#[path]` module, a non-`std` Cargo
  dependency (whose capabilities live in source the scan never opens), or a
  source that does not lex — are each a **refuse** trigger, never a silent
  "no capability found". The scan is biased to over-refuse: a false positive
  costs an author a narrowing; a false negative would admit an unconstrained
  capability.
- Only wrappers whose declared **and** inferred sets are confined to the
  containable axes — `clock` / `random` (non-determinism, not exfiltration), or
  empty (pure compute) — install. These leak no authority even unenforced.

This is strictly a security *improvement* over the prior state, in which such a
wrapper installed with **no capability gate at all**: it turns "silently
unconstrained wrapper Rust" into "refused until the runtime jail lands". When the
emitted-app runtime jail arrives, the refused axes re-open one at a time, each
gated on its jail actually scoping the syscall fail-closed.

---

## 6. Where the proc-macro / trait-impl escape hatch fits

*Landed.* A `#[ipe::provide]` companion crate — a hand-written `impl Trait` for a
crate type whose derive is outside the `MODELLABLE_5` set (Bevy
`Component`/`Resource`) — is a **special case of Tier 2**, not a separate
mechanism: a wrapper crate that exposes a trait impl. It is folded into the Tier
2 pipeline as the "provide a trait impl" shape, not a bespoke Tier-1 extension —
which is why the escape-hatch work is designed here.

**The marker.** A tiny companion proc-macro crate `ipe_provide` (workspace member
`src/ffi-provide-macro`) exports one **inert** attribute macro, `#[ipe::provide]`
(spelled `#[ipe_provide::provide]`, or re-exported as `ipe::provide`). It re-emits
the annotated item token-for-token and prepends exactly one pure-data breadcrumb —
a `#[doc = " ipe-ffi-provide-marker"]` string rustdoc folds into the item's `docs`
field. The macro generates NO trait impl, NO glue, NO logic; it only tags. So the
author's Rust stays entirely authored (and thus source-panic-scannable, §3.4) —
nothing is injected into the trusted emission set.

**How it rides the wrapper pipeline.** When the inspector runs over a `[rust.wrapper]`
`--path` crate, it reads the marker as a boolean "is this item author-marked"
(exactly as `doc_hidden` reads `attrs`, matched as a whole trimmed line so prose
cannot forge it), then **auto-adds** every marked item to the exposed set: a
marked TYPE's methods (`recvType`), a marked free fn (`name`), and any function
whose params/results name a marked type (whole-segment matched, so `Sprite`
matches `sprite::Sprite`/`&Sprite`/`Vec<Sprite>` but never `SpriteSheet`). The
mark only widens WHICH candidate symbols are considered — each still passes the
same carrier-compatibility over-drop, the source panic-scan (§3.4), and the
capability gate (§5) as any other wrapper symbol. The marked type then surfaces
as an Ipê-held opaque nominal + forwarders through the SAME inspect → sandbox →
generate → over-drop path Phases 1-3 landed; a borrowed-return method on a marked
type still over-drops rather than emit-and-cargo-fail. The wrapper is bound
exactly like any other exposed symbol — the emit path does not distinguish a
marked type, which is the point.

**Guardian verdict.** The `security-soundness-guardian` reviewed the design pre-write
and returned PROCEED-WITH-CONDITIONS: the new proc-macro crate adds no privileged
context (it runs at the wrapper's compile time inside the RCE sandbox that already
builds the wrapper's proc-macro deps); the inert pass-through is load-bearing (a
code-generating macro would move panic-scan's target off the authored source and
evade the provenance gate); auto-expose only enlarges the candidate set, never a
gate bypass; and the marker is read as a boolean, never rendered, so it is no
injection vector. All conditions are honored — inertness snapshot test, exact
whole-sentinel match, borrowed-return over-drop + positive `IPE_E2E` build/run
fixtures, `ipe_provide` as a wrapper-author build-dep only, panic-scan on authored
source, marker read as data.

**SEAL fixture.** `src/compiler/ffi/tests/provide_trait_impl_seal.rs` proves the
whole path: under `IPE_E2E`, the real inspector runs over a wrapper crate that
depends on `ipe_provide`, tags a `Sprite` with `#[ipe_provide::provide]`, and
hand-writes an `impl Render for Sprite` (a fixture trait standing in for a Bevy
`Component`); the marker surfaces `Sprite` and its reader even though `expose`
names ONLY the constructor, then the emitted app crate + the wrapper `path` dep
cargo-build and run exit 0, round-tripping the value through the hand-written
trait impl. A marked borrowed-return method over-drops in the default gate.

---

## 7. Boundary rules — when each tier applies

- Type is a plain record/union of carriers, or a closure of a closed signature →
  **Tier 1** (`provide.*`). No user Rust; safe by construction. Prefer it.
- Type needs a real `impl Trait`, generics, builder logic, or glue → **Tier 2**
  (wrapper crate). Accept checked-and-attributed for the expressiveness.
- The compiler should *suggest* Tier 1 when a Tier 2 wrapper only does what a
  `provide.*` form could express (a lint / `ipe` diagnostic), to keep authors on
  the stronger guarantee whenever possible.

---

## 8. Adversarial surface (threats and containment)

- **Malicious build script / proc-macro** in the wrapper or its deps → contained
  by the RCE sandbox at build (existing).
- **Runtime capability escape** (undeclared net/fs/process) → contained by
  runtime capability scoping, fail-closed (§5.3).
- **Authored abrupt failure** (`panic!`/`unwrap`) that would crash the app →
  caught by source panic-scan → user error (§3.4).
- **Borrowed / lifetime-escaping return** → refused by the owned-only invariant
  in binding generation (existing over-drop).
- **Non-compiling wrapper** → caught by the sandboxed build → refused before exit
  0 (§4, SEAL).
- **Supply-chain via the wrapper's own deps** → the wrapper's `Cargo.toml` deps
  are audited by the same supply-chain gate as `[rust.dependencies]`
  (`cargo-deny`, lockfile pinning).

---

## 9. Open questions

- **Capability inference precision.** How far to push static inference before it
  is noise? Proposal: flag the coarse std capability paths only; rely on
  enforcement for the rest. Revisit if false-negatives in inference erode the
  "reviewed declaration" value.
- **`std`/dependency panics inside a wrapper.** Source panic-scan catches
  *authored* panics; a dependency's internal panic still crashes the app. Do we
  require the wrapper to catch-unwind at its boundary (turning a dep panic into a
  `Result` the Ipê side folds), symmetric with the Tier 1 closure adapter?
  Leaning yes — a wrapper boundary that catch-unwinds keeps the app's no-crash
  property.
- **Handle identity / lifetime.** Opaque handles returned by a wrapper are
  `'static`-owned (the owned-only invariant). Confirm no wrapper API can smuggle
  a borrow across the boundary (the inspector's `own_ref_idx` strip must apply to
  path-crate symbols identically to crates.io ones).
- **Incrementality.** The wrapper is a local crate; `ipe watch` must re-inspect +
  re-sandbox only when the wrapper changes (salsa input keyed on the wrapper
  crate's content hash).

---

## 10. Implementation phases (for the impl lane)

1. **Manifest surface** — decode `[rust.wrapper]` (`path`, `expose`,
   `capabilities`) through validating newtypes (path is sandbox-jailed; `expose`
   entries are `RustIdent`; `capabilities` is the closed capability enum). No raw
   string reaches emission.
2. **Inspect a path crate** — point the existing inspector at the wrapper crate;
   reuse `PkgInfo` decode + over-drop. Fixture: a wrapper with one carrier-typed
   fn binds; a borrowed-return fn over-drops with a diagnostic.
3. **Generate + emit bindings + interface forwarders** — reuse the generator and
   the `interface.rs` opaque-forwarder path; the wrapper becomes a path dep of the
   emitted crate. SEAL fixture: `ipe`-0 ⇒ emitted crate (bindings + wrapper)
   cargo-builds + runs under `IPE_E2E`.
4. **Source panic-scan gate** — run `tools/panic-scan` over the wrapper source in
   the package/CI gate; a hit is a user-facing diagnostic.
5. **Capability inference + enforcement** — static proposer + manifest
   reconciliation + fail-closed refuse (§5). **Landed.** Guardian-reviewed on the
   design and the diff; the runtime-enforcement half is refused-until-available
   per §5.4 (there is no emitted-app runtime jail yet, so any runtime-enforced
   capability is hard-refused at install rather than admitted unenforced).
6. **Fold the trait-impl escape hatch** — the `#[ipe::provide]` companion crate as
   a Tier 2 wrapper shape. *Landed* (§6): the inert `ipe_provide` marker macro +
   inspector auto-expose of marked items + SEAL fixture
   (`provide_trait_impl_seal.rs`).

Phases 1-3 are the mechanical reuse; 4-5 are the provenance gates and carry the
security weight; 6 closes the Bevy-derive case.
