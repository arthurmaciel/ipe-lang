// Postgres `ipe_runtime/config.rs` template.
//
// NOT compiled into the standalone `ipe-runtime-rust` crate's default build
// (the crate root always uses `config.rs`, the sqlite variant). This file
// exists purely as an `include_str!` source for `ipe_backend_rust::project`
// (see `RUNTIME_CONFIG_RS_DB_POSTGRES`), which writes it verbatim to
// `<emitted-project>/src/ipe_runtime/config.rs` when `package.ipe`'s
// database driver is `Package.postgres`.
//
// Every symbol name matches `config.rs`'s sqlite variant exactly (`DbPool`,
// `DbRow`, `ipe_db_url`, `db_last_insert_id`, `db_format_sql`,
// `DB_USES_RETURNING_ID`, `db_auto_id_column`) so `db.rs` — copied verbatim
// across both driver builds — never needs a driver-specific `#[cfg]`.

#[cfg(feature = "db")]
pub type DbPool = sqlx::postgres::PgPool;
#[cfg(feature = "db")]
pub type DbRow = sqlx::postgres::PgRow;
#[cfg(feature = "db")]
pub fn ipe_db_url() -> String {
    // One config precedence `env > setting-in-code > fallback`: `DATABASE_URL`
    // wins, else an installed `Db.url` setting (a `Secret`, revealed only here at
    // the point of use, never logged), else the built-in local fallback.
    crate::system::read_env_var("DATABASE_URL")
        .ok()
        .or_else(crate::app_config::resolve_db_url_override)
        .unwrap_or_else(|| "postgres://postgres@localhost/ipe".to_string())
}

#[cfg(not(feature = "db"))]
pub type DbPool = ();
#[cfg(not(feature = "db"))]
pub type DbRow = ();
#[cfg(not(feature = "db"))]
pub fn ipe_db_url() -> String {
    String::new()
}

// Backend-portability helpers. Mirrors `config.rs`'s sqlite shape.
#[cfg(feature = "db")]
pub fn db_last_insert_id(_res: &sqlx::postgres::PgQueryResult) -> i64 {
    // Postgres's `QueryResult` carries no last-insert-id concept.
    // `DB_USES_RETURNING_ID = true` routes every `db_insert_row` call through
    // the `RETURNING id` branch instead, so this fn is never called on the
    // success path for `db_insert_row`. It IS still reachable from
    // `db_insert_fields` / `db_update_fields` on the non-`RETURNING` path — but
    // `db_insert_fields` (`db.rs`) gates its own call to this fn behind
    // `DB_USES_RETURNING_ID`, exactly like `db_insert_row`, so it is unreached
    // there too. Returning 0 unconditionally would be a fabricated value if it
    // WERE ever reached; kept as a documented "should be unreachable" stub
    // rather than a panic, matching the pre-existing sqlite behaviour for a
    // table with no autoincrement column (never a new regression surface).
    0
}

