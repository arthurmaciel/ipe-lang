use crate::store::Store;
use anyhow::Result;
use std::collections::HashSet;

pub fn cmd_locate(db: &str, name: &str) -> Result<()> {
    use std::io::Write;
    let s = Store::open(db)?;
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();

    macro_rules! writeln_bp {
        ($($arg:tt)*) => {
            if let Err(e) = writeln!(locked, $($arg)*) {
                if e.kind() == std::io::ErrorKind::BrokenPipe { return Ok(()); }
                return Err(e.into());
            }
        };
    }

    // Look up symbols
    let sym_rows: Vec<(String, i64, i64, String)> = {
        let mut st = s
            .conn
            .prepare("SELECT file, line, col, kind FROM symbols WHERE name=? ORDER BY file")?;

        st.query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    let mut found = false;
    for (file, line, col, kind) in sym_rows {
        writeln_bp!("{file}:{line}:{col}  {kind}");
        found = true;
    }
    if !found {
        writeln_bp!("(no results for {name:?})");
    }
    Ok(())
}

pub fn cmd_rdeps(db: &str, module: &str, count: bool, subtree: bool) -> Result<()> {
    use std::io::Write;
    let s = Store::open(db)?;
    // Determine whether the arg looks like a file path (contains '/' or ends with a
    // file extension) so we can match on `resolved` instead of `dst`.
    let looks_like_path = module.contains('/') || module.contains('.');
    // Build the WHERE clause:
    //   - exact dst match (default): `dst = ?`
    //   - exact resolved match when arg is path-shaped: `resolved = ?`
    //   - subtree: additionally `dst LIKE 'module.%'`
    // We do NOT use unanchored LIKE so `rdeps "List"` can't accidentally
    // fold in `Data.List`, `container/list`, or `*ListSpec` files.
    if count {
        let n: i64 = if subtree {
            s.conn.query_row(
                "SELECT COUNT(DISTINCT src) FROM edges \
                 WHERE kind='import' AND (dst=?1 OR dst LIKE ?2 OR resolved=?1)",
                rusqlite::params![module, format!("{module}.%")],
                |r| r.get(0),
            )?
        } else if looks_like_path {
            s.conn.query_row(
                "SELECT COUNT(DISTINCT src) FROM edges \
                 WHERE kind='import' AND (dst=?1 OR resolved=?1)",
                rusqlite::params![module],
                |r| r.get(0),
            )?
        } else {
            s.conn.query_row(
                "SELECT COUNT(DISTINCT src) FROM edges \
                 WHERE kind='import' AND dst=?1",
                rusqlite::params![module],
                |r| r.get(0),
            )?
        };
        println!("{n}");
    } else {
        let stdout = std::io::stdout();
        let sql_and_params: (&str, Vec<String>);
        let (sql, params) = if subtree {
            sql_and_params = (
                "SELECT DISTINCT src FROM edges \
                 WHERE kind='import' AND (dst=?1 OR dst LIKE ?2 OR resolved=?1) ORDER BY src",
                vec![module.to_string(), format!("{module}.%")],
            );
            (&sql_and_params.0, &sql_and_params.1)
        } else if looks_like_path {
            sql_and_params = (
                "SELECT DISTINCT src FROM edges \
                 WHERE kind='import' AND (dst=?1 OR resolved=?1) ORDER BY src",
                vec![module.to_string()],
            );
            (&sql_and_params.0, &sql_and_params.1)
        } else {
            sql_and_params = (
                "SELECT DISTINCT src FROM edges \
                 WHERE kind='import' AND dst=?1 ORDER BY src",
                vec![module.to_string()],
            );
            (&sql_and_params.0, &sql_and_params.1)
        };
        let mut st = s.conn.prepare(sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            r.get::<_, String>(0)
        })?;
        let mut locked = stdout.lock();
        for r in rows {
            let src = r?;
            if let Err(e) = writeln!(locked, "{src}") {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.into());
            }
        }
    }
    Ok(())
}

pub fn cmd_deps(db: &str, module: &str) -> Result<()> {
    let s = Store::open(db)?;
    // NOTE: deliberately unanchored substring match (the CLI documents `deps` as
    // "substring match"). `?1` is bound as a parameter, so this is injection-safe;
    // the looseness — `deps "List"` folding in any path containing "List" — is the
    // documented CLI contract, not a bug. Use `rdeps` for exact dst/resolved match.
    let mut st = s.conn.prepare(
        "SELECT DISTINCT dst FROM edges WHERE src LIKE ?1 AND kind='import' ORDER BY dst",
    )?;
    let rows = st.query_map([format!("%{module}%")], |r| r.get::<_, String>(0))?;
    for r in rows {
        println!("{}", r?);
    }
    Ok(())
}

