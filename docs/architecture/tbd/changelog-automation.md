# Changelog & release automation

## Decision: release-please

The project uses **[release-please](https://github.com/googleapis/release-please)**
for versioning, changelog generation, and release cutting. It is wired in
`.github/workflows/release-please.yml`, configured by `release-please-config.json`
+ `.github/.release-please-manifest.json`, and reconciled with the binary-build
`release.yml`. The developer-facing summary lives in DEVELOPMENT.md
(§ Versioning & Releases); this document records the tool choice and the
mechanics that back it.

### Why release-please over git-cliff

An earlier draft of this document recommended `git-cliff`, on the premise that
the repo pushed directly to `main` with few or no PRs, making a PR-centric tool
a poor fit. That premise no longer holds: the project adopted a
**PR workflow** (branch → PR → fast required gate → auto-merge; see
DEVELOPMENT.md § Contributing / PR workflow) and wants **automated versioning**,
not only a changelog. release-please does all three — version bump, changelog,
and release-PR — in one bot, and its standing release PR fits the PR workflow
natively:

| Need | release-please | git-cliff |
|---|---|---|
| Auto version bump (SemVer from commits) | built in — bumps `Cargo.toml` + seeds the tag | not its job; changelog only |
| Changelog from Conventional Commits | built in (keepachangelog-style sections) | built in (via `cliff.toml` template) |
| Release PR in a PR-based workflow | the core model — one standing release PR, "merge to ship" | none — designed for direct-push changelog regeneration |
| Tag + GitHub release creation | built in, on release-PR merge | out of scope (left to `gh release`) |

git-cliff remains an excellent commit-history-to-changelog renderer for a
direct-push repo. Once the workflow became PR-based and auto-versioning was in
scope, release-please's single-bot coverage of bump + changelog + release-PR
won.

## Conventional Commits are the shared input

Both tools — and the whole scheme — rest on the repo's Conventional Commits
discipline. That discipline is what release-please parses to decide the bump and
group the changelog. Pre-1.0 mapping (enforced by `release-please-config.json`;
see DEVELOPMENT.md § Versioning & Releases for the authoritative table):

| Commit type | Effect (pre-1.0) | Changelog section |
|---|---|---|
| `feat` | patch | Features |
| `fix` | patch | Bug Fixes |
| `perf` | patch | Performance |
| `!` / `BREAKING CHANGE:` | minor | Breaking / prefixed note |
| `docs`, `chore`, `ci`, `test`, `refactor`, `style`, `build` | none | omitted |

`1.0.0` is never reached automatically — a breaking change while `0.x` bumps the
minor. Cutting `1.0` is a deliberate stability promise.

## CHANGELOG.md format (keepachangelog 1.1.0)

`CHANGELOG.md` at the repo root follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). release-please owns
everything below the header, prepending a new versioned section each time the
release PR merges. The bootstrap file carries the header plus the seed `[0.1.0]`
entry so the first automated append lands cleanly above it.

## Workspace-version handling (the load-bearing config detail)

The root `Cargo.toml` is a **virtual workspace manifest**: it has `[workspace]`
and `[workspace.package].version`, but no `[package]` section. release-please's
stock `rust` release-type updates `[package].version` and errors on a manifest
that has none. So the config uses `release-type: "simple"` (for Conventional-
Commit parsing, changelog, tags, and the pre-major bump flags) plus a `generic`
`extra-files` entry pointed at `Cargo.toml`. The version line carries an inline
annotation:

```toml
version = "0.1.0" # x-release-please-version
```

release-please replaces the value on that annotated line, so the single
workspace-wide version stays the source of truth and every member crate inherits
it via `version.workspace = true`. The `.github/.release-please-manifest.json`
is seeded `{".": "0.1.0"}` so release-please starts from the already-published
`v0.1.0` and proposes `0.1.1` (feat/fix/perf) or `0.2.0` (breaking) next.

## Reconciling with release.yml

release-please owns tag + GitHub-release creation and the changelog body.
`release.yml` owns only the 5-platform binary build. To avoid two release
objects per tag, `release.yml` triggers on `release: published` — the event
release-please fires when it publishes the release for the tag it just pushed —
and its final step **uploads** assets to that existing release
(`gh release upload "$TAG" … --clobber`) instead of `gh release create`. Keying
on the release event (not the tag push) also guarantees the release object
exists before the upload runs, so there is no create/upload race. `--generate-
notes` is dropped: the release body is the release-please changelog.

Net path: push to `main` → release-please opens/refreshes the release PR (inert)
→ merge it → tag `vX.Y.Z` + GitHub release created with changelog body →
`release.yml` builds binaries → uploads them to that one release.
