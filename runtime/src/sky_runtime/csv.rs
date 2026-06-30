//! Std.Csv — CSV parse / encode via the `csv` crate.
//!
//! The Sky `Csv` record (`{ header : List String, rows : List (List String) }`)
//! is mapped to `CsvDoc` below via the runtimeOpaqueTypes registry, so the
//! generated `StdCsvCsv` is a `pub use` alias of this struct. That lets the
//! kernels return/take the record DIRECTLY (no kernel can name a generated
//! per-project struct), and Sky field access (`doc.header`) + the synthesized
//! record constructor resolve straight onto these `pub` fields.

use super::*;
use std::future::ready;

/// Runtime representation of the Sky `Std.Csv.Csv` record. Field names + types
/// must match the Sky alias exactly (List String -> Vec<String>, etc.).
#[derive(Clone, Debug, PartialEq)]
pub struct CsvDoc {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Validate that `delim` is exactly one ASCII byte, as required by the csv
/// crate. A multi-byte string (e.g. a UTF-8 character) or an empty string is
/// silently mishandled by the old `first_byte` helper — the multi-byte case
/// takes only the first (possibly continuation) byte, producing a nonsense
/// delimiter; the empty case silently falls back to `,`, which is wrong for
/// callers that passed an explicit delimiter. Return `Err` for both cases.
fn validated_delimiter<E: From<String>>(delim: &str) -> SkyResult<E, u8> {
    match delim.as_bytes() {
        [b] if b.is_ascii() => SkyResult::Ok(*b),
        _ => SkyResult::Err(
            format!(
                "Csv: delimiter must be a single ASCII byte, got {:?}",
                delim
            )
            .into(),
        ),
    }
}

fn parse_delim<E: From<String>>(text: &str, delim: u8) -> SkyResult<E, CsvDoc> {
    let mut rdr = ::csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let header: Vec<String> = match rdr.headers() {
        Ok(h) => h.iter().map(|s| s.to_string()).collect(),
        Err(e) => return SkyResult::Err(format!("Csv.parse: {}", e).into()),
    };
    // Row cap: a large/untrusted input would otherwise accumulate unbounded into
    // `rows`. Bound it (SKY_CSV_MAX_ROWS, default 10M) → Err rather than OOM.
    // Mirrors csv_parse_stream_from_file's cap.
    let max_rows: usize = std::env::var("SKY_CSV_MAX_ROWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10_000_000);
    let mut rows = Vec::new();
    for rec in rdr.records() {
        match rec {
            Ok(r) => {
                if rows.len() >= max_rows {
                    return SkyResult::Err(
                        format!(
                            "Csv.parse: exceeds row cap of {} (raise SKY_CSV_MAX_ROWS)",
                            max_rows
                        )
                        .into(),
                    );
                }
                rows.push(r.iter().map(|s| s.to_string()).collect());
            }
            Err(e) => return SkyResult::Err(format!("Csv.parse: {}", e).into()),
        }
    }
    SkyResult::Ok(CsvDoc { header, rows })
}

/// Spreadsheet formula-injection guard (CWE-1236 / OWASP). A cell beginning with
/// `=`, `+`, `-`, `@`, TAB, or CR is interpreted as a FORMULA by Excel/Sheets when
/// the CSV is opened — an injection vector for attacker-controlled cell data.
/// OPT-IN via `SKY_CSV_SANITIZE_FORMULAS` because the only mitigation (prefix the
/// cell with `'`) is LOSSY: it alters exported data (e.g. `-5` → `'-5`) and breaks
/// the lossless parse↔encode round-trip. Default OFF preserves round-trip; the
/// caller opts in when serving CSV to spreadsheet users, accepting the tradeoff.
fn csv_formula_guard_enabled() -> bool {
    matches!(
        std::env::var("SKY_CSV_SANITIZE_FORMULAS").ok().as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    )
}

fn guard_formula(cell: &str) -> std::borrow::Cow<'_, str> {
    match cell.as_bytes().first() {
        Some(b'=') | Some(b'+') | Some(b'-') | Some(b'@') | Some(b'\t') | Some(b'\r') => {
            std::borrow::Cow::Owned(format!("'{}", cell))
        }
        _ => std::borrow::Cow::Borrowed(cell),
    }
}

fn encode_delim(doc: &CsvDoc, delim: u8) -> String {
    // flexible(true): a parsed-then-encoded doc may carry ragged rows (row width ≠
    // header width) since the reader is flexible. Without this the writer errors on
    // the first mismatch and the swallowed error silently DROPS that row — breaking
    // lossless round-trip. Flexible emits every row verbatim.
    let mut wtr = ::csv::WriterBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .from_writer(vec![]);
    let guard = csv_formula_guard_enabled();
    for row in std::iter::once(&doc.header).chain(doc.rows.iter()) {
        if guard {
            let safe: Vec<String> = row.iter().map(|c| guard_formula(c).into_owned()).collect();
            let _ = wtr.write_record(&safe);
        } else {
            let _ = wtr.write_record(row);
        }
    }
    let bytes = wtr.into_inner().unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Csv.parse : String -> Result Error Csv
pub fn csv_parse<E: From<String>>(text: String) -> SkyResult<E, CsvDoc> {
    parse_delim(&text, b',')
}

/// Csv.parseWithDelimiter : String -> String -> Result Error Csv
pub fn csv_parse_with_delimiter<E: From<String>>(
    delim: String,
    text: String,
) -> SkyResult<E, CsvDoc> {
    let byte = match validated_delimiter::<E>(&delim) {
        SkyResult::Ok(b) => b,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    parse_delim(&text, byte)
}

/// Csv.encode : Csv -> String
pub fn csv_encode(doc: CsvDoc) -> String {
    encode_delim(&doc, b',')
}

/// Csv.encodeWithDelimiter : String -> Csv -> String
pub fn csv_encode_with_delimiter(delim: String, doc: CsvDoc) -> String {
    // Sky's `encodeWithDelimiter` returns `String` (no Result), so on an
    // invalid delimiter we fall back to the standard comma rather than
    // silently taking a partial/wrong byte. This matches Go's behaviour
    // (the Go csv.Writer panics on a non-ASCII Comma — we degrade gracefully).
    let byte = match validated_delimiter::<String>(&delim) {
        SkyResult::Ok(b) => b,
        SkyResult::Err(_) => b',',
    };
    encode_delim(&doc, byte)
}

/// Csv.parseStreamFromFile : String -> Task Error (List (List String))
/// Returns every row (including the header).
pub fn csv_parse_stream_from_file<E: From<String> + Send + 'static>(
    path: String,
) -> SkyTask<E, Vec<Vec<String>>> {
    let result = (|| -> Result<Vec<Vec<String>>, String> {
        // Stream rows from a BufReader<File> rather than slurping the whole file
        // into a String first — the csv reader pulls records incrementally, so a
        // large/untrusted file no longer forces a full-file in-memory copy.
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut rdr = ::csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(std::io::BufReader::new(file));
        // Row cap: although rows stream in, they all accumulate in `out`, so an
        // untrusted huge file is still an unbounded allocation. Bound it
        // (SKY_CSV_MAX_ROWS, default 10M) → Err rather than OOM.
        let max_rows: usize = std::env::var("SKY_CSV_MAX_ROWS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(10_000_000);
        let mut out = Vec::new();
        for rec in rdr.records() {
            let r = rec.map_err(|e| e.to_string())?;
            if out.len() >= max_rows {
                return Err(format!(
                    "exceeds row cap of {} (raise SKY_CSV_MAX_ROWS)",
                    max_rows
                ));
            }
            out.push(r.iter().map(|s| s.to_string()).collect());
        }
        Ok(out)
    })();
    Box::pin(ready(match result {
        Ok(v) => ok_res(v),
        Err(e) => SkyResult::Err(format!("Csv.parseStreamFromFile: {}", e).into()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_guard_is_opt_in() {
        let doc = CsvDoc {
            header: vec!["a".into()],
            rows: vec![vec!["=SUM(A1)".into()]],
        };
        // Default OFF: lossless (formula cell emitted verbatim, just CSV-quoted).
        std::env::remove_var("SKY_CSV_SANITIZE_FORMULAS");
        assert!(encode_delim(&doc, b',').contains("=SUM(A1)"));
        // ON: dangerous-leading cell is prefixed with a single quote.
        std::env::set_var("SKY_CSV_SANITIZE_FORMULAS", "1");
        assert!(encode_delim(&doc, b',').contains("'=SUM(A1)"));
        std::env::remove_var("SKY_CSV_SANITIZE_FORMULAS");
    }

    #[test]
    fn parse_then_encode_roundtrip() {
        let doc: SkyResult<String, CsvDoc> = csv_parse("a,b\n1,2\n3,4".to_string());
        let d = match doc {
            SkyResult::Ok(d) => d,
            _ => panic!("parse failed"),
        };
        assert_eq!(d.header, vec!["a", "b"]);
        assert_eq!(d.rows, vec![vec!["1", "2"], vec!["3", "4"]]);
        let out = csv_encode(d);
        assert_eq!(out, "a,b\n1,2\n3,4\n");
    }

    #[test]
    fn quoting() {
        let doc = CsvDoc {
            header: vec!["x".into()],
            rows: vec![vec!["a,b".into()]],
        };
        assert_eq!(csv_encode(doc), "x\n\"a,b\"\n");
    }
}
