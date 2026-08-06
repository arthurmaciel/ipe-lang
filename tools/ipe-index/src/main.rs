mod coverage;
mod diff;
mod extract;
mod model;
mod pipeline;
mod query;
mod store;
mod walk;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ipe-index", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build the index from scratch across all configured repos.
    Index {
        /// Repo to index, as `tag:path` (repeatable). Default: this repo
        /// (`ipe:.`). Symbols/files are stored path-prefixed with the tag.
        #[arg(long, default_values_t = default_repos())]
        repo: Vec<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Incrementally refresh the index: per-repo `last_sha..HEAD` git diff,
    /// re-extract only changed files. Falls back to a full `index` when the DB
    /// is absent or a repo has no recorded sha.
    Update {
        #[arg(long, default_values_t = default_repos())]
        repo: Vec<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Import dependencies of a module (substring match).
    Deps {
        module: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// File counts per role.
    Roles {
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Compiler-stage module counts.
    Pipeline {
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Fixtures/examples covering a module (substring match).
    Covers {
        kernel: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// One-screen digest of the index.
    Wakeup {
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Find all occurrences of a symbol name across the index.
    Locate {
        name: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Reverse dependencies: files/modules that import a given module or path.
    Rdeps {
        module: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
        #[arg(long)]
        count: bool,
        /// Also match submodules (e.g. `Ipe.Core.List` also matches `Ipe.Core.List.Foo`).
        #[arg(long)]
        subtree: bool,
    },
    /// Outgoing links + calls of a unit (by uid).
    Links {
        uid: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Links + callgraph neighbors of a unit in both directions (by uid).
    Neighbors {
        uid: String,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Change-queue rows as JSON lines. `--since <sha>` excludes rows enqueued
    /// by that update run; `--limit N` caps the output.
    Pending {
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        limit: Option<i64>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Find all edit sites for a path rename (whole-segment match). `--to <new>`
    /// emits replacement paths. Read-only.
    RenamePath {
        old: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Find all resolved occurrences of a symbol name (units/links). `--to <new>`
    /// emits replacements; `--preserve <regex>...` skips matches; `--map k=v,...`
    /// correlates longest-key-first. Read-only.
    RenameSymbol {
        old: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        preserve: Vec<String>,
        #[arg(long)]
        map: Option<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
}

fn read_capped(repo: &str, rel: &str) -> Option<String> {
    let p = std::path::Path::new(repo).join(rel);
    let md = std::fs::metadata(&p).ok()?;
    if md.len() > walk::MAX_FILE_BYTES {
        eprintln!("ipe-index: skipping oversized {rel} ({} bytes)", md.len());
        return None;
    }
    std::fs::read_to_string(&p).ok()
}

/// Default repo set: this repo (`ipe:.`).
pub fn default_repos() -> Vec<String> {
    vec!["ipe:.".to_string()]
}

/// Parse a `tag:path` repo spec into `(tag, root)`. A missing `:` is an error —
/// the tag is load-bearing (path-prefix disambiguation + role classification).
fn parse_repo(spec: &str) -> Result<(String, String)> {
    match spec.split_once(':') {
        Some((tag, root)) if !tag.is_empty() && !root.is_empty() => {
            Ok((tag.to_string(), root.to_string()))
        }
        _ => Err(anyhow::anyhow!(
            "bad --repo spec {spec:?}; expected tag:path (e.g. ipe:.)"
        )),
    }
}

fn cmd_index(repo_specs: &[String], db: &str) -> Result<()> {
    if let Some(parent) = std::path::Path::new(db).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }
    // `index` builds from scratch. The non-PK `symbols`/`edges` tables would
    // accumulate duplicate rows across re-index if opened against a stale DB, so
    // fail loudly if an existing DB can't be removed (NotFound on first run is
    // expected and ignored).
    match std::fs::remove_file(db) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::anyhow!("cannot remove stale index db {db}: {e}")),
    }
    let repos: Vec<(String, String)> = repo_specs
        .iter()
        .map(|s| parse_repo(s))
        .collect::<Result<_>>()?;
    let store = store::Store::open(db)?;
    store.begin()?;

    let mut total_files = 0usize;

    for (tag, root) in &repos {
        let files = walk::tracked(root)?;
        let sha = match walk::head_sha(root) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ipe-index: no HEAD sha for {root}: {e}");
                String::new()
            }
        };
        for f in &files {
            let Some(src) = read_capped(root, &f.path) else {
                continue;
            };
            // Store every path prefixed with the repo tag so multiple repos never
            // collide (each has Cargo.toml, README.md, scripts/*, tools/*).
            let tagged = format!("{tag}:{}", f.path);
            // `walk` classified the role on the UNTAGGED path (before the tag is
            // known). Recompute the role on the tagged path so the tag-aware
            // classifier fires.
            let role = model::role_of(&tagged);
            store.put_file(
                &tagged,
                f.lang.as_str(),
                role.as_str(),
                src.len() as i64,
                "",
            )?;
            extract::extract_file(&store, &tagged, f.lang, &src, &sha)?;
            pipeline::record_stage(&store, &tagged)?;
            if role == model::Role::Fixture || role == model::Role::Example {
                coverage::record_coverage(&store, &tagged, &src)?;
            }
        }
        // Per-repo HEAD sha so an incremental `update` can diff each.
        if !sha.is_empty() {
            store.set_meta(&format!("last_sha:{tag}"), &sha)?;
        }
        total_files += files.len();
    }

    // Resolution pass over the merged store (tagged paths). Best-effort across tags.
    query::resolve_edges(&store, ".")?;
    store.commit()?;
    eprintln!(
        "ipe-index: indexed {total_files} files across {} repo(s)",
        repos.len()
    );
    Ok(())
}

/// Incremental refresh: for each repo, diff `last_sha:<tag>..HEAD`, re-extract
/// only the changed files. Falls back to a full `index` when the DB is absent or
/// a repo has no recorded sha.
fn cmd_update(repo_specs: &[String], db: &str) -> Result<()> {
    if !std::path::Path::new(db).exists() {
        return cmd_index(repo_specs, db);
    }
    let repos: Vec<(String, String)> = repo_specs
        .iter()
        .map(|s| parse_repo(s))
        .collect::<Result<_>>()?;
    let store = store::Store::open(db)?;
    // Any repo without a recorded sha can't be diffed → full rebuild.
    for (tag, _) in &repos {
        if store.get_meta(&format!("last_sha:{tag}"))?.is_none() {
            drop(store);
            return cmd_index(repo_specs, db);
        }
    }
    store.begin()?;
    let mut changed_count = 0usize;
    for (tag, root) in &repos {
        let since = store
            .get_meta(&format!("last_sha:{tag}"))?
            .unwrap_or_default();
        let sha = match walk::head_sha(root) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ipe-index: no HEAD sha for {root}: {e}");
                String::new()
            }
        };
        let (ups, dels) = walk::changed(root, &since)?;
        // One timestamp per repo so A6 events order stably within the run
        // (enqueued_at is a tiebreaker in `pending`'s ORDER BY).
        let now = diff::now_millis();
        for d in &dels {
            let tagged = format!("{tag}:{d}");
            // Snapshot the removed units BEFORE the drop so each one can be
            // queued as `deleted` with its last-known body hash.
            let old = store.units_for_path(&tagged)?;
            store.drop_file(&tagged)?;
            for (uid, h) in old {
                store.enqueue_change(&uid, "deleted", Some(&h), None, &sha, now)?;
            }
        }
        for f in &ups {
            let tagged = format!("{tag}:{}", f.path);
            let old = store.units_for_path(&tagged)?;
            store.drop_file(&tagged)?;
            let Some(src) = read_capped(root, &f.path) else {
                continue;
            };
            let role = model::role_of(&tagged);
            store.put_file(
                &tagged,
                f.lang.as_str(),
                role.as_str(),
                src.len() as i64,
                "",
            )?;
            extract::extract_file(&store, &tagged, f.lang, &src, &sha)?;
            pipeline::record_stage(&store, &tagged)?;
            if role == model::Role::Fixture || role == model::Role::Example {
                coverage::record_coverage(&store, &tagged, &src)?;
            }
            // A6: hash-diff the pre-update vs post-extract unit snapshots into
            // change_queue events (new / modified / deleted per unit).
            let new = store.units_for_path(&tagged)?;
            for ev in diff::diff_units(&old, &new) {
                store.enqueue_change(
                    &ev.uid,
                    ev.change,
                    ev.old_hash.as_deref(),
                    ev.new_hash.as_deref(),
                    &sha,
                    now,
                )?;
            }
        }
        if !sha.is_empty() {
            store.set_meta(&format!("last_sha:{tag}"), &sha)?;
        }
        changed_count += ups.len() + dels.len();
    }
    query::resolve_edges(&store, ".")?;
    store.commit()?;
    eprintln!(
        "ipe-index: updated {changed_count} changed path(s) across {} repo(s)",
        repos.len()
    );
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Index { repo, db } => cmd_index(&repo, &db),
        Cmd::Update { repo, db } => cmd_update(&repo, &db),
        Cmd::Deps { module, db } => query::cmd_deps(&db, &module),
        Cmd::Roles { db } => query::cmd_roles(&db),
        Cmd::Pipeline { db } => query::cmd_pipeline(&db),
        Cmd::Covers { kernel, db } => query::cmd_covers(&db, &kernel),
        Cmd::Wakeup { db } => query::cmd_wakeup(&db),
        Cmd::Locate { name, db } => query::cmd_locate(&db, &name),
        Cmd::Rdeps {
            module,
            db,
            count,
            subtree,
        } => query::cmd_rdeps(&db, &module, count, subtree),
        Cmd::Links { uid, db } => query::cmd_links(&db, &uid),
        Cmd::Neighbors { uid, db } => query::cmd_neighbors(&db, &uid),
        Cmd::Pending { since, limit, db } => query::cmd_pending(&db, since.as_deref(), limit),
        Cmd::RenamePath { old, to, db } => query::cmd_rename_path(&db, &old, to.as_deref()),
        Cmd::RenameSymbol {
            old,
            to,
            preserve,
            map,
            db,
        } => query::cmd_rename_symbol(&db, &old, to.as_deref(), &preserve, map.as_deref()),
    }
}
