Status: Accepted
Date: 2026-09-02

# 0065. Per-capability web disclosure with a fail-closed app-boundary gate

## Context

A program's port surface lets it talk to page JavaScript over a raw transport.
That transport carries a single coarse capability meaning "this program talks to
page JS." It does *not* say *which* host capability the far side reaches — a port
to a geolocation handler and a port to a clipboard handler are
indistinguishable, both merely "talks to page JS."

As a web standard library grows thin wrappers over that transport — geolocation,
clipboard, camera, notifications, each a hand-written handler reaching one Web
API — the coarse axis becomes a supply-chain hazard. A dependency that imports a
clipboard wrapper reaches the clipboard *invisibly*: the app's capability set
shows only the coarse "talks to JS" grant, which the app already made for its
own unrelated use. One grant silently authorizes every Web capability any
transitive dependency can reach — capability-escalation through a coarse axis.

## Decision

Replace the single coarse axis at the web boundary with a **closed per-capability
vocabulary the compiler owns**, and gate grants at the application boundary,
fail-closed.

- **Closed vocabulary.** The set of web capabilities (geolocation, clipboard,
  camera, notifications, …) is a fixed vocabulary the compiler owns, modelled as
  a **sub-axis** of the port capability, not as flat sibling variants. A wrapper
  cannot invent a capability outside the vocabulary.

- **Port-bound disclosure.** Each capability-bearing wrapper is bound to its
  specific sub-axis so the capability scan infers the *precise* capability from
  the reachable code, rather than collapsing everything to the coarse axis.

- **Fail-closed app-boundary consent.** Only the top-level application may grant
  a web capability. A dependency that reaches an un-granted capability is a
  **compile error that names the dependency**. Absence of a grant denies; there
  is no implicit or inherited grant, and a coarse grant for one capability does
  not cover any other.

Rejected alternatives:

- **Keep the single coarse axis.** Leaves the escalation open — one grant
  authorizes every reachable Web capability.
- **A flat sibling variant per capability instead of a sub-axis.** Loses the
  structural relationship "these are all refinements of the port transport" and
  invites the same coarse grant to be re-granted piecemeal without the
  compiler's totality over the vocabulary.
- **Grant anywhere in the dependency tree.** Lets a dependency self-authorize;
  concentrating the grant at the app boundary is what makes an un-granted reach
  a detectable, named compile error.

## Consequences

The application's capability set names exactly which Web capabilities its whole
dependency closure can reach; adding a dependency that reaches a new one is a
compile error until the app explicitly grants it. Capability escalation through
the coarse axis is closed by construction.

The invariant that must hold: every web-reaching wrapper is bound to a capability
in the closed vocabulary, the scan infers capabilities from reachable code (not
from declaration alone), and the grant lives only at the app boundary and
fails closed. Introducing a wrapper that reaches a Web API without binding it to
a vocabulary capability would re-open the invisible-reach hole this decision
closes.
