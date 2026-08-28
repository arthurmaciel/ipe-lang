#![allow(clippy::type_complexity)]
// Ipe.Config — typed TOML / YAML / JSON config decoders.
//
// Config reuses the JSON Decoder representation (`Decoder<E, T>` over a
// `serde_json::Value`): TOML and YAML are parsed into the same `Value`, then the
// decoder runs unchanged. The combinators (string / int / float / bool / field /
// at / list / map / andThen / succeed / fail) ARE the shared `decode_*` kernels — the
// Ipê codegen maps `Config.*` straight onto them. Only the format front-ends,
// `nullable`, and `loadFromFile` live here, because Config's signatures put the
// source `String` FIRST (`decodeToml : String -> Decoder a -> Result Error a`),
// the opposite of `decode_from_json_string`'s decoder-first argument order.
use super::json::{Decoder, JsonVal};
use super::*;

// Config.nullable : Decoder a -> Decoder (Maybe a)
// Returns Ipê's IpeMaybe (not Rust Option) so the decoded value matches the
// `Maybe a` the Ipê annotation lowers to.
pub fn config_nullable<E: From<String> + 'static, T: 'static + Send>(
    decoder: Decoder<E, T>,
) -> Decoder<E, IpeMaybe<T>> {
    let inner_fields = decoder.fields.clone();
    Decoder::new(
        Box::new(move |v| match v {
            JsonVal::Null => IpeResult::Ok(IpeMaybe::Nothing),
            _ => match (decoder.run)(v) {
                IpeResult::Ok(t) => IpeResult::Ok(IpeMaybe::Just(t)),
                IpeResult::Err(e) => IpeResult::Err(e),
            },
        }),
        inner_fields,
    )
}

// Config.maybe : Decoder a -> Decoder (Maybe a)
// Unlike `nullable` (which only tolerates null/missing), `maybe` catches ANY
// decode failure: `Just` on success, `Nothing` on any error. Never fails.
pub fn config_maybe<E: From<String> + 'static, T: 'static + Send>(
    decoder: Decoder<E, T>,
) -> Decoder<E, IpeMaybe<T>> {
    let inner_fields = decoder.fields.clone();
    Decoder::new(
        Box::new(move |v| match (decoder.run)(v) {
            IpeResult::Ok(t) => IpeResult::Ok(IpeMaybe::Just(t)),
            IpeResult::Err(_) => IpeResult::Ok(IpeMaybe::Nothing),
        }),
        inner_fields,
    )
}

// Config.dict : Decoder a -> Decoder (Dict String a)
// Decode every object entry into a `Dict String a` (runtime `IpeDict<T>`),
// applying `decoder` to each value. Non-object input is `Err`; the first entry
// whose value fails short-circuits with its real error.
pub fn config_dict<E: From<String> + 'static, T: 'static + Send>(
    decoder: Decoder<E, T>,
) -> Decoder<E, IpeDict<T>> {
    Decoder::new(
        Box::new(move |v| match v.as_object() {
            Some(obj) => {
                let d = &decoder;
                let mut out: IpeDict<T> = IpeDict::new();
                for (key, val) in obj {
                    match (d.run)(val) {
                        IpeResult::Ok(t) => {
                            out.insert(key.clone(), t);
                        }
                        IpeResult::Err(e) => return IpeResult::Err(e),
                    }
                }
                IpeResult::Ok(out)
            }
            None => IpeResult::Err(str_err("expected object")),
        }),
        vec![],
    )
}

fn run_decoder<E: From<String> + 'static, T>(
    parsed: Result<JsonVal, String>,
    decoder: Decoder<E, T>,
) -> IpeResult<E, T> {
    match parsed {
        Ok(v) => (decoder.run)(&v),
        Err(e) => IpeResult::Err(str_err(&e)),
    }
}

// Config.decodeJson : String -> Decoder a -> Result Error a
pub fn config_decode_json<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> IpeResult<E, T> {
    run_decoder(
        serde_json::from_str(&s).map_err(|e| format!("json parse: {}", e)),
        decoder,
    )
}

// Config.decodeToml : String -> Decoder a -> Result Error a
pub fn config_decode_toml<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> IpeResult<E, T> {
    run_decoder(
        toml::from_str(&s).map_err(|e| format!("toml parse: {}", e)),
        decoder,
    )
}

/// Default cap on a YAML source string parsed directly via `Config.decodeYaml`
/// (the file-load path enforces its own `IPE_CONFIG_MAX_BYTES` cap before reading).
/// 4 MiB; override via `IPE_YAML_MAX_BYTES`.
const YAML_SOURCE_CAP_DEFAULT: usize = 4 * 1024 * 1024;

