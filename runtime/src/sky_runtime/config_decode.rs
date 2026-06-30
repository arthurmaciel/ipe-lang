#![allow(clippy::type_complexity)]
// Std.Config — typed TOML / YAML / JSON config decoders.
//
// Config reuses the JSON Decoder representation (`Decoder<E, T>` over a
// `serde_json::Value`): TOML and YAML are parsed into the same `Value`, then the
// decoder runs unchanged. The combinators (string / int / float / bool / field /
// at / list / map / andThen / succeed / fail) ARE the shared `decode_*` kernels — the
// Sky codegen maps `Config.*` straight onto them. Only the format front-ends,
// `nullable`, and `loadFromFile` live here, because Config's signatures put the
// source `String` FIRST (`decodeToml : String -> Decoder a -> Result Error a`),
// the opposite of `decode_from_json_string`'s decoder-first argument order.
use super::json::{Decoder, JsonVal};
use super::*;

// Config.nullable : Decoder a -> Decoder (Maybe a)
// Returns Sky's SkyMaybe (not Rust Option) so the decoded value matches the
// `Maybe a` the Sky annotation lowers to.
pub fn config_nullable<E: From<String> + 'static, T: 'static + Send>(
    decoder: Decoder<E, T>,
) -> Decoder<E, SkyMaybe<T>> {
    let inner_fields = decoder.fields.clone();
    Decoder::new(
        Box::new(move |v| match v {
            JsonVal::Null => SkyResult::Ok(SkyMaybe::Nothing),
            _ => match (decoder.run)(v) {
                SkyResult::Ok(t) => SkyResult::Ok(SkyMaybe::Just(t)),
                SkyResult::Err(e) => SkyResult::Err(e),
            },
        }),
        inner_fields,
    )
}

fn run_decoder<E: From<String> + 'static, T>(
    parsed: Result<JsonVal, String>,
    decoder: Decoder<E, T>,
) -> SkyResult<E, T> {
    match parsed {
        Ok(v) => (decoder.run)(&v),
        Err(e) => SkyResult::Err(str_err(&e)),
    }
}

// Config.decodeJson : String -> Decoder a -> Result Error a
pub fn config_decode_json<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> SkyResult<E, T> {
    run_decoder(
        serde_json::from_str(&s).map_err(|e| format!("json parse: {}", e)),
        decoder,
    )
}

// Config.decodeToml : String -> Decoder a -> Result Error a
pub fn config_decode_toml<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> SkyResult<E, T> {
    run_decoder(
        toml::from_str(&s).map_err(|e| format!("toml parse: {}", e)),
        decoder,
    )
}

// Config.decodeYaml : String -> Decoder a -> Result Error a
pub fn config_decode_yaml<E: From<String> + 'static, T>(
    s: String,
    decoder: Decoder<E, T>,
) -> SkyResult<E, T> {
    run_decoder(
        serde_yaml::from_str(&s).map_err(|e| format!("yaml parse: {}", e)),
        decoder,
    )
}

// Config.loadFromFile : String -> Decoder a -> Task Error a
// Extension dispatch: .toml / .yaml|.yml / .json (default json).
pub fn config_load_from_file<E: From<String> + Send + 'static, T: Send + 'static>(
    path: String,
    decoder: Decoder<E, T>,
) -> SkyTask<E, T> {
    Box::pin(async move {
        // Cap the file size before slurping it into memory so a Config.loadFromFile
        // on an attacker-influenced path can't force an unbounded in-memory copy
        // (memory DoS). Default 16 MiB; override via SKY_CONFIG_MAX_BYTES.
        let cap: u64 = std::env::var("SKY_CONFIG_MAX_BYTES")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(16 * 1024 * 1024);
        // Open first, then enforce the cap THROUGH a capped reader rather than
        // trusting a metadata-only precheck: std::fs::metadata reports len()==0
        // for non-regular files (FIFO, /dev/zero, char devices), so a metadata
        // gate would pass and the subsequent slurp would read unbounded bytes.
        // Reject non-regular files outright, then bound the read at cap+1 bytes.
        use std::io::Read;
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => return SkyResult::Err(str_err(&format!("{}", e))),
        };
        match file.metadata() {
            Ok(meta) => {
                if !meta.file_type().is_file() {
                    return SkyResult::Err(str_err(&format!(
                        "config file {:?} is not a regular file",
                        path
                    )));
                }
                if meta.len() > cap {
                    return SkyResult::Err(str_err(&format!(
                        "config file {:?} is {} bytes, over the {} byte cap (SKY_CONFIG_MAX_BYTES)",
                        path,
                        meta.len(),
                        cap
                    )));
                }
            }
            Err(e) => return SkyResult::Err(str_err(&format!("{}", e))),
        }
        let mut contents = String::new();
        // take(cap+1): if the file grew between the metadata check and the read,
        // or reports a misleading size, the read still can't exceed cap+1 bytes.
        if let Err(e) = file
            .take(cap.saturating_add(1))
            .read_to_string(&mut contents)
        {
            return SkyResult::Err(str_err(&format!("{}", e)));
        }
        if contents.len() as u64 > cap {
            return SkyResult::Err(str_err(&format!(
                "config file {:?} exceeds the {} byte cap (SKY_CONFIG_MAX_BYTES)",
                path, cap
            )));
        }
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".toml") {
            config_decode_toml(contents, decoder)
        } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            config_decode_yaml(contents, decoder)
        } else {
            config_decode_json(contents, decoder)
        }
    })
}
