// Crypto kernel stubs — generic over E where needed.
use super::*;

// `Crypto.randomBytes : Int -> Task Error String`. Go returns the entropy as a
// LOWERCASE HEX string (rt.go ~l6543: `hex.EncodeToString(b)`), NOT a byte list —
// the Sky signature is `String`, so the Rust side must return a hex `String` too.
// (A prior `Vec<i64>` return diverged from both the Sky type and Go: a Sky call
// site treating the result as a String/Bytes mismatched at codegen.)
pub fn crypto_random_bytes<E: From<String> + Send + 'static>(n: i64) -> SkyTask<E, String> {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    Box::pin(async move {
        // SECURITY: Mirror Go oracle exactly: reject size <= 0 || size > 1024
        // (rt.go ~l6536: `if size <= 0 || size > 1024 { return ErrInvalidInput }`)
        // to prevent unbounded attacker-controlled allocation (DoS vector).
        if n <= 0 || n > 1024 {
            return SkyResult::Err(
                "Crypto.randomBytes: size must be 1..1024"
                    .to_string()
                    .into(),
            );
        }
        let count = n as usize;
        let mut buf = vec![0u8; count];
        OsRng.fill_bytes(&mut buf);
        ok_res(hex_lower(&buf))
    })
}

/// Lowercase hex encoding, byte-order + nibble-order identical to Go's
/// `hex.EncodeToString` (high nibble first, then low). Total: the `& 0x0f` index
/// is always < 16 so `.get` never falls back.
fn hex_lower(buf: &[u8]) -> String {
    let hex = b"0123456789abcdef";
    let mut out = String::with_capacity(buf.len() * 2);
    for &b in buf {
        out.push(hex.get(((b >> 4) & 0x0f) as usize).copied().unwrap_or(b'0') as char);
        out.push(hex.get((b & 0x0f) as usize).copied().unwrap_or(b'0') as char);
    }
    out
}

// `Crypto.randomToken : Int -> Task Error String`. Go returns URL-safe base64
// WITHOUT padding (rt.go ~l6560: `base64.RawURLEncoding.EncodeToString(b)`) — the
// `-_` alphabet, no `=` pad. Width `n` is bytes of ENTROPY; the returned string is
// longer (ceil(n*4/3) chars). (A prior hex encoding diverged from Go's base64.)
pub fn crypto_random_token<E: From<String> + Send + 'static>(n: i64) -> SkyTask<E, String> {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    Box::pin(async move {
        // SECURITY: Mirror Go oracle exactly: reject size <= 0 || size > 1024
        // (rt.go ~l6553: `if size <= 0 || size > 1024 { return ErrInvalidInput }`)
        // to prevent unbounded attacker-controlled allocation (DoS vector).
        if n <= 0 || n > 1024 {
            return SkyResult::Err(
                "Crypto.randomToken: size must be 1..1024"
                    .to_string()
                    .into(),
            );
        }
        let count = n as usize;
        let mut buf = vec![0u8; count];
        OsRng.fill_bytes(&mut buf);
        ok_res(URL_SAFE_NO_PAD.encode(&buf))
    })
}

pub fn crypto_sha256(s: String) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let result = h.finalize();
    result
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
}

