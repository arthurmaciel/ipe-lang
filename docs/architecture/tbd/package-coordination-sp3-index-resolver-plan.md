# SP3 — Index schema + resolver + lockfile: implementation plan

> **For agentic workers:** implement task-by-task, TDD, one commit per task. Steps
> use checkbox (`- [ ]`). All fenced blocks are **illustrative targets, not commands
> to run** — bind exact tokens against the real tree; `cargo` lines are the TDD loop.

**Goal:** make `ipe add <name>` resolve a public Ipê package through the curated
index — read the entry, pick the version, fetch the source at its pinned revision,
verify its content hash, write the lockfile, and record it in `ipe.toml` — plus the
`{git=}` / `{path=}` escape resolution. Replaces SP2's `ipe add` stub.

**Architecture:** the index is a git repository (per the accepted design); fetching
uses `git` (already required) — no new HTTP-client dependency. Content integrity is
a sha256 over the fetched tree (reuse `src/ipe-cli/src/cache.rs`'s `sha2`). The
lockfile pins exact resolved versions + hashes so a build is reproducible from the
lock, not from the index being reachable. Tests run against a **local fixture index
git repo** built in a temp dir — no network, no real GitHub dependency.

**Tech Stack:** `ipe-cli` (`src/ipe-cli`), `git` (subprocess), `sha2` (already a
dep), `semver` (already a dep, used by SP2's `IpeDep::Index(VersionReq)`), `toml`.

## Global Constraints

- Principle order (strict tie-breaker): Security > Correctness > Soundness >
  Efficiency > Completeness > Readability.
- The SEAL is untouched (CLI + resolution surface, not emission).
- **Parse, don't validate:** the index entry and lockfile are parsed into typed
  values (a typed `IndexEntry`, `LockedDep`), never stringly-typed maps threaded on.
- **Security at the resolve boundary:** a fetched package is trusted only after its
  sha256 matches the index-pinned hash; a mismatch is a hard, typed error (never a
  warning). Capabilities from the index entry are surfaced at `ipe add` for consent
  (loud on `native-ffi`), reusing SP1's `Capability`.
- **Deterministic + offline-reproducible:** a resolved build reads the lockfile's
  pinned hashes; the index need not be reachable once locked.
- `{git=}` / `{path=}` escapes bypass the index by design (the loud escape hatch) but
  still carry lockfile + checksum integrity.
- Comments say WHAT not HOW; no archaeology outside `docs/adr/`; self-explaining
  names. Commits scoped, plain messages, no AI attribution / no trailer (hook-enforced).

## Out of scope (later / deliberate)

- **Provisioning the real public index git repo** (its GitHub location, seeding) — an
  outward-facing step done deliberately, not in this lane. SP3 targets the *resolver*
  and is validated against a fixture index.
- `ipe package publish` (opens the index PR) — SP3 defines the entry schema it will
  write; the publish command + its PR flow is SP4/its own slice.
- The gate CI + sandbox enforcement — SP4.

## File structure

- `src/ipe-cli/src/index.rs` (**create**) — the index entry schema (`IndexEntry`,
  `EntryVersion`) + reading an entry from an index checkout (a file per package).
- `src/ipe-cli/src/lockfile.rs` (**create**) — `ipe.lock` format + read/write
  (`Lockfile`, `LockedDep`), deterministic serialization (sorted).
- `src/ipe-cli/src/resolve.rs` (**create**) — `resolve_and_add`/`resolve_and_remove`:
  index resolution, git fetch at rev, sha256 verify, lockfile + `ipe.toml` update;
  `{git=}`/`{path=}` escape resolution.
- `src/ipe-cli/src/pkg.rs` (**modify**) — replace the `run_add`/`run_remove` stubs
  with calls into `resolve`.
- `src/ipe-cli/src/project.rs` (**modify**, minimal) — a helper to write a resolved
  dependency back into `[dependencies]` (rename/upgrade touches manifest, not source).

## Index + lockfile formats (concrete)

Index entry — one file per package at `packages/<name>.toml` in the index repo:
```toml
# Illustrative index entry (not a command)
name = "http-extras"
publisher = "arthurmaciel"
[[version]]
version = "1.2.0"
source = "https://github.com/arthurmaciel/http-extras"
rev = "9f2c…"            # exact commit
sha256 = "abcd…"          # hash of the fetched source tree
capabilities = ["network"]
```

Lockfile — `ipe.lock` at project root:
```toml
# Illustrative ipe.lock (not a command)
[[package]]
name = "http-extras"
version = "1.2.0"
source = "https://github.com/arthurmaciel/http-extras"
rev = "9f2c…"
sha256 = "abcd…"
```

---

### Task 1: Index entry schema + reader

**Files:** Create `src/ipe-cli/src/index.rs`; Test: inline `#[cfg(test)]`.

**Interfaces:**
- Consumes: `ipe_kernels::Capability` (SP1), `semver::Version`.
- Produces: `struct IndexEntry { name: String, publisher: String, versions: Vec<EntryVersion> }`;
  `struct EntryVersion { version: semver::Version, source: String, rev: String, sha256: String, capabilities: BTreeSet<Capability> }`;
  `fn read_entry(index_root: &Path, name: &str) -> Result<IndexEntry, CliError>` (reads
  `packages/<name>.toml`); `fn resolve_version(entry: &IndexEntry, req: &semver::VersionReq) -> Result<&EntryVersion, CliError>`
  (highest version matching the req; none → typed error).

- [ ] **Step 1: Write the failing test** — a fixture entry file parses; version
  resolution picks the highest match; an unknown name / unmatched req errors.

```rust
// Illustrative test (not a command)
#[test]
fn reads_and_resolves_the_highest_matching_version() {
    let root = write_fixture_index(&[("http-extras", &["1.0.0", "1.2.0", "2.0.0"])]);
    let e = read_entry(&root, "http-extras").unwrap();
    let v = resolve_version(&e, &"^1.0".parse().unwrap()).unwrap();
    assert_eq!(v.version.to_string(), "1.2.0");
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe index`.
- [ ] **Step 3: Implement** `IndexEntry`/`EntryVersion` + `read_entry`/`resolve_version`
  (parse via `toml`; capability strings via SP1's `Capability::from_str`).
- [ ] **Step 4: Run, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): index entry schema + version resolver`.

---

### Task 2: Lockfile format + read/write

**Files:** Create `src/ipe-cli/src/lockfile.rs`; Test: inline.

**Interfaces:**
- Produces: `struct LockedDep { name, version: semver::Version, source, rev, sha256 }`;
  `struct Lockfile { packages: Vec<LockedDep> }`; `Lockfile::read(project_root)` /
  `Lockfile::write(project_root)` (deterministic: packages sorted by name);
  `Lockfile::upsert(dep)` / `remove(name)`.

- [ ] **Step 1: Write the failing test** — write then read round-trips; output is
  deterministic (sorted); upsert replaces, remove deletes.
- [ ] **Step 2: Run, verify it fails** — `cargo nextest run -p ipe lockfile`.
- [ ] **Step 3: Implement** the struct + `toml` serialize (sorted) + read/write/upsert/remove.
- [ ] **Step 4: Run, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): ipe.lock format + deterministic read/write`.

---

### Task 3: `ipe add` resolver — fetch, verify, lock

**Files:** Create `src/ipe-cli/src/resolve.rs`; Modify `src/ipe-cli/src/pkg.rs`,
`src/ipe-cli/src/project.rs`; Test: `src/ipe-cli/tests/add_resolve.rs`.

**Interfaces:**
- Consumes: Tasks 1–2; `git` (subprocess); `sha2` via `cache.rs` (factor the
  tree-hash helper if needed); SP2's `IpeDep` + `parse_manifest`.
- Produces: `fn resolve_and_add(project_root, name, req_or_latest, index_root) -> Result<(), CliError>`:
  read entry → resolve version → `git` clone/fetch `source` at `rev` into the package
  cache → sha256 the fetched tree → **verify == entry.sha256 (mismatch = hard error)**
  → `Lockfile::upsert` + write → write `IpeDep::Index(req)` into `ipe.toml [dependencies]`
  → print the resolved version + its capability set (loud on `native-ffi`). Plus
  `resolve_escape` for `IpeDep::Git`/`IpeDep::Path` (fetch/copy, hash, lock — no index).

- [ ] **Step 1: Write the failing test** (against a fixture index + a fixture source
  git repo, both in temp dirs — no network):

```rust
// Illustrative test (not a command)
#[test]
fn add_resolves_verifies_and_locks() {
    let (index_root, source_url, sha) = fixture_index_and_source("http-extras", "1.2.0");
    let proj = scaffold_project();
    resolve_and_add(&proj, "http-extras", "^1", &index_root).unwrap();
    let lock = Lockfile::read(&proj).unwrap();
    assert_eq!(lock.packages[0].sha256, sha);
    assert!(parse_manifest(&proj.join("ipe.toml")).unwrap()
        .dependencies.contains_key("http-extras"));
}
#[test]
fn add_rejects_a_hash_mismatch() {
    let (index_root, _, _) = fixture_index_with_wrong_hash("http-extras", "1.2.0");
    assert!(resolve_and_add(&scaffold_project(), "http-extras", "^1", &index_root).is_err());
}
```

- [ ] **Step 2: Run, verify it fails** — `cargo nextest run -p ipe --test add_resolve`.
- [ ] **Step 3: Implement** `resolve_and_add` + `resolve_escape`; wire `pkg::run_add`
  to it (index root from an env/config override, defaulting to the standard location;
  tests pass the fixture root). The hash-mismatch path is a typed `CliError`.
- [ ] **Step 4: Run, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): ipe add index resolution — fetch, verify, lock`.

---

### Task 4: `ipe remove` + acceptance + README

**Files:** Modify `src/ipe-cli/src/pkg.rs`, `resolve.rs`; `README.md`; Test:
`src/ipe-cli/tests/add_resolve.rs` (extend).

- [ ] **Step 1: Failing test** — `ipe remove <name>` drops the dep from `ipe.toml` +
  `ipe.lock`; a full add→remove cycle leaves both clean; the capability set is shown
  on add.
- [ ] **Step 2: Run, verify it fails**.
- [ ] **Step 3: Implement** `resolve_and_remove` + wire `pkg::run_remove`.
- [ ] **Step 4: Full green** — `cargo nextest run -p ipe` all green; golden suite
  unchanged (`--test golden_basics`); clippy `--all-targets --workspace -- -D warnings`
  clean; `cargo fmt --check` clean.
- [ ] **Step 5: README** — document `ipe add`/`ipe remove` (index resolution, the
  lockfile, the `{git=}`/`{path=}` escapes) with a runnable example against a local
  fixture index. Verify shown commands run.
- [ ] **Step 6: Commit** — `feat(cli): ipe remove + index-resolution acceptance + README`.

## Self-review

- **Spec coverage:** index resolution (`ipe add … through the curated index`) = Tasks
  1+3; content-hash pinning + lockfile = Tasks 2+3; `{git=}`/`{path=}` escapes = Task 3;
  `ipe remove` = Task 4. Publish + gate + real-repo provisioning deliberately deferred.
- **Security-first:** hash-verify-before-trust is a hard error (Task 3); capabilities
  surfaced for consent. No unvetted path silently resolves.
- **Invalid-states-unrepresentable:** typed `IndexEntry`/`LockedDep`; escapes reuse
  SP2's `IpeDep` enum (index xor git xor path).
- **Type consistency:** `semver::Version`/`VersionReq`, `Capability` (SP1), `IpeDep`
  (SP2) are the shared types threaded through; no re-invented stringly types.
- **Determinism:** lockfile sorted; resolution reads the lock offline.

## Handoff

SP4's gate CI runs the universal + native tiers on a submission and consumes the
entry schema (Task 1) + the capability set. `ipe package publish` (a later slice)
writes an `IndexEntry` (Task 1's schema) and opens the index PR. The real public
index git repo is provisioned as its own deliberate, outward-facing step.
