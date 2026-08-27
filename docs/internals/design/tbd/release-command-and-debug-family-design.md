# The `release` production command and the `Ipe.Debug` family

> Design proposal — not yet implemented. Every command, flag, type signature, and
> diagnostic shown below describes the target design; the code blocks are
> illustrative, not runnable against the current tree.

## Problem

Ipê has one development-only escape hatch today — `Debug.log : String -> a -> a`
(`Ipe.Debug`) — guarded by a rule that a production build rejects any `Debug.*`
use fail-closed (IPE-L0140). Two things are wrong with how "production" is
currently expressed, and both must be settled before the debug family grows:

1. **The gate is opt-in and misplaced.** "Production" is a `--optimize` flag on
   `ipe build`, default off. The flag does *only* the `Debug.*` rejection — it
   does not optimize (the real optimization, `opt-level = "z"` under a release
   cargo profile, is done elsewhere). A plain `ipe build` binary ships `Debug.*`
   happily, and nothing forces the flag.

2. **The one shippable-artifact path does not set it.** The command that produces
   an optimized, jailed, shippable artifact builds with production off, so a
   `Debug.*` call can currently reach a shipped bundle. That command also refuses
   pure Ipê apps (no native/FFI content ⇒ no capability profile ⇒ nothing to jail)
   and refuses the browser/wasm target — so pure apps and every browser app ship
   through `ipe build` instead, entirely outside the gate.

The consequence: the "a debug construct can never ship" guarantee is not real. It
must become a property of *the act of producing a shippable artifact*, covering
every app kind, before `Debug.todo` and `Debug.explain` (which rely on the same
guarantee) are added.

## Design overview

Three parts, staged. Part A is the foundation the other two rest on.

- **A. `ipe release`** — one command that produces the shippable artifact for
  every app kind, and is the single production gate.
- **B. `Debug.todo`** — a typed hole that inhabits every type, compiles in
  development, and is rejected by `release`.
- **C. `Debug.explain`** — a layout-debug `Ui` modifier, gated the same way by
  module membership (independent of `Debug.todo`).

A time-travelling debugger for TEA apps is a separate, larger subsystem and is
out of scope here (see *Out of scope*).

## A. The `release` production command

### Verb, not flag

Production is expressed by the verb. The command matrix becomes orthogonal:

| Command                       | Meaning                              | `Debug.*` |
|-------------------------------|--------------------------------------|-----------|
| `ipe run`                     | development execution                | permitted |
| `ipe build`                   | development artifact (native)        | permitted |
| `ipe build --target wasm`     | development artifact (browser)       | permitted |
| `ipe release`                 | production artifact (native/bundle)  | rejected  |
| `ipe release --target wasm`   | production artifact (browser bundle) | rejected  |

`--target` stays the single "which artifact" axis, consistent with the existing
`ipe build --target`. Bare `ipe release` infers the target from the app's shape /
manifest; `--target wasm` overrides for a Web-shape app you want as a browser
bundle rather than a server binary.

`--optimize` is removed. "Production" is no longer a flag on any command; it is
what `release` *is*.

### One command, every app kind

`release` replaces the narrower predecessor command and drops its two refusals,
producing the right artifact per app:

- **Native app with native/FFI content** — the jailed bundle (the existing embed
  / wrapper modes and the capability profile), unchanged in substance.
- **Pure native app** (no native/FFI) — a plain optimized binary. No jail wrapper
  is produced (there is no capability surface to confine); the refusal that
  exists today is dropped. The binary is still built under the release cargo
  profile and still passes the `Debug.*` gate.
- **Browser/wasm app** (`--target wasm`) — the production browser bundle
  (optimized `.wasm` + generated glue + assets, content-hash-pinned with SRI, as
  the browser transport already serves). The refusal that exists today is dropped.

In all cases `release` (a) drives the real optimization (release cargo profile),
and (b) sets the production flag, so the `Debug.*` gate fires.

### The gate

The whole family collapses to one rule:

> A `release` build (any target) rejects any reachable `Ipe.Debug.*` construct
> fail-closed with a located diagnostic (IPE-L0140). `build` and `run` permit it.

The gate keys on **module membership**: anything exported from `Ipe.Debug` is a
development-only construct and is rejected by `release`. This is why parts B and C
need no per-construct wiring and no dependency on each other — being an
`Ipe.Debug` export *is* the gate.

Enforcement is at emit demand in the compiler (the existing IPE-L0140 site),
keyed on the build's production flag, which `release` sets for every target. This
does not — and cannot — stop a developer from copying a `build` binary elsewhere;
it guarantees that the sanctioned production path never emits a debug construct,
and that dev and production build caches stay disjoint (a dev-cached project is
never served to a `release` build).

### Naming note