/// Sky `sha512 : String -> String` — hex-encoded SHA-512 digest.
pub fn crypto_sha512(s: String) -> String {
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(s.as_bytes());
    let result = h.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// SECURITY (parity-locked): `sha1` and `md5` below are COLLISION-BROKEN and are
/// exposed ONLY as named checksum/interop hashes, matching the Go backend's
/// surface. They MUST NOT be used as a security primitive (password hashing,
/// signatures, integrity against an adversary) — those paths use SHA-256/512 +
/// HMAC + bcrypt/PBKDF2 elsewhere in this module. Removing them would break Go
/// parity; the hardening is this contract note. (Audit 2026-06-19, low/weak-crypto.)
///
/// Sky `sha1 : String -> String` — hex-encoded SHA-1 digest.
pub fn crypto_sha1(s: String) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Sky `md5 : String -> String` — hex-encoded MD5 digest.
pub fn crypto_md5(s: String) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Sky `hmacSha256 : String -> String -> String` (key, message → hex tag).
pub fn crypto_hmac_sha256(key: String, msg: String) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    // INFALLIBLE: HMAC accepts any key length (new_from_slice never returns Err);
    // the kernel is a pure `String -> String -> String` Sky surface with no Result
    // channel, and a fallback MAC would be a silently-wrong hash (a security defect).
    // SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — infallible HMAC ctor; pure kernel has no Result channel; a fallback MAC is a security defect [ledger #1]
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha256::new_from_slice(key.as_bytes()).expect("Hmac<Sha256> accepts any key length");
    mac.update(msg.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Sky `hmacSha512 : String -> String -> String`.
pub fn crypto_hmac_sha512(key: String, msg: String) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;
    type HmacSha512 = Hmac<Sha512>;
    // INFALLIBLE: HMAC accepts any key length (new_from_slice never returns Err);
    // the kernel is a pure `String -> String -> String` Sky surface with no Result
    // channel, and a fallback MAC would be a silently-wrong hash (a security defect).
    // SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — infallible HMAC ctor; pure kernel has no Result channel; a fallback MAC is a security defect [ledger #1]
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha512::new_from_slice(key.as_bytes()).expect("Hmac<Sha512> accepts any key length");
    mac.update(msg.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Sky `rsaSha256Sign : String -> String -> Result Error String`
/// Sign `msg` with the PKCS#1 v1.5 SHA-256 RSA scheme using `key_pem`.
/// Accepts PKCS#1 (`-----BEGIN RSA PRIVATE KEY-----`) and PKCS#8
/// (`-----BEGIN PRIVATE KEY-----`) PEM private keys — mirrors Go oracle
/// (rt.go ~l6472: tries ParsePKCS1PrivateKey then ParsePKCS8PrivateKey).
/// Returns standard-base64-encoded signature (base64.StdEncoding, rt.go ~l6488).
#[cfg(feature = "crypto")]
pub fn crypto_rsa_sha256_sign<E: From<String>>(
    key_pem: String,
    msg: String,
) -> SkyResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rsa::{
        pkcs1::DecodeRsaPrivateKey,
        pkcs1v15::SigningKey,
        pkcs8::DecodePrivateKey,
        signature::{SignatureEncoding, Signer},
    };
    use sha2::Sha256;

    // Try PKCS#8 first (the openssl default), then fall back to PKCS#1 — mirrors Go.
    let priv_key = if let Ok(k) = rsa::RsaPrivateKey::from_pkcs8_pem(&key_pem) {
        k
    } else if let Ok(k) = rsa::RsaPrivateKey::from_pkcs1_pem(&key_pem) {
        k
    } else {
        return SkyResult::Err(
            "Crypto.rsaSha256Sign: could not parse the private key"
                .to_string()
                .into(),
        );
    };
    let signing_key = SigningKey::<Sha256>::new(priv_key);
    // try_sign (not sign): `Signer::sign` PANICS on an internal signing failure
    // (e.g. a key too small for the digest) — a Sky-reachable abort inside a
    // Result-returning crypto fn. Route the failure to Err instead.
    let signature = match signing_key.try_sign(msg.as_bytes()) {
        Ok(s) => s,
        Err(e) => {
            return SkyResult::Err(format!("Crypto.rsaSha256Sign: signing failed: {}", e).into());
        }
    };
    // Go returns base64.StdEncoding (standard base64, with padding) — match exactly.
    SkyResult::Ok(STANDARD.encode(signature.to_bytes()))
}

/// Sky `rsaSha256Verify : String -> String -> String -> Bool`
/// (pemPublicKey, msg, base64Signature). Returns `false` on any failure — never panics.
/// Accepts SPKI/PKIX public keys (`-----BEGIN PUBLIC KEY-----`, the common openssl form)
/// and PKCS#1 public keys (`-----BEGIN RSA PUBLIC KEY-----`) — mirrors Go oracle
/// (rt.go ~l6500: tries ParsePKIXPublicKey then ParsePKCS1PublicKey).
/// Signature is standard-base64 (base64.StdEncoding, rt.go ~l6511).
#[cfg(feature = "crypto")]
pub fn crypto_rsa_sha256_verify(key_pem: String, msg: String, sig_b64: String) -> bool {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rsa::{
        pkcs1::DecodeRsaPublicKey,
        pkcs1v15::{Signature, VerifyingKey},
        pkcs8::DecodePublicKey,
        signature::Verifier,
    };
    use sha2::Sha256;

    // Try SPKI/PKIX first (-----BEGIN PUBLIC KEY-----), then PKCS#1 — mirrors Go.
    let pub_key = if let Ok(k) = rsa::RsaPublicKey::from_public_key_pem(&key_pem) {
        k
    } else if let Ok(k) = rsa::RsaPublicKey::from_pkcs1_pem(&key_pem) {
        k
    } else {
        return false;
    };
    // Go decodes with base64.StdEncoding (standard base64, with padding) — match exactly.
    let sig_bytes = match STANDARD.decode(sig_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let verifying_key: VerifyingKey<Sha256> = VerifyingKey::<Sha256>::new(pub_key);
    let signature = match Signature::try_from(sig_bytes.as_slice()) {
        Ok(s) => s,
        Err(_) => return false,
    };
    verifying_key.verify(msg.as_bytes(), &signature).is_ok()
}

/// Sky `constantTimeEqual : String -> String -> Bool` — timing-safe byte compare.
pub fn crypto_constant_time_equal(a: String, b: String) -> bool {
    use subtle::ConstantTimeEq;
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    bool::from(ab.ct_eq(bb))
}

// ═══════════════════════════════════════════════════════════
// Symmetric AEAD — AES-256-GCM + ChaCha20-Poly1305
// ═══════════════════════════════════════════════════════════
//
// Output format mirrors the Go backend: base64( nonce[12] || ciphertext ||
// tag[16] ) — a single opaque UTF-8 string. The 32-byte KEY, however, is
// base64-encoded here (the Go backend passes raw bytes). Keys are opaque and
// never cross the backend boundary, so this backend-local encoding is sound and
// is what lets a PBKDF2-derived key (arbitrary bytes) live in a Rust `String`
// (which must be valid UTF-8). aesKeyFromPassword emits the base64 form; the
// AEAD fns base64-decode it back to 32 raw bytes.

const AEAD_KEY_BYTES: usize = 32;
// PBKDF2-HMAC-SHA256 work factor. PINNED to the Go backend's value: a
// password-derived key/blob produced on one backend must verify/decrypt on the
// other, so this is a cross-backend interop contract, NOT a Rust-local knob.
// It is below current OWASP guidance (≈600k for PBKDF2-SHA256); raising it is a
// COORDINATED cross-backend migration (re-derive + re-encrypt existing data),
// not a unilateral Rust change. (Audit 2026-06-19, low/weak-crypto — accepted,
// parity/key-compat-locked.)
const PBKDF2_ITERS: u32 = 100_000;

// Decode a base64 key string to exactly 32 bytes, or an error message.
fn aead_read_key(name: &str, key: &str) -> Result<Vec<u8>, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let k = STANDARD.decode(key.as_bytes()).map_err(|_| {
        format!(
            "{}: key must be a 32-byte key from Crypto.aesKeyFromPassword",
            name
        )
    })?;
    if k.len() != AEAD_KEY_BYTES {
        return Err(format!(
            "{}: key must be {} bytes, got {} (derive via Crypto.aesKeyFromPassword)",
            name,
            AEAD_KEY_BYTES,
            k.len()
        ));
    }
    Ok(k)
}

// Crypto.aesGcmEncrypt : String -> String -> Result Error String
pub fn crypto_aes_gcm_encrypt<E: From<String>>(
    key: String,
    plaintext: String,
) -> SkyResult<E, String> {
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, OsRng, rand_core::RngCore},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    let k = match aead_read_key("Crypto.aesGcmEncrypt", &key) {
        Ok(k) => k,
        Err(e) => return SkyResult::Err(e.into()),
    };
    // aead_read_key validated len == 32 just above, so the Err is structurally
    // unreachable — but propagate into the existing SkyResult channel rather than panic.
    let cipher = match Aes256Gcm::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return SkyResult::Err(format!("Crypto.aesGcmEncrypt: {}", e).into()),
    };
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ct) => {
            let mut out = nonce_bytes.to_vec();
            out.extend_from_slice(&ct);
            SkyResult::Ok(STANDARD.encode(out))
        }
        Err(e) => SkyResult::Err(format!("Crypto.aesGcmEncrypt: {}", e).into()),
    }
}

