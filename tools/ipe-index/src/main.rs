mod coverage;
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
        for f in &files {
            let Some(src) = read_capped(root, &f.path) else {
                continue;
            };
            // Store every path prefixed with the repo tag so multiple repos never
            // collide (each has Cargo.toml, README.md, tools/scripts/*, tools/*).
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
            extract::extract_file(&store, &tagged, f.lang, &src)?;
            pipeline::record_stage(&store, &tagged)?;
            if role == model::Role::Fixture || role == model::Role::Example {
                coverage::record_coverage(&store, &tagged, &src)?;
            }
        }
        // Per-repo HEAD sha so an incremental `update` can diff each.
        if let Ok(sha) = walk::head_sha(root) {
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
        let (ups, dels) = walk::changed(root, &since)?;
        for d in &dels {
            store.drop_file(&format!("{tag}:{d}"))?;
        }
        for f in &ups {
            let tagged = format!("{tag}:{}", f.path);
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
            extract::extract_file(&store, &tagged, f.lang, &src)?;
            pipeline::record_stage(&store, &tagged)?;
            if role == model::Role::Fixture || role == model::Role::Example {
                coverage::record_coverage(&store, &tagged, &src)?;
            }
        }
        if let Ok(sha) = walk::head_sha(root) {
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
    }
}
