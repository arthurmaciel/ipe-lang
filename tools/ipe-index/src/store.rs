use crate::model::{Facing, Kind, Unit};
use anyhow::{bail, Result};
use rusqlite::Connection;

pub struct Store { pub conn: Connection }

/// Schema v2 — additive over v1: `units`/`links`/`callgraph`/`change_queue`
/// are the review backbone. All `CREATE … IF NOT EXISTS` so an old DB gains
/// the new tables on open; `index` deletes the file anyway. The CHECK
/// constraints make an invalid enum literal unrepresentable at the DB layer
/// (the extractor is the only writer and only emits the allowed values).
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files   (path TEXT PRIMARY KEY, lang TEXT, role TEXT, size INTEGER, sha TEXT);
CREATE TABLE IF NOT EXISTS symbols (file TEXT, name TEXT, kind TEXT, line INTEGER, col INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS edges   (src TEXT, dst TEXT, kind TEXT, resolved TEXT);
CREATE TABLE IF NOT EXISTS meta    (k TEXT PRIMARY KEY, v TEXT);
CREATE INDEX IF NOT EXISTS i_sym_name ON symbols(name);
CREATE INDEX IF NOT EXISTS i_edge_src ON edges(src);
CREATE INDEX IF NOT EXISTS i_edge_dst ON edges(dst);
CREATE UNIQUE INDEX IF NOT EXISTS u_edge ON edges(src, dst, kind);
CREATE TABLE IF NOT EXISTS units (
  uid         TEXT PRIMARY KEY,
  path        TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('module','file','fn','struct','enum','impl','const','binding','block','trait')),
  name        TEXT NOT NULL,
  qualified   TEXT NOT NULL,
  line_start  INTEGER NOT NULL,
  line_end    INTEGER NOT NULL,
  facing      TEXT NOT NULL CHECK (facing IN ('user','internal','test')),
  purpose     TEXT,
  body_hash   TEXT NOT NULL,
  updated_sha TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS i_units_path ON units(path);
CREATE INDEX IF NOT EXISTS i_units_name ON units(name);
CREATE INDEX IF NOT EXISTS i_units_qual ON units(qualified);
CREATE TABLE IF NOT EXISTS links (
  from_uid TEXT NOT NULL,
  to_kind  TEXT NOT NULL CHECK (to_kind IN ('internal','external')),
  to_uid   TEXT,
  to_ref   TEXT NOT NULL,
  line     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS i_links_from ON links(from_uid);
CREATE INDEX IF NOT EXISTS i_links_to   ON links(to_uid);
CREATE TABLE IF NOT EXISTS callgraph (
  caller_uid TEXT NOT NULL,
  callee_uid TEXT NOT NULL,
  UNIQUE(caller_uid, callee_uid)
);
CREATE INDEX IF NOT EXISTS i_cg_caller ON callgraph(caller_uid);
CREATE INDEX IF NOT EXISTS i_cg_callee ON callgraph(callee_uid);
CREATE TABLE IF NOT EXISTS change_queue (
  uid          TEXT PRIMARY KEY,
  change       TEXT NOT NULL CHECK (change IN ('new','modified','deleted')),
  old_hash     TEXT,
  new_hash     TEXT,
  enqueued_sha TEXT NOT NULL,
  enqueued_at  INTEGER NOT NULL
);
";

/// Current schema version, recorded in `meta` after the additive DDL runs.
/// `open` bumps it on the first connection to a v2 DB; a future destructive
/// migration keys off this value instead of guessing from table presence.
const SCHEMA_VERSION: &str = "2";

/// Stable unit id: blake3 of `path|kind|qualified`. Content-stable across
/// re-indexes; a rename of the symbol or path changes the id by design.
pub fn unit_uid(path: &str, kind: Kind, qualified: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(path.as_bytes());
    h.update(b"|");
    h.update(kind.as_str().as_bytes());
    h.update(b"|");
    h.update(qualified.as_bytes());
    h.finalize().to_hex().to_string()
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        ensure_schema_version(&conn)?;
        Ok(Store { conn })
    }
    pub fn begin(&self) -> Result<()> { self.conn.execute_batch("BEGIN;")?; Ok(()) }
    pub fn commit(&self) -> Result<()> { self.conn.execute_batch("COMMIT;")?; Ok(()) }
    pub fn put_file(&self, path:&str, lang:&str, role:&str, size:i64, sha:&str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO files VALUES (?,?,?,?,?)", rusqlite::params![path,lang,role,size,sha])?; Ok(())
    }
    pub fn put_symbol(&self, file:&str, name:&str, kind:&str, line:i64, col:i64) -> Result<()> {
        self.conn.execute("INSERT INTO symbols VALUES (?,?,?,?,?)", rusqlite::params![file,name,kind,line,col])?; Ok(())
    }
    pub fn put_edge(&self, src:&str, dst:&str, kind:&str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO edges(src,dst,kind) VALUES (?,?,?)", rusqlite::params![src,dst,kind])?; Ok(())
    }
    pub fn put_unit(&self, u: &Unit) -> Result<()> {
        let uid = unit_uid(&u.path, u.kind, &u.qualified);
        self.conn.execute(
            "INSERT OR REPLACE INTO units VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                uid, u.path, u.kind.as_str(), u.name, u.qualified,
                u.line_start, u.line_end, u.facing.as_str(), u.purpose,
                u.body_hash, u.updated_sha
            ],
        )?;
        Ok(())
    }
    pub fn put_link(&self, from_uid:&str, to_kind:&str, to_uid:Option<&str>, to_ref:&str, line:i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO links(from_uid,to_kind,to_uid,to_ref,line) VALUES (?,?,?,?,?)",
            rusqlite::params![from_uid, to_kind, to_uid, to_ref, line],
        )?;
        Ok(())
    }
    pub fn put_call(&self, caller_uid:&str, callee_uid:&str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO callgraph(caller_uid,callee_uid) VALUES (?,?)",
            rusqlite::params![caller_uid, callee_uid],
        )?;
        Ok(())
    }
    pub fn enqueue_change(&self, uid:&str, change:&str, old_hash:Option<&str>, new_hash:Option<&str>, sha:&str, at:i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO change_queue VALUES (?,?,?,?,?,?)",
            rusqlite::params![uid, change, old_hash, new_hash, sha, at],
        )?;
        Ok(())
    }
    pub fn drop_file(&self, path:&str) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE path=?", [path])?;
        self.conn.execute("DELETE FROM symbols WHERE file=?", [path])?;
        self.conn.execute("DELETE FROM edges WHERE src=?", [path])?;
        // Units/links/callgraph are keyed by uid; delete everything owned by
        // this path's units (and links pointing at them).
        self.conn.execute(
            "DELETE FROM links WHERE from_uid IN (SELECT uid FROM units WHERE path=?) \
             OR to_uid IN (SELECT uid FROM units WHERE path=?)",
            rusqlite::params![path, path],
        )?;
        self.conn.execute(
            "DELETE FROM callgraph WHERE caller_uid IN (SELECT uid FROM units WHERE path=?) \
             OR callee_uid IN (SELECT uid FROM units WHERE path=?)",
            rusqlite::params![path, path],
        )?;
        self.conn.execute("DELETE FROM units WHERE path=?", [path])?;
        Ok(())
    }
    pub fn set_meta(&self, k:&str, v:&str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO meta VALUES (?,?)", [k,v])?; Ok(())
    }
    pub fn get_meta(&self, k:&str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get(0)).ok())
    }
    pub fn count(&self, table: &str) -> Result<i64> {
        // Defense-in-depth: map table names to static SQL literals instead of formatting.
        // This ensures no caller value (even allowlisted) is interpolated into the SQL string.
        let sql = match table {
            "files" => "SELECT COUNT(*) FROM files",
            "symbols" => "SELECT COUNT(*) FROM symbols",
            "edges" => "SELECT COUNT(*) FROM edges",
            "units" => "SELECT COUNT(*) FROM units",
            "links" => "SELECT COUNT(*) FROM links",
            "callgraph" => "SELECT COUNT(*) FROM callgraph",
            "change_queue" => "SELECT COUNT(*) FROM change_queue",
            _ => bail!("store::count: unexpected table name {table:?}"),
        };
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }
    // Used only in unit tests (the CLI `locate` path runs its own SQL);
    // suppress the dead_code lint the non-test build would otherwise fire.
    #[allow(dead_code)]
    pub fn symbols_named(&self, name:&str) -> Result<Vec<(String,i64,i64)>> {
        let mut st = self.conn.prepare("SELECT file,line,col FROM symbols WHERE name=? ORDER BY file")?;
        let rows = st.query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_,_>>()?)
    }
}

