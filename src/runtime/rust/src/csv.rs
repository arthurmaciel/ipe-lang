//! Ipe.Csv — CSV parse / encode via the `csv` crate.
//!
//! The Ipê `Csv` record (`{ header : List String, rows : List (List String) }`)
//! is mapped to `CsvDoc` below via the runtimeOpaqueTypes registry, so the
//! generated `StdCsvCsv` is a `pub use` alias of this struct. That lets the
//! kernels return/take the record DIRECTLY (no kernel can name a generated
//! per-project struct), and Ipê field access (`doc.header`) + the synthesized
//! record constructor resolve straight onto these `pub` fields.

use super::*;

// ── shared blocking-pool helper ───────────────────────────────────────
//
// `csv_parse_stream_from_file` does a blocking `std::fs::File::open` +
// incremental CSV read (up to `IPE_CSV_MAX_ROWS`, default 10M rows, with NO
// byte cap) inline. Pre-fix that work ran EAGERLY, before `Box::pin` was even
// constructed — i.e. calling the kernel function itself blocked the caller,
// not just polling the returned future. Offload to tokio's blocking pool so
// a large/slow file can't stall the tokio worker thread. This module is
// gated on the raw `csv` Cargo feature (`#[cfg(feature = "csv")]` in
// `mod.rs`), NOT the composite `csv_kernel = ["csv", "tokio"]` feature, so
// `tokio` is not guaranteed present — same constraint `file.rs` documents
// for its own `run_blocking` helper (see
// `docs/adr/0014-kernel-robustness-blocking-offload-and-toctou.md` §2.2).
#[cfg(feature = "tokio")]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(_) => Err("background csv task panicked".to_string()),
    }
}

#[cfg(not(feature = "tokio"))]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    f()
}

/// Runtime representation of the Ipê `Ipe.Csv.Csv` record. Field names + types
/// must match the Ipê alias exactly (List String -> Vec<String>, etc.).
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
fn validated_delimiter<E: From<String>>(delim: &str) -> IpeResult<E, u8> {
    match delim.as_bytes() {
        [b] if b.is_ascii() => IpeResult::Ok(*b),
        _ => IpeResult::Err(
            format!(
                "Csv: delimiter must be a single ASCII byte, got {:?}",
                delim
            )
            .into(),
        ),
    }
}

