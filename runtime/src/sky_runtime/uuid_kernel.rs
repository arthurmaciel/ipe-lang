//! Sky.Core.Uuid kernels — v4 / v7 / parse via the `uuid` crate.
//!
//! Module is `uuid_kernel` (not `uuid`) to avoid clashing with the `uuid`
//! crate; functions use the `::uuid::` extern path. NOTE: the `Uuid_v4`/`Uuid_v7`
//! kernels are dual-typed in the stdlib — `Sky.Core.Uuid.{v4,v7} : String` (the
//! shapes implemented here) vs `Sky.Core.Pure.uuidV{4,7}Kernel : Task Error
//! String`. A single Rust fn can't be both; the String surface is implemented,
//! the Pure Task surface is unsupported on target=rust (use the `uuid` crate via
//! auto-FFI, or Sky.Core.Uuid, instead).

use super::*;

/// Sky.Core.Uuid.v4 : String
pub fn uuid_v4() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// Sky.Core.Uuid.v7 : String  (time-ordered)
///
/// SECURITY: a v7 UUID embeds a millisecond timestamp and is SORTABLE/guessable
/// by design — it is NOT a secret. Use it for ordered ids, never as a bearer
/// token / session id / password-reset nonce (use `crypto_random_token` for
/// those). `v4` is random (getrandom/CSPRNG) but UUIDs are still only 122 bits of
/// formatted entropy — prefer `crypto_random_token` for security tokens.
/// (Audit 2026-06-19, low — documented contract.)
pub fn uuid_v7() -> String {
    ::uuid::Uuid::now_v7().to_string()
}

/// Sky.Core.Uuid.parse : String -> Maybe String  (canonicalise or Nothing)
pub fn uuid_parse(s: String) -> SkyMaybe<String> {
    match ::uuid::Uuid::parse_str(&s) {
        Ok(u) => SkyMaybe::Just(u.to_string()),
        Err(_) => SkyMaybe::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_shape_and_parse() {
        let id = uuid_v4();
        assert_eq!(id.len(), 36); // 8-4-4-4-12
        assert!(matches!(uuid_parse(id), SkyMaybe::Just(_)));
        assert!(matches!(
            uuid_parse("not-a-uuid".to_string()),
            SkyMaybe::Nothing
        ));
    }

    #[test]
    fn v7_is_valid() {
        assert!(matches!(uuid_parse(uuid_v7()), SkyMaybe::Just(_)));
    }
}
