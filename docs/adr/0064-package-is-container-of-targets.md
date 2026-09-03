Status: Accepted
Date: 2026-09-02

# 0064. A package is a container of disjoint targets

## Context

A project needs one unit the toolchain builds, publishes, and depends on. That
unit must express two different kinds of thing: a *library surface* (the public
modules other packages import) and *runnable programs* (executables, each with a
shape and an entry point). A naive model blurs these — a "project" that is
sometimes a library, sometimes a binary, with optional fields for whichever it
is not — which forces `Maybe`-typed configuration and makes illegal combinations
representable (a shape on a library, an exposed-module list on a bare script).

There is also a cardinality question. How many library surfaces may one package
have? How many programs? And where do build settings that apply to the whole
build (dependency set, toolchain profile, database driver) live relative to
settings that only make sense for one program shape (a browser bundle's hydrate
mode)?

## Decision

A **package** is a container of **targets** carrying shared identity — one
manifest, one entry in the registry, one published namespace. It holds:

- **at most one library surface** — its exposed modules; the empty list means
  "no public API";
- **zero or more programs** — each a `{ name, shape, entry }`; the empty list
  means "ships no executable".

`Program` and `Library` are **disjoint target kinds**. A library is just its
exposed modules — no shape, no entry, no build config. A program has a shape and
an entry. The package composes the two; it never merges them into one
ambiguous kind.

Emptiness carries "none", so no `Maybe` is needed: `exposedModules = []` is a
pure program, `programs = []` is a pure library, both non-empty is a library
plus thin programs over it.

Cardinality is fixed as **one library surface, N programs**. One package is one
published namespace and therefore has exactly one public API; several
*independent* libraries are not one package but a **workspace** of packages.
Multiple programs sharing one internal core is common and useful, so programs is
a list.

Build configuration (dependency resolution, toolchain profile, database driver,
static/target/allocator settings) is **package-wide** — one profile produces all
of a package's programs, mirroring how a package resolves one dependency set.
Shape-specific options are the exception and ride with each program's `shape`
variant, so an option irrelevant to a given shape is unrepresentable there.

Rejected alternatives:

- **A flat "project" that is a library or a binary with optional fields.**
  Makes illegal shapes representable and forces `Maybe` everywhere; the disjoint
  target kinds remove both problems.
- **Many library surfaces per package.** Breaks the one-package/one-namespace
  identity; independent libraries belong in a workspace.
- **Per-program build profiles.** A package resolves one dependency set and one
  toolchain; letting each program diverge would fracture that single resolution.

## Consequences

Scaffolding follows the target kinds directly: the default creates a program, a
`--lib` flag creates a library, and the "both" path creates a library with a
thin program over it — the same tree the manifest expresses.

The invariant that must hold: a package is exactly one shared identity plus one
optional library surface plus a list of programs, with the library/program kinds
kept disjoint. A future need for several independent libraries developed
together is met by composing packages into a workspace, not by relaxing the
one-library-surface rule.