fn parse_delim<E: From<String>>(text: &str, delim: u8) -> IpeResult<E, CsvDoc> {
    let mut rdr = ::csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let header: Vec<String> = match rdr.headers() {
        Ok(h) => h.iter().map(|s| s.to_string()).collect(),
        Err(e) => return IpeResult::Err(format!("Csv.parse: {}", e).into()),
    };
    // Row cap: a large/untrusted input would otherwise accumulate unbounded into
    // `rows`. Bound it (IPE_CSV_MAX_ROWS, default 10M) → Err rather than OOM.
    // Mirrors csv_parse_stream_from_file's cap.
    let max_rows: usize = crate::system::read_env_var("IPE_CSV_MAX_ROWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10_000_000);
    let mut rows = Vec::new();
    for rec in rdr.records() {
        match rec {
            Ok(r) => {
                if rows.len() >= max_rows {
                    return IpeResult::Err(
                        format!(
                            "Csv.parse: exceeds row cap of {} (raise IPE_CSV_MAX_ROWS)",
                            max_rows
                        )
                        .into(),
                    );
                }
                rows.push(r.iter().map(|s| s.to_string()).collect());
            }
            Err(e) => return IpeResult::Err(format!("Csv.parse: {}", e).into()),
        }
    }
    IpeResult::Ok(CsvDoc { header, rows })
}

/// Spreadsheet formula-injection guard (CWE-1236 / OWASP). A cell beginning with
/// `=`, `+`, `-`, `@`, TAB, or CR is interpreted as a FORMULA by Excel/Sheets when
/// the CSV is opened — an injection vector for attacker-controlled cell data.
/// OPT-IN via `IPE_CSV_SANITIZE_FORMULAS` because the only mitigation (prefix the
/// cell with `'`) is LOSSY: it alters exported data (e.g. `-5` → `'-5`) and breaks
/// the lossless parse↔encode round-trip. Default OFF preserves round-trip; the
/// caller opts in when serving CSV to spreadsheet users, accepting the tradeoff.
fn csv_formula_guard_enabled() -> bool {
    matches!(
        crate::system::read_env_var("IPE_CSV_SANITIZE_FORMULAS")
            .ok()
            .as_deref(),
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
pub fn csv_parse<E: From<String>>(text: String) -> IpeResult<E, CsvDoc> {
    parse_delim(&text, b',')
}

/// Csv.parseWithDelimiter : String -> String -> Result Error Csv
pub fn csv_parse_with_delimiter<E: From<String>>(
    delim: String,
    text: String,
) -> IpeResult<E, CsvDoc> {
    let byte = match validated_delimiter::<E>(&delim) {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    parse_delim(&text, byte)
}

/// Csv.encode : Csv -> String
pub fn csv_encode(doc: CsvDoc) -> String {
    encode_delim(&doc, b',')
}

/// Csv.encodeWithDelimiter : String -> Csv -> String
pub fn csv_encode_with_delimiter(delim: String, doc: CsvDoc) -> String {
    // Ipê's `encodeWithDelimiter` returns `String` (no Result), so on an
    // invalid delimiter we fall back to the standard comma rather than
    // silently taking a partial/wrong byte. This matches Go's behaviour
    // (the Go csv.Writer panics on a non-ASCII Comma — we degrade gracefully).
    let byte = match validated_delimiter::<String>(&delim) {
        IpeResult::Ok(b) => b,
        IpeResult::Err(_) => b',',
    };
    encode_delim(&doc, byte)
}

fn csv_parse_stream_from_file_sync(path: &str) -> Result<Vec<Vec<String>>, String> {
    // Stream rows from a BufReader<File> rather than slurping the whole file
    // into a String first — the csv reader pulls records incrementally, so a
    // large/untrusted file no longer forces a full-file in-memory copy.
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut rdr = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(std::io::BufReader::new(file));
    // Row cap: although rows stream in, they all accumulate in `out`, so an
    // untrusted huge file is still an unbounded allocation. Bound it
    // (IPE_CSV_MAX_ROWS, default 10M) → Err rather than OOM.
    let max_rows: usize = crate::system::read_env_var("IPE_CSV_MAX_ROWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10_000_000);
    let mut out = Vec::new();
    for rec in rdr.records() {
        let r = rec.map_err(|e| e.to_string())?;
        if out.len() >= max_rows {
            return Err(format!(
                "exceeds row cap of {} (raise IPE_CSV_MAX_ROWS)",
                max_rows
            ));
        }
        out.push(r.iter().map(|s| s.to_string()).collect());
    }
    Ok(out)
}

/// Csv.parseStreamFromFile : Path -> Task Error (List (List String))
/// Returns every row (including the header).
///
/// file I/O + incremental CSV parsing (up to `IPE_CSV_MAX_ROWS`, no
/// byte cap) is offloaded to tokio's blocking pool via `run_blocking` — see
/// the module-level doc comment on `run_blocking` above.
pub fn csv_parse_stream_from_file<E: From<String> + Send + 'static>(
    path: Path,
) -> IpeTask<E, Vec<Vec<String>>> {
    let path_str = path.into_string();
    Box::pin(async move {
        match run_blocking(move || csv_parse_stream_from_file_sync(&path_str)).await {
            Ok(v) => ok_res(v),
            Err(e) => IpeResult::Err(format!("Csv.parseStreamFromFile: {}", e).into()),
        }
    })
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
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CSV_SANITIZE_FORMULAS") };
        assert!(encode_delim(&doc, b',').contains("=SUM(A1)"));
        // ON: dangerous-leading cell is prefixed with a single quote.
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_CSV_SANITIZE_FORMULAS", "1") };
        assert!(encode_delim(&doc, b',').contains("'=SUM(A1)"));
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CSV_SANITIZE_FORMULAS") };
    }

    #[test]
    fn parse_then_encode_roundtrip() {
        let doc: IpeResult<String, CsvDoc> = csv_parse("a,b\n1,2\n3,4".to_string());
        let d = match doc {
            IpeResult::Ok(d) => d,
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

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// Functional correctness (independent of whether `run_blocking` takes
    /// the real `spawn_blocking` path or the no-tokio-feature fallback —
    /// both paths must return the same rows).
    #[test]
    fn parse_stream_from_file_reads_all_rows() {
        let p = std::env::temp_dir().join(format!("ipe_csv_stream_{}.csv", std::process::id()));
        std::fs::write(&p, "a,b\n1,2\n3,4\n").unwrap();
        let path: Path = match path_from_string::<String>(p.to_string_lossy().into_owned()) {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("path_from_string failed: {e}"),
        };
        let res: IpeResult<String, Vec<Vec<String>>> = block(csv_parse_stream_from_file(path));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(rows) => {
                assert_eq!(
                    rows,
                    vec![
                        vec!["a".to_string(), "b".to_string()],
                        vec!["1".to_string(), "2".to_string()],
                        vec!["3".to_string(), "4".to_string()],
                    ]
                );
            }
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    #[test]
    fn parse_stream_from_file_missing_file_errs() {
        let path: Path = match path_from_string::<String>(
            "/nonexistent/ipe/csv/path/does-not-exist.csv".to_string(),
        ) {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("path_from_string failed: {e}"),
        };
        let res: IpeResult<String, Vec<Vec<String>>> = block(csv_parse_stream_from_file(path));
        assert!(matches!(res, IpeResult::Err(_)));
    }

    #[test]
    fn parse_stream_from_file_respects_row_cap() {
        let p = std::env::temp_dir().join(format!("ipe_csv_stream_cap_{}.csv", std::process::id()));
        std::fs::write(&p, "a\n1\n2\n3\n4\n5\n").unwrap();
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_CSV_MAX_ROWS", "2") };
        let path: Path = match path_from_string::<String>(p.to_string_lossy().into_owned()) {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("path_from_string failed: {e}"),
        };
        let res: IpeResult<String, Vec<Vec<String>>> = block(csv_parse_stream_from_file(path));
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CSV_MAX_ROWS") };
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, IpeResult::Err(_)),
            "6-row file under a 2-row cap must Err"
        );
    }
}

#[cfg(all(test, feature = "tokio"))]
mod stream_from_file_spawn_blocking_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reactor-starvation guard: on a SINGLE-WORKER (current_thread) runtime, the
    /// blocking file read + CSV parse inside `csv_parse_stream_from_file`
    /// would starve every other task on that runtime until it completes
    /// (worse still, pre-fix this work ran EAGERLY before the returned
    /// future was even polled — see the module-level doc comment on
    /// `run_blocking` above). This proves the work is offloaded to tokio's
    /// blocking-thread pool: a concurrently-spawned cheap ticker task must
    /// make progress (ticks > 0) WHILE the parse is in flight.
    ///
    /// Pre-fix this is NOT a flaky race: the ticker makes EXACTLY zero
    /// progress deterministically, because the worker thread never yields
    /// back to the executor until the parse completes.
    #[test]
    fn csv_parse_stream_from_file_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let p = std::env::temp_dir().join(format!(
            "ipe_csv_spawn_blocking_probe_{}.csv",
            std::process::id()
        ));
        // A large CSV file so the read + parse takes measurable wall time.
        {
            let mut content = String::from("a,b\n");
            for i in 0..500_000 {
                content.push_str(&format!("{},{}\n", i, i * 2));
            }
            std::fs::write(&p, content).unwrap();
        }
        let path: Path = match path_from_string::<String>(p.to_string_lossy().into_owned()) {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("path_from_string failed: {e}"),
        };

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let parse_fut: IpeTask<String, Vec<Vec<String>>> = csv_parse_stream_from_file(path);
            let _res: IpeResult<String, Vec<Vec<String>>> = parse_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        let _ = std::fs::remove_file(&p);

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while csv_parse_stream_from_file ran — \
             the blocking read+parse is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}
