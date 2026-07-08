mod coverage;
mod extract;
mod model;
mod parity;
mod pipeline;
mod query;
mod store;
mod walk;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashSet;

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
        /// Repo to index, as `tag:path` (repeatable). Default: both the Ipê repo
        /// (`ipe:.`) and the Sky reference repo (`sky:../sky`). Symbols/files are
        /// stored path-prefixed with the tag so the two repos never collide and
        /// parity can compare Sky-Go kernels against Ipê-Rust kernels.
        #[arg(long, default_values_t = default_repos())]
        repo: Vec<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Refresh the index. Full reindex of every configured repo (bounded + cheap;
    /// the tagged multi-repo layout makes per-repo incremental diffing moot for v1).
    Update {
        #[arg(long, default_values_t = default_repos())]
        repo: Vec<String>,
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
    },
    /// Cross-language kernel parity (Go vs Rust impls of Sky kernels).
    Parity {
        #[arg(long, default_value = ".ipe-index/index.db")]
        db: String,
        #[arg(long)]
        gaps: bool,
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
    /// Fixtures/examples covering a kernel/module (substring match).
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
        /// Also match submodules (e.g. `Sky.Core.List` also matches `Sky.Core.List.Foo`).
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

/// Default repo set: the Ipê repo (this one) + the Sky reference repo (sibling).
/// A hook running inside `../sky` overrides this with `--repo sky:. --repo ipe:../sky-rust`.
pub fn default_repos() -> Vec<String> {
    vec!["ipe:.".to_string(), "sky:../sky".to_string()]
}

/// Parse a `tag:path` repo spec into `(tag, root)`. A missing `:` is an error —
/// the tag is load-bearing (path-prefix disambiguation + role classification).
fn parse_repo(spec: &str) -> Result<(String, String)> {
    match spec.split_once(':') {
        Some((tag, root)) if !tag.is_empty() && !root.is_empty() => {
            Ok((tag.to_string(), root.to_string()))
        }
        _ => Err(anyhow::anyhow!(
            "bad --repo spec {spec:?}; expected tag:path (e.g. ipe:. or sky:../sky)"
        )),
    }
}

fn cmd_index(repo_specs: &[String], db: &str) -> Result<()> {
    if let Some(parent) = std::path::Path::new(db).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
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
    let repos: Vec<(String, String)> = repo_specs.iter()
        .map(|s| parse_repo(s))
        .collect::<Result<_>>()?;
    let store = store::Store::open(db)?;
    store.begin()?;

    // Parity inputs accumulate ACROSS ALL repos: this is what lets `parity`
    // compare Sky-Go kernel impls (from ../sky) against Ipê-Rust kernel impls
    // (from this repo). Bounded: small name sets, one file's contents at a time.
    let mut go_fns: HashSet<String> = HashSet::new();
    let mut rust_fns: HashSet<String> = HashSet::new();
    let mut sky_kernel_decls: HashSet<String> = HashSet::new();
    let mut kernel_hs_sources: Vec<(String, String)> = Vec::new();
    let mut total_files = 0usize;

    for (tag, root) in &repos {
        let files = walk::tracked(root)?;
        for f in &files {
            let Some(src) = read_capped(root, &f.path) else {
                continue;
            };
            // Store every path prefixed with the repo tag so the two repos never
            // collide (both have Cargo.toml, README.md, scripts/*, tools/*).
            let tagged = format!("{tag}:{}", f.path);
            // `walk` classified the role on the UNTAGGED path (before the tag is
            // known), so an Ipê `crates/*.rs` mis-lands as `other`. Recompute the
            // role on the tagged path so the repo-aware classifier fires.
            let role = model::role_of(&tagged);
            store.put_file(&tagged, f.lang.as_str(), role.as_str(), src.len() as i64, "")?;
            extract::extract_file(&store, &tagged, f.lang, &src)?;
            pipeline::record_stage(&store, &tagged)?;
            // Parity inputs (union across repos).
            if f.path.ends_with("Kernel.hs") {
                kernel_hs_sources.push((tagged.clone(), src.clone()));
            }
            if f.lang == model::Lang::Go {
                for c in extract::treesitter_defs(&src, model::Lang::Go) {
                    go_fns.insert(c);
                }
                for (name, _line) in extract::go_registered_kernels(&src) {
                    go_fns.insert(name);
                }
            }
            if f.lang == model::Lang::Rust {
                for c in extract::treesitter_defs(&src, model::Lang::Rust) {
                    rust_fns.insert(c);
                }
            }
            // Ffi.kernel declarations from any Sky stdlib source (Sky or Ipê).
            if matches!(role, model::Role::StdlibSky | model::Role::IpeStdlibSky) {
                let scan = extract::sky::scan_sky(&src);
                for kernel_name in scan.kernels {
                    sky_kernel_decls.insert(kernel_name);
                }
            }
            if role == model::Role::Fixture || role == model::Role::Example {
                coverage::record_coverage(&store, &tagged, &src)?;
            }
        }
        // Per-repo HEAD sha so an incremental `update` (future) can diff each.
        if let Ok(sha) = walk::head_sha(root) {
            store.set_meta(&format!("last_sha:{tag}"), &sha)?;
        }
        total_files += files.len();
    }

    // Cross-repo parity reconcile: Sky-Go kernels vs Ipê-Rust kernels, keyed by
    // the Sky Ffi.kernel decl set + Kernel.hs routes from either repo.
    let pairs: Vec<(&str, &str)> = kernel_hs_sources.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
    let routes = parity::parse_routes_with_locs(&pairs);
    for k in parity::reconcile_with_locs(&routes, &go_fns, &rust_fns, &sky_kernel_decls) {
        let go_impl_loc = if k.go_impl {
            lookup_sym_loc_lang(&store, &k.name.replace('.', "_"), "go")?
        } else {
            None
        };
        let rust_impl_loc = if k.rust_impl {
            lookup_sym_loc_lang(&store, &k.rust_fn, "rs")?
        } else {
            None
        };
        store.conn.execute(
            "INSERT OR REPLACE INTO kernels VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                k.name, k.sky_decl as i64, k.rust_fn,
                k.hs_route_loc.as_deref(),
                k.go_impl as i64, k.rust_impl as i64,
                go_impl_loc, rust_impl_loc,
                k.parity
            ],
        )?;
    }
    // Resolution pass over the merged store (tagged paths). Best-effort across tags.
    query::resolve_edges(&store, ".")?;
    store.commit()?;
    eprintln!("ipe-index: indexed {total_files} files across {} repo(s)", repos.len());
    Ok(())
}

/// Look up `"file:line"` for the first `def` symbol matching `name` in `lang`,
/// excluding test and example files.
fn lookup_sym_loc_lang(store: &store::Store, name: &str, lang: &str) -> Result<Option<String>> {
    let hits = store.symbols_named_in_lang(name, lang)?;
    Ok(hits.into_iter()
        .map(|(file, line, _)| format!("{file}:{line}"))
        .next())
}

/// Entry for `update`: a full reindex of every configured repo. The tagged
/// multi-repo layout makes per-repo incremental git-diffing moot for v1 (a full
/// rebuild of both repos is bounded and cheap — ~1-2 s), and it keeps the parity
/// reconcile correct without re-deriving cross-repo kernel sets from a diff.
pub fn cmd_index_pub(repo_specs: &[String], db: &str) -> Result<()> {
    cmd_index(repo_specs, db)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Index { repo, db } => cmd_index(&repo, &db),
        Cmd::Update { repo, db } => cmd_index_pub(&repo, &db),
        Cmd::Parity { db, gaps } => query::cmd_parity(&db, gaps),
        Cmd::Deps { module, db } => query::cmd_deps(&db, &module),
        Cmd::Roles { db } => query::cmd_roles(&db),
        Cmd::Pipeline { db } => query::cmd_pipeline(&db),
        Cmd::Covers { kernel, db } => query::cmd_covers(&db, &kernel),
        Cmd::Wakeup { db } => query::cmd_wakeup(&db),
        Cmd::Locate { name, db } => query::cmd_locate(&db, &name),
        Cmd::Rdeps { module, db, count, subtree } => query::cmd_rdeps(&db, &module, count, subtree),
    }
}
