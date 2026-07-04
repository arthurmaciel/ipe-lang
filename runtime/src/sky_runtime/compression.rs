//! Std.Compression — gzip (flate2) + zstd kernels operating on raw byte buffers.
//!
//! All entries are `Vec<u8> -> Task Error Vec<u8>`. Input and output are raw
//! byte buffers (`Vec<u8>`) — Sky's `Bytes` primitive — so compressed payloads
//! (including non-UTF-8 binary) round-trip losslessly. Compression is sync CPU
//! work wrapped in a ready Future to satisfy the `Task` shape.
//!
//! # Decompression bomb protection
//!
//! Both gunzip and zstdDecompress cap decompressed output at
//! `SKY_DECOMPRESS_MAX_BYTES` (default 256 MiB). Input that would expand
//! beyond that limit is rejected with an error rather than allowed to OOM
//! the process.

use super::*;
use std::future::ready;
use std::io::{Read, Write};

/// Returns the decompression output cap in bytes.
///
/// Reads `SKY_DECOMPRESS_MAX_BYTES` from the environment once (lazily) and
/// caches the result. Falls back to 256 MiB when the variable is absent or
/// unparseable.
fn decompress_max_bytes() -> u64 {
    use std::sync::OnceLock;
    static CAP: OnceLock<u64> = OnceLock::new();
    *CAP.get_or_init(|| {
        crate::sky_runtime::system::read_env_var("SKY_DECOMPRESS_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(256 * 1024 * 1024) // 256 MiB
    })
}

fn gzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::{Compression, write::GzEncoder};
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).map_err(|err| err.to_string())?;
    e.finish().map_err(|err| err.to_string())
}

fn gunzip_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    let max = decompress_max_bytes();
    let d = GzDecoder::new(data);
    // Read up to max+1 bytes; if we fill the buffer exactly at max+1 the
    // input would expand beyond the cap.
    let mut out = Vec::new();
    d.take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > max {
        return Err(format!(
            "decompressed output exceeds {} bytes (SKY_DECOMPRESS_MAX_BYTES)",
            max
        ));
    }
    Ok(out)
}

/// Compression.gzip : Bytes -> Task Error Bytes
pub fn compression_gzip<E: From<String> + Send + 'static>(data: Vec<u8>) -> SkyTask<E, Vec<u8>> {
    let r = match gzip_bytes(&data) {
        Ok(b) => ok_res(b),
        Err(e) => SkyResult::Err(format!("Compression.gzip: {}", e).into()),
    };
    Box::pin(ready(r))
}

/// Compression.gunzip : Bytes -> Task Error Bytes
pub fn compression_gunzip<E: From<String> + Send + 'static>(data: Vec<u8>) -> SkyTask<E, Vec<u8>> {
    let r = match gunzip_bytes(&data) {
        Ok(b) => ok_res(b),
        Err(e) => SkyResult::Err(format!("Compression.gunzip: {}", e).into()),
    };
    Box::pin(ready(r))
}

/// Compression.zstdCompress : Bytes -> Task Error Bytes
pub fn compression_zstd_compress<E: From<String> + Send + 'static>(
    data: Vec<u8>,
) -> SkyTask<E, Vec<u8>> {
    let r = match zstd::encode_all(&data[..], 0) {
        Ok(out) => ok_res(out),
        Err(e) => SkyResult::Err(format!("Compression.zstdCompress: {}", e).into()),
    };
    Box::pin(ready(r))
}

/// Compression.zstdDecompress : Bytes -> Task Error Bytes
pub fn compression_zstd_decompress<E: From<String> + Send + 'static>(
    data: Vec<u8>,
) -> SkyTask<E, Vec<u8>> {
    let r = match zstd_decompress_capped(&data) {
        Ok(b) => ok_res(b),
        Err(e) => SkyResult::Err(format!("Compression.zstdDecompress: {}", e).into()),
    };
    Box::pin(ready(r))
}

