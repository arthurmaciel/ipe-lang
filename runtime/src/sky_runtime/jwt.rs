//! JWT kernels for Sky.Core.Jwt — HS256 / RS256 encode + decode.
//!
//! ## Token byte-layout parity with the Go backend
//!
//! Encoding here reproduces, byte-for-byte, the token the Go backend's
//! `Sky.Core.Jwt.encode` produces for the same key + claims. The Go module
//! (sky-stdlib `Sky/Core/Jwt.sky`) builds the compact JWS in pure Sky on top of
//! `Json.Encode`, `Crypto`, and `Encoding`:
//!
//! * header  = `Json.Encode.encode 0 (object [("alg", …), ("typ", "JWT")])`
//! * payload = `Json.Encode.encode 0 <claims value>`
//! * sig(HS) = base64url( raw-bytes( hmacSha256 secret signingInput ) )
//! * sig(RS) = standardToUrl( rsaSha256Sign privKey signingInput )
//!
//! This file rebuilds the token through the SAME primitives already ported and
//! locked to Go byte-parity — `json_enc_encode` (the Go-formatted JSON encoder,
//! sorted object keys + Go float/HTML-escape shape) and `crypto_*` — rather than
//! `jsonwebtoken::encode`, whose header field order (`typ` before `alg`) and
//! claims serialization differ from Go and would yield a different signature.
//! See `tests/golden/m5b_jwt_hs256_bytes` / `m5b_jwt_rs256_bytes` for the
//! captured-Go-token byte-equality goldens, and
//! `crates/skyc/tests/golden_m5b_uuid_jwt.rs` for the byte-parity assertions.
//!
//! ## API-surface divergence from the Go backend (M5b interim)
//!
//! The Go backend exposes JWT through a builder API —
//! `Jwt.encode (Jwt.hs256 secret) (Jwt.claims |> Jwt.subject … |> …)` and
//! `Jwt.decode (Jwt.hs256 secret) now token`. The Rust backend currently
//! surfaces the four FLAT kernels below (`encodeHs256` / `decodeHs256` /
//! `encodeRs256` / `decodeRs256`) taking a claims JSON string directly. The
//! token BYTES are identical; the call surface is not, so a Go-targeted program
//! using the builder API does not yet compile on the Rust backend. This is a
//! recorded interim limitation — see `docs/architecture/divergence-policy.md`
//! ("Sky.Core.Jwt API surface").

use super::SkyResult;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// base64url, no padding (RFC 7515) — the encoding every JWS segment uses.
/// Equivalent to Go's `standardToUrl(base64Encode(bytes))` in `Jwt.sky`.
fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Convert standard base64 (with padding) to base64url with no padding.
/// Mirrors `Jwt.sky`'s `standardToUrl`: `+`→`-`, `/`→`_`, strip `=`.
fn standard_to_url(b64: &str) -> String {
    b64.replace('+', "-").replace('/', "_").replace('=', "")
}

/// Render the JWS header `{"alg":"<alg>","typ":"JWT"}` through the Go-parity JSON
/// encoder so the bytes (sorted keys → `alg` before `typ`) match the Go backend.
fn header_json(alg: &str) -> String {
    let value = super::json_enc_object(vec![
        ("alg".to_string(), super::json_enc_string(alg.to_string())),
        ("typ".to_string(), super::json_enc_string("JWT".to_string())),
    ]);
    super::json_enc_encode(0, value)
}

/// Re-encode a claims JSON string through the Go-parity JSON encoder so the
/// payload bytes (sorted object keys, Go float/HTML-escape shape) match what the
/// Go backend's `Json.Encode.encode 0 <claims>` emits. Returns the bad-claims
/// error message on a parse failure.
fn payload_json(claims_json: &str) -> Result<String, String> {
    let value: JsonValue =
        serde_json::from_str(claims_json).map_err(|e| format!("bad claims json: {}", e))?;
    Ok(super::json_enc_encode(0, value))
}

/// True when the token's payload carries `exp == 0`.
///
/// The decoders set `reject_tokens_expiring_in_less_than = 1` so jsonwebtoken's
/// reject condition becomes `exp - 1 < now` (≡ Go's `now >= exp`). It evaluates
/// `exp - 1` in `u64`, so an `exp` of `0` underflows — a panic in a debug build,
/// a wrap to `u64::MAX` (silently accepting an always-expired token) in release.
/// An `exp` of 0 is 1970-01-01, unconditionally in the past, so Go's `now >= exp`
/// rejects it; we detect that case here and short-circuit to the same rejection
/// before the subtraction can run. Reading the unverified payload is safe: the
/// only action taken is the conservative one (reject).
fn exp_is_zero(token: &str) -> bool {
    let mut parts = token.split('.');
    let payload = match (parts.next(), parts.next()) {
        (Some(_header), Some(payload)) => payload,
        _ => return false,
    };
    let bytes = match URL_SAFE_NO_PAD.decode(payload) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let value: JsonValue = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return false,
    };
    matches!(value.get("exp").and_then(JsonValue::as_u64), Some(0))
}

