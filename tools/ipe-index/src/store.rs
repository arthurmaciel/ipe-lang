use anyhow::{bail, Result};
use rusqlite::Connection;

pub struct Store { pub conn: Connection }

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files   (path TEXT PRIMARY KEY, lang TEXT, role TEXT, size INTEGER, sha TEXT);
CREATE TABLE IF NOT EXISTS symbols (file TEXT, name TEXT, kind TEXT, line INTEGER, col INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS edges   (src TEXT, dst TEXT, kind TEXT, resolved TEXT);
CREATE TABLE IF NOT EXISTS kernels (name TEXT PRIMARY KEY, sky_decl INTEGER, hs_route TEXT, hs_route_loc TEXT, go_impl INTEGER, rust_impl INTEGER, go_impl_loc TEXT, rust_impl_loc TEXT, parity TEXT);
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
            "kernels" => "SELECT COUNT(*) FROM kernels",
            _ => bail!("store::count: unexpected table name {table:?}"),
        };
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }
    // Used in unit tests and by cmd_locate's direct SQL path; suppress dead_code
    // lint that fires because the integration path uses symbols_named_in_lang.
    #[allow(dead_code)]
    pub fn symbols_named(&self, name:&str) -> Result<Vec<(String,i64,i64)>> {
        let mut st = self.conn.prepare("SELECT file,line,col FROM symbols WHERE name=? ORDER BY file")?;
        let rows = st.query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_,_>>()?)
    }

    /// Like `symbols_named` but restricted to a specific language and excluding test/example paths.
    /// Returns only `kind='def'` rows with `line > 0`.
    pub fn symbols_named_in_lang(&self, name: &str, lang: &str) -> Result<Vec<(String, i64, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT s.file, s.line, s.col \
             FROM symbols s JOIN files f ON s.file = f.path \
             WHERE s.name = ?1 AND f.lang = ?2 AND s.kind = 'def' AND s.line > 0 \
               AND s.file NOT LIKE '%_test.go' \
               AND s.file NOT LIKE '%/tests/%' \
               AND s.file NOT LIKE 'examples/%' \
               AND s.file NOT LIKE '%tests/sky/%' \
             ORDER BY s.file"
        )?;
        let rows = st.query_map(rusqlite::params![name, lang], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_,_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = Store::open(":memory:").unwrap();
        s.put_file("a.rs", "rs", "runtime-rust", 10, "deadbeef").unwrap();
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

    #[test]
    fn symbols_named_in_lang_filters_language() {
        let s = Store::open(":memory:").unwrap();
        // same symbol name in both Go and Rust, plus a Go test file
        s.put_file("runtime-go/rt/fmt.go", "go", "runtime-go", 0, "").unwrap();
        s.put_file("runtime-go/rt/fmt_test.go", "go", "runtime-go", 0, "").unwrap();
        s.put_file("runtime-rust/src/fmt.rs", "rs", "runtime-rust", 0, "").unwrap();
        // Definitions
        s.conn.execute("INSERT INTO symbols VALUES ('runtime-go/rt/fmt.go','Fmt_sprint','def',10,0)", []).unwrap();
        s.conn.execute("INSERT INTO symbols VALUES ('runtime-go/rt/fmt_test.go','Fmt_sprint','def',250,0)", []).unwrap();
        s.conn.execute("INSERT INTO symbols VALUES ('runtime-rust/src/fmt.rs','fmt_sprint','def',5,0)", []).unwrap();

        // Go lookup: should NOT return the _test.go file
        let go_hits = s.symbols_named_in_lang("Fmt_sprint", "go").unwrap();
        assert_eq!(go_hits.len(), 1, "expected exactly the non-test Go file");
        assert_eq!(go_hits[0].0, "runtime-go/rt/fmt.go");

        // Rust lookup: should return the Rust file
        let rs_hits = s.symbols_named_in_lang("fmt_sprint", "rs").unwrap();
        assert_eq!(rs_hits.len(), 1);
        assert_eq!(rs_hits[0].0, "runtime-rust/src/fmt.rs");

        // Wrong lang: Rust name in Go lookup → empty
        let miss = s.symbols_named_in_lang("fmt_sprint", "go").unwrap();
        assert!(miss.is_empty());
    }

    #[test]
    fn symbols_named_in_lang_excludes_examples() {
        let s = Store::open(":memory:").unwrap();
        s.put_file("examples/01-hello/sky-out/main.go", "go", "example", 0, "").unwrap();
        s.put_file("runtime-go/rt/list.go", "go", "runtime-go", 0, "").unwrap();
        s.conn.execute("INSERT INTO symbols VALUES ('examples/01-hello/sky-out/main.go','List_head','def',10,0)", []).unwrap();
        s.conn.execute("INSERT INTO symbols VALUES ('runtime-go/rt/list.go','List_head','def',42,0)", []).unwrap();

        let hits = s.symbols_named_in_lang("List_head", "go").unwrap();
        assert_eq!(hits.len(), 1, "expected only the runtime file, not the example");
        assert_eq!(hits[0].0, "runtime-go/rt/list.go");
    }
}
