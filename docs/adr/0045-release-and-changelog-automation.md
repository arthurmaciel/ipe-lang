# 45. Release and changelog automation via release-please

Date: 2026-07-25

## Status

Accepted and implemented. Wired in `.github/workflows/release-please.yml`, configured
by `release-please-config.json` and `.github/.release-please-manifest.json`, and
reconciled with the binary-build workflow `release.yml`. The developer-facing summary
is in DEVELOPMENT.md (§ Versioning & Releases).

## Context

The project needs three things from its release process: a version bump derived from
commit history, a changelog, and an actual release cut (tag + release object with
binaries). It also runs a PR-based workflow — branch, PR, fast required gate,
auto-merge — rather than pushing directly to the default branch. The release tooling
has to fit that workflow rather than fight it.

The root manifest is a *virtual workspace*: it has `[workspace]` and a single
`[workspace.package].version` that every member crate inherits, and no `[package]`
section at all. A release tool that assumes a conventional crate manifest (updating
`[package].version`) errors on this layout.

## Decision

Use **release-please** for versioning, changelog generation, and release cutting.
Conventional Commits are the shared input: the commit history is what determines the
bump and groups the changelog. Because the project is pre-1.0, a major bump is
reserved — a breaking change bumps the minor, ordinary changes bump the patch — so
`1.0.0` is never reached automatically; cutting it is a deliberate stability promise.

Rejected: **git-cliff**. An earlier draft favored it on the premise of a direct-push
repo where a PR-centric tool would be a poor fit. That premise no longer holds once
the project adopted the PR workflow and wanted automated *versioning*, not only a
rendered changelog. git-cliff renders a changelog but does not bump the version or cut
a release; release-please covers bump, changelog, and a standing "merge to ship"
release PR in one bot, which matches the PR workflow natively. git-cliff remains a fine
choice for a direct-push repo that only needs changelog rendering — that is simply not
this repo's shape.

Two load-bearing configuration decisions follow from the constraints:

- **Single version source of truth on a virtual workspace.** The stock Rust
  release-type cannot update a manifest with no `[package]` section, so the config uses
  the `simple` release-type (for commit parsing, changelog, tags, and the pre-1.0 bump
  flags) plus a generic extra-file pointed at the annotated workspace version line.
  release-please rewrites that one line; every member crate inherits it. The invariant
  is that there is exactly one version in the tree, workspace-wide.
- **One release object per tag.** release-please owns tag creation, the release object,
  and the changelog body; the binary-build workflow owns only the multi-platform build.
  To avoid two release objects per tag and a create/upload race, the binary workflow
  triggers on the release-published event (not the tag push) and *uploads* assets to
  the existing release rather than creating one. The release body is the changelog, so
  auto-generated notes are not used.

## Consequences

- The release path is: merge to the default branch refreshes the standing release PR;
  merging that PR cuts the tag and release with the changelog body; the binary workflow
  then builds and uploads assets to that one release.
- The single-workspace-version invariant must hold: any future crate must inherit the
  workspace version rather than declaring its own, or the "one version in the tree"
  guarantee breaks and release-please's single rewrite no longer covers it.
- The one-release-object-per-tag invariant depends on the binary workflow keying off the
  release-published event and uploading (not creating). Switching it back to create-on-
  tag-push would reintroduce the duplicate-release and race hazards.
- The whole scheme rests on Conventional Commit discipline; commits that do not follow
  it are invisible to the bump and changelog logic, which is the intended, documented
  behavior for non-shipping change types.