/// Sky `Jwt_encodeHs256 : String -> String -> Result Error String`
///
/// Byte-identical to the Go backend's `Jwt.encode (Jwt.hs256 secret) claims`.
pub fn jwt_encode_hs256<E: From<String>>(
    secret: String,
    claims_json: String,
) -> SkyResult<E, String> {
    // An HMAC key shorter than 32 bytes (256 bits) is below the RFC 7518 §3.2
    // floor for HS256 and yields a low-entropy / forgeable signing secret —
    // a 1-byte key mints a token anyone can re-sign. Reject it rather than emit
    // a weakly-keyed token. This mirrors the 32-byte floor auth.rs enforces and
    // Std.Auth applies upstream, closing the gap for a direct misconfigured
    // Jwt.* caller that bypasses Std.Auth.
    if secret.len() < 32 {
        return SkyResult::Err(
            "jwt-encode: HS256 secret must be at least 32 bytes (RFC 7518 §3.2)"
                .to_string()
                .into(),
        );
    }
    let payload = match payload_json(&claims_json) {
        Ok(p) => p,
        Err(e) => return SkyResult::Err(format!("jwt-encode: {}", e).into()),
    };
    let signing_input = format!(
        "{}.{}",
        b64u(header_json("HS256").as_bytes()),
        b64u(payload.as_bytes())
    );

    // Mirror Go's pipeline exactly: hmacSha256 returns lowercase hex, hexDecode
    // back to the raw MAC bytes, then base64url. `crypto_hmac_sha256` is the same
    // Go-parity primitive `Crypto.hmacSha256` lowers to.
    let mac_hex = super::crypto::crypto_hmac_sha256(secret, signing_input.clone());
    let mac_bytes = match hex::decode(&mac_hex) {
        Ok(b) => b,
        // Unreachable: crypto_hmac_sha256 always returns valid lowercase hex.
        // Route to Err rather than panic to keep the kernel total.
        Err(e) => return SkyResult::Err(format!("jwt-encode: internal hmac decode: {}", e).into()),
    };
    let sig = b64u(&mac_bytes);
    SkyResult::Ok(format!("{}.{}", signing_input, sig))
}

/// Sky `Jwt_decodeHs256 : String -> String -> Result Error String`
pub fn jwt_decode_hs256<E: From<String>>(secret: String, token: String) -> SkyResult<E, String> {
    // Reject verification under a sub-32-byte HMAC key — see jwt_encode_hs256.
    // A token "verified" with a low-entropy key carries no real authenticity
    // guarantee; mirror the 32-byte floor in auth.rs / Std.Auth (RFC 7518 §3.2).
    if secret.len() < 32 {
        return SkyResult::Err(
            "jwt-decode: HS256 secret must be at least 32 bytes (RFC 7518 §3.2)"
                .to_string()
                .into(),
        );
    }
    // Guard the u64 underflow that reject_tokens_expiring_in_less_than = 1
    // (set below) would hit on an `exp` of 0. See `exp_is_zero` — exp 0 is
    // always expired, so this matches Go's `now >= exp` rejection.
    if exp_is_zero(&token) {
        return SkyResult::Err("jwt-decode: token has expired".to_string().into());
    }
    let key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    // Go's oracle rejects at `now >= exp` with zero clock skew (validateTime →
    // pastClaim: `now >= ts`). jsonwebtoken's native boundary with leeway = 0 is
    // `exp < now` — it rejects only once `now` is STRICTLY past `exp`, so at the
    // exact instant `now == exp` it would ACCEPT a token Go rejects. Setting
    // reject_tokens_expiring_in_less_than = 1 shifts the reject condition to
    // `exp - 1 < now` (≡ `now >= exp`), restoring parity. leeway stays 0 so no
    // other skew is introduced; the exp == 0 underflow of `exp - 1` is guarded
    // above. nbf parity needs no shift: Go rejects at `now < nbf` (accept at
    // `now == nbf`), and jsonwebtoken with leeway 0 rejects at `nbf > now`
    // (accept at `now == nbf`) — already identical.
    validation.leeway = 0;
    validation.reject_tokens_expiring_in_less_than = 1;
    // exp/nbf are OPTIONAL per the JWT spec (RFC 7519 §4.1.4-5) and per Go's
    // behaviour: a token with no `exp` is treated as non-expiring and ACCEPTED;
    // a token with no `nbf` has no not-before constraint and is ACCEPTED.
    // jsonwebtoken's Validation::new() puts "exp" in required_spec_claims by
    // default, which would reject any no-exp token. Clear the set so exp/nbf
    // are not required — but keep validate_exp/validate_nbf = true so that
    // WHEN these claims are present they are still validated (expired → Err,
    // nbf in the future → Err). This matches Go exactly.
    validation.required_spec_claims = HashSet::new();
    validation.validate_exp = true;
    validation.validate_nbf = true;
    // These are GENERIC decoders with no expected-audience argument, so a specific
    // `aud` cannot be enforced here. jsonwebtoken's default `validate_aud = true`
    // would then REJECT any token that merely CARRIES an `aud` claim (error
    // InvalidAudience) — breaking the documented audience feature + Go parity.
    // Disable aud validation; audience-scoped checks belong to a future
    // expected-audience decoder variant.
    validation.validate_aud = false;
    match decode::<JsonValue>(&token, &key, &validation) {
        Ok(data) => match serde_json::to_string(&data.claims) {
            Ok(s) => SkyResult::Ok(s),
            Err(e) => SkyResult::Err(format!("jwt-decode: re-encode claims: {}", e).into()),
        },
        Err(e) => SkyResult::Err(format!("jwt-decode: {}", e).into()),
    }
}

/// Sky `Jwt_encodeRs256 : String -> String -> Result Error String`
///
/// Byte-identical to the Go backend's `Jwt.encode (Jwt.rs256 privKeyPem) claims`.
pub fn jwt_encode_rs256<E: From<String>>(
    key_pem: String,
    claims_json: String,
) -> SkyResult<E, String> {
    let payload = match payload_json(&claims_json) {
        Ok(p) => p,
        Err(e) => return SkyResult::Err(format!("jwt-encode-rs: {}", e).into()),
    };
    let signing_input = format!(
        "{}.{}",
        b64u(header_json("RS256").as_bytes()),
        b64u(payload.as_bytes())
    );

    // Mirror Go's `standardToUrl(rsaSha256Sign privKey signingInput)`.
    // `crypto_rsa_sha256_sign` is the same Go-parity primitive `Crypto.rsaSha256Sign`
    // lowers to and returns standard (padded) base64; convert to base64url.
    // Its Err message already suppresses key-structure detail (no key leak).
    let std_b64: String =
        match super::crypto::crypto_rsa_sha256_sign::<String>(key_pem, signing_input.clone()) {
            SkyResult::Ok(s) => s,
            // Keep the message generic so no structural hint about the key leaks.
            SkyResult::Err(_) => {
                return SkyResult::Err("jwt-encode-rs: invalid RSA key".to_string().into());
            }
        };
    let sig = standard_to_url(&std_b64);
    SkyResult::Ok(format!("{}.{}", signing_input, sig))
}

