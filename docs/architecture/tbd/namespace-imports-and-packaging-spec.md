# Namespace, imports & packaging redesign — spec

> **Status: Accepted (design) — not yet implemented.** Becomes an ADR once
> landed (per `docs/adr/README.md`).
>
> Scope: the canonical import surface for the language after the Ipê→Ipê rename.
> Governs first-party stdlib, the FFI boundary, local modules, and third-party
> packages — and which library *origins* are allowed for each.

## Decisions

### D1. Two compiler-meaningful prefixes: `Ipe.` (first-party) + `Rust.` (FFI)

The namespace *encodes the trust boundary*:

- **`Ipe.*`** — first-party stdlib. **Reserved and compiler-owned**: it resolves
  only to the blessed stdlib; a user/third-party module can never *be* `Ipe.X`
  (extends the existing IPE-N0025 hostile-std-squat gate to the whole prefix).
- **`Rust.*`** — the FFI boundary (native crate calls). **Invariant, not
  convention:** *every* native crossing is spelled `Rust.`, everywhere,
  regardless of which library ships it — so a third-party package that does FFI
  still surfaces as `Rust.` at the call site. `rg '\bRust\.'` enumerates every
  native crossing in a program — an audit primitive. The compiler applies
  FFI-specific rules (sandbox, eff/`Task` typing, unsafe audit) exactly at these
  sites.

Collapses today's `Ipe.*` **and** `Ipe.*` into the single `Ipe.*` surface.

### D2. The implicit ambient surface — see the tiered-auto-import ADR

`Ipe.List` is the canonical, enforced name. Which core names are ambient without
an import — and the rule that library functions are always explicit and
qualified — is decided in
[`docs/adr/0047-basics-and-tiered-auto-import.md`](../../adr/0047-basics-and-tiered-auto-import.md)
(`Ipe.Basics` + Tier A/B implicit, Tier C explicit). Principle: **loud on the
dangerous thing (`Rust.`), quiet on the safe default.**

### D3. Name-shadowing: precedence + explicit alias, never global reservation

The `Ipe.` *prefix* is reserved (D1). Bare *names* are **not** reserved across
the ecosystem — reserving the word `List` would punish library authors and make
every future stdlib module a backward-compat break.

- Bare `List` always resolves to the canonical `Ipe.List`.
- A third-party module may be named `List`; it does **not** silently shadow the
  ambient name. Using it requires an explicit alias: `import Acme.List as L`, or
  a deliberate, visible `import Acme.List as List` to rebind.
- **Silent shadow → compile error; explicit `as` shadow → allowed.** The reader
  always knows bare `List` = stdlib unless a visible `as List` says otherwise.

### D4. Names describe *what*; the manifest/lockfile owns *where* + *which version*

Reject Ipê's `import Github.Com.Stripe.StripeGo.V84 as Stripe` pattern — baking
origin+version into the module name is brittle (a repo move breaks source),
verbose, and leaks infra into code. Instead:

- Code uses a short alias: `import Rust.Stripe as Stripe` / `import Db`.
- `ipe.toml` maps the alias to a source + version; a lockfile pins the exact
  resolved version + content hash. Renames/upgrades touch the manifest, never
  source.

### D5. FFI origins — crates.io default, git/path as escape hatch

We emit Rust, so crates.io *is* the FFI registry: versioned, immutable
(yanked ≠ deleted), checksummed, lockfile-native. FFI binds a crates.io crate by
name+version by default; `{ git = … }` / `{ path = … }` are allowed escape
hatches exactly as Cargo provides. No bespoke github-path naming scheme.

### D6. External Ipê packages — decentralized source, integrity by lockfile, no central gate

Learn from Elm's centralization pain (one gatekeeper, one server, no private/git
deps). Adopt the Go model:

- **Any git repo resolves** as a dependency; integrity via lockfile + a checksum
  database. Private and forked deps work out of the box.
- A central **index is optional and discovery-only** (short-name → source URL);
  it must never be required to publish or build. Reproducibility comes from the
  lockfile, not from a registry gate.

## Compiler enforcement points (where each rule lives)

| Rule | Enforced at |
|---|---|
| `Ipe.*` reserved / unsquattable (D1) | resolver — reject a user module resolving under `Ipe.` (generalize IPE-N0025) |
| `Rust.*` = FFI, greppable invariant (D1) | resolver + FFI lowering — only `Rust.*` reaches the native-call path; sandbox/`Task` typing applied there |
| auto-prelude set (D2) | resolver import-injection; fixed documented list |
| no silent shadow of a prelude name (D3) | resolver — bare name binds `Ipe.*`; a colliding unaliased import is an error; `as` rebind allowed |
| origin/version out of module names (D4) | manifest + lockfile resolver; module names carry no origin |
| crates.io default + git escape (D5) | dependency resolver / `ipe.toml` schema |
| decentralized packages + lockfile integrity (D6) | package fetcher + lockfile/checksum store |

## Migration relationship

- Executes as part of / after the Ipê→Ipê rename (`docs/rename/`), since it
  renames the stdlib import surface. The rename's rule table already **DEFERS**
  the stdlib namespace to this redesign — this spec is that deferred work.
- `flat-namespace-redesign.md` is superseded by this document.

## Open items (decide before implementation)

- Exact auto-prelude module set (which `Ipe.*` modules are bare by default).
- `ipe.toml` dependency-table schema (crates.io vs git vs path; FFI vs Ipê dep).
- Lockfile format + checksum DB location (self-hosted vs reuse an existing sumdb
  shape).
- Whether local project modules need any prefix at all, or are simply "any name
  that is not preluded and not `Ipe.`/`Rust.`" resolved from the source tree.
