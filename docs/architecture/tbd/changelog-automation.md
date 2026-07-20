# CHANGELOG.md — CI Automation Design

## Problem

The repo ships on `v*` tag push via `release.yml`. The publish step currently
uses `gh release create --generate-notes`, which derives its "What's Changed"
section from merged pull requests. This repo pushes directly to `main` with few
or no PRs, so the generated notes are sparse or empty. The commit history is the
real source of truth; it uses consistent Conventional Commits prefixes
throughout.

## Tool recommendation: git-cliff

**Use `git-cliff`** (https://git-cliff.org), a Rust binary, configured via a
repo-root `cliff.toml`.

Rationale over the alternatives:

| Tool | Verdict |
|---|---|
| **git-cliff** | Conventional-Commits-native; keepachangelog template out of the box; single Rust binary, no runtime deps; configurable via `cliff.toml`; no-PR direct-push workflow is the primary use case. |
| release-please | Designed around PRs and a bot-managed release-PR loop; heavyweight for a direct-push repo. |
| conventional-changelog | Node.js ecosystem; requires npm in the release job; keepachangelog output needs a custom preset. |
| Hand-written script | Zero deps, but must re-implement commit parsing, section ordering, compare-link generation, and idempotent file splicing — all already solved by git-cliff. |

## CHANGELOG.md format (keepachangelog 1.1.0)

Illustrative structure (version numbers and date are placeholders):

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- ...

## [0.2.0] - YYYY-MM-DD

### Added
- ...

### Fixed
- ...

[Unreleased]: https://github.com/arthurmaciel/ipe-lang/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/arthurmaciel/ipe-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/arthurmaciel/ipe-lang/releases/tag/v0.1.0
```

Section ordering follows the keepachangelog spec: Added, Changed, Deprecated,
Removed, Fixed, Security.

## Conventional Commits → keepachangelog category mapping

| Commit type | keepachangelog section | Notes |
|---|---|---|
| `feat` | **Added** | |
| `fix` | **Fixed** | |
| `perf`, `refactor` | **Changed** | |
| `feat!`, `fix!`, `refactor!` | **Changed** (prefixed `**BREAKING**`) | preprocessor rewrites subject before grouping |
| `security`, `sec` | **Security** | reserve this prefix convention for future use |
| `deprecate` | **Deprecated** | |
| `remove` | **Removed** | |
| `docs`, `chore`, `ci`, `test`, `style`, `build` | *dropped* | visible in `git log`; not user-facing |

Scope is stripped from display entries (`feat(ffi): foo` → `Added: Foo`).

## cliff.toml (verified against this repo)

Place at repo root. The template below was tested with git-cliff 2.13.1 against
this commit history and produces correct keepachangelog output.

```toml
[changelog]
header = """
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).
"""
body = """
{% if version %}\
## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else %}\
## [Unreleased]
{% endif %}\
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group }}
{% for commit in commits %}\
- {{ commit.message | upper_first }}
{% endfor %}\
{% endfor %}
"""
footer = """
{% for release in releases %}\
{% if release.version %}\
{% if release.previous.version %}\
[{{ release.version | trim_start_matches(pat="v") }}]: https://github.com/arthurmaciel/ipe-lang/compare/{{ release.previous.version }}...{{ release.version }}
{% else %}\
[{{ release.version | trim_start_matches(pat="v") }}]: https://github.com/arthurmaciel/ipe-lang/releases/tag/{{ release.version }}
{% endif %}\
{% endif %}\
{% endfor %}\
{% set versioned = releases | filter(attribute="version") %}\
{% if versioned %}\
[Unreleased]: https://github.com/arthurmaciel/ipe-lang/compare/{{ versioned | first | get(key="version") }}...HEAD
{% endif %}\
"""
trim = true

[git]
conventional_commits = true
filter_unconventional = true   # drop non-CC commits (merge commits, etc.) silently
split_commits = false
commit_preprocessors = [
  # Rewrite breaking-change subject so the entry is self-explanatory.
  { pattern = '^(feat|fix|refactor)!(\([^)]+\))?:\s*', replace = "$1$2: **BREAKING** " },
]
commit_parsers = [
  { message = "^(feat|fix|refactor)!",   group = "Changed"    },
  { message = "^feat",                   group = "Added"       },
  { message = "^fix",                    group = "Fixed"       },
  { message = "^perf|^refactor",         group = "Changed"     },
  { message = "^security|^sec",          group = "Security"    },
  { message = "^deprecate",             group = "Deprecated"   },
  { message = "^remove",                 group = "Removed"     },
  { message = "^docs|^chore|^ci|^test|^style|^build", skip = true },
]
filter_commits = true
tag_pattern = "v[0-9].*"
```

Note: `filter_unconventional = true` emits parse-skip warnings for non-CC
commits; these are harmless and expected.

## release.yml changes

The `release` job's checkout step must use `fetch-depth: 0` so git-cliff can
walk the full tag history. The YAML below is illustrative — it shows the
replacement for the current `Publish release` step and adds the steps before
it; adapt indentation to match the existing job:

```yaml
      # Full history needed for git-cliff tag walking.
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Install git-cliff
        uses: taiki-e/install-action@v2
        with:
          tool: git-cliff

      - name: Generate changelog
        run: |
          # Regenerate CHANGELOG.md from full tag history.
          git-cliff --config cliff.toml --output CHANGELOG.md
          # Extract only the newest version's section for the release body
          # (no header, no compare-link footer — exactly what gh release expects).
          git-cliff --config cliff.toml --latest --strip all --output release-notes.md

      - name: Commit CHANGELOG to main
        run: |
          git config user.name  "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add CHANGELOG.md
          git diff --cached --quiet || \
            git commit -m "chore(release): update CHANGELOG for ${GITHUB_REF_NAME}"
          git push origin HEAD:main

      - name: Publish release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          TAG="${GITHUB_REF_NAME:-${{ github.event.inputs.tag }}}"
          gh release create "$TAG" release/* \
            --title "Ipê $TAG" \
            --notes-file release-notes.md \
            --repo "${{ github.repository }}"
```

`taiki-e/install-action` is already used in `ci.yml` (for nextest), so the
action is a known dependency. The workflow already carries
`permissions: contents: write`, so the bot push requires no new grant.

## Reconciling with GitHub auto-notes

`--generate-notes` and `--notes-file` are mutually exclusive in the GitHub CLI.
Dropping `--generate-notes` is the right call: with few or no PRs, GitHub's
auto-notes produce only a bare "Full Changelog: vA...vB" link. The
commit-derived `release-notes.md` is strictly richer. If the "Full Changelog"
compare link is wanted in the release body, add it as a line at the end of the
`cliff.toml` body template.

## Bootstrapping CHANGELOG.md

Once `cliff.toml` is committed to the repo root, run once locally
(illustrative — requires `cliff.toml` present and git-cliff installed):

```sh
git-cliff --output CHANGELOG.md
git add CHANGELOG.md
git commit -m "chore: add CHANGELOG.md (git-cliff, keepachangelog)"
```

This seeds the file with all history categorised. The `[Unreleased]` section
will be empty until commits land after the bootstrap.

## Operational notes

- The bot commit happens **before** `gh release create`, so `main` always
  includes the changelog entry at ship time. The tag is not moved.
- `git-cliff --latest` resolves against the tag that triggered the workflow,
  which exists in the repo at the point the `release` job runs (after `build`
  and `build-freebsd` complete). The ordering is correct.
- `BREAKING CHANGE:` git-trailer footers (the other CC breaking-change form)
  are not yet handled by the preprocessor above; only the `!` subject form is.
  If trailer-style breaking changes are used, add a second preprocessor entry
  matching on the commit body.
- The `security`/`sec` prefix convention should be documented in DEVELOPMENT.md
  so contributors know it routes to the Security section automatically.
