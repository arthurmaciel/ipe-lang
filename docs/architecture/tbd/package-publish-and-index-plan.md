# `ipe package publish` + the curated index — spec & impl plan

Status: plan (tbd). The last outward-facing package-coordination piece. Every
piece it composes is built: capability inference, the `ipe rust`/manifest
surface, the index reader + resolver + lockfile (`index.rs` / `resolve.rs` /
`lockfile.rs`), the `ipe diff` enforced-semver check, and the universal Tier-1
`ipe package audit` gate. What is missing is the **publish path** (how an author
gets a version into the index) and the **index repository itself** (its layout,
its per-entry schema, and its CI trust model). This plan specifies both.

Fenced blocks are illustrative unless the prose says otherwise.

---

## 0. The shape in one paragraph

The curated index is a public git repository holding one entry file per package.
`ipe package publish` is a **thin, non-privileged** helper: it runs the same gate
`ipe package audit` runs, prepares the package's index entry, and opens a pull
request against the index repo. It has no write access to the index and no server
to trust. The index repo's **CI re-runs the gate** on the proposed entry and
merges only when green — so the authoritative accept/reject lives in CI, on
infrastructure the project controls, not on the author's machine. The resolver
and lockfile (already built) consume a merged entry unchanged: they read the
entry file, pick the version, fetch the pinned revision, and verify its content
hash before trusting a byte.

The trust chain is one line: **the index CI vouches that a merged entry's pinned
`sha256` names gate-passing source, and the resolver refuses any fetched tree
whose hash is not that pin.** Verify-before-trust at both ends.

Two integrity anchors are on the table — the `sha256`-of-tree the resolver
already computes, and the git commit `rev` — and the plan pins **both** (§3.5).
The tree hash must first be widened to cover a full checkout (symlinks + file
modes), because a source host that serves the same `rev` to CI and to a user
could otherwise smuggle a symlink past a hash that only covers regular-file bytes
(§3.5). Pinning the commit `rev` in addition closes that gap independently: a git
commit is content-addressed over the full tree (modes and symlinks included), so
a host cannot serve two different trees under one commit id.

---

## 1. `ipe package publish`

### 1.1 What it does

`publish` operates on the working package (the project rooted at `ipe.toml`) and
performs, in order:

1. **Gate locally** — run the exact `run_audit` path `ipe package audit` runs
   (universal Tier-1 always; the native tier when the package carries native
   `Rust.` code). A local reject aborts publish with the gate's own diagnostic —
   a submission is green before a PR is ever opened, so the index CI is not the
   author's first feedback.
2. **Compute the entry delta** — read the package's name, the version being
   published (from `ipe.toml`), its source URL, the exact revision to pin, the
   content `sha256` of the source tree at that revision (via
   `resolve::hash_source_tree`, the *same* hash the resolver later verifies), and
   the inferred + declared capability set. Render these into a new
   `[[version]]` block appended to the package's index entry file
   (`packages/<name>.toml`), creating the file if this is a first publish.
3. **Open the index PR** — push the entry-file change to a branch and open a pull
   request against the index repo. The command never writes to the index directly.

`publish` is *optional* and *thin* by design: the index accepts a version through
its own PR flow, so publishing stays an "open the index PR" helper rather than a
privileged command holding index credentials. An author who prefers to edit the
entry file and open the PR by hand gets the identical result — `publish` only
removes the toil and guarantees the pinned `sha256` matches what the resolver
will compute.

### 1.2 Inputs

| Input | Source | Notes |
|---|---|---|
| package name | `ipe.toml` `name` | the entry-file stem (`packages/<name>.toml`) |
| version | `ipe.toml` | the version being published; must be a clean semver |
| source URL | `ipe.toml` publish config or `--source` | the public git URL the resolver fetches |
| revision | resolved from the source repo `HEAD` (or `--rev`) | pinned exactly; never a moving branch |
| content sha256 | `resolve::hash_source_tree(source_tree)` | the exact hash the resolver re-computes and verifies |
| capabilities | inferred (Ipê) ∪ declared (native) | the same set `ipe capabilities` reports and the gate checks |

