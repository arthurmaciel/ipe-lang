use crate::store::Store;
use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};

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

/// A5 `links <uid>`: outgoing links and callgraph calls of one unit.
/// Internal link targets resolve to the callee unit's qualified name (via the
/// `to_uid` join); external refs keep the raw URL. One text line per row.
pub fn cmd_links(db: &str, uid: &str) -> Result<()> {
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

    let mut any = false;
    let mut st = s.conn.prepare(
        "SELECT l.to_kind, COALESCE(u.qualified, l.to_ref), l.line \
         FROM links l LEFT JOIN units u ON u.uid = l.to_uid \
         WHERE l.from_uid = ?1 ORDER BY l.line, l.to_kind",
    )?;
    let rows = st.query_map([uid], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for r in rows {
        let (kind, target, line) = r?;
        writeln_bp!("{kind:<8} {target} (line {line})");
        any = true;
    }
    let mut st = s.conn.prepare(
        "SELECT u.qualified FROM callgraph c JOIN units u ON u.uid = c.callee_uid \
         WHERE c.caller_uid = ?1 ORDER BY u.qualified",
    )?;
    let rows = st.query_map([uid], |r| r.get::<_, String>(0))?;
    for r in rows {
        writeln_bp!("call     {}", r?);
        any = true;
    }
    if !any {
        writeln_bp!("(no links or calls for {uid})");
    }
    Ok(())
}

/// A5 `neighbors <uid>`: links + callgraph edges in BOTH directions around a
/// unit. `link-out`/`call` originate at the unit; `link-in`/`called-by` point
/// at it. One text line per row.
pub fn cmd_neighbors(db: &str, uid: &str) -> Result<()> {
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

    let mut st = s.conn.prepare(
        "SELECT 'link-out', COALESCE(u.qualified, l.to_ref), l.line \
         FROM links l LEFT JOIN units u ON u.uid = l.to_uid \
         WHERE l.from_uid = ?1 \
         UNION ALL \
         SELECT 'link-in', COALESCE(u.qualified, l.to_ref), l.line \
         FROM links l LEFT JOIN units u ON u.uid = l.from_uid \
         WHERE l.to_uid = ?1 \
         UNION ALL \
         SELECT 'call', u.qualified, 0 \
         FROM callgraph c JOIN units u ON u.uid = c.callee_uid WHERE c.caller_uid = ?1 \
         UNION ALL \
         SELECT 'called-by', u.qualified, 0 \
         FROM callgraph c JOIN units u ON u.uid = c.caller_uid WHERE c.callee_uid = ?1 \
         ORDER BY 1, 3, 2",
    )?;
    let rows = st.query_map([uid], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut any = false;
    for r in rows {
        let (dir, target, line) = r?;
        writeln_bp!("{dir:<9} {target} (line {line})");
        any = true;
    }
    if !any {
        writeln_bp!("(no neighbors for {uid})");
    }
    Ok(())
}

/// A6 `pending`: change-queue rows as JSON lines (one object per queued unit).
/// `--since <sha>` excludes rows enqueued by that update run (empty matches
/// everything); `--limit N` caps the result. Deleted units join to NULL unit
/// columns (their `units` row is gone), so `qualified`/`path`/`kind` are null.
pub fn cmd_pending(db: &str, since: Option<&str>, limit: Option<i64>) -> Result<()> {
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

    // `?1 = ''` short-circuits when no --since was given (match all rows).
    // Ordering: modified first, then new, then deleted; ties by enqueue time.
    // `LIMIT -1` is SQLite's "no upper bound" — binding NULL would raise a
    // datatype-mismatch error, so an absent --limit becomes -1.
    let since_str = since.unwrap_or("");
    let limit_val = limit.unwrap_or(-1);
    let mut st = s.conn.prepare(
        "SELECT q.uid, q.change, q.old_hash, q.new_hash, q.enqueued_sha, q.enqueued_at, \
                u.qualified, u.path, u.kind \
         FROM change_queue q LEFT JOIN units u ON u.uid = q.uid \
         WHERE (?1 = '' OR q.enqueued_sha != ?1) \
         ORDER BY CASE q.change WHEN 'modified' THEN 0 WHEN 'new' THEN 1 ELSE 2 END, \
                  q.enqueued_at, q.uid \
         LIMIT ?2",
    )?;
    let rows = st.query_map(rusqlite::params![since_str, limit_val], |r| {
        Ok(serde_json::json!({
            "uid": r.get::<_, String>(0)?,
            "change": r.get::<_, String>(1)?,
            "old_hash": r.get::<_, Option<String>>(2)?,
            "new_hash": r.get::<_, Option<String>>(3)?,
            "enqueued_sha": r.get::<_, String>(4)?,
            "enqueued_at": r.get::<_, i64>(5)?,
            "qualified": r.get::<_, Option<String>>(6)?,
            "path": r.get::<_, Option<String>>(7)?,
            "kind": r.get::<_, Option<String>>(8)?,
        }))
    })?;
    for r in rows {
        writeln_bp!("{}", serde_json::to_string(&r?)?);
    }
    Ok(())
}

/// A7 `rename-path <old> [--to <new>]`: whole-segment path match across
/// files, symbols, units, import edges (dst/resolved), and links (to_ref).
/// Outputs JSON lines: {kind,path,line,col,context,replacement?}.
/// kind ∈ {file,symbol,unit,import,link}. Read-only.
pub fn cmd_rename_path(db: &str, old: &str, to: Option<&str>) -> Result<()> {
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

    let mut sites = Vec::new();

    // files.path == old OR LIKE old + '/%'
    {
        let mut st = s
            .conn
            .prepare("SELECT path FROM files WHERE path = ?1 OR path LIKE ?2")?;
        let rows = st.query_map([old, &format!("{old}/%")], |r| r.get::<_, String>(0))?;
        for r in rows {
            let path = r?;
            let replacement = to.map(|t| {
                if path == old {
                    t.to_string()
                } else {
                    format!("{t}{}", &path[old.len()..])
                }
            });
            sites.push(serde_json::json!({
                "kind": "file",
                "path": path,
                "line": 0,
                "col": 0,
                "context": path,
                "replacement": replacement,
            }));
        }
    }

    // symbols.file == old OR LIKE old + '/%'
    {
        let mut st = s
            .conn
            .prepare("SELECT file, line, col, name FROM symbols WHERE file = ?1 OR file LIKE ?2")?;
        let rows = st.query_map([old, &format!("{old}/%")], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for r in rows {
            let (file, line, col, name) = r?;
            let replacement = to.map(|t| {
                if file == old {
                    t.to_string()
                } else {
                    format!("{t}{}", &file[old.len()..])
                }
            });
            sites.push(serde_json::json!({
                "kind": "symbol",
                "path": file,
                "line": line,
                "col": col,
                "context": name,
                "replacement": replacement,
            }));
        }
    }

    // units.path == old OR LIKE old + '/%'
    {
        let mut st = s.conn.prepare(
            "SELECT path, line_start, qualified FROM units WHERE path = ?1 OR path LIKE ?2",
        )?;
        let rows = st.query_map([old, &format!("{old}/%")], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (path, line, qualified) = r?;
            let replacement = to.map(|t| {
                if path == old {
                    t.to_string()
                } else {
                    format!("{t}{}", &path[old.len()..])
                }
            });
            sites.push(serde_json::json!({
                "kind": "unit",
                "path": path,
                "line": line,
                "col": 0,
                "context": qualified,
                "replacement": replacement,
            }));
        }
    }

    // import edges: dst == old OR resolved == old OR dst LIKE old + '.%' OR resolved LIKE old + '.%'
    {
        let mut st = s.conn.prepare(
            "SELECT src, dst, resolved FROM edges WHERE kind='import' AND (dst = ?1 OR resolved = ?1 OR dst LIKE ?2 OR resolved LIKE ?2)",
        )?;
        let rows = st.query_map([old, &format!("{old}.%")], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for r in rows {
            let (src, dst, resolved) = r?;
            let hit = if dst == old || resolved.as_deref() == Some(old) {
                old
            } else if dst.starts_with(&format!("{old}.")) {
                &dst
            } else {
                &resolved.unwrap_or_default()
            };
            let replacement = to.map(|t| {
                if hit == old {
                    t.to_string()
                } else {
                    format!("{t}{}", &hit[old.len()..])
                }
            });
            sites.push(serde_json::json!({
                "kind": "import",
                "path": src,
                "line": 0,
                "col": 0,
                "context": dst,
                "replacement": replacement,
            }));
        }
    }

    // links.to_ref == old OR LIKE old + '.%' (join units for path/line)
    {
        let mut st = s.conn.prepare(
            "SELECT l.to_ref, u.path, l.line FROM links l JOIN units u ON u.uid = l.from_uid WHERE l.to_ref = ?1 OR l.to_ref LIKE ?2",
        )?;
        let rows = st.query_map([old, &format!("{old}.%")], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for r in rows {
            let (to_ref, path, line) = r?;
            let replacement = to.map(|t| {
                if to_ref == old {
                    t.to_string()
                } else {
                    format!("{t}{}", &to_ref[old.len()..])
                }
            });
            sites.push(serde_json::json!({
                "kind": "link",
                "path": path,
                "line": line,
                "col": 0,
                "context": to_ref,
                "replacement": replacement,
            }));
        }
    }

    // Sort for deterministic output
    sites.sort_by(|a, b| {
        let a_path = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let b_path = b.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let a_line = a.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
        let b_line = b.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
        let a_kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let b_kind = b.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let a_ctx = a.get("context").and_then(|v| v.as_str()).unwrap_or("");
        let b_ctx = b.get("context").and_then(|v| v.as_str()).unwrap_or("");
        a_path
            .cmp(b_path)
            .then(a_line.cmp(&b_line))
            .then(a_kind.cmp(b_kind))
            .then(a_ctx.cmp(b_ctx))
    });

    for site in sites {
        writeln_bp!("{}", serde_json::to_string(&site)?);
    }
    Ok(())
}

/// A8 `rename-symbol <old> [--to <new>] [--preserve <regex>...] [--map k=v,...]`:
/// finds resolved occurrences of a symbol name (units/links). Outputs JSON lines:
/// {kind,path,line,col,context,replacement?}. kind ∈ {symbol,occurrence}.
/// --preserve: user regexes + baked defaults (URL-ish, kebab-case attrs).
/// --map: k=v comma-separated, longest-key-first overrides correlated replacements.
/// Read-only.
pub fn cmd_rename_symbol(
    db: &str,
    old: &str,
    to: Option<&str>,
    preserves: &[String],
    map: Option<&str>,
) -> Result<()> {
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

    // Compile user preserves + baked defaults
    let mut preserve_regexes = Vec::new();
    // Baked defaults: URL-ish (contains ://), kebab-case attrs
    preserve_regexes.push(Regex::new(r".*://.*").unwrap());
    preserve_regexes.push(Regex::new(r"(?i)^[a-z]+(-[a-z]+)+$").unwrap());
    for p in preserves {
        preserve_regexes.push(
            Regex::new(p).map_err(|e| anyhow::anyhow!("invalid --preserve regex {p:?}: {e}"))?,
        );
    }

    // Parse --map k=v,... longest-key-first
    let mut map_vec = Vec::new();
    if let Some(m) = map {
        for pair in m.split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                map_vec.push((k.to_string(), v.to_string()));
            }
        }
        map_vec.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }

    let mut sites = Vec::new();

    // 1. Symbol-table sites: units whose qualified ends with ::old or .old (final component)
    {
        let mut st = s.conn.prepare(
            "SELECT path, line_start, qualified FROM units WHERE qualified = ?1 OR qualified LIKE ?2 OR qualified LIKE ?3",
        )?;
        let rows = st.query_map([old, &format!("%::{old}"), &format!("%.{old}")], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (path, line, qualified) = r?;
            let replacement = to.map(|t| {
                // Replace final segment
                if let Some(idx) = qualified.rfind(':').or_else(|| qualified.rfind('.')) {
                    format!("{}::{t}", &qualified[..idx + 1])
                } else {
                    t.to_string()
                }
            });
            sites.push(serde_json::json!({
                "kind": "symbol",
                "path": path,
                "line": line,
                "col": 0,
                "context": qualified,
                "replacement": replacement,
            }));
        }
    }

    // 2. Occurrence sites: scan unit source spans for whole-token matches
    // Load all units with their spans
    let units: Vec<(String, String, i64, i64, String)> = {
        let mut st = s
            .conn
            .prepare("SELECT uid, path, line_start, line_end, qualified FROM units")?;
        st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?
    };

    // Cache file lines per path
    let mut line_cache: HashMap<String, Vec<String>> = HashMap::new();

    for (_uid, path, line_start, line_end, _qualified) in units {
        // Read file lines (tagged path: tag:rel)
        let lines = if let Some(cached) = line_cache.get(&path) {
            cached.clone()
        } else {
            // Extract repo root from tagged path
            let (tag, rel) = if let Some(idx) = path.find(':') {
                (&path[..idx], &path[idx + 1..])
            } else {
                ("", path.as_str())
            };
            // Find repo root from tag (we only have one repo in practice: ipe:.)
            let repo_root = if tag == "ipe" { "." } else { "." };
            let full_path = std::path::Path::new(repo_root).join(rel);
            let content = std::fs::read_to_string(&full_path).unwrap_or_default();
            let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            line_cache.insert(path.clone(), lines.clone());
            lines
        };

        // Clamp span to file bounds
        let start = (line_start as usize).max(1).min(lines.len());
        let end = (line_end as usize).max(start).min(lines.len());

        for line_no in start..=end {
            let line = &lines[line_no - 1];
            // Simple whole-token scan for `old`
            // Token = [A-Za-z0-9_$]+
            let mut col = 0usize;
            let bytes = line.as_bytes();
            while col < bytes.len() {
                // Skip non-ident chars
                while col < bytes.len() && !is_ident_start(bytes[col]) {
                    col += 1;
                }
                if col >= bytes.len() {
                    break;
                }
                let token_start = col;
                while col < bytes.len() && is_ident_char(bytes[col]) {
                    col += 1;
                }
                let token = &line[token_start..col];
                if token == old {
                    // Check preserves
                    let mut skip = false;
                    for re in &preserve_regexes {
                        if re.is_match(token) {
                            skip = true;
                            break;
                        }
                    }
                    if skip {
                        continue;
                    }

                    // Check map override
                    let replacement = to.map(|t| {
                        map_vec
                            .iter()
                            .find(|(k, _)| k == token)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_else(|| t.to_string())
                    });

                    sites.push(serde_json::json!({
                        "kind": "occurrence",
                        "path": path,
                        "line": line_no as i64,
                        "col": token_start as i64 + 1,
                        "context": line.trim(),
                        "replacement": replacement,
                    }));
                }
            }
        }
    }

    // Deduplicate by (path, line, col, kind)
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for s in sites {
        let key = (
            s.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            s.get("line").and_then(|v| v.as_i64()).unwrap_or(0),
            s.get("col").and_then(|v| v.as_i64()).unwrap_or(0),
            s.get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        );
        if seen.insert(key) {
            deduped.push(s);
        }
    }
    let mut sites = deduped;

    // Sort for deterministic output
    sites.sort_by(|a, b| {
        let a_path = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let b_path = b.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let a_line = a.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
        let b_line = b.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
        let a_kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let b_kind = b.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        a_path
            .cmp(b_path)
            .then(a_line.cmp(&b_line))
            .then(a_kind.cmp(b_kind))
    });

    for site in sites {
        writeln_bp!("{}", serde_json::to_string(&site)?);
    }
    Ok(())
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}