// Crypto.aesGcmDecrypt : String -> String -> Result Error String
pub fn crypto_aes_gcm_decrypt<E: From<String>>(
    key: String,
    encoded: String,
) -> SkyResult<E, String> {
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    use base64::{Engine, engine::general_purpose::STANDARD};
    let k = match aead_read_key("Crypto.aesGcmDecrypt", &key) {
        Ok(k) => k,
        Err(e) => return SkyResult::Err(e.into()),
    };
    let buf = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return SkyResult::Err(format!("Crypto.aesGcmDecrypt: invalid base64: {}", e).into());
        }
    };
    if buf.len() < 12 {
        return SkyResult::Err(
            "Crypto.aesGcmDecrypt: ciphertext too short"
                .to_string()
                .into(),
        );
    }
    let (nonce_bytes, ct) = buf.split_at(12);
    let cipher = match Aes256Gcm::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return SkyResult::Err(format!("Crypto.aesGcmDecrypt: {}", e).into()),
    };
    match cipher.decrypt(Nonce::from_slice(nonce_bytes), ct) {
        // A Sky String is UTF-8 by construction. Go's oracle returns string(pt)
        // (Go strings are arbitrary bytes), but lossy-replacing invalid UTF-8 here
        // would silently corrupt the plaintext. Reject non-UTF-8 plaintext with a
        // structured Err instead — total, and surfaces the mismatch at the boundary.
        Ok(pt) => match String::from_utf8(pt) {
            Ok(s) => SkyResult::Ok(s),
            Err(_) => SkyResult::Err(
                "Crypto.aesGcmDecrypt: decrypted plaintext is not valid UTF-8"
                    .to_string()
                    .into(),
            ),
        },
        Err(e) => SkyResult::Err(format!("Crypto.aesGcmDecrypt: {}", e).into()),
    }
}