fn ensure_schema_version(conn: &Connection) -> Result<()> {
    let current: Option<String> = conn
        .query_row("SELECT v FROM meta WHERE k='schema_version'", [], |r| r.get(0))
        .ok();
    if current.as_deref() != Some(SCHEMA_VERSION) {
        conn.execute(
            "INSERT OR REPLACE INTO meta VALUES ('schema_version', ?)",
            [SCHEMA_VERSION],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = Store::open(":memory:").unwrap();
        s.put_file("a.rs", "rs", "runtime-rs", 10, "deadbeef").unwrap();
        s.put_symbol("a.rs", "list_head", "fn", 5, 0).unwrap();
        s.put_edge("a.rs", "b.rs", "import").unwrap();
        assert_eq!(s.count("files").unwrap(), 1);
        assert_eq!(s.count("symbols").unwrap(), 1);
        assert_eq!(s.count("edges").unwrap(), 1);
        let fns = s.symbols_named("list_head").unwrap();
        assert_eq!(fns, vec![("a.rs".to_string(), 5, 0)]);
    }

    #[test]
    fn test_symbol_col_captured() {
        let s = Store::open(":memory:").unwrap();
        s.put_symbol("b.rs", "my_fn", "def", 10, 5).unwrap();
        let hits = s.symbols_named("my_fn").unwrap();
        assert_eq!(hits, vec![("b.rs".to_string(), 10, 5)]);
    }

    #[test]
    fn test_edge_dedup() {
        let s = Store::open(":memory:").unwrap();
        s.put_edge("a.rs", "b.rs", "import").unwrap();
        s.put_edge("a.rs", "b.rs", "import").unwrap(); // duplicate — should be ignored
        assert_eq!(s.count("edges").unwrap(), 1);
    }

    fn sample_unit(path: &str, name: &str, qualified: &str) -> Unit {
        Unit {
            path: path.to_string(),
            kind: Kind::Fn,
            name: name.to_string(),
            qualified: qualified.to_string(),
            line_start: 1,
            line_end: 5,
            facing: Facing::Internal,
            purpose: Some("does the thing".to_string()),
            body_hash: "deadbeef".to_string(),
            updated_sha: "cafe".to_string(),
        }
    }

    #[test]
    fn unit_roundtrip_dedups_by_uid() {
        let s = Store::open(":memory:").unwrap();
        s.put_unit(&sample_unit("src/a.rs", "foo", "crate::foo")).unwrap();
        s.put_unit(&sample_unit("src/a.rs", "foo", "crate::foo")).unwrap(); // same uid — replace
        assert_eq!(s.count("units").unwrap(), 1);
        // A different qualified name is a different unit.
        s.put_unit(&sample_unit("src/a.rs", "bar", "crate::bar")).unwrap();
        assert_eq!(s.count("units").unwrap(), 2);
    }

    #[test]
    fn schema_version_is_gated() {
        let s = Store::open(":memory:").unwrap();
        assert_eq!(s.get_meta("schema_version").unwrap(), Some("2".to_string()));
    }

    #[test]
    fn units_kind_check_rejects_invalid() {
        let s = Store::open(":memory:").unwrap();
        let err = s
            .conn
            .execute(
                "INSERT INTO units VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    "uid", "p", "bogus", "n", "q", 1, 2, "internal", None::<String>,
                    "h", "sha"
                ],
            )
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"), "got: {err}");
    }

    #[test]
    fn drop_file_removes_owned_rows() {
        let s = Store::open(":memory:").unwrap();
        s.put_file("src/a.rs", "rs", "compiler-rs", 10, "").unwrap();
        s.put_unit(&sample_unit("src/a.rs", "foo", "crate::foo")).unwrap();
        let uid = unit_uid("src/a.rs", Kind::Fn, "crate::foo");
        s.put_link(&uid, "internal", Some(&uid), "crate::foo", 3).unwrap();
        s.put_call(&uid, &uid).unwrap();
        assert_eq!(s.count("units").unwrap(), 1);
        assert_eq!(s.count("links").unwrap(), 1);
        assert_eq!(s.count("callgraph").unwrap(), 1);
        s.drop_file("src/a.rs").unwrap();
        assert_eq!(s.count("files").unwrap(), 0);
        assert_eq!(s.count("units").unwrap(), 0);
        assert_eq!(s.count("links").unwrap(), 0);
        assert_eq!(s.count("callgraph").unwrap(), 0);
    }

    #[test]
    fn count_rejects_unknown_table() {
        let s = Store::open(":memory:").unwrap();
        assert!(s.count("nope").is_err());
    }
}