"release" also names version tags and changelog automation. `ipe release` = "build
the release artifact" is a distinct, conventional sense; the overlap is contextual,
not a collision. It is the honest name: the command produces the artifact, it does
not push it to a running environment.

## B. `Debug.todo` — a typed unfinished-code marker

```ipe
todo : String -> a
```

A polymorphic expression accepted in any position, so a module with holes still
compiles and its finished parts run and test. The `String` is the developer's
note.

- **Type.** The result `a` is unconstrained (it appears only in the result). The
  construct never produces a value of `a` — it diverges — so type safety is
  preserved exactly as `Debug.log`'s polymorphic passthrough already is: no value
  of the wrong type is ever constructed. This inhabits-every-type, never-return
  shape is the soundness-sensitive core and requires the security-soundness review
  before it ships (a language boundary).
- **Runtime.** Reaching a `todo` at runtime aborts through the runtime's Error
  path — `main : Task Error ()` fails with `TODO at <file:line>: <note>`, writing
  the Error to stderr and exiting non-zero. Never a bare panic (consistent with
  the no-panic production rule). The `<file:line>` is the call site: the lowerer
  injects the call-site span, since the note string alone does not locate it.
  Implemented as a compiler-recognised `Ipe.Debug` kernel (mirroring
  `Debug.log`'s `Ffi.kernel` shape) whose call sites are decorated with their
  source location.
- **Exhaustiveness.** A `todo` in a case arm is a real inhabitant, not a wildcard
  pattern — it does **not** excuse a non-exhaustive `case`. (Distinct from the
  value-position wildcard, which is a pattern binder, not an expression.)
- **Gate.** It ships nothing: as an `Ipe.Debug` export it is rejected by `release`
  (part A). This is why `Debug.explain` (part C) needs no dependency on it — both
  are gated by membership, not by threading one through the other.

## C. `Debug.explain` — a layout-debug `Ui` modifier

```ipe
explain : Attribute msg
```

A modifier applied to one `Element` that draws visible outlines around that
element **and every descendant**, so the box model (bounds, padding, spacing) an
invisible layout tree produces becomes visible at a glance. Prior art: elm-ui's
`Element.explain`.

- **Placement and gate.** `explain` lives in `Ipe.Debug` (not `Ipe.Ui`), so
  membership alone makes `release` reject it. This is the decoupling: it does
  **not** take the `Debug.todo` marker as an argument (the mechanism other
  ecosystems use to make the call un-shippable). Ipê already has a cleaner,
  make-invalid-states-unrepresentable gate — module membership — so `explain` is a
  plain `Attribute msg` and `todo` and `explain` are independent siblings.
- **Semantics.** When the renderer encounters the `explain` attribute on an
  element, it outlines that element and, recursively, every descendant element,
  using distinct colors for element bounds versus padding (as elm-ui does). It
  only adds borders/outlines — it never changes layout, so what you see is the
  real box tree. The recursion is over the element's own child tree (`node` /
  `taggedNode` children), which the renderer already walks.
- **Targets.** Applies across the shapes `Ui` targets for the browser box model
  (Web / WebView). A Terminal-cell analog is a nice-to-have, not required.
- **Dependency.** Needs the `Ui` element/box tree, which exists (`node` /
  `taggedNode` → `Element msg`, `Attribute msg` modifiers rendered to inline
  style). No dependency on part B.

## Testing

- **A.** `release` on each app kind — pure native (plain optimized binary, no
  jail), native/FFI (jailed bundle), wasm (SRI-pinned browser bundle) — each
  builds and each rejects a program containing `Debug.log` with IPE-L0140; the
  same programs build under `build` / `run`. A pure app and a wasm app that the
  predecessor command refused now succeed under `release`. `--optimize` is gone
  (its removal does not silently turn the gate off for any path). Dev and release
  build caches stay disjoint.
- **B.** A module with a `Debug.todo` hole in one branch compiles and its other
  branches run/test; reaching the hole aborts with `TODO at <file:line>: <note>`
  through the Error path and a non-zero exit, never a panic; a `todo` in a case
  arm does not satisfy exhaustiveness; a `release` build rejects it (IPE-L0140).
- **C.** An element carrying `explain` renders outlines on itself and every
  descendant (bounds vs padding distinct) without altering layout; a `release`
  build rejects it (IPE-L0140).

## Out of scope

A **time-travelling debugger for TEA apps** — record every `(Msg, Model)`, scrub /
replay, export a session — is a much larger subsystem (a dev-mode message-loop
wrapper, an overlay UI, session import/export). It shares this family's dev-only
gate (it would live behind the same `release` rejection) and the total value
renderer, but it is designed separately. This document does not cover it.

The existing `Debug.log` value-inspection helper already ships; no change is
proposed to it here beyond inheriting the `release`-verb gate along with the rest
of `Ipe.Debug`.