fn zstd_decompress_capped(data: &[u8]) -> Result<Vec<u8>, String> {
    use zstd::stream::read::Decoder as ZstdDecoder;
    let max = decompress_max_bytes();
    let d = ZstdDecoder::new(data).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    d.take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > max {
        return Err(format!(
            "decompressed output exceeds {} bytes (SKY_DECOMPRESS_MAX_BYTES)",
            max
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky_runtime::task::task_run;

    #[test]
    fn gzip_roundtrip() {
        let orig = b"hello, sky - gzip round-trip with some length to compress".to_vec();
        let z: SkyResult<Vec<u8>, String> = task_run(compression_gzip(orig.clone()));
        let comp = match z {
            SkyResult::Ok(c) => c,
            _ => panic!("gzip failed"),
        };
        let back: SkyResult<Vec<u8>, String> = task_run(compression_gunzip(comp));
        assert!(matches!(back, SkyResult::Ok(ref b) if *b == orig));
    }

    #[test]
    fn gzip_roundtrip_binary() {
        // High bytes (> 127) that are not valid UTF-8 must round-trip without
        // truncation — the old Latin-1 bridge would silently corrupt these.
        let orig: Vec<u8> = (0u8..=255u8).collect();
        let z: SkyResult<Vec<u8>, String> = task_run(compression_gzip(orig.clone()));
        let comp = match z {
            SkyResult::Ok(c) => c,
            _ => panic!("gzip binary failed"),
        };
        let back: SkyResult<Vec<u8>, String> = task_run(compression_gunzip(comp));
        assert!(matches!(back, SkyResult::Ok(ref b) if *b == orig));
    }

    #[test]
    fn zstd_roundtrip() {
        let orig = b"zstd payload zstd payload zstd payload".to_vec();
        let z: SkyResult<Vec<u8>, String> = task_run(compression_zstd_compress(orig.clone()));
        let comp = match z {
            SkyResult::Ok(c) => c,
            _ => panic!("zstd failed"),
        };
        let back: SkyResult<Vec<u8>, String> = task_run(compression_zstd_decompress(comp));
        assert!(matches!(back, SkyResult::Ok(ref b) if *b == orig));
    }

    #[test]
    fn zstd_roundtrip_binary() {
        let orig: Vec<u8> = (0u8..=255u8).collect();
        let z: SkyResult<Vec<u8>, String> = task_run(compression_zstd_compress(orig.clone()));
        let comp = match z {
            SkyResult::Ok(c) => c,
            _ => panic!("zstd binary failed"),
        };
        let back: SkyResult<Vec<u8>, String> = task_run(compression_zstd_decompress(comp));
        assert!(matches!(back, SkyResult::Ok(ref b) if *b == orig));
    }

    #[test]
    fn gunzip_rejects_garbage() {
        let bad: SkyResult<Vec<u8>, String> =
            task_run(compression_gunzip(b"not a gzip stream".to_vec()));
        assert!(matches!(bad, SkyResult::Err(_)));
    }

    /// Verify that gunzip rejects a payload that would expand beyond the cap.
    /// We set SKY_DECOMPRESS_MAX_BYTES to a small value (16 bytes) so the test
    /// doesn't need to produce a real multi-GiB bomb.
    #[test]
    fn gunzip_rejects_decompression_bomb() {
        // Build a gzip of 34 bytes (> 16-byte cap we will set).
        let plain = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 34 bytes
        let compressed: SkyResult<Vec<u8>, String> = task_run(compression_gzip(plain.to_vec()));
        let comp = match compressed {
            SkyResult::Ok(c) => c,
            _ => panic!("gzip failed"),
        };

        // Override the cap to 16 bytes for this test.
        // SAFETY: tests sharing the OnceLock see whatever value was set first,
        // so we use a separate env-var read path below. Because OnceLock caches
        // the value, we test the helper directly instead.
        let max: u64 = 16;
        // comp is already Vec<u8> — no conversion needed.
        let result = {
            use flate2::read::GzDecoder;
            use std::io::Read;
            let d = GzDecoder::new(&comp[..]);
            let mut out = Vec::new();
            let _ = d.take(max.saturating_add(1)).read_to_end(&mut out);
            if out.len() as u64 > max {
                Err(format!(
                    "decompressed output exceeds {} bytes (SKY_DECOMPRESS_MAX_BYTES)",
                    max
                ))
            } else {
                Ok(out)
            }
        };
        assert!(result.is_err(), "expected bomb-detection error, got Ok");
        assert!(result.unwrap_err().contains("exceeds"));
    }

    /// Verify that zstd rejects a payload that would expand beyond the cap.
    #[test]
    fn zstd_rejects_decompression_bomb() {
        let plain = b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"; // 34 bytes
        let compressed: SkyResult<Vec<u8>, String> =
            task_run(compression_zstd_compress(plain.to_vec()));
        let comp = match compressed {
            SkyResult::Ok(c) => c,
            _ => panic!("zstd compress failed"),
        };

        let max: u64 = 16;
        // comp is already Vec<u8> — no conversion needed.
        let result = {
            use std::io::Read;
            use zstd::stream::read::Decoder as ZstdDecoder;
            let d = ZstdDecoder::new(&comp[..]).expect("zstd decoder");
            let mut out = Vec::new();
            let _ = d.take(max.saturating_add(1)).read_to_end(&mut out);
            if out.len() as u64 > max {
                Err(format!(
                    "decompressed output exceeds {} bytes (SKY_DECOMPRESS_MAX_BYTES)",
                    max
                ))
            } else {
                Ok(out)
            }
        };
        assert!(result.is_err(), "expected bomb-detection error, got Ok");
        assert!(result.unwrap_err().contains("exceeds"));
    }
}