/// Sky `Jwt_decodeRs256 : String -> String -> Result Error String`
pub fn jwt_decode_rs256<E: From<String>>(key_pem: String, token: String) -> SkyResult<E, String> {
    // Guard the u64 underflow that reject_tokens_expiring_in_less_than = 1
    // (set below) would hit on an `exp` of 0. See `exp_is_zero` — exp 0 is
    // always expired, so this matches Go's `now >= exp` rejection.
    if exp_is_zero(&token) {
        return SkyResult::Err("jwt-decode-rs: token has expired".to_string().into());
    }
    let key = match DecodingKey::from_rsa_pem(key_pem.as_bytes()) {
        Ok(k) => k,
        // Suppress the parse-error detail to avoid leaking structural hints
        // about the key material (e.g. PEM framing, DER structure).
        Err(_) => return SkyResult::Err("jwt-decode-rs: invalid RSA key".to_string().into()),
    };
    let mut validation = Validation::new(Algorithm::RS256);
    // Go's oracle rejects at `now >= exp` with zero clock skew (validateTime →
    // pastClaim: `now >= ts`). jsonwebtoken's native boundary with leeway = 0 is
    // `exp < now` — it rejects only once `now` is STRICTLY past `exp`, so at the
    // exact instant `now == exp` it would ACCEPT a token Go rejects. Setting
    // reject_tokens_expiring_in_less_than = 1 shifts the reject condition to
    // `exp - 1 < now` (≡ `now >= exp`), restoring parity. leeway stays 0 so no
    // other skew is introduced; the exp == 0 underflow of `exp - 1` is guarded
    // above. nbf parity needs no shift: Go rejects at `now < nbf` (accept at
    // `now == nbf`), and jsonwebtoken with leeway 0 rejects at `nbf > now`
    // (accept at `now == nbf`) — already identical.
    validation.leeway = 0;
    validation.reject_tokens_expiring_in_less_than = 1;
    // exp/nbf are OPTIONAL per the JWT spec (RFC 7519 §4.1.4-5) and per Go's
    // behaviour: a token with no `exp` is treated as non-expiring and ACCEPTED;
    // a token with no `nbf` has no not-before constraint and is ACCEPTED.
    // jsonwebtoken's Validation::new() puts "exp" in required_spec_claims by
    // default, which would reject any no-exp token. Clear the set so exp/nbf
    // are not required — but keep validate_exp/validate_nbf = true so that
    // WHEN these claims are present they are still validated (expired → Err,
    // nbf in the future → Err). This matches Go exactly.
    validation.required_spec_claims = HashSet::new();
    validation.validate_exp = true;
    validation.validate_nbf = true;
    // These are GENERIC decoders with no expected-audience argument, so a specific
    // `aud` cannot be enforced here. jsonwebtoken's default `validate_aud = true`
    // would then REJECT any token that merely CARRIES an `aud` claim (error
    // InvalidAudience) — breaking the documented audience feature + Go parity.
    // Disable aud validation; audience-scoped checks belong to a future
    // expected-audience decoder variant.
    validation.validate_aud = false;
    match decode::<JsonValue>(&token, &key, &validation) {
        Ok(data) => match serde_json::to_string(&data.claims) {
            Ok(s) => SkyResult::Ok(s),
            Err(e) => SkyResult::Err(format!("jwt-decode-rs: re-encode: {}", e).into()),
        },
        Err(e) => SkyResult::Err(format!("jwt-decode-rs: {}", e).into()),
    }
}

// ── Concrete aliases for generated Rust code ─────────────────────────────────
//
// The generic `jwt_encode_hs256<E: From<String>>` and friends cannot be
// called directly from generated code where the `Err` arm may be discarded —
// Rust cannot infer `E` in that context. These monomorphic aliases pin
// `E = String` and are what `naming::kernel_name()` maps the Jwt kernels to,
// mirroring the `sky_aes_gcm_encrypt` pattern in `crypto.rs`.
//
// SECURITY: only the error message crosses the `From<String>` boundary; no
// key material is included in error text (see the suppressed RSA key detail
// above). These wrappers add no new logging surface.

/// Generated-code alias for `jwt_encode_hs256` with `E = String`.
pub fn sky_jwt_encode_hs256(secret: String, claims_json: String) -> SkyResult<String, String> {
    jwt_encode_hs256(secret, claims_json)
}

/// Generated-code alias for `jwt_decode_hs256` with `E = String`.
pub fn sky_jwt_decode_hs256(secret: String, token: String) -> SkyResult<String, String> {
    jwt_decode_hs256(secret, token)
}

/// Generated-code alias for `jwt_encode_rs256` with `E = String`.
pub fn sky_jwt_encode_rs256(key_pem: String, claims_json: String) -> SkyResult<String, String> {
    jwt_encode_rs256(key_pem, claims_json)
}

/// Generated-code alias for `jwt_decode_rs256` with `E = String`.
pub fn sky_jwt_decode_rs256(key_pem: String, token: String) -> SkyResult<String, String> {
    jwt_decode_rs256(key_pem, token)
}

// ── Builder API (D-00, #152) ─────────────────────────────────────────────────
//
// The Go backend exposes JWT through a builder pattern:
//   `Jwt.encode (Jwt.hs256 secret) (Jwt.claims |> Jwt.subject "alice" |> …)`
//
// Here we implement the same API surface.  `Claims` is represented as a
// `serde_json::Value` (the `IrType::Json` opaque).  `Algorithm` is encoded
// as a `String` with a prefix: `"HS256:<secret>"` or `"RS256:<pem>"` (the
// `IrType::Str` opaque).  The encode / decode functions parse the prefix to
// dispatch to the appropriate flat kernel, preserving byte-for-byte parity
// with the Go backend through the same Go-format JSON encoder used by the
// flat `encodeHs256` / `decodeHs256` kernels above.