fn yaml_source_cap() -> usize {
    crate::system::read_env_var("IPE_YAML_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(YAML_SOURCE_CAP_DEFAULT)
}

// Config.decodeYaml : String -> Decoder a -> Result Error a
pub fn config_decode_yaml<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> IpeResult<E, T> {
    // Defence-in-depth against YAML "billion laughs" / anchor-alias bombs:
    //   1. Bound the SOURCE size so a huge input can't be parsed at all (this is
    //      the cheap, behaviour-preserving guard the audit asks for).
    //   2. serde_yaml 0.9 itself bounds alias/anchor EXPANSION — a recursive
    //      anchor bomb trips its built-in "repetition limit exceeded" (verified),
    //      so a small-but-exponential input cannot expand without bound.
    let cap = yaml_source_cap();
    if s.len() > cap {
        return IpeResult::Err(str_err(&format!(
            "yaml parse: input is {} bytes, over the {} byte cap (IPE_YAML_MAX_BYTES)",
            s.len(),
            cap
        )));
    }
    run_decoder(
        serde_yaml::from_str(&s).map_err(|e| format!("yaml parse: {}", e)),
        decoder,
    )
}

// ── shared blocking-pool helper ───────────────────────────────────────
//
// `config_load_from_file` does a blocking `std::fs::File::open` + capped
// `read_to_string` (up to `IPE_CONFIG_MAX_BYTES`, default 16 MiB) inline.
// Offload it to tokio's blocking pool so a large/slow-filesystem config read
// can't stall the tokio worker thread polling this future. This module is
// gated on the `config` Cargo feature (`config = ["json", "toml",
// "serde_yaml"]`, runtime/Cargo.toml), which does NOT pull in `tokio`, so
// `tokio` is not guaranteed present here — same constraint `file.rs`
// documents for its own `run_blocking` helper (see
// `docs/adr/0014-kernel-robustness-blocking-offload-and-toctou.md` §2.2).
#[cfg(feature = "tokio")]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(_) => Err("background config-file task panicked".to_string()),
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

fn config_read_capped(path: &str, cap: u64) -> Result<String, String> {
    // Open first, then enforce the cap THROUGH a capped reader rather than
    // trusting a metadata-only precheck: std::fs::metadata reports len()==0
    // for non-regular files (FIFO, /dev/zero, char devices), so a metadata
    // gate would pass and the subsequent slurp would read unbounded bytes.
    // Reject non-regular files outright, then bound the read at cap+1 bytes.
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let meta = file.metadata().map_err(|e| format!("{}", e))?;
    if !meta.file_type().is_file() {
        return Err(format!("config file {:?} is not a regular file", path));
    }
    if meta.len() > cap {
        return Err(format!(
            "config file {:?} is {} bytes, over the {} byte cap (IPE_CONFIG_MAX_BYTES)",
            path,
            meta.len(),
            cap
        ));
    }
    let mut contents = String::new();
    // take(cap+1): if the file grew between the metadata check and the read,
    // or reports a misleading size, the read still can't exceed cap+1 bytes.
    file.take(cap.saturating_add(1))
        .read_to_string(&mut contents)
        .map_err(|e| format!("{}", e))?;
    if contents.len() as u64 > cap {
        return Err(format!(
            "config file {:?} exceeds the {} byte cap (IPE_CONFIG_MAX_BYTES)",
            path, cap
        ));
    }
    Ok(contents)
}

// Config.loadFromFile : Path -> Decoder a -> Task Error a
// Extension dispatch: .toml / .yaml|.yml / .json (default json).
//
// the file read is offloaded via `run_blocking` (see the module-level
// doc comment above) so a large/slow config read can't stall the tokio
// worker thread. The decode dispatch itself runs back on the calling task
// after the read completes — decoding an already-in-memory, size-capped
// (≤16 MiB default) string is fast enough not to warrant its own offload.
pub fn config_load_from_file<E: From<String> + Send + 'static, T: Send + 'static>(
    path: Path,
    decoder: Decoder<E, T>,
) -> IpeTask<E, T> {
    let path_str = path.into_string();
    Box::pin(async move {
        // Cap the file size before slurping it into memory so a Config.loadFromFile
        // on an attacker-influenced path can't force an unbounded in-memory copy
        // (memory DoS). Default 16 MiB; override via IPE_CONFIG_MAX_BYTES.
        let cap: u64 = crate::system::read_env_var("IPE_CONFIG_MAX_BYTES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(16 * 1024 * 1024);
        let contents = match run_blocking({
            let path_str = path_str.clone();
            move || config_read_capped(&path_str, cap)
        })
        .await
        {
            Ok(c) => c,
            Err(e) => return IpeResult::Err(str_err(&e)),
        };
        let lower = path_str.to_ascii_lowercase();
        if lower.ends_with(".toml") {
            config_decode_toml(contents, decoder)
        } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            config_decode_yaml(contents, decoder)
        } else {
            config_decode_json(contents, decoder)
        }
    })
}

#[cfg(test)]
mod load_from_file_tests {
    use super::*;
    use crate::json::{decode_field, json_decode_string};

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn name_decoder() -> Decoder<String, String> {
        decode_field("name".to_string(), json_decode_string::<String>())
    }

    /// Functional correctness (independent of whether `run_blocking` takes
    /// the real `spawn_blocking` path or the no-tokio-feature fallback —
    /// both paths must return the same decoded result).
    #[test]
    fn loads_and_decodes_json() {
        let p = std::env::temp_dir().join(format!("ipe_cfg_json_{}.json", std::process::id()));
        std::fs::write(&p, r#"{"name": "ipe"}"#).unwrap();
        let path: Path = path_from_string(p.to_string_lossy().into_owned()).unwrap();
        let res: IpeResult<String, String> = block(config_load_from_file(
            path,
            name_decoder(),
        ));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s, "ipe"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    #[test]
    fn loads_and_decodes_toml() {
        let p = std::env::temp_dir().join(format!("ipe_cfg_toml_{}.toml", std::process::id()));
        std::fs::write(&p, "name = \"ipe\"\n").unwrap();
        let path: Path = path_from_string(p.to_string_lossy().into_owned()).unwrap();
        let res: IpeResult<String, String> = block(config_load_from_file(
            path,
            name_decoder(),
        ));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s, "ipe"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    #[test]
    fn over_cap_file_errs() {
        let p = std::env::temp_dir().join(format!("ipe_cfg_over_cap_{}.json", std::process::id()));
        std::fs::write(&p, vec![b'a'; 8192]).unwrap();
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_CONFIG_MAX_BYTES", "1024") };
        let path: Path = path_from_string(p.to_string_lossy().into_owned()).unwrap();
        let res: IpeResult<String, String> = block(config_load_from_file(
            path,
            name_decoder(),
        ));
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CONFIG_MAX_BYTES") };
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, IpeResult::Err(_)),
            "8 KiB config file under a 1 KiB cap must Err"
        );
    }

    #[test]
    fn missing_file_errs() {
        let path: Path =
            path_from_string("/nonexistent/ipe/config/path/does-not-exist.json".to_string())
                .unwrap();
        let res: IpeResult<String, String> = block(config_load_from_file(path, name_decoder()));
        assert!(matches!(res, IpeResult::Err(_)));
    }
}

#[cfg(all(test, feature = "tokio"))]
mod load_from_file_spawn_blocking_tests {
    use super::*;
    use crate::json::{decode_field, json_decode_string};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reactor-starvation guard: on a SINGLE-WORKER (current_thread) runtime, the
    /// blocking file read inside `config_load_from_file` would starve every
    /// other task on that runtime until the read completes. This proves the
    /// read is offloaded to tokio's blocking-thread pool: a concurrently-
    /// spawned cheap ticker task must make progress (ticks > 0) WHILE the
    /// read is in flight.
    ///
    /// Pre-fix this is NOT a flaky race: the ticker makes EXACTLY zero
    /// progress deterministically, because the worker thread never yields
    /// back to the executor until the read completes.
    #[test]
    fn config_load_from_file_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let p = std::env::temp_dir().join(format!(
            "ipe_cfg_spawn_blocking_probe_{}.json",
            std::process::id()
        ));
        // A big JSON string value, large enough that the read takes
        // measurable (not instant) wall time. Stays under the default 16 MiB
        // cap.
        let big = "x".repeat(12 * 1024 * 1024);
        std::fs::write(&p, format!(r#"{{"name": "{}"}}"#, big)).unwrap();
        let path: Path = path_from_string(p.to_string_lossy().into_owned()).unwrap();

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let decoder =
                decode_field::<String, String>("name".to_string(), json_decode_string::<String>());
            let load_fut: IpeTask<String, String> = config_load_from_file(path, decoder);
            let _res: IpeResult<String, String> = load_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        let _ = std::fs::remove_file(&p);

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while config_load_from_file ran — \
             the blocking file read is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}
