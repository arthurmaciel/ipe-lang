use anyhow::{bail, Result};
use rusqlite::Connection;

pub struct Store { pub conn: Connection }

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files   (path TEXT PRIMARY KEY, lang TEXT, role TEXT, size INTEGER, sha TEXT);
CREATE TABLE IF NOT EXISTS symbols (file TEXT, name TEXT, kind TEXT, line INTEGER, col INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS edges   (src TEXT, dst TEXT, kind TEXT, resolved TEXT);
CREATE TABLE IF NOT EXISTS meta    (k TEXT PRIMARY KEY, v TEXT);
CREATE INDEX IF NOT EXISTS i_sym_name ON symbols(name);
CREATE INDEX IF NOT EXISTS i_edge_src ON edges(src);
CREATE INDEX IF NOT EXISTS i_edge_dst ON edges(dst);
CREATE UNIQUE INDEX IF NOT EXISTS u_edge ON edges(src, dst, kind);
";

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
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
    pub fn drop_file(&self, path:&str) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE path=?", [path])?;
        self.conn.execute("DELETE FROM symbols WHERE file=?", [path])?;
        self.conn.execute("DELETE FROM edges WHERE src=?", [path])?;
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
}