The revision and sha256 are **computed, not authored** — the author cannot
mistype the pin, and a hand-edited pin that does not match the tree is caught by
the resolver on the first `ipe add` regardless.

### 1.3 Relationship to `ipe package audit`

`publish` and `audit` share one code path. `audit` *is* the gate;
`publish` = `audit` + entry rendering + PR open. The index CI runs `audit` a
third time as the source of truth. Three runs, one `run_audit` — author
pre-flight, publish pre-flight, and authoritative CI cannot diverge, because
there is a single implementation. If `audit` passes locally and the index CI
rejects, the cause is an environment difference (toolchain, scanner version), not
a different check — which is why the gate pins its toolchain (§4, open question).

### 1.4 Opening the PR — no `gh` requirement

`publish` must not hard-require the `gh` CLI. Three layered mechanisms,
most-independent first (the design's accepted ordering):

- **Default — `git` push + browser-prefilled PR.** Push the entry branch to the
  author's fork with existing git credentials, then open the browser at GitHub's
  pre-filled compare URL (`…/compare/main...<author>:<branch>?quick_pull=1&title=…&body=…`).
  The author clicks "Create pull request" in an already-authenticated browser.
  Needs only `git` (already required) plus a browser opener — no `gh`, no HTTP
  client, no token stored in `ipe`. One-time cost: a fork of the index repo.
- **Headless / CI — GitHub API with a token.** For non-interactive publishing,
  call the API with `GITHUB_TOKEN` or a token from an `ipe login` device-code
  OAuth flow. The only path that needs a token.
- **Opportunistic — `gh pr create`.** If `gh` is detected and authenticated, use it.

Default to the first: it keeps `ipe` dependency-light, mirroring the choice to
resolve the index over `git` rather than add an HTTP client.

---

## 2. The curated index repository

### 2.1 Layout

The index is a public git repository. One file per package, sharded by first
character to keep any single directory small as the ecosystem grows:

```
index/                         # illustrative layout
├─ packages/
│  ├─ h/
│  │  └─ http-extras.toml       # packages/<shard>/<name>.toml
│  ├─ j/
│  │  └─ json-tools.toml
│  └─ …
├─ .github/workflows/
│  └─ gate.yml                  # the authoritative CI gate (§3)
└─ README.md                    # what the index is, how to publish
```

The single-char shard (`packages/h/http-extras.toml`) is a forward-looking
refinement of the resolver's current flat `packages/<name>.toml`. **Sharding is a
resolver change** — the reader computes the shard from the name and reads
`packages/<shard>/<name>.toml`. It is a small, isolated edit to `entry_path` in
`index.rs`; the entry *format* is unchanged. If the ecosystem stays small the
flat layout is retained and this refinement is skipped. Either way the entry
schema below is identical.

### 2.2 The per-entry schema

An entry maps a canonical name to every published version. The schema is exactly
what the built reader (`index.rs` `IndexEntry` / `EntryVersion`) already parses,
stated as the authoring contract:

- **`name`** (top-level, informational) — the authoritative name is the file
  stem; a mismatch is ignored, the stem wins.
- **`publisher`** (top-level, required) — the publishing account; provenance for
  the entry, shown in the PR and retained for attribution.
- one **`[[version]]`** block per published version, each with:
  - **`version`** — the exact semver (`semver::Version`); a malformed value is a
    hard parse error.
  - **`source`** — the source repository URL the resolver `git`-fetches.
  - **`rev`** — the exact commit fetched; a version pins a commit, never a
    moving branch.
  - **`sha256`** — the content hash of the source tree at `rev`, the integrity
    anchor the resolver verifies against before trusting a byte.
  - **`capabilities`** — the capability set (`BTreeSet<Capability>`): the inferred
    Ipê set ∪ the declared native set, each a known wire name (`network`,
    `filesystem`, `database`, `env`, `subprocess`, `clock`, `random`,
    `native-ffi`). An unknown name is a hard reject — a typo can never become a
    silently-dropped capability the user is not warned about.

Required per version: `version`, `source`, `rev`, `sha256`. `capabilities` absent
means "no capabilities". Unknown keys are ignored (forward-compatible), but a
malformed known value is a hard error.

#### A concrete example entry

```toml
# packages/h/http-extras.toml — illustrative, not a command
name = "http-extras"
publisher = "arthurmaciel"

[[version]]
version = "1.2.0"
source = "https://github.com/arthurmaciel/http-extras"
rev = "9f2c7b1e0a4d5c6f8b2a1e3d4c5b6a7f8e9d0c1b"
sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
capabilities = ["network"]

[[version]]
version = "1.3.0"
source = "https://github.com/arthurmaciel/http-extras"
rev = "a1b2c3d4e5f60718293a4b5c6d7e8f901a2b3c4d"
sha256 = "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"
capabilities = ["network", "clock"]
```

A native-bearing package additionally lists `native-ffi`, which the resolver
surfaces loudly at `ipe add` and which routes the submission through the native
review path (§3.3).

### 2.3 How the resolver + lockfile consume it

Unchanged from the built `resolve_and_add` flow — the index repo is exactly the
checkout the resolver already reads (`IPE_INDEX_DIR`, else the per-user cache):

1. **Read** the entry file for `<name>` (`index::read_entry`) → typed `IndexEntry`.
2. **Resolve** the highest `[[version]]` satisfying the requirement
   (`index::resolve_version`).
3. **Fetch** that version's `source` at its exact `rev` into the package cache
   (`git` subprocess — no HTTP-client dependency).
4. **Verify** the fetched tree's sha256 equals the entry's `sha256`
   (`verify_hash`). **A mismatch is a hard `CliError::HashMismatch`, never a
   warning** — nothing derived from an unverified fetch is written.
5. **Record** — only after the hash matches: pin the exact `(name, version,
   source, rev, sha256)` in `ipe.lock` (`Lockfile::upsert`), write the
   requirement into `ipe.toml` `[dependencies]`, and print the resolved version +
   its capability set for consent (loud on `native-ffi`).

The lockfile makes a build **reproducible from the pins, not from the index being
reachable**: a later build reads `ipe.lock`, re-fetches each pinned revision, and
re-verifies each `sha256` (`verify_lockfile_hashes`). The index can be offline,
renamed, or gone — the locked build is unaffected and still integrity-checked.

The `{git=}` / `{path=}` escapes (`resolve_escape`) bypass the index entirely —
the loud, visible escape hatch for private/fork/experimental dependencies — but
still hash-and-lock what they point at, so a reader sees exactly which
dependencies are un-vetted and every dependency carries lockfile integrity.

---

## 3. The trust model

### 3.1 Verify-before-trust, at both ends

Two independent checks pin the chain, and an entry is trusted only when both hold:

- **At the index (submission time):** the CI gate re-runs `ipe package audit` on
  the proposed entry — fetch the source at the pinned `rev`, recompute the
  `sha256` and confirm it equals the entry's pin, and run the full universal gate
  (provenance panic-scan, capability consistency, enforced semver, supply chain)
  plus the native tier when `native-ffi` is present. The entry merges only when
  green. This vouches that a merged pin names gate-passing source.
- **At the user (install/run time):** the resolver refuses any fetched tree whose
  sha256 is not the entry's pin, and the runtime sandbox confines the package to
  exactly the consented capability set. This holds even if the index or the
  source host is later compromised — the pin is immutable once locked, and a
  tampered tree fails the hash.

Neither check alone suffices: a submission-only gate says nothing about what a
package does on a user's machine; a resolver-only check would faithfully install
a malicious-but-hash-honest package. Both are required.

### 3.2 Who can merge to the index

- **No human merges on judgment.** Merge is gated on the CI gate passing —
  green-required branch protection on the index repo's default branch, so a
  maintainer cannot click-merge a red submission and a passing submission needs
  no maintainer availability. The gate, not a person, is the authority.
- **The gate runs on untrusted PR content in a sandbox.** A submission is
  attacker-controlled source; its build/test/scan runs in the `ipe_sandbox` RCE
  jail (the same jail crate inspection uses), so a malicious `build.rs` or
  proc-macro is contained on the CI host. The gate's own workflow uses
  least-privilege CI tokens and does not expose index-write credentials to
  submission code.
- **The CI trigger must not hand a fork PR write-scoped secrets.** The gate
  workflow runs on the *untrusted head* of a fork PR, so it must use the trigger
  that gives the fork run a read-only token with no repository secrets — never the
  variant that runs in the base-repo trust context with write scope. A merge is
  performed by a *separate*, trusted job that runs only after the gate is green
  and only reads the already-validated entry file; the attacker-controlled build
  never shares a process, a token, or a secret with the merge step. Getting this
  trigger wrong is the single highest-value attack on the whole scheme (index-write
  credential exfiltration), so it is called out explicitly rather than left to the
  CI author.

### 3.3 How a malicious or typo'd entry is rejected

| Attack / mistake | Rejected by |
|---|---|
| **Typosquatted name** (`htttp-extras`) | one canonical name line per package (no squatting on an existing name); a *new* name is a new entry file the gate + maintainer review scrutinises, and the visible publisher makes an impersonating publisher attributable |
| **Hidden effect** (code uses network, manifest omits it) | capability-consistency check — used-but-undeclared is a hard reject; the declared set must *equal* the inferred set |
| **Over-broad claim** (declares more than it uses) | same check — declared-but-unused rejects, so the consented set is exactly the truth |
| **Tampered / wrong pin** (sha256 ≠ tree) | the gate recomputes the sha256 from the fetched `rev` and rejects a mismatch; the resolver re-rejects at install |
| **Silent breaking change as a patch** | enforced-semver check (`ipe diff` vs the previous published version) — an under-bump rejects |
| **Malicious build-time code** (`build.rs`, proc-macro) | sandboxed build in `ipe_sandbox`; native tier only |
| **Unknown / typo'd capability** (`netwrok`) | the entry parser rejects an unknown wire name at read time — never a silently-dropped capability |
| **Compromised source host post-publish** | the pin is immutable; a swapped tree fails the resolver's hash verify at every install and every locked rebuild — **once the tree hash covers a full checkout (§3.5)** and the git `rev` is re-verified against the pin |
| **Symlink / mode smuggling** (same `rev`, CI sees a clean tree, user sees an added `evil -> /etc/passwd` symlink or an execute bit) | closed by widening the tree hash to include symlink targets + file modes, and by re-verifying the fetched git `rev` equals the pin (a git commit is content-addressed over the full tree). §3.5 — **a design correction, not yet in the built `hash_tree`** |
| **Un-vetted transitive dependency via a `{git=}`/`{path=}` escape** (a gate-passing package pulls an un-gated dependency whose network/native effects the top-level capability set never declared) | the gate must run capability inference + verify-before-trust over the **escape closure**, not just the package's own source, and surface any escape-reached capability at `ipe add`. §3.6 — **a design correction** |

A native-bearing package (`native-ffi` present) additionally runs miri, cargo-audit
+ cargo-deny over its crate graph, the declared-native-capability sandbox check,
carries a visible "contains native code" label on its entry, and takes a longer
review path (manual sign-off / verified publisher). Native code is the residual
surface: declaration + consent + fail-closed sandbox + the full audit gate + the
visible label make it loud, attributable, gated, and opt-in — not magically safe.

### 3.4 Honest limits

- **Pure-Ipê packages** carry no native foothold — effects are kernel-mediated and
  capability-inferred, so a malicious effect must appear as a declared capability
  the user consents to. It cannot hide.
- **Native-bearing packages** are the residual surface; the v1 coarse sandbox
  (namespaces + `prlimit`) covers the high-value capabilities (network,
  filesystem, env, subprocess) fail-closed, and the fine-grained seccomp
  capability→syscall map is a tracked v2. v1 is honest about the tier rather than
  overclaiming airtight native enforcement.
- **A first publish of a brand-new name** is the one place a machine-checkable
  identity check still matters (typosquat / impersonation): the automated gate
  cannot know a name is *meant* to deceive. The defense is a **verified-publisher**
  binding, not maintainer taste — the `publisher` field must be an
  authenticated GitHub identity (the account that opened the index PR), so a new
  name is attributable to a real account and an impersonating publisher is
  refused mechanically rather than by a reviewer's judgment. This keeps §3.2's
  "the gate, not a person, is the authority" intact even for new names: the human
  step, if any, is a bounded appeal, not the merge gate.

### 3.5 The integrity anchor must cover a full checkout

The `sha256`-of-tree the resolver computes today (`cache::hash_tree`) hashes
regular-file bytes only — it does **not** hash symlink targets or file modes.
Left as-is, that under-covers a real git checkout: a malicious source host could
serve CI a clean tree and a user a tree with an added `evil -> /etc/passwd`
symlink (or an added execute bit) at the *same* revision, and both would produce
the same tree hash. Two independent corrections, both required:

- **Widen the tree hash** to include symlink presence + target and file mode, so
  the hash actually characterises the checkout it claims to.
- **Pin and re-verify the git commit `rev`.** A git commit id is content-addressed
  over the full tree (modes and symlinks included), so a host cannot serve two
  different trees under one commit. The resolver must confirm the fetched checkout
  is exactly the pinned `rev`, not merely that its tree hash matches — belt and
  braces, since the two anchors fail independently.

Until the tree hash is widened, the sole load-bearing anchor is the git `rev`;
the plan pins both so neither is a single point of failure.

### 3.6 The gate must cover the escape closure

Capability inference and verify-before-trust run over a package's *own* source.
A gate-passing package that pulls a transitive dependency through a
`{git=}`/`{path=}` escape can therefore reach effects (network, native FFI) that
the top-level capability set never declared — the escape is un-gated by design,
but its capabilities must not become invisible just because a gated package sits
above it. The gate must:

- run capability inference over the **escape closure** (the package plus every
  dependency it reaches, including escape-resolved ones), and require the declared
  set to cover the closure, not just the root;
- apply verify-before-trust (hash-and-lock) to every escape-reached dependency, as
  the resolver already does per dependency; and
- surface any escape-reached capability at `ipe add`, loud on a native-ffi reached
  only through an escape.

A pure-Ipê package with no escapes is unaffected — its closure is its own source.
The correction matters precisely for a package that launders un-vetted code in
through an escape while presenting a clean top-level capability set.

---

## 4. Impl breakdown

Bite-sized, one commit per task, TDD against fixtures (a fixture index git repo +
a fixture source repo in temp dirs — no network), mirroring the resolver and gate
lanes that came before.

1. **Entry rendering** — a pure `render_entry_version(name, publisher, EntryVersion)`
   that produces the `[[version]]` TOML the reader parses, and a
   `merge_into_entry` that appends a version to an existing entry file (or creates
   a first entry). Fixture: rendered output round-trips through `index::read_entry`
   back to the same `EntryVersion`; a second version appends without disturbing the
   first. No network, no PR.
2. **Publish pre-flight + entry compute** — `ipe package publish` skeleton:
   resolve the package's name/version/source/rev, compute the `sha256` via
   `resolve::hash_source_tree`, gather the capability set, run `run_audit`
   (abort on reject with its diagnostic), and write the rendered entry to a
   scratch entry file. Fixture: a clean package produces the right entry; a
   package failing any universal check aborts before any entry is written.
3. **PR open — default path** — `git` push to the author's fork + browser-prefilled
   compare URL; `--no-open` prints the URL for headless/manual use. Fixture: the
   branch/commit is created and the compare URL is well-formed (assert the URL, do
   not actually open a browser or hit GitHub).
4. **Index-repo CI gate** — the index repo's `gate.yml`: on a PR, check out the
   proposed entry, run `ipe package audit` against the pinned source, and require
   green to merge (branch protection). The emitted-Rust panic-scan provenance case
   (a compiler bug) routes to the *compiler's* CI, not the index PR. This step is
   index-repo config, not `ipe` source.
5. **Sharded layout (optional)** — teach `entry_path` the single-char shard and
   migrate fixtures; skip if the flat layout suffices. Isolated `index.rs` edit,
   entry format unchanged.
6. **Headless token path + `gh` opportunistic path (optional)** — the API-token
   and `gh pr create` mechanisms for non-interactive publishing, behind the same
   entry-compute core.

Two **security corrections from the guardian review** are prerequisites, not
optional refinements, and land before the index serves untrusted packages:

- **Widen `cache::hash_tree`** to include symlink targets + file modes, and have
  the resolver re-verify the fetched git `rev` equals the pin (§3.5). This is an
  isolated `cache.rs` + `resolve.rs` edit with a fixture: a tree differing only by
  an added symlink or an execute bit must produce a different hash and be rejected.
- **Extend the gate + resolver to the escape closure** (§3.6): capability
  inference and hash-and-lock over escape-reached dependencies, with escape-reached
  capabilities surfaced at `ipe add`. Fixture: a package whose only network access
  is through a `{git=}` escape has `network` in its required set or is rejected.

Steps 1–4 deliver a working publish + authoritative gate for pure-Ipê and
hash/semver/capability-honest packages, once the two corrections above are in.
Steps 5–6 are refinements. Native-tier enforcement in the gate lands with the FFI
Tier 2 capability layer (per the gate plan), not here — this plan wires the
*publish path and the index repo* around the gate that already exists.

---

## 5. Open questions

- **New-name review policy.** The exact human step for a first-publish of a
  brand-new name (typosquat / impersonation defense): a maintainer approval, a
  publisher-verification step, or a cooling-off window. A new *version* is fully
  automated; a new *name* is the one place judgment still enters.
- **Publish config in `ipe.toml`.** Where the source URL lives when it is not
  inferable from the git remote — a `[publish]` section vs a `--source` flag vs
  reading the `origin` remote. Prefer explicit-in-manifest over a magic remote name.
- **Auto-merge wait times per tier.** The design leaves per-tier merge timing
  (immediate for pure-Ipê vs a review window for native) as an open decision.
- **Index bootstrap.** Provisioning and seeding the real public index git repo is
  a deliberate outward-facing step (its GitHub location, its branch protection,
  its first entries), done once, outside this lane.
- **Toolchain pinning for gate reproducibility.** The author's local `audit` and
  the index CI must reach the same verdict — pin the toolchain + scanner version
  the gate runs with, so a green local run is a green CI run.
- **Revocation / yank.** The plan makes publishing safe but says nothing about
  *un*-publishing a version later found malicious or vulnerable. Decide the
  mechanism: a `yanked` flag on a `[[version]]` the resolver refuses for a fresh
  add but honours for an existing lock, versus outright entry removal (which breaks
  a locked rebuild). A yank flag is the safer default — reproducibility of an
  existing lock is preserved while new installs are steered away.

---

## Relationship to sibling specs

- Realizes the publish path and index repo left open by
  `package-coordination-and-capabilities-design.md` (the accepted design,
  superseding D6 of `namespace-imports-and-packaging-spec.md`) and the shipped
  package gate (`docs/adr/0044-package-coordination-manifest-index-gate.md`).
- Consumes, unchanged, the resolver + lockfile (`index.rs`, `resolve.rs`,
  `lockfile.rs`) and the `ipe package audit` gate (`audit.rs`) — this plan adds
  the *authoring* and *hosting* halves around them.
- The universal gate rests on the existing SEAL guarantee; the native tier rests
  on the existing FFI `ipe_sandbox`.

The trust model in §3 was reviewed by the security-soundness guardian:
**sound with caveats**. The two-ended verify-before-trust architecture is
correct; the review surfaced two must-fix gaps now folded in as prerequisites —
the tree hash under-covering a checkout (symlinks + modes, §3.5) and the un-gated
escape closure (§3.6) — plus the CI-trigger hardening (§3.2) and the
verified-publisher resolution of the new-name case (§3.4). Those corrections are
first-class steps in §4, not deferred.