// Crypto.chacha20Encrypt : String -> String -> Result Error String
pub fn crypto_chacha20_encrypt<E: From<String>>(
    key: String,
    plaintext: String,
) -> SkyResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chacha20poly1305::{
        ChaCha20Poly1305, KeyInit, Nonce,
        aead::{Aead, OsRng, rand_core::RngCore},
    };
    let k = match aead_read_key("Crypto.chacha20Encrypt", &key) {
        Ok(k) => k,
        Err(e) => return SkyResult::Err(e.into()),
    };
    let cipher = match ChaCha20Poly1305::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return SkyResult::Err(format!("Crypto.chacha20Encrypt: {}", e).into()),
    };
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ct) => {
            let mut out = nonce_bytes.to_vec();
            out.extend_from_slice(&ct);
            SkyResult::Ok(STANDARD.encode(out))
        }
        Err(e) => SkyResult::Err(format!("Crypto.chacha20Encrypt: {}", e).into()),
    }
}

// Crypto.chacha20Decrypt : String -> String -> Result Error String
pub fn crypto_chacha20_decrypt<E: From<String>>(
    key: String,
    encoded: String,
) -> SkyResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};
    let k = match aead_read_key("Crypto.chacha20Decrypt", &key) {
        Ok(k) => k,
        Err(e) => return SkyResult::Err(e.into()),
    };
    let buf = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return SkyResult::Err(format!("Crypto.chacha20Decrypt: invalid base64: {}", e).into());
        }
    };
    if buf.len() < 12 {
        return SkyResult::Err(
            "Crypto.chacha20Decrypt: ciphertext too short"
                .to_string()
                .into(),
        );
    }
    let (nonce_bytes, ct) = buf.split_at(12);
    let cipher = match ChaCha20Poly1305::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return SkyResult::Err(format!("Crypto.chacha20Decrypt: {}", e).into()),
    };
    match cipher.decrypt(Nonce::from_slice(nonce_bytes), ct) {
        // A Sky String is UTF-8 by construction (see aesGcmDecrypt note). Reject
        // non-UTF-8 plaintext with a structured Err rather than lossy-corrupting it.
        Ok(pt) => match String::from_utf8(pt) {
            Ok(s) => SkyResult::Ok(s),
            Err(_) => SkyResult::Err(
                "Crypto.chacha20Decrypt: decrypted plaintext is not valid UTF-8"
                    .to_string()
                    .into(),
            ),
        },
        Err(e) => SkyResult::Err(format!("Crypto.chacha20Decrypt: {}", e).into()),
    }
}