/// `Jwt.claims : Claims` — returns an empty JSON object to start the builder
/// chain.  Backed as `serde_json::Value::Object` (IrType::Json).
pub fn sky_jwt_claims() -> JsonValue {
    JsonValue::Object(serde_json::Map::new())
}

/// `Jwt.hs256 : String -> Algorithm` — builds an HS256 algorithm descriptor.
/// The algorithm is encoded as `"HS256:<secret>"` so `sky_jwt_encode` /
/// `sky_jwt_decode` can parse out the algorithm and the key in one pass.
pub fn sky_jwt_hs256(secret: String) -> String {
    format!("HS256:{}", secret)
}

/// `Jwt.rs256 : String -> Algorithm` — builds an RS256 algorithm descriptor.
/// Encoded as `"RS256:<pem>"`.
pub fn sky_jwt_rs256(key_pem: String) -> String {
    format!("RS256:{}", key_pem)
}

/// Helper: insert (or overwrite) a key-value pair in a JSON object.
/// If `claims` is not an object, a new object is created containing only this
/// claim (defensive; the builder chain guarantees object inputs from `claims`).
fn claims_set(claims: JsonValue, key: &str, value: JsonValue) -> JsonValue {
    let mut map = match claims {
        JsonValue::Object(m) => m,
        other => {
            // Defensive: wrap the non-object in a fresh map.  This should
            // never happen in a well-typed builder chain.
            let mut m = serde_json::Map::new();
            m.insert("_value".to_string(), other);
            m
        }
    };
    map.insert(key.to_string(), value);
    JsonValue::Object(map)
}

/// `Jwt.subject : String -> Claims -> Claims` — sets the `sub` claim.
pub fn sky_jwt_subject(sub: String, claims: JsonValue) -> JsonValue {
    claims_set(claims, "sub", JsonValue::String(sub))
}

/// `Jwt.issuer : String -> Claims -> Claims` — sets the `iss` claim.
pub fn sky_jwt_issuer(iss: String, claims: JsonValue) -> JsonValue {
    claims_set(claims, "iss", JsonValue::String(iss))
}

/// `Jwt.audience : String -> Claims -> Claims` — sets the `aud` claim.
pub fn sky_jwt_audience(aud: String, claims: JsonValue) -> JsonValue {
    claims_set(claims, "aud", JsonValue::String(aud))
}

/// `Jwt.expiresAt : Int -> Claims -> Claims` — sets the `exp` claim (Unix
/// seconds).  The Sky stdlib documents `expiresAt` as accepting Unix
/// milliseconds but the JWT spec and the Go oracle use Unix SECONDS.  The
/// Sky stdlib's `Jwt.sky` passes the value straight through as a JSON number,
/// so we mirror that — the caller is responsible for providing the right unit.
pub fn sky_jwt_expires_at(exp: i64, claims: JsonValue) -> JsonValue {
    claims_set(claims, "exp", JsonValue::Number(serde_json::Number::from(exp)))
}

/// `Jwt.notBefore : Int -> Claims -> Claims` — sets the `nbf` claim.
pub fn sky_jwt_not_before(nbf: i64, claims: JsonValue) -> JsonValue {
    claims_set(claims, "nbf", JsonValue::Number(serde_json::Number::from(nbf)))
}

/// `Jwt.issuedAt : Int -> Claims -> Claims` — sets the `iat` claim.
pub fn sky_jwt_issued_at(iat: i64, claims: JsonValue) -> JsonValue {
    claims_set(claims, "iat", JsonValue::Number(serde_json::Number::from(iat)))
}

/// `Jwt.jwtId : String -> Claims -> Claims` — sets the `jti` claim.
pub fn sky_jwt_jwt_id(jti: String, claims: JsonValue) -> JsonValue {
    claims_set(claims, "jti", JsonValue::String(jti))
}

/// `Jwt.withClaim : String -> String -> Claims -> Claims` — inserts an
/// arbitrary string-valued claim.  Both key and value are strings; the value
/// is stored as a JSON string.
pub fn sky_jwt_with_claim(key: String, value: String, claims: JsonValue) -> JsonValue {
    claims_set(claims, &key, JsonValue::String(value))
}

/// `Jwt.encode : Algorithm -> Claims -> Result Error String` — signs the claims
/// using the algorithm encoded in `algorithm_descriptor`.
/// Delegates to `jwt_encode_hs256` / `jwt_encode_rs256` after serialising the
/// claims through the Go-parity JSON encoder (sorted keys).
pub fn sky_jwt_encode(
    algorithm_descriptor: String,
    claims: JsonValue,
) -> SkyResult<String, String> {
    // Serialise the claims through the Go-parity encoder so the payload bytes
    // match those produced by `Jwt.encode` in the Go backend.
    let claims_json = super::json_enc_encode(0, claims);
    if let Some(secret) = algorithm_descriptor.strip_prefix("HS256:") {
        jwt_encode_hs256(secret.to_string(), claims_json)
    } else if let Some(pem) = algorithm_descriptor.strip_prefix("RS256:") {
        jwt_encode_rs256(pem.to_string(), claims_json)
    } else {
        SkyResult::Err(format!(
            "jwt-encode: unknown algorithm descriptor (expected HS256:… or RS256:…): {}",
            &algorithm_descriptor[..algorithm_descriptor.len().min(20)]
        )
        .into())
    }
}

