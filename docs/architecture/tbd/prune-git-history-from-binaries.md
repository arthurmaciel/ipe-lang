# Prune Git history from binaries and build artifacts

## Current state

```text
$ git count-objects -vH
count: 4379
size: 209.17 MiB        # loose objects
in-pack: 40124
packs: 8
size-pack: 249.43 MiB    # packed objects ≈ 458 MB total
.disk: 468 M

$ git rev-list --objects --all | git cat-file --batch-check='%(objecttype) %(objectsize) %(objectname) %(rest)' | awk '/^blob/ {print $2 " " $4}' | sort -rn | head -5
47799072 tools/oracle/bin/sky              # 48 MB binary
20045228 examples/01-hello-world/target/…  # 20 MB .rlib
17775424 examples/01-hello-world/target/…  # 18 MB .rmeta
…
```

Biggest bloat sources:

| Source | Estimated size in history |
|---|---|
| `target/` (Rust build artifacts across all examples) | ~490 MB |
| `tools/oracle/bin/sky` (committed binary) | 48 MB |
| `.ipe/cache/` (FFI cache) | ~34 MB |
| `.db-wal` / `.db` (sqlite WALs) | ~5 MB |
| 187 stale branches pinning objects | unknown |

## Prerequisites

1. **Coordinate a team-wide cut-off.** Every contributor must merge (or stash) unmerged work, then re-clone after the rewrite. The rewrite replaces every commit hash — unrebased branches will be orphaned.
2. **Install git-filter-repo** — `pip install git-filter-repo` or `brew install git-filter-repo`.
3. **Backup** — `cp -a .git ../sky-rust.git.bak` before anything destructive.

## Plan

### Step 1: Prune stale branches

187 local + remote tracking branches that no longer exist upstream pin objects. Delete them:

```bash
# Delete local branches that are gone on remote
git remote prune origin

# Delete local branches whose upstream is gone
git branch -vv | awk '/: gone]/{print $1}' | xargs -r git branch -d

# Force-delete merged local branches older than 90 days
git branch --merged main | grep -v 'main\|*' | xargs -r git branch -d
```

### Step 2: Add permanent guardrails

Before rewriting history, add `.gitignore` patterns so the offending paths never re-enter:

Edit `.gitignore` — verify these entries exist (they likely do):

```
target/
.ipe/cache/
*.db
*.db-wal
*.db-shm
```

Add `.gitattributes` for binaries that should use Git LFS going forward:

```gitattributes
tools/oracle/bin/sky filter=lfs diff=lfs merge=lfs -text
*.wasm filter=lfs diff=lfs merge=lfs -text
```

Commit these changes before the filter-repo step so the initial commit of the rewritten repo contains them.

### Step 3: Purge from history with git-filter-repo

```bash
# Analysze first (dry-run with --analyze)
git filter-repo --analyze

# Remove target/ directories, oracle binary, .ipe/cache/, db files
git filter-repo \
  --path-glob '**/target/**' \
  --path-glob '**/.ipe/cache/**' \
  --path-glob '**/*.db' \
  --path-glob '**/*.db-wal' \
  --path-glob '**/*.db-shm' \
  --path tools/oracle/bin/sky \
  --invert-paths
```

Flags:
- `--invert-paths` = remove the matching paths (i.e. KEEP everything else)
- `--path-glob` supports recursive globs (`**`)
- `--force` needed if working tree is dirty; run from clean state

After this runs:
- Every commit that ONLY touched removed paths is deleted entirely.
- Remaining commits have those paths stripped.
- All commit hashes change.
- The `.git` directory is now ~150–200 MB.
- The ref `refs/original/*` is NOT created (filter-repo doesn't create one unless `--refs` used).

### Step 4: Post-filter cleanup

```bash
# Already done by filter-repo automatically, but verify:
git reflog expire --expire=now --all
git gc --aggressive --prune=now
```

### Step 5: Set up Git LFS for future binaries

```bash
git lfs track "tools/oracle/bin/sky"
git lfs track "*.wasm"
git add .gitattributes
git commit -m "chore: track binaries with git-lfs"
```

## After the rewrite — everyone must re-clone

```bash
git remote set-url origin <url>
git fetch origin
git reset --hard origin/main
```

Anyone with unmerged branches must:
1. Before the rewrite: note the commit hash of their branch tip.
2. After the rewrite: `git cherry-pick` their commits onto the new history using the ORIGINAL commit's tree content (filter-repo preserves content, just rewrites history).

The author's `scripts/hooks/pretooluse-bash-guard.sh` and `scripts/guards/` scripts are in `misc/scripts/` — if you have local branches referencing the old scripts paths, migrate them after re-clone.

## Estimated savings

| Phase | .git size |
|---|---|
| Before | ~468 MB |
| After stale branch prune | ~400 MB |
| After filter-repo + gc | ~100–150 MB |
| With LFS (ongoing) | stable below 200 MB |

## When to run

Pick a low-activity window. Friday afternoon or before a holiday works — anyone with uncommitted work gets a full weekend to re-clone. Send a heads-up in the team channel at least 24 hours before.