pub fn cmd_roles(db: &str) -> Result<()> {
    let s = Store::open(db)?;
    let mut st = s
        .conn
        .prepare("SELECT role,COUNT(*) FROM files GROUP BY role ORDER BY 2 DESC")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for r in rows {
        let (role, n) = r?;
        println!("{role:<14} {n}");
    }
    Ok(())
}

pub fn cmd_pipeline(db: &str) -> Result<()> {
    let s = Store::open(db)?;
    let mut st = s.conn.prepare(
        "SELECT dst,COUNT(*) FROM edges WHERE kind='in-stage' GROUP BY dst ORDER BY 2 DESC",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for r in rows {
        let (st_, n) = r?;
        println!("{st_:<14} {n} modules");
    }
    Ok(())
}

pub fn cmd_covers(db: &str, kernel: &str) -> Result<()> {
    let s = Store::open(db)?;
    // Unanchored substring match by design (the CLI documents `covers` as
    // "substring match"); `?1` is a bound parameter so it stays injection-safe.
    let mut st = s
        .conn
        .prepare("SELECT src FROM edges WHERE kind='covers' AND dst LIKE ?1 ORDER BY src")?;
    let rows = st.query_map([format!("%{kernel}%")], |r| r.get::<_, String>(0))?;
    for r in rows {
        println!("{}", r?);
    }
    Ok(())
}

pub fn cmd_wakeup(db: &str) -> Result<()> {
    let s = Store::open(db)?;
    println!("# ipe-index digest");
    println!("files: {}", s.count("files")?);
    println!(
        "symbols: {}, edges: {}",
        s.count("symbols")?,
        s.count("edges")?,
    );
    cmd_roles(db)
}

/// Resolution pass: for every import edge, try to resolve `dst` to a canonical
/// file path within the repo. Updates `edges.resolved` in a single transaction.
/// Bounded: loads tracked file paths into a HashSet (bounded by file count);
/// buffers unresolved import edges into a Vec (bounded by import-edge count)
/// to work around the borrow-checker's prohibition on simultaneous read and
/// write `Connection` statements — the buffer is freed after all UPDATEs commit.
pub fn resolve_edges(s: &Store, repo: &str) -> Result<()> {
    // Load all known file paths into a set for fast membership test.
    let mut known: HashSet<String> = HashSet::new();
    {
        let mut st = s.conn.prepare("SELECT path FROM files")?;
        for row in st.query_map([], |r| r.get::<_, String>(0))? {
            known.insert(row?);
        }
    }
    // Collect rows to update (buffered to avoid borrow-checker issue with conn:
    // rusqlite does not permit a prepared SELECT and an execute() on the same
    // Connection simultaneously; the buffer is bounded by import-edge count).
    let to_update: Vec<(i64, String, String, String)> = {
        let mut st = s.conn.prepare(
            "SELECT rowid, src, dst, kind FROM edges WHERE kind='import' AND resolved IS NULL",
        )?;

        st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };

    // Use unchecked_transaction only if not already inside a transaction.
    // When called from cmd_index (inside BEGIN/COMMIT), we can write directly.
    // SAFETY of `unchecked_transaction`: the SELECT result above is fully
    // materialised into `to_update` BEFORE any write below, so no prepared
    // statement is live on `conn` during the UPDATEs. Do NOT reorder to stream
    // rows from a live statement into the writes — that would alias `conn`.
    let is_autocommit = s.conn.is_autocommit();
    if is_autocommit {
        // Not in a transaction — wrap in one for efficiency.
        let tx = s.conn.unchecked_transaction()?;
        for (rowid, src, dst, _kind) in to_update {
            if let Some(resolved) = resolve_import(&src, &dst, repo, &known) {
                tx.execute(
                    "UPDATE edges SET resolved=? WHERE rowid=?",
                    rusqlite::params![resolved, rowid],
                )?;
            }
        }
        tx.commit()?;
    } else {
        // Already inside a transaction (e.g., cmd_index's BEGIN). Write directly.
        for (rowid, src, dst, _kind) in to_update {
            if let Some(resolved) = resolve_import(&src, &dst, repo, &known) {
                s.conn.execute(
                    "UPDATE edges SET resolved=? WHERE rowid=?",
                    rusqlite::params![resolved, rowid],
                )?;
            }
        }
    }
    Ok(())
}

/// Attempt to resolve one import edge to a canonical repo-relative path.
/// Returns `None` if the import is external (npm pkg, go module, etc.) or
/// cannot be reliably determined.
fn resolve_import(src: &str, dst: &str, repo: &str, known: &HashSet<String>) -> Option<String> {
    // Determine language from source extension.
    let src_ext = std::path::Path::new(src)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let _ = repo;
    match src_ext {
        // ── Ipê modules ───────────────────────────────────────────────────────
        "ipe" => resolve_module_style(dst, known),

        // ── TypeScript / JavaScript ───────────────────────────────────────────
        "ts" | "tsx" | "js" | "mjs" | "jsx" => {
            if dst.starts_with('.') {
                resolve_relative_js(src, dst, known)
            } else {
                None // npm package — external
            }
        }

        // ── Rust ──────────────────────────────────────────────────────────────
        "rs" => resolve_rust_import(src, dst, known),

        _ => None,
    }
}

/// Resolve a dotted `.ipe` module name like `Ipe.Core.List` to a file path.
fn resolve_module_style(module: &str, known: &HashSet<String>) -> Option<String> {
    // `Ipe.Core.List` → `Ipe/Core/List`
    let slash_path = module.replace('.', "/");
    let candidate = format!("{slash_path}.ipe");
    if known.contains(&candidate) {
        return Some(candidate);
    }
    // Also check under a `src/` prefix (an app's module tree).
    let with_src = format!("src/{slash_path}.ipe");
    if known.contains(&with_src) {
        return Some(with_src);
    }
    None
}

/// Resolve a relative JS/TS import like `./bar` or `../util/helper`.
fn resolve_relative_js(src: &str, dst: &str, known: &HashSet<String>) -> Option<String> {
    let src_dir = std::path::Path::new(src).parent()?;
    let raw = src_dir.join(dst);
    // Normalise the path (remove .., .) without requiring the path to exist on disk.
    let normalised = normalise_path(&raw);
    // Try each extension, then index file variants.
    let exts = ["ts", "tsx", "js", "mjs", "jsx"];
    for ext in &exts {
        let cand = format!("{normalised}.{ext}");
        if known.contains(&cand) {
            return Some(cand);
        }
    }
    // index file inside a directory.
    for ext in &exts {
        let cand = format!("{normalised}/index.{ext}");
        if known.contains(&cand) {
            return Some(cand);
        }
    }
    // Already has an extension?
    if known.contains(&normalised) {
        return Some(normalised);
    }
    None
}

/// Normalise a path by resolving `..` and `.` components lexically.
fn normalise_path(p: &std::path::Path) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            Normal(s) => parts.push(s.to_str().unwrap_or("")),
            ParentDir => {
                parts.pop();
            }
            CurDir => {}
            RootDir => parts.clear(),
            Prefix(_) => {}
        }
    }
    parts.join("/")
}