// Crypto.aesKeyFromPassword : String -> String -> String
// PBKDF2-HMAC-SHA256, 100k iters, 32-byte key, returned base64-encoded.
pub fn crypto_aes_key_from_password(password: String, salt: String) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut key = [0u8; AEAD_KEY_BYTES];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
        password.as_bytes(),
        salt.as_bytes(),
        PBKDF2_ITERS,
        &mut key,
    );
    STANDARD.encode(key)
}

// Crypto.chachaKeyFromPassword : String -> String -> String  (same derivation)
pub fn crypto_chacha_key_from_password(password: String, salt: String) -> String {
    crypto_aes_key_from_password(password, salt)
}

// ── Concrete (non-generic) wrappers for generated Sky code (M5a) ─────────────
//
// The generic `crypto_aes_gcm_encrypt<E>`, `crypto_aes_gcm_decrypt<E>`,
// `crypto_chacha20_encrypt<E>`, `crypto_chacha20_decrypt<E>`,
// `crypto_rsa_sha256_sign<E>` above use a flexible `E: From<String>` bound so
// the error type can be inferred from context. Generated Sky code sets
// `SkyError = sky_runtime::error::SkyError` (backlog #85/#160), but Rust's
// type inference cannot pin `E` when the error arm is discarded (e.g.
// `Err _ ->` in a case expression). These concrete aliases pin `E = SkyError`
// up-front, eliminating the ambiguity without changing runtime semantics.

/// Generated-code alias for `crypto_aes_gcm_encrypt` with `E = String`.
pub fn sky_aes_gcm_encrypt(key: String, plaintext: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    crypto_aes_gcm_encrypt(key, plaintext)
}

/// Generated-code alias for `crypto_aes_gcm_decrypt` with `E = String`.
pub fn sky_aes_gcm_decrypt(key: String, encoded: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    crypto_aes_gcm_decrypt(key, encoded)
}

/// Generated-code alias for `crypto_chacha20_encrypt` with `E = String`.
pub fn sky_chacha20_encrypt(key: String, plaintext: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    crypto_chacha20_encrypt(key, plaintext)
}

/// Generated-code alias for `crypto_chacha20_decrypt` with `E = String`.
pub fn sky_chacha20_decrypt(key: String, encoded: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    crypto_chacha20_decrypt(key, encoded)
}

/// Generated-code alias for `crypto_rsa_sha256_sign` with `E = String`.
#[cfg(feature = "crypto")]
pub fn sky_crypto_rsa_sha256_sign(key_pem: String, msg: String) -> SkyResult<crate::sky_runtime::error::SkyError, String> {
    crypto_rsa_sha256_sign(key_pem, msg)
}

#[cfg(test)]
mod tests_more_hashes {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const EMPTY_SHA512: &str = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
    const EMPTY_SHA1: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    const EMPTY_MD5: &str = "d41d8cd98f00b204e9800998ecf8427e";
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn test_sha256_empty_and_abc() {
        assert_eq!(crypto_sha256(String::new()), EMPTY_SHA256);
        assert_eq!(crypto_sha256("abc".to_string()), ABC_SHA256);
    }