/// `Jwt.decode : Algorithm -> Int -> String -> Result Error String`
///
/// Verifies the JWT signature and applies caller-supplied `now` (Unix seconds)
/// for exp/nbf validation, matching the reference `Sky.Core.Jwt.decode` contract:
///   pastClaim:   now >= exp  → Err "Jwt.decode: token has expired"
///   futureClaim: now <  nbf  → Err "Jwt.decode: token is not yet valid"
///   absent claim              → accept (optional)
/// Returns the raw payload JSON string (base64url-decoded middle segment).
/// No wall-clock access; deterministic on `now`.
pub fn sky_jwt_decode(
    algorithm_descriptor: String,
    now: i64,
    token: String,
) -> SkyResult<String, String> {
    // 1. Split token into three segments.
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() != 3 {
        return SkyResult::Err("jwt-decode: malformed token (expected 3 segments)".into());
    }

    // 2. Verify the signature only — disable jsonwebtoken's built-in time checks.
    //    We apply reference-exact time validation manually below.
    if let Some(secret) = algorithm_descriptor.strip_prefix("HS256:") {
        if secret.len() < 32 {
            return SkyResult::Err(
                "jwt-decode: HS256 secret must be at least 32 bytes (RFC 7518 §3.2)".into(),
            );
        }
        let key = DecodingKey::from_secret(secret.as_bytes());
        let mut val = Validation::new(Algorithm::HS256);
        val.validate_exp = false;
        val.validate_nbf = false;
        val.required_spec_claims = HashSet::new();
        val.validate_aud = false;
        if decode::<JsonValue>(&token, &key, &val).is_err() {
            return SkyResult::Err("jwt-decode: invalid signature".into());
        }
    } else if let Some(pem) = algorithm_descriptor.strip_prefix("RS256:") {
        let key = match DecodingKey::from_rsa_pem(pem.as_bytes()) {
            Ok(k) => k,
            Err(_) => return SkyResult::Err("jwt-decode: invalid RS256 public key".into()),
        };
        let mut val = Validation::new(Algorithm::RS256);
        val.validate_exp = false;
        val.validate_nbf = false;
        val.required_spec_claims = HashSet::new();
        val.validate_aud = false;
        if decode::<JsonValue>(&token, &key, &val).is_err() {
            return SkyResult::Err("jwt-decode: invalid signature".into());
        }
    } else {
        return SkyResult::Err(format!(
            "jwt-decode: unknown algorithm descriptor (expected HS256:… or RS256:…): {}",
            &algorithm_descriptor[..algorithm_descriptor.len().min(20)]
        )
        .into());
    }

    // 3. Extract payload JSON via base64url-decode (parse, don't validate).
    let payload_bytes = match URL_SAFE_NO_PAD.decode(parts[1].as_bytes()) {
        Ok(b) => b,
        Err(_) => return SkyResult::Err("jwt-decode: payload base64url decode failed".into()),
    };
    let payload_json = match String::from_utf8(payload_bytes) {
        Ok(s) => s,
        Err(_) => return SkyResult::Err("jwt-decode: payload is not valid UTF-8".into()),
    };

    // 4. Manual time validation matching reference semantics exactly.
    let claims_val: JsonValue =
        serde_json::from_str(&payload_json).unwrap_or(JsonValue::Null);

    if let Some(exp) = claims_val.get("exp").and_then(|v| v.as_i64()) {
        // pastClaim: now >= exp  → expired
        if now >= exp {
            return SkyResult::Err("Jwt.decode: token has expired".into());
        }
    }
    if let Some(nbf) = claims_val.get("nbf").and_then(|v| v.as_i64()) {
        // futureClaim: now < nbf  → not yet valid
        if now < nbf {
            return SkyResult::Err("Jwt.decode: token is not yet valid".into());
        }
    }

    SkyResult::Ok(payload_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The genuine Go-backend token for the equivalent builder-API program
    /// `Jwt.encode (Jwt.hs256 secret) (claims |> subject "alice" |> expiresAt …)`
    /// with `secret = "test-secret-key-0123456789abcdef"`. Captured from the Go
    /// reference compiler. The flat `encodeHs256` kernel must reproduce it byte
    /// for byte.
    const GO_HS256_TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTksInN1YiI6ImFsaWNlIn0.O6u4Zgjn9lL3myvfLfP5QFaGIHx-KBfzZ7lgkbJL_N0";

    #[test]
    fn hs256_token_is_byte_identical_to_go() {
        let secret = "test-secret-key-0123456789abcdef".to_string();
        let claims = r#"{"sub":"alice","exp":9999999999}"#.to_string();
        let token: SkyResult<String, String> = jwt_encode_hs256(secret, claims);
        match token {
            SkyResult::Ok(t) => assert_eq!(
                t, GO_HS256_TOKEN,
                "HS256 token must match the Go backend byte-for-byte"
            ),
            SkyResult::Err(e) => panic!("encode: {}", e),
        }
    }

    // RSA-2048 PKCS#8 test key pair (test-only; never used outside tests) — the
    // same key embedded in tests/golden/m5b_jwt_rs256_roundtrip/Main.sky.
    const RS256_PRIV_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDKt5KX9AzdrNIl\nKj9WqLI1vI1E+s6ydOuFtJaX+3eyvByRf++qKeSca9WQhFWih6INNSPyHiI1w570\njeglmwcQe8WXyW15w/c3a/TkYJ6thqyFOBTColjuOv+nUkIyHkOFx4GTtTwjcIQ1\no+sgDy90NrIyjvZhKGxv/BoRsvmcPNcO95MC2lTZrhjIwPpTCDn/jD1DhNmRtcsv\ng0WmoXKGfFm+YYALT49Cs2hU2z8kQrlqo5q8Zzsxa3+wh4yF7W/O5aAoUBKtcYNS\nOvumvh7aBnuHZT45ZvYGCNdTPFX+4E27JWyAycp+4GqfvcbcfcDwjhpkphAhYozi\nZ9zgc+4FAgMBAAECggEAVoIbcXQpD2qCbXDHgdRQ5MS3prG/hoGFxtPHlkkujhxf\ntqnZnYzuLeCIzXjj0I3AFpHQarD4WWhHS8bJRE8RpzOioYFIkjeSJtkPs2wWGyhH\nNDy4A01j1RphYkak0B2BJDR89AtaBCeui/ONUeuZDSeQSSogM1scV3fGqjnt8oFz\nlhaOell+Z/csmbLW+YhJEpUuKmA/V4ehXKn6TvTWCvfYOupVauZeUcwAzMXCtWIx\n6CvEoe5We/0MafIeqwnUSeoAVXvmhx5QtTC2x3rYlSvr3RtGPFTH4wD+cBavFOW8\nzD1u6SC5mDQi0L5Z1tSdV8g8cF2sSyTf3peNXI2DKwKBgQDbfFt9Vg3gwBFJNSRk\nOhi59+KYisxFSEcOj9X+MdrCys8Zm/4XWtpU06rvuM3e8C1+ubK978BdLamZrnQO\n/w2XOzD3WNbX/eGSQci7BGFQBUBP98ABDWctjTHu7Ph3BZZqaj+b4ZWEBNsvrkkw\nfqNE3m5dx+JtbSAgThD1eq5elwKBgQDscQ5qZoXmlUChTpJzqvHm5eEiV+tOS/o8\noxaH2ygPqwAJkWtiFmXLIWUW+dx0hwYEocbKUkBx9HBS0yMDe2aJV5sZwo0fzUV2\nHCwxJ2cVB28bQ1mVETTuSE8Ok/Cb/zxHjlVx4NMDUEmf+KTaWQ2JXysI4yv4Vi2u\npkt0DIpHwwKBgQCABuYHEi8+Lkrm/QyhOhI6SBHxEOVedG6eW+BjSgllHo/3TDrG\nvMQmPuGyu4W6yTaAeSl+CV+X+o63ij9AkB4JXQmO/k8z5m+xtJW2ITPyTV3aR5XE\nB2Fr/LRnveqg4q1+nUNFViy0uXBxO6SNmRD7lxOhuHqngcP/lAnoZwtXOQKBgAuF\n7wfsezYjrASwiZ6thCCWr4Q2+LbWKRnvcNeqLKem09ejiLI9GTTvKbgW8VGUiwyK\nvd96Zr2nBhpjQ9+Vkge7h0mYG7yjCnGZKeYzX2i89gNEIweK0SOTzpaNSzqvE8cA\n/tUP+fi9Xvk26wHhOTGqu7QxLiFqQcuzOxYqzkp1AoGADuFoA0w4zwXsr/6sF1Vx\ne20UCYmRkiiE6CbgicFsMYhaD5w2F5Ss26Zb8f09oaAZw2xwDCNY7LX2OQSTgSuX\nzzBBQrEfmqxPLztxMa0e3qjSBMeo3m0m2Yoen67ie5b53snOT3t704JrE6kP6DxC\nkxJKRSX7IM4caBhDN+Khm2k=\n-----END PRIVATE KEY-----\n";

    /// SPKI public key matching `RS256_PRIV_PEM` — the RS256 decode path verifies
    /// with a public PEM (same key embedded in
    /// tests/golden/m5b_jwt_rs256_roundtrip/Main.sky).
    const RS256_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAyreSl/QM3azSJSo/Vqiy\nNbyNRPrOsnTrhbSWl/t3srwckX/vqinknGvVkIRVooeiDTUj8h4iNcOe9I3oJZsH\nEHvFl8ltecP3N2v05GCerYashTgUwqJY7jr/p1JCMh5DhceBk7U8I3CENaPrIA8v\ndDayMo72YShsb/waEbL5nDzXDveTAtpU2a4YyMD6Uwg5/4w9Q4TZkbXLL4NFpqFy\nhnxZvmGAC0+PQrNoVNs/JEK5aqOavGc7MWt/sIeMhe1vzuWgKFASrXGDUjr7pr4e\n2gZ7h2U+OWb2BgjXUzxV/uBNuyVsgMnKfuBqn73G3H3A8I4aZKYQIWKM4mfc4HPu\nBQIDAQAB\n-----END PUBLIC KEY-----\n";

    /// The genuine Go-backend token for the equivalent builder-API program
    /// `Jwt.encode (Jwt.rs256 privKey) (claims |> subject "bob" |> expiresAt …)`
    /// with the key above. Captured from the Go reference compiler. RS256
    /// (PKCS#1 v1.5) is deterministic, so the flat `encodeRs256` kernel must
    /// reproduce it byte for byte.
    const GO_RS256_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTksInN1YiI6ImJvYiJ9.GJ29fLyt4u8M_CMSvhSizRpjWXEDsrVtDL92QOX27HwB9YvKI4_ksftEN8-wK1xiT5y1tmrWmUs3_UHPTepyCJ9Y02JDphZ5X4k0784CIKxNvdr1RcAn-V24Wyc_rTFOELDR9XeBPNIhYRzVuQnaQ27PbmpF3skoyH40eOI7emrTVlbPhkgnWsoULuKOEI3yF9VU62QFoPDEuio_59LMcuk2EZrnh-Rql1zF5cNixt30_Vu5mUwBHkYZ2J2ZEm_S2VIrXvIluIfp5pzNmOK1TdLv9yQHY1PPcfcvHizHK4IKnMNTXrkk8W0NCaP5faf4hzaZVPIoqJ7D220PHPgWEg";

    #[test]
    fn rs256_token_is_byte_identical_to_go() {
        let claims = r#"{"sub":"bob","exp":9999999999}"#.to_string();
        let token: SkyResult<String, String> = jwt_encode_rs256(RS256_PRIV_PEM.to_string(), claims);
        match token {
            SkyResult::Ok(t) => assert_eq!(
                t, GO_RS256_TOKEN,
                "RS256 token must match the Go backend byte-for-byte"
            ),
            SkyResult::Err(e) => panic!("encode-rs: {}", e),
        }
    }

    #[test]
    fn test_hs256_roundtrip() {
        // >= 32 bytes to clear the RFC 7518 §3.2 HS256 secret floor.
        let secret = "roundtrip-secret-0123456789abcdef".to_string();
        let claims = r#"{"sub":"alice","exp":9999999999}"#.to_string();
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims.clone());
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        let decoded = match decoded {
            SkyResult::Ok(s) => s,
            SkyResult::Err(e) => panic!("decode: {}", e),
        };
        assert!(decoded.contains("alice"));
    }

    #[test]
    fn test_hs256_wrong_secret_fails() {
        // Both secrets >= 32 bytes (RFC 7518 §3.2 floor); they differ so verify fails.
        let token: SkyResult<String, String> = jwt_encode_hs256(
            "right-secret-0123456789abcdef0123".to_string(),
            r#"{"sub":"x","exp":9999999999}"#.to_string(),
        );
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let bad: SkyResult<String, String> =
            jwt_decode_hs256("wrong-secret-0123456789abcdef0123".to_string(), token);
        assert!(matches!(bad, SkyResult::Err(_)));
    }

    /// Seconds since the Unix epoch, for building boundary-exercising claims.
    fn now_unix() -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(_) => 0,
        }
    }

    #[test]
    fn test_hs256_expired_token_rejected() {
        // exp 30s in the PAST. With jsonwebtoken's default 60s leeway this would
        // be ACCEPTED — the strict Go oracle (zero clock skew) rejects it. Guards
        // `validation.leeway = 0` against silent regression; every other golden
        // uses a far-future exp and never crosses the boundary.
        let secret = "expiry-secret-0123456789abcdef0123".to_string();
        let claims = format!(r#"{{"sub":"x","exp":{}}}"#, now_unix() - 30);
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        assert!(
            matches!(decoded, SkyResult::Err(_)),
            "an HS256 token expired 30s ago must be rejected (no clock-skew leeway)"
        );
    }

    #[test]
    fn test_rs256_expired_token_rejected() {
        // exp 30s in the PAST — RS256 counterpart of the HS256 leeway guard.
        let claims = format!(r#"{{"sub":"bob","exp":{}}}"#, now_unix() - 30);
        let token: SkyResult<String, String> = jwt_encode_rs256(RS256_PRIV_PEM.to_string(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode-rs: {}", e),
        };
        // Verify with the matching SPKI public key (the decode path takes a
        // public PEM). A successful signature check then trips the expiry guard.
        let decoded: SkyResult<String, String> = jwt_decode_rs256(RS256_PUB_PEM.to_string(), token);
        assert!(
            matches!(decoded, SkyResult::Err(_)),
            "an RS256 token expired 30s ago must be rejected (no clock-skew leeway)"
        );
    }

    #[test]
    fn test_hs256_empty_secret_rejected() {
        // Empty HMAC secret → forgeable token; both encode and verify must refuse.
        let enc: SkyResult<String, String> =
            jwt_encode_hs256(String::new(), r#"{"sub":"x","exp":9999999999}"#.to_string());
        assert!(matches!(enc, SkyResult::Err(_)));
        let dec: SkyResult<String, String> = jwt_decode_hs256(String::new(), "a.b.c".to_string());
        assert!(matches!(dec, SkyResult::Err(_)));
    }

    /// Parity edge: `now == exp` must REJECT, matching Go's `now >= exp`
    /// (validateTime → pastClaim). jsonwebtoken's native leeway-0 boundary is
    /// `exp < now` (accepts at `now == exp`); reject_tokens_expiring_in_less_than
    /// = 1 shifts it to `now >= exp`. This test pins the exact second-boundary —
    /// far-from-boundary goldens never cross it. Robust against the live clock:
    /// at decode jsonwebtoken's clock is `>= exp`, so the reject is stable.
    #[test]
    fn test_hs256_now_eq_exp_rejected() {
        let secret = "exp-edge-secret-0123456789abcdef0".to_string();
        let exp = now_unix(); // now == exp
        let claims = format!(r#"{{"sub":"x","exp":{}}}"#, exp);
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        assert!(
            matches!(decoded, SkyResult::Err(_)),
            "now == exp must be rejected to match Go's `now >= exp`"
        );
    }

    /// Parity edge: `now == exp - 1` (one second before expiry) must ACCEPT.
    /// Go rejects only at `now >= exp`, so one second earlier is still valid.
    /// We anchor `exp = now + 1`. A second-boundary tick between building the
    /// claim and jsonwebtoken's internal clock read would push `now` up to `exp`
    /// and flip the result, so retry on that rare straddle — each attempt is
    /// deterministic, the bounded loop only guards the cross-second race (5
    /// consecutive straddles is astronomically unlikely and keeps the test
    /// finite per the timeout principle).
    #[test]
    fn test_hs256_now_eq_exp_minus_1_accepted() {
        let secret = "exp-edge-secret-0123456789abcdef0".to_string();
        let mut accepted = false;
        for _ in 0..5 {
            let exp = now_unix() + 1; // now == exp - 1
            let claims = format!(r#"{{"sub":"x","exp":{}}}"#, exp);
            let token = match jwt_encode_hs256::<String>(secret.clone(), claims) {
                SkyResult::Ok(t) => t,
                SkyResult::Err(e) => panic!("encode: {}", e),
            };
            if matches!(
                jwt_decode_hs256::<String>(secret.clone(), token),
                SkyResult::Ok(_)
            ) {
                accepted = true;
                break;
            }
        }
        assert!(
            accepted,
            "now == exp - 1 must be accepted (one second before expiry is valid)"
        );
    }

    /// Parity edge: `now == nbf` must ACCEPT, matching Go's futureClaim
    /// (`now < nbf` rejects → `now >= nbf` accepts). Robust against the live
    /// clock: jsonwebtoken's clock at decode is `>= nbf`, so the accept is
    /// stable. No `exp` claim → non-expiring, isolating the nbf boundary.
    #[test]
    fn test_hs256_now_eq_nbf_accepted() {
        let secret = "nbf-edge-secret-0123456789abcdef0".to_string();
        let nbf = now_unix(); // now == nbf
        let claims = format!(r#"{{"sub":"x","nbf":{}}}"#, nbf);
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        assert!(
            matches!(decoded, SkyResult::Ok(_)),
            "now == nbf must be accepted to match Go's `now >= nbf`"
        );
    }

    /// Golden (c): token with NO `exp` claim must be ACCEPTED — matching Go's
    /// behaviour where an absent `exp` means "non-expiring". Guarding against
    /// a regression where `required_spec_claims` includes "exp" (the
    /// jsonwebtoken default), which would reject valid non-expiring tokens.
    #[test]
    fn test_hs256_no_exp_accepted() {
        let secret = "no-exp-secret-0123456789abcdef0123".to_string();
        // claims with only `sub` — no `exp`, no `nbf`.
        let claims = r#"{"sub":"alice"}"#.to_string();
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        assert!(
            matches!(decoded, SkyResult::Ok(_)),
            "an HS256 token with no exp claim must be accepted (non-expiring, matching Go)"
        );
        if let SkyResult::Ok(s) = decoded {
            assert!(s.contains("alice"), "decoded claims must include sub:alice");
        }
    }

    /// Golden (d): token with `nbf` in the FUTURE must be REJECTED — matching
    /// Go's behaviour where `nbf` is validated when present with no clock slack.
    /// Guards `validation.validate_nbf = true` + `leeway = 0`.
    #[test]
    fn test_hs256_future_nbf_rejected() {
        let secret = "nbf-secret-0123456789abcdef0123456".to_string();
        // nbf 300s in the future, no exp (non-expiring).
        let claims = format!(r#"{{"sub":"x","nbf":{}}}"#, now_unix() + 300);
        let token: SkyResult<String, String> = jwt_encode_hs256(secret.clone(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_hs256(secret, token);
        assert!(
            matches!(decoded, SkyResult::Err(_)),
            "an HS256 token with nbf 300s in the future must be rejected (no leeway, matching Go)"
        );
    }

    /// Golden (c-RS): RS256 counterpart — no-exp token must be ACCEPTED.
    #[test]
    fn test_rs256_no_exp_accepted() {
        let claims = r#"{"sub":"bob"}"#.to_string();
        let token: SkyResult<String, String> = jwt_encode_rs256(RS256_PRIV_PEM.to_string(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode-rs: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_rs256(RS256_PUB_PEM.to_string(), token);
        assert!(
            matches!(decoded, SkyResult::Ok(_)),
            "an RS256 token with no exp claim must be accepted (non-expiring, matching Go)"
        );
        if let SkyResult::Ok(s) = decoded {
            assert!(s.contains("bob"), "decoded claims must include sub:bob");
        }
    }

    /// Golden (d-RS): RS256 counterpart — future-nbf token must be REJECTED.
    #[test]
    fn test_rs256_future_nbf_rejected() {
        let claims = format!(r#"{{"sub":"bob","nbf":{}}}"#, now_unix() + 300);
        let token: SkyResult<String, String> = jwt_encode_rs256(RS256_PRIV_PEM.to_string(), claims);
        let token = match token {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode-rs: {}", e),
        };
        let decoded: SkyResult<String, String> = jwt_decode_rs256(RS256_PUB_PEM.to_string(), token);
        assert!(
            matches!(decoded, SkyResult::Err(_)),
            "an RS256 token with nbf 300s in the future must be rejected (no leeway, matching Go)"
        );
    }

    // ── sky_jwt_decode (3-arg builder-API) unit tests ─────────────────────────

    fn make_token_with_time(exp: Option<i64>, nbf: Option<i64>) -> String {
        let secret = "test-secret-key-0123456789abcdef".to_string();
        let mut parts = vec![r#""sub":"x""#.to_string()];
        if let Some(e) = exp {
            parts.push(format!(r#""exp":{}"#, e));
        }
        if let Some(n) = nbf {
            parts.push(format!(r#""nbf":{}"#, n));
        }
        let claims = format!("{{{}}}", parts.join(","));
        match jwt_encode_hs256::<String>(secret, claims) {
            SkyResult::Ok(t) => t,
            SkyResult::Err(e) => panic!("encode: {}", e),
        }
    }

    /// now=500, exp=1000, nbf=100 → Ok (valid window).
    #[test]
    fn sky_jwt_decode_valid_window() {
        let tok = make_token_with_time(Some(1000), Some(100));
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 500, tok), SkyResult::Ok(_)),
            "now=500 inside [nbf=100, exp=1000) must succeed"
        );
    }

    /// now=1000 >= exp=1000 → Err (expired; boundary: >= not >).
    #[test]
    fn sky_jwt_decode_expired_at_boundary() {
        let tok = make_token_with_time(Some(1000), None);
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 1000, tok), SkyResult::Err(_)),
            "now==exp must be rejected (now >= exp semantics)"
        );
    }

    /// now=99 < nbf=100 → Err (not yet valid).
    #[test]
    fn sky_jwt_decode_nbf_future() {
        let tok = make_token_with_time(None, Some(100));
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 99, tok), SkyResult::Err(_)),
            "now=99 < nbf=100 must be rejected"
        );
    }

    /// now=100 == nbf=100 → Ok (boundary: now < nbf is false → accept).
    #[test]
    fn sky_jwt_decode_nbf_at_boundary() {
        let tok = make_token_with_time(Some(1000), Some(100));
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 100, tok), SkyResult::Ok(_)),
            "now==nbf must be accepted (now < nbf is false)"
        );
    }

    /// Token with no exp/nbf → Ok for any now.
    #[test]
    fn sky_jwt_decode_no_time_claims() {
        let tok = make_token_with_time(None, None);
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 9999999999, tok), SkyResult::Ok(_)),
            "token without exp/nbf must be accepted regardless of now"
        );
    }

    /// Wrong key → Err (invalid signature, constant-time path).
    #[test]
    fn sky_jwt_decode_wrong_key() {
        let tok = make_token_with_time(Some(9999999999), None);
        let desc = "HS256:wrong-secret-key-0123456789abcde".to_string();
        assert!(
            matches!(sky_jwt_decode(desc, 500, tok), SkyResult::Err(_)),
            "wrong key must be rejected"
        );
    }

    /// Return value is the payload JSON string (verified base64url-decode).
    #[test]
    fn sky_jwt_decode_returns_payload_json() {
        let tok = make_token_with_time(Some(9999999999), None);
        let desc = "HS256:test-secret-key-0123456789abcdef".to_string();
        match sky_jwt_decode(desc, 500, tok) {
            SkyResult::Ok(payload) => {
                assert!(
                    payload.contains("\"sub\"") || payload.contains("\"exp\""),
                    "payload must be the decoded claims JSON: {payload}"
                );
            }
            SkyResult::Err(e) => panic!("unexpected err: {e}"),
        }
    }
}