/// Resolve a Rust `use` path to a source file.
/// `crate::a::b` → strip `crate::`, try `a/b.rs` or `a/b/mod.rs` relative to
/// the crate's src root (inferred from `src` file location).
/// External crates (`::` paths not starting with `crate` / `super` / `self`)
/// → None.
fn resolve_rust_import(src: &str, dst: &str, known: &HashSet<String>) -> Option<String> {
    // External crate reference: doesn't start with crate/super/self.
    let first_seg = dst.split("::").next().unwrap_or("");
    if first_seg != "crate" && first_seg != "super" && first_seg != "self" && !first_seg.is_empty()
    {
        return None;
    }

    // Find the crate root (directory containing Cargo.toml, inferred as parent of `src/`).
    let src_path = std::path::Path::new(src);
    let crate_root = find_crate_root(src_path)?;

    // Build candidate path segments by stripping `crate::` and splitting on `::`.
    let rel = if let Some(stripped) = dst.strip_prefix("crate::") {
        stripped
    } else if dst.starts_with("super::") {
        // super:: refers to parent module — too ambiguous to resolve reliably.
        return None;
    } else {
        dst
    };

    let parts: Vec<&str> = rel.split("::").collect();
    let slash_path = parts.join("/");
    let base = format!("{crate_root}/{slash_path}");

    // Try file.rs then file/mod.rs.
    let as_file = format!("{base}.rs");
    if known.contains(&as_file) {
        return Some(as_file);
    }
    let as_mod = format!("{base}/mod.rs");
    if known.contains(&as_mod) {
        return Some(as_mod);
    }
    None
}

/// Find the nearest ancestor directory that looks like a Rust crate root (has a `src/` child
/// in the known file set). Returns the repo-relative prefix for that crate root.
fn find_crate_root(src_file: &std::path::Path) -> Option<String> {
    // Walk up from the file's directory looking for `src/` parent.
    let mut dir = src_file.parent()?;
    loop {
        let dir_str = dir.to_str().unwrap_or("");
        // If the current directory is named `src`, its parent is the crate root.
        if dir.file_name().and_then(|n| n.to_str()) == Some("src") {
            let parent = dir.parent().unwrap_or(std::path::Path::new(""));
            let parent_str = parent.to_str().unwrap_or("");
            return Some(if parent_str.is_empty() {
                "src".to_string()
            } else {
                format!("{parent_str}/src")
            });
        }
        // Stopping condition: we've hit the root.
        if dir_str.is_empty() || dir == std::path::Path::new("") {
            break;
        }
        dir = match dir.parent() {
            Some(p) => p,
            None => break,
        };
    }
    None
}