#[cfg(feature = "db")]
pub fn db_format_sql(sql: String) -> String {
    // Postgres's extended query protocol uses numbered placeholders ($1, $2,
    // …), not sqlite's positional `?`. Every SQL string built throughout
    // `db.rs` is written with `?` placeholders in emission order, matching the
    // order args are bound, so a sequential rewrite (first `?` → $1, second →
    // $2, …) maps placeholders to args backend-independently.
    //
    // The rewrite is quote-aware: `Db.exec`/`Db.unsafeQuery`/`Db.queryDecode`
    // route app-authored SQL text through here, and that text may contain a
    // literal `?` inside a quoted string, a quoted identifier, or a comment
    // (`SELECT * FROM t WHERE note = 'why?'`). Only a `?` in ordinary SQL text
    // is a placeholder; a `?` inside a span below is copied verbatim so the
    // literal and every later placeholder's numbering stay intact.
    //
    // Skipped spans (Postgres lexical rules):
    //   - single-quoted string literal `'…'`, `''` an embedded quote
    //   - double-quoted identifier `"…"`, `""` an embedded quote
    //   - dollar-quoted string `$tag$…$tag$` (tag may be empty: `$$…$$`)
    //   - line comment `-- …` to end of line
    //   - block comment `/* … */` (Postgres nests these)
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0u32;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'?' => {
                n += 1;
                out.push('$');
                out.push_str(&n.to_string());
                i += 1;
            }
            b'\'' | b'"' => {
                let quote = b;
                out.push(quote as char);
                i += 1;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c == quote {
                        // A doubled quote is an escaped quote inside the span,
                        // not its terminator.
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            out.push(quote as char);
                            out.push(quote as char);
                            i += 2;
                            continue;
                        }
                        out.push(quote as char);
                        i += 1;
                        break;
                    }
                    push_byte(&sql, &mut out, &mut i, c);
                }
            }
            b'$' => {
                if let Some(tag_end) = dollar_tag_end(bytes, i) {
                    // `bytes[i..tag_end]` is `$tag$`; copy the whole
                    // dollar-quoted body verbatim up to the matching closer.
                    let tag = &sql[i..tag_end];
                    out.push_str(tag);
                    i = tag_end;
                    if let Some(close_at) = find_subslice(bytes, tag.as_bytes(), i) {
                        out.push_str(&sql[i..close_at + tag.len()]);
                        i = close_at + tag.len();
                    } else {
                        // Unterminated dollar-quote: copy the remainder as-is
                        // (sqlx surfaces the malformed SQL as an error).
                        out.push_str(&sql[i..]);
                        i = bytes.len();
                    }
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                let nl = find_byte(bytes, b'\n', i);
                let end = nl.map_or(bytes.len(), |p| p + 1);
                out.push_str(&sql[i..end]);
                i = end;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let end = block_comment_end(bytes, i);
                out.push_str(&sql[i..end]);
                i = end;
            }
            _ => {
                push_byte(&sql, &mut out, &mut i, b);
            }
        }
    }
    out
}

// Push one UTF-8 codepoint starting at `*i` (byte `first`) to `out`, advancing
// `*i` past its full byte length. A multi-byte character never collides with
// the ASCII delimiters scanned above, so treating it as opaque bytes is safe.
#[cfg(feature = "db")]
fn push_byte(src: &str, out: &mut String, i: &mut usize, first: u8) {
    let len = utf8_len(first);
    let end = (*i + len).min(src.len());
    out.push_str(&src[*i..end]);
    *i = end;
}

#[cfg(feature = "db")]
fn utf8_len(first: u8) -> usize {
    match first {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        b if b >> 3 == 0b11110 => 4,
        _ => 1,
    }
}

// If `bytes[start] == b'$'` begins a dollar-quote opening tag `$tag$` (tag is
// zero or more letters/digits/underscores, not starting with a digit), return
// the byte index just past the closing `$` of that opening tag.
#[cfg(feature = "db")]
fn dollar_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut j = start + 1;
    let mut first = true;
    while j < bytes.len() {
        let c = bytes[j];
        if c == b'$' {
            return Some(j + 1);
        }
        let ok = c == b'_' || c.is_ascii_alphabetic() || (!first && c.is_ascii_digit());
        if !ok {
            return None;
        }
        first = false;
        j += 1;
    }
    None
}

#[cfg(feature = "db")]
fn find_byte(bytes: &[u8], target: u8, from: usize) -> Option<usize> {
    bytes.get(from..).and_then(|s| {
        s.iter()
            .position(|&c| c == target)
            .map(|p| from + p)
    })
}

#[cfg(feature = "db")]
fn find_subslice(bytes: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= bytes.len() {
        return None;
    }
    bytes
        .get(from..)
        .and_then(|s| s.windows(needle.len()).position(|w| w == needle))
        .map(|p| from + p)
}

// Index just past the closing `*/` of a block comment that opens at `start`
// (`bytes[start..start+2] == b"/*"`). Postgres nests block comments, so track
// depth; an unterminated comment consumes the remainder.
#[cfg(feature = "db")]
fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut j = start + 2;
    let mut depth = 1usize;
    while j + 1 < bytes.len() {
        if bytes[j] == b'/' && bytes[j + 1] == b'*' {
            depth += 1;
            j += 2;
        } else if bytes[j] == b'*' && bytes[j + 1] == b'/' {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    bytes.len()
}

// Whether INSERT must use `… RETURNING id` to recover the auto-id (Postgres
// has no LastInsertId).
#[cfg(feature = "db")]
pub const DB_USES_RETURNING_ID: bool = true;

// DDL fragment for an auto-incrementing primary key column.
#[cfg(feature = "db")]
pub fn db_auto_id_column() -> &'static str {
    "id BIGSERIAL PRIMARY KEY"
}
