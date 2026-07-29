Status: Accepted
Date: 2026-07-29

# 0053. The module namespace encodes the trust boundary: `Ipe.` (first-party) and `Rust.` (FFI)

## Context

A module's name is the first thing a reader and the compiler see about where its
code comes from. Two origins carry different trust: the first-party standard
library, which the compiler ships and vouches for, and foreign native code
reached through FFI, which crosses out of the safe, sound Ipê surface into
arbitrary Rust. A reviewer auditing a program for its native footprint, and the
compiler deciding which import-resolution and FFI rules to apply, both need that
distinction to be unambiguous and mechanical — not a naming convention an author
can accidentally or maliciously blur.

Two failure modes motivate encoding the boundary in the name itself:

- **Std-squatting.** If a user or third-party module could resolve under the
  first-party prefix, it could impersonate the blessed standard library — an
  attacker foothold, since imports of a trusted name would silently bind to
  untrusted code.
- **Invisible native crossings.** If native FFI calls were spelled like any
  other qualified call, no purely-syntactic pass could enumerate a program's
  native footprint. The sites that need sandboxing, `Task` effect typing, and
  the unsafe audit would have to be discovered semantically, and any missed site
  would be an unaudited crossing.

## Decision

Exactly two module prefixes are compiler-meaningful, and each names a side of
the trust boundary.

- **`Ipe.*` is reserved and compiler-owned.** It resolves only to the blessed
  first-party stdlib. A user or third-party module can never *be* `Ipe.X`; the
  resolver rejects any such module with the reserved-namespace gate
  (IPE-N0025 / IPE-N0026). Trust is the *tag*, not the spelling — a hostile
  module that declares `module Ipe.Palette` is rejected precisely because it is
  not blessed, independent of the name it chose.

- **`Rust.*` is the FFI boundary, and this is an invariant, not a convention.**
  *Every* native crossing is spelled `Rust.`, everywhere, regardless of which
  library ships it — a third-party package that itself does FFI still surfaces
  as `Rust.` at the call site. Only `Rust.*` reaches the native-call lowering
  path, where the FFI-specific rules (sandbox admission, `Task Error a` effect
  typing, unsafe audit) are applied. The payoff is an audit primitive: a plain
  textual scan for `Rust.` enumerates every native crossing in a program.

Bare *names* are not globally reserved (only the `Ipe.` *prefix* is). Reserving
the word `List` across the ecosystem would punish library authors and make every
future stdlib module a backward-compatibility break. Instead, a bare name
resolves to its canonical `Ipe.*` module, and a colliding third-party module of
the same name never *silently* shadows it: a silent shadow is a compile error,
while a deliberate, visible `import Acme.List as List` rebind is allowed. The
reader always knows a bare `List` is the stdlib unless a visible `as List` says
otherwise.

Alternatives rejected:

- **Origin-and-version baked into the module name** (an `import
  Github.Com.…V84 as Stripe` shape) was rejected as brittle (a repository move
  breaks source), verbose, and a leak of infrastructure into code. Names
  describe *what*; the manifest and lockfile own *where* and *which version*.
- **A naming convention rather than an enforced invariant** for native crossings
  was rejected because a convention an author can forget or evade cannot back an
  audit primitive.

The origin and versioning rules for the libraries *behind* these prefixes are
separate decisions: the ambient-import tiering is fixed by ADR 0047, and package
coordination, FFI crate origins, and the curated index are fixed by ADR 0044.
This decision is only the boundary the prefixes name.

## Consequences

- The reserved-namespace gate must hold for the entire `Ipe.` prefix, not just
  individual stdlib module names, so that no untrusted module can ever resolve
  under it. This is fail-closed by construction: absent proof a module is
  blessed, resolving it under `Ipe.` is rejected.
- Native-call lowering and the FFI rule set (sandbox, effect typing, unsafe
  audit) key on the `Rust.` prefix, so a value that reaches the native path
  without that spelling is a bug; the invariant is what lets a syntactic scan be
  a complete audit.
- Adding a first-party stdlib module is a non-breaking change: it claims a name
  under the reserved prefix, and any third-party module of the same bare name
  keeps working through the explicit-`as` shadowing rule.
- The distinction stays a purely mechanical property of the name, so tooling
  (audit, review, the resolver) never needs semantic analysis to answer "is this
  first-party, or a native crossing?"
