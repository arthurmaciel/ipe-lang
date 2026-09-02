# 44. Package coordination — manifest, index, resolver, lockfile, gate, enforced semver

Date: 2026-07-25

## Status

Accepted and implemented. The manifest schema lives in `src/ipe-cli/src/project.rs`;
the index reader in `index.rs`; the resolver in `resolve.rs`; the lockfile in
`lockfile.rs`; the interface diff and semver check in `diff.rs` and `api_surface.rs`;
the package gate in `audit.rs` (`ipe package audit`). The outward-facing
`ipe package publish` command and its index-repo PR flow are a separate, later
decision and are not covered here.

## Context

Ipê needs to depend on other Ipê packages and on native Rust crates, reproducibly and
without trusting an author's word about what a package does. Two axes of trust are in
play. A dependency's *source* must be pinned so a build is reproducible and a swapped
tree is detectable. And a dependency's *effects* — its capability set — must be
knowable, because a package that silently gains network or filesystem access is a
supply-chain hazard. A plain "publish and hope" registry gives neither guarantee.

The manifest also has to keep two dependency kinds distinct — Ipê packages and native
Rust crates — because they carry different trust models: a pure Ipê package's
capabilities are *inferred* by the compiler, while a native crate's must be *declared*
and enforced, since the compiler cannot see through native code.

## Decision

Coordinate packages through four cooperating pieces, each a parse-don't-validate
boundary, plus an enforced-semver rule and a merge-time gate.

**Manifest (`ipe.toml`).** Three typed sections: `[dependencies]` (Ipê packages),
`[rust.dependencies]` (native crates), and `[capabilities] declared` (the security
axes the project's native code is permitted). Each raw section is parsed into a typed
value on read; the capability vocabulary is re-exported from the compiler's kernel
registry so the manifest's declared set and the compiler's inferred set are the *same*
type, never two drifting string lists.

**Index.** One entry file per package at `packages/<name>.toml` in an index repository.
An entry carries the package name, its publisher, and a list of versions; each version
pins `source`, `rev` (an exact commit), `sha256` (a hash of the fetched source tree),
and the version's `capabilities`. The hash is what makes a swapped source tree
detectable; the pinned commit is what makes a build reproducible.

**Resolver.** `ipe add` reads the index entry, resolves the highest version matching
the request, fetches the pinned commit, and **verifies the fetched tree's hash against
the entry before trusting it** — hash-verify is the trust boundary, at the fetch point,
not later. Resolved dependencies are written to a lockfile.

**Lockfile (`ipe.lock`).** The exact resolved set, one `[[package]]` block per
dependency pinning name, version, source, rev, and sha256, held sorted by name so the
file is deterministic and diff-stable across machines.

**Enforced semver (`ipe diff`).** A package's public interface is projected into an
order-independent canonical form; two versions are diffed and each change classified as
compatible or breaking, fail-closed (an unproven change is treated as breaking). Ipê is
pre-1.0, so **major is reserved**: a breaking delta requires a *minor* bump, a
compatible delta (or a no-op re-release) a *patch* bump. The required bump is derived,
not trusted from the author.

**Gate (`ipe package audit`).** A universal Tier-1 check every package must pass before
an index entry is merged, run in security-first order: provenance panic-scan, capability
consistency (declared equals inferred over the Ipê-inferable set), enforced-semver
(the derived bump must be satisfied), and supply-chain. A failure is a typed rejection,
not a warning. The author's local run is advisory; the index CI's run is authoritative —
the trust chain is that the index vouches for a merged entry's pinned, hashed source.

Rejected: trusting author-declared capabilities without a consistency check; a mutable
"latest" pointer instead of pinned commit + hash; a floating (unlocked) dependency set;
and letting the author choose the version bump. Each reopens a trust or reproducibility
hole the pinned/hashed/gated design closes.

## Consequences

- A build is reproducible from the lockfile alone, and a tampered source tree fails the
  hash check at fetch time rather than running.
- Capability consistency is provable only over the Ipê-inferable set; native code's true
  set is *declared and enforced at runtime* (the runtime capability jail), not proven by
  this gate. This tiering must stay stated honestly: pure-Ipê capabilities are proven,
  native capabilities are declared-and-contained.
- The manifest's capability vocabulary must remain a single type shared with the kernel
  registry. If the manifest and the compiler ever key capabilities off two independent
  lists, the consistency check silently weakens.
- Enforced semver means a version number cannot lie about compatibility: the gate rejects
  an under-bumped breaking change. The fail-closed classification (unproven ⇒ breaking)
  must be preserved; relaxing it to "unproven ⇒ compatible" would let a breaking change
  ship as a patch.
- The gate is the merge boundary for the index. Publishing *into* that index — the
  `ipe package publish` command, the login/credential flow, and the PR-opening tooling —
  is deliberately a separate, later decision; the resolver and gate are sufficient to
  *receive and trust* a merged entry without it.
- The index *repository* side — the entry schema, an example entry, the fail-closed
  admission CI, and the entry validator (`ipe package validate-entry`, the resolver's own
  parser reused so validator and reader cannot drift) — was scaffolded in the compiler repo and
  now lives in the hosted registry repository `arthurmaciel/ipe-registry`. Only the hosted repo itself, its
  branch-protection rule making the admission workflow a *required* check, and its live
  entries are deferred to that repo; they cannot exist inside the compiler repo.