    #[test]
    fn test_sha512_empty() {
        assert_eq!(crypto_sha512(String::new()), EMPTY_SHA512);
    }

    #[test]
    fn test_sha1_empty() {
        assert_eq!(crypto_sha1(String::new()), EMPTY_SHA1);
    }

    #[test]
    fn test_md5_empty() {
        assert_eq!(crypto_md5(String::new()), EMPTY_MD5);
    }

    // RFC 4231 test case 1: key = 0x0b*20, data = "Hi There"
    const HMAC_SHA256_RFC1: &str =
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7";
    const HMAC_SHA512_RFC1: &str = "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854";

    #[test]
    fn test_hmac_sha256_rfc4231() {
        let key: String = (0..20).map(|_| '\u{000b}').collect();
        assert_eq!(
            crypto_hmac_sha256(key.clone(), "Hi There".to_string()),
            HMAC_SHA256_RFC1
        );
        assert_eq!(
            crypto_hmac_sha512(key, "Hi There".to_string()),
            HMAC_SHA512_RFC1
        );
    }

    const RSA_PRIV_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIBOgIBAAJBAK1QGnsdSyVv+JT4WDnGIIr3QA75yZTiTsgxkiXH9sjXrPHT1hXn
2tKCv9MkR8MD1Ndh6jo7inBZUK0YG7H6Jx0CAwEAAQJAX9bpHeXAFW7K5w5CM4il
nFNIAEAPQh63dCs9Z1kh1kPNGKQYujFQ9KgNuw1keQDKhkzd5jCauNJ6Db/xDpdL
PQIhANidlZLm430yH5JrNG9hZpFIM80tUn+cf7J5F4KLIF2zAiEAzNL87wCFzVrt
xE9IhVClKFPemDjO9Mre3Db/V53uH+8CIQC2/BfYatcNcYQeKhW3aS492CJ6Vqj0
R/3PhF+J1YFX5QIgG9S7a5pNlAa78gW32+2GU4F56IMnk9mRCKksbvJVrd8CIFuA
y7anow7/QOtvB1/UdyrxegB+sHZoBWA9+SsMl2zn
-----END RSA PRIVATE KEY-----";

    // SPKI/PKIX public key derived from RSA_PRIV_PEM (`openssl rsa -pubout`).
    // Sky's rsaSha256Verify takes a PUBLIC key — this is the correct pairing.
    const RSA_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----
MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAK1QGnsdSyVv+JT4WDnGIIr3QA75yZTi
TsgxkiXH9sjXrPHT1hXn2tKCv9MkR8MD1Ndh6jo7inBZUK0YG7H6Jx0CAwEAAQ==
-----END PUBLIC KEY-----";

    #[test]
    fn test_rsa_sign_verify_roundtrip() {
        let msg = "hello, sky".to_string();
        let sig: SkyResult<String, String> =
            crypto_rsa_sha256_sign(RSA_PRIV_PEM.to_string(), msg.clone());
        // Sign returns standard base64 (mirrors Go's base64.StdEncoding).
        let sig_b64 = match sig {
            SkyResult::Ok(s) => s,
            SkyResult::Err(e) => panic!("sign failed: {}", e),
        };
        // Verify takes the PUBLIC key, not the private key (mirrors Go oracle).
        assert!(crypto_rsa_sha256_verify(
            RSA_PUB_PEM.to_string(),
            msg,
            sig_b64
        ));
    }

    #[test]
    fn test_rsa_verify_wrong_sig() {
        // "deadbeef" is not valid standard base64 with padding → decodes to false.
        assert!(!crypto_rsa_sha256_verify(
            RSA_PUB_PEM.to_string(),
            "hello".to_string(),
            "deadbeef".to_string()
        ));
    }

    #[test]
    fn test_constant_time_equal() {
        assert!(crypto_constant_time_equal(
            "abc".to_string(),
            "abc".to_string()
        ));
        assert!(!crypto_constant_time_equal(
            "abc".to_string(),
            "abd".to_string()
        ));
        assert!(!crypto_constant_time_equal(
            "abc".to_string(),
            "ab".to_string()
        ));
    }
}
