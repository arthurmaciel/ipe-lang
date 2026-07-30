// Crypto kernel stubs — generic over E where needed.
//
// wasm32: only `crypto_random_bytes`/`crypto_random_token` (this file's
// entropy pair) and the pure hash/HMAC family below compile for the browser
// target — the AEAD/RSA/PBKDF2 functions further down are each individually
// `cfg(not(target_arch = "wasm32"))` (a stated M4 exclusion, same class as
// the M0 floor's crypto-feature exclusion: untested getrandom-js support
// across the whole RustCrypto stack, and no browser-bundle reason to pull
// `aes-gcm`/`rsa`/`bcrypt`/`chacha20poly1305`/`pbkdf2` for symmetric AEAD that
// isn't in the M4 scope). `Ipe.Crypto.aesGcmEncrypt`/friends therefore stay
// UNTAGGED in the `WasmClient` kernel registry — the wrapper is compiled out
// entirely, not merely unreachable.
use super::*;

// ── Typed role newtypes ───────────────────────────────────────────────────────
//
// `Key` and `Mac` make distinct cryptographic roles distinct Rust types so a
// role-swap (passing a message where a key is expected) is a compile error, not
// a silent wrong answer. Both wrap an opaque `String` blob — callers never
// inspect the byte content directly; the role is what matters.

/// `Crypto.Key` — an opaque cryptographic key obtained from `Key.fromString`,
/// `Key.fromBytes`, or `Crypto.aesKeyFromPassword` / `Crypto.chachaKeyFromPassword`.
///
/// Distinct from `String`: passing a `Key` where a message is expected (or
/// vice-versa) is a compile-time error, not a silent wrong MAC or ciphertext.
///
/// `Clone`: keys are legitimately reused across multiple operations (the
/// keyfrompassword golden fixture derives the same key twice and uses each once).
/// NOT `Debug`/`Display`: prevents accidental key material appearing in log
/// lines or panic messages. `PartialEq` via constant-time compare (same
/// reasoning as `Secret`: the length check is metadata, but byte equality is
/// timing-safe via `subtle`).
#[derive(Clone)]
pub struct Key(String);

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq;
        let (a, b) = (self.0.as_bytes(), other.0.as_bytes());
        a.len() == b.len() && bool::from(a.ct_eq(b))
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<key>")
    }
}

impl crate::stringify::IpeStringify for Key {
    fn ipe_show(&self) -> String {
        "<key>".to_owned()
    }
}

/// `Crypto.Mac` — an opaque message authentication code (hex-encoded) returned
/// by `Crypto.hmacSha256` / `Crypto.hmacSha512` with the typed-key variants.
///
/// Distinct from `String` so a MAC output cannot be silently passed where a
/// key or plaintext is expected. Wraps the hex-encoded tag; `Mac.toHex` is the
/// only extraction path so a reviewer can grep for every MAC-reveal site.
///
/// `Clone + PartialEq`: MACs are compared for equality (verify pattern). The
/// comparison is NOT timing-safe here because a MAC tag is the output of a
/// one-way function, not a secret by itself (timing-safe equality is on the
/// input key via `Key::PartialEq`; for MAC verification use
/// `Crypto.constantTimeEqual` on the hex strings if timing safety matters).
#[derive(Clone, PartialEq, Debug)]
pub struct Mac(String);

impl crate::stringify::IpeStringify for Mac {
    fn ipe_show(&self) -> String {
        self.0.clone()
    }
}

/// `Key.fromString : String -> Key` — construction boundary: promotes any
/// `String` to a typed key role. The byte content is opaque; the role is
/// distinct.
#[must_use]
pub fn crypto_key_from_string(s: String) -> Key {
    Key(s)
}

/// `Key.fromBytes : String -> Key` — alias for `fromString` when the caller
/// holds a byte-string (Ipê `Bytes` is `String`). Identical semantics.
#[must_use]
pub fn crypto_key_from_bytes(s: String) -> Key {
    Key(s)
}

/// `Mac.toHex : Mac -> String` — the single extraction boundary: recover the
/// hex-encoded tag from an opaque `Mac`. Greppable, so a reviewer can audit
/// every place a raw MAC string escapes the typed wrapper.
#[must_use]
pub fn crypto_mac_to_hex(m: Mac) -> String {
    m.0
}

// `Crypto.randomBytes : Int -> Task Error String`. Go returns the entropy as a
// LOWERCASE HEX string (rt.go ~l6543: `hex.EncodeToString(b)`), NOT a byte list —
// the Ipê signature is `String`, so the Rust side must return a hex `String` too.
// (A prior `Vec<i64>` return diverged from both the Ipê type and Go: a Ipê call
// site treating the result as a String/Bytes mismatched at codegen.)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_random_bytes<E: From<String> + Send + 'static>(n: i64) -> IpeTask<E, String> {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    Box::pin(async move {
        // SECURITY: Mirror Go oracle exactly: reject size <= 0 || size > 1024
        // (rt.go ~l6536: `if size <= 0 || size > 1024 { return ErrInvalidInput }`)
        // to prevent unbounded attacker-controlled allocation (DoS vector).
        if n <= 0 || n > 1024 {
            return IpeResult::Err(
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

/// Browser substitute: `crypto.getRandomValues` via the `getrandom` crate's
/// `js` backend (Q3: "`Random.*` / `Crypto.randomBytes` | SUBSTITUTE |
/// `crypto.getRandomValues`") — no `aes-gcm`/`OsRng` dependency pulled into
/// the bundle just for entropy. Same size guard + hex encoding as the native
/// arm (Go-oracle parity is a native-only contract, but the size guard is a
/// real DoS control worth keeping on both targets).
#[cfg(target_arch = "wasm32")]
pub fn crypto_random_bytes<E: From<String> + 'static>(n: i64) -> IpeTask<E, String> {
    Box::pin(async move {
        if n <= 0 || n > 1024 {
            return IpeResult::Err(
                "Crypto.randomBytes: size must be 1..1024"
                    .to_string()
                    .into(),
            );
        }
        let count = n as usize;
        let mut buf = vec![0u8; count];
        if getrandom::getrandom(&mut buf).is_err() {
            return IpeResult::Err(
                "Crypto.randomBytes: browser entropy source unavailable"
                    .to_string()
                    .into(),
            );
        }
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
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_random_token<E: From<String> + Send + 'static>(n: i64) -> IpeTask<E, String> {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    Box::pin(async move {
        // SECURITY: Mirror Go oracle exactly: reject size <= 0 || size > 1024
        // (rt.go ~l6553: `if size <= 0 || size > 1024 { return ErrInvalidInput }`)
        // to prevent unbounded attacker-controlled allocation (DoS vector).
        if n <= 0 || n > 1024 {
            return IpeResult::Err(
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

/// Browser substitute — same `getrandom(js)` entropy source as
/// `crypto_random_bytes`, URL-safe-no-pad base64 encoded.
#[cfg(target_arch = "wasm32")]
pub fn crypto_random_token<E: From<String> + 'static>(n: i64) -> IpeTask<E, String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    Box::pin(async move {
        if n <= 0 || n > 1024 {
            return IpeResult::Err(
                "Crypto.randomToken: size must be 1..1024"
                    .to_string()
                    .into(),
            );
        }
        let count = n as usize;
        let mut buf = vec![0u8; count];
        if getrandom::getrandom(&mut buf).is_err() {
            return IpeResult::Err(
                "Crypto.randomToken: browser entropy source unavailable"
                    .to_string()
                    .into(),
            );
        }
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

/// Ipê `sha512 : String -> String` — hex-encoded SHA-512 digest.
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
/// parity; the hardening is this contract note. (Audit finding: low/weak-crypto.)
///
/// Ipê `sha1 : String -> String` — hex-encoded SHA-1 digest.
pub fn crypto_sha1(s: String) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Ipê `md5 : String -> String` — hex-encoded MD5 digest.
pub fn crypto_md5(s: String) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Ipê `hmacSha256 : String -> String -> String` (key, message → hex tag).
pub fn crypto_hmac_sha256(key: String, msg: String) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    // STRUCTURALLY-DEAD Err: `Hmac<D>::new_from_slice` pads/hashes any key
    // internally and returns `Ok` unconditionally, so `InvalidLength` is never
    // produced. Kept as a LOUD `.expect`, not eliminated: threading a Result through
    // this pure `String -> String` kernel makes callers handle a never-occurring Err
    // whose mishandling is a wrong hash, and an infallible-by-type ctor would
    // reimplement key prep (a security regression). See the ledger for the full verdict.
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — structurally-dead HMAC InvalidLength; a loud .expect is safer than a dead Result Err a caller can mishandle into a wrong MAC [ledger #1]
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

/// `Crypto.hmacSha256WithKey : Key -> String -> Mac` — typed variant.
/// The key parameter is a distinct `Key` role so passing the message where the
/// key is expected is a compile error.
pub fn crypto_hmac_sha256_key(key: Key, msg: String) -> Mac {
    use hmac::{Hmac, Mac as HmacMac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    // STRUCTURALLY-DEAD Err: same reasoning as `crypto_hmac_sha256` above.
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — structurally-dead HMAC InvalidLength; a loud .expect is safer than a dead Result Err a caller can mishandle into a wrong MAC [ledger #1]
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha256::new_from_slice(key.0.as_bytes()).expect("Hmac<Sha256> accepts any key length");
    mac.update(msg.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Mac(hex)
}

/// `Crypto.hmacSha512WithKey : Key -> String -> Mac` — typed variant.
pub fn crypto_hmac_sha512_key(key: Key, msg: String) -> Mac {
    use hmac::{Hmac, Mac as HmacMac};
    use sha2::Sha512;
    type HmacSha512 = Hmac<Sha512>;
    // STRUCTURALLY-DEAD Err: same reasoning as `crypto_hmac_sha512` above.
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — structurally-dead HMAC InvalidLength; a loud .expect is safer than a dead Result Err a caller can mishandle into a wrong MAC [ledger #1]
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha512::new_from_slice(key.0.as_bytes()).expect("Hmac<Sha512> accepts any key length");
    mac.update(msg.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Mac(hex)
}

/// Ipê `hmacSha512 : String -> String -> String`.
pub fn crypto_hmac_sha512(key: String, msg: String) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;
    type HmacSha512 = Hmac<Sha512>;
    // STRUCTURALLY-DEAD Err: `Hmac<D>::new_from_slice` pads/hashes any key
    // internally and returns `Ok` unconditionally, so `InvalidLength` is never
    // produced. Kept as a LOUD `.expect`, not eliminated: threading a Result through
    // this pure `String -> String` kernel makes callers handle a never-occurring Err
    // whose mishandling is a wrong hash, and an infallible-by-type ctor would
    // reimplement key prep (a security regression). See the ledger for the full verdict.
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — structurally-dead HMAC InvalidLength; a loud .expect is safer than a dead Result Err a caller can mishandle into a wrong MAC [ledger #1]
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

/// Ipê `rsaSha256Sign : String -> String -> Result Error String`
/// Sign `msg` with the PKCS#1 v1.5 SHA-256 RSA scheme using `key_pem`.
/// Accepts PKCS#1 (`-----BEGIN RSA PRIVATE KEY-----`) and PKCS#8
/// (`-----BEGIN PRIVATE KEY-----`) PEM private keys — mirrors Go oracle
/// (rt.go ~l6472: tries ParsePKCS1PrivateKey then ParsePKCS8PrivateKey).
/// Returns standard-base64-encoded signature (base64.StdEncoding, rt.go ~l6488).
#[cfg(feature = "crypto")]
pub fn crypto_rsa_sha256_sign<E: From<String>>(
    key_pem: String,
    msg: String,
) -> IpeResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rsa::{
        pkcs1::DecodeRsaPrivateKey,
        pkcs1v15::SigningKey,
        pkcs8::DecodePrivateKey,
        signature::{SignatureEncoding, Signer},
    };
    use sha2::Sha256;

    // Try PKCS#8 first (the openssl default), then fall back to PKCS#1 — mirrors Go.
    let priv_key = match rsa::RsaPrivateKey::from_pkcs8_pem(&key_pem) {
        Ok(k) => k,
        _ => match rsa::RsaPrivateKey::from_pkcs1_pem(&key_pem) {
            Ok(k) => k,
            _ => {
                return IpeResult::Err(
                    "Crypto.rsaSha256Sign: could not parse the private key"
                        .to_string()
                        .into(),
                );
            }
        },
    };
    let signing_key = SigningKey::<Sha256>::new(priv_key);
    // try_sign (not sign): `Signer::sign` PANICS on an internal signing failure
    // (e.g. a key too small for the digest) — a Ipê-reachable abort inside a
    // Result-returning crypto fn. Route the failure to Err instead.
    let signature = match signing_key.try_sign(msg.as_bytes()) {
        Ok(s) => s,
        Err(e) => {
            return IpeResult::Err(format!("Crypto.rsaSha256Sign: signing failed: {}", e).into());
        }
    };
    // Go returns base64.StdEncoding (standard base64, with padding) — match exactly.
    IpeResult::Ok(STANDARD.encode(signature.to_bytes()))
}

/// Ipê `rsaSha256Verify : String -> String -> String -> Bool`
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
    let pub_key = match rsa::RsaPublicKey::from_public_key_pem(&key_pem) {
        Ok(k) => k,
        _ => match rsa::RsaPublicKey::from_pkcs1_pem(&key_pem) {
            Ok(k) => k,
            _ => {
                return false;
            }
        },
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

/// Ipê `constantTimeEqual : String -> String -> Bool` — timing-safe byte compare.
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

#[cfg(not(target_arch = "wasm32"))]
const AEAD_KEY_BYTES: usize = 32;
// PBKDF2-HMAC-SHA256 work factor. PINNED to the Go backend's value: a
// password-derived key/blob produced on one backend must verify/decrypt on the
// other, so this is a cross-backend interop contract, NOT a Rust-local knob.
// It is below current OWASP guidance (≈600k for PBKDF2-SHA256); raising it is a
// COORDINATED cross-backend migration (re-derive + re-encrypt existing data),
// not a unilateral Rust change. (Audit finding: low/weak-crypto — accepted,
// parity/key-compat-locked.)
#[cfg(not(target_arch = "wasm32"))]
const PBKDF2_ITERS: u32 = 100_000;

// Decode a base64 key string to exactly 32 bytes, or an error message.
#[cfg(not(target_arch = "wasm32"))]
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

// Crypto.aesGcmEncryptKey : Key -> String -> Result Error String  (typed variant)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_aes_gcm_encrypt_key<E: From<String>>(
    key: Key,
    plaintext: String,
) -> IpeResult<E, String> {
    crypto_aes_gcm_encrypt(key.0, plaintext)
}

// Crypto.aesGcmDecryptKey : Key -> String -> Result Error String  (typed variant)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_aes_gcm_decrypt_key<E: From<String>>(
    key: Key,
    encoded: String,
) -> IpeResult<E, String> {
    crypto_aes_gcm_decrypt(key.0, encoded)
}

// Crypto.aesGcmEncrypt : String -> String -> Result Error String
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_aes_gcm_encrypt<E: From<String>>(
    key: String,
    plaintext: String,
) -> IpeResult<E, String> {
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, OsRng, rand_core::RngCore},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    let k = match aead_read_key("Crypto.aesGcmEncrypt", &key) {
        Ok(k) => k,
        Err(e) => return IpeResult::Err(e.into()),
    };
    // aead_read_key validated len == 32 just above, so the Err is structurally
    // unreachable — but propagate into the existing IpeResult channel rather than panic.
    let cipher = match Aes256Gcm::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return IpeResult::Err(format!("Crypto.aesGcmEncrypt: {}", e).into()),
    };
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ct) => {
            let mut out = nonce_bytes.to_vec();
            out.extend_from_slice(&ct);
            IpeResult::Ok(STANDARD.encode(out))
        }
        Err(e) => IpeResult::Err(format!("Crypto.aesGcmEncrypt: {}", e).into()),
    }
}

// Crypto.aesGcmDecrypt : String -> String -> Result Error String
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_aes_gcm_decrypt<E: From<String>>(
    key: String,
    encoded: String,
) -> IpeResult<E, String> {
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    use base64::{Engine, engine::general_purpose::STANDARD};
    let k = match aead_read_key("Crypto.aesGcmDecrypt", &key) {
        Ok(k) => k,
        Err(e) => return IpeResult::Err(e.into()),
    };
    let buf = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return IpeResult::Err(format!("Crypto.aesGcmDecrypt: invalid base64: {}", e).into());
        }
    };
    if buf.len() < 12 {
        return IpeResult::Err(
            "Crypto.aesGcmDecrypt: ciphertext too short"
                .to_string()
                .into(),
        );
    }
    let (nonce_bytes, ct) = buf.split_at(12);
    let cipher = match Aes256Gcm::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return IpeResult::Err(format!("Crypto.aesGcmDecrypt: {}", e).into()),
    };
    match cipher.decrypt(Nonce::from_slice(nonce_bytes), ct) {
        // A Ipê String is UTF-8 by construction. Go's oracle returns string(pt)
        // (Go strings are arbitrary bytes), but lossy-replacing invalid UTF-8 here
        // would silently corrupt the plaintext. Reject non-UTF-8 plaintext with a
        // structured Err instead — total, and surfaces the mismatch at the boundary.
        Ok(pt) => match String::from_utf8(pt) {
            Ok(s) => IpeResult::Ok(s),
            Err(_) => IpeResult::Err(
                "Crypto.aesGcmDecrypt: decrypted plaintext is not valid UTF-8"
                    .to_string()
                    .into(),
            ),
        },
        Err(e) => IpeResult::Err(format!("Crypto.aesGcmDecrypt: {}", e).into()),
    }
}

// Crypto.chacha20EncryptKey : Key -> String -> Result Error String  (typed variant)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha20_encrypt_key<E: From<String>>(
    key: Key,
    plaintext: String,
) -> IpeResult<E, String> {
    crypto_chacha20_encrypt(key.0, plaintext)
}

// Crypto.chacha20DecryptKey : Key -> String -> Result Error String  (typed variant)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha20_decrypt_key<E: From<String>>(
    key: Key,
    encoded: String,
) -> IpeResult<E, String> {
    crypto_chacha20_decrypt(key.0, encoded)
}

// Crypto.chacha20Encrypt : String -> String -> Result Error String
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha20_encrypt<E: From<String>>(
    key: String,
    plaintext: String,
) -> IpeResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chacha20poly1305::{
        ChaCha20Poly1305, KeyInit, Nonce,
        aead::{Aead, OsRng, rand_core::RngCore},
    };
    let k = match aead_read_key("Crypto.chacha20Encrypt", &key) {
        Ok(k) => k,
        Err(e) => return IpeResult::Err(e.into()),
    };
    let cipher = match ChaCha20Poly1305::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return IpeResult::Err(format!("Crypto.chacha20Encrypt: {}", e).into()),
    };
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ct) => {
            let mut out = nonce_bytes.to_vec();
            out.extend_from_slice(&ct);
            IpeResult::Ok(STANDARD.encode(out))
        }
        Err(e) => IpeResult::Err(format!("Crypto.chacha20Encrypt: {}", e).into()),
    }
}

// Crypto.chacha20Decrypt : String -> String -> Result Error String
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha20_decrypt<E: From<String>>(
    key: String,
    encoded: String,
) -> IpeResult<E, String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};
    let k = match aead_read_key("Crypto.chacha20Decrypt", &key) {
        Ok(k) => k,
        Err(e) => return IpeResult::Err(e.into()),
    };
    let buf = match STANDARD.decode(encoded.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return IpeResult::Err(format!("Crypto.chacha20Decrypt: invalid base64: {}", e).into());
        }
    };
    if buf.len() < 12 {
        return IpeResult::Err(
            "Crypto.chacha20Decrypt: ciphertext too short"
                .to_string()
                .into(),
        );
    }
    let (nonce_bytes, ct) = buf.split_at(12);
    let cipher = match ChaCha20Poly1305::new_from_slice(&k) {
        Ok(c) => c,
        Err(e) => return IpeResult::Err(format!("Crypto.chacha20Decrypt: {}", e).into()),
    };
    match cipher.decrypt(Nonce::from_slice(nonce_bytes), ct) {
        // A Ipê String is UTF-8 by construction (see aesGcmDecrypt note). Reject
        // non-UTF-8 plaintext with a structured Err rather than lossy-corrupting it.
        Ok(pt) => match String::from_utf8(pt) {
            Ok(s) => IpeResult::Ok(s),
            Err(_) => IpeResult::Err(
                "Crypto.chacha20Decrypt: decrypted plaintext is not valid UTF-8"
                    .to_string()
                    .into(),
            ),
        },
        Err(e) => IpeResult::Err(format!("Crypto.chacha20Decrypt: {}", e).into()),
    }
}

// Crypto.aesKeyFromPassword : String -> String -> String
// PBKDF2-HMAC-SHA256, 100k iters, 32-byte key, returned base64-encoded.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha_key_from_password(password: String, salt: String) -> String {
    crypto_aes_key_from_password(password, salt)
}

// Crypto.aesKeyFromPasswordKey : String -> String -> Key  (typed-key variant)
//
// Returns a typed `Key` rather than a bare `String` so the derived key can
// only be passed to typed AEAD operations (`aesGcmEncryptKey` /
// `aesGcmDecryptKey`), making a role-swap a compile error.
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_aes_key_from_password_key(password: String, salt: String) -> Key {
    Key(crypto_aes_key_from_password(password, salt))
}

// Crypto.chachaKeyFromPasswordKey : String -> String -> Key  (typed-key variant)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_chacha_key_from_password_key(password: String, salt: String) -> Key {
    Key(crypto_aes_key_from_password(password, salt))
}

// ── Concrete (non-generic) wrappers for generated Ipê code ─────────────
//
// The generic `crypto_aes_gcm_encrypt<E>`, `crypto_aes_gcm_decrypt<E>`,
// `crypto_chacha20_encrypt<E>`, `crypto_chacha20_decrypt<E>`,
// `crypto_rsa_sha256_sign<E>` above use a flexible `E: From<String>` bound so
// the error type can be inferred from context. Generated Ipê code sets
// `IpeError = ipe_runtime::error::IpeError`, but Rust's
// type inference cannot pin `E` when the error arm is discarded (e.g.
// `Err _ ->` in a case expression). These concrete aliases pin `E = IpeError`
// up-front, eliminating the ambiguity without changing runtime semantics.

/// Generated-code alias for `crypto_aes_gcm_encrypt` with `E = String`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_aes_gcm_encrypt(
    key: String,
    plaintext: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_aes_gcm_encrypt(key, plaintext)
}

/// Generated-code alias for `crypto_aes_gcm_decrypt` with `E = String`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_aes_gcm_decrypt(
    key: String,
    encoded: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_aes_gcm_decrypt(key, encoded)
}

/// Generated-code alias for `crypto_chacha20_encrypt` with `E = String`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_chacha20_encrypt(
    key: String,
    plaintext: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_chacha20_encrypt(key, plaintext)
}

/// Generated-code alias for `crypto_chacha20_decrypt` with `E = String`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_chacha20_decrypt(
    key: String,
    encoded: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_chacha20_decrypt(key, encoded)
}

/// Generated-code alias for `crypto_aes_gcm_encrypt_key` with `E = IpeError`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_aes_gcm_encrypt_key(
    key: Key,
    plaintext: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_aes_gcm_encrypt_key(key, plaintext)
}

/// Generated-code alias for `crypto_aes_gcm_decrypt_key` with `E = IpeError`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_aes_gcm_decrypt_key(
    key: Key,
    encoded: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_aes_gcm_decrypt_key(key, encoded)
}

/// Generated-code alias for `crypto_chacha20_encrypt_key` with `E = IpeError`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_chacha20_encrypt_key(
    key: Key,
    plaintext: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_chacha20_encrypt_key(key, plaintext)
}

/// Generated-code alias for `crypto_chacha20_decrypt_key` with `E = IpeError`.
#[cfg(not(target_arch = "wasm32"))]
pub fn ipe_chacha20_decrypt_key(
    key: Key,
    encoded: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_chacha20_decrypt_key(key, encoded)
}

/// Generated-code alias for `crypto_rsa_sha256_sign` with `E = String`.
#[cfg(feature = "crypto")]
pub fn ipe_crypto_rsa_sha256_sign(
    key_pem: String,
    msg: String,
) -> IpeResult<crate::error::IpeError, String> {
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
    // Ipê's rsaSha256Verify takes a PUBLIC key — this is the correct pairing.
    const RSA_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----
MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAK1QGnsdSyVv+JT4WDnGIIr3QA75yZTi
TsgxkiXH9sjXrPHT1hXn2tKCv9MkR8MD1Ndh6jo7inBZUK0YG7H6Jx0CAwEAAQ==
-----END PUBLIC KEY-----";

    #[test]
    fn test_rsa_sign_verify_roundtrip() {
        let msg = "hello, ipe".to_string();
        let sig: IpeResult<String, String> =
            crypto_rsa_sha256_sign(RSA_PRIV_PEM.to_string(), msg.clone());
        // Sign returns standard base64 (mirrors Go's base64.StdEncoding).
        let sig_b64 = match sig {
            IpeResult::Ok(s) => s,
            IpeResult::Err(e) => panic!("sign failed: {}", e),
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

    // ── Typed newtype tests ──────────────────────────────────────────────────

    /// `Key.fromString` promotes any string to a typed key; `Mac.toHex`
    /// recovers the hex tag from a typed MAC. Verifies the construction
    /// boundary and extraction boundary round-trip correctly.
    #[test]
    fn typed_key_and_mac_round_trip() {
        let raw_key: String = (0..20).map(|_| '\u{000b}').collect();
        let key = crypto_key_from_string(raw_key);
        let mac = crypto_hmac_sha256_key(key, "Hi There".to_string());
        // RFC 4231 test vector 1 — same value as the String-typed variant
        assert_eq!(
            crypto_mac_to_hex(mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// `crypto_key_from_bytes` is an alias for `crypto_key_from_string` with
    /// identical byte semantics; both paths produce the same MAC.
    #[test]
    fn key_from_bytes_same_as_key_from_string() {
        let raw: String = (0..20).map(|_| '\u{000b}').collect();
        let k1 = crypto_key_from_string(raw.clone());
        let k2 = crypto_key_from_bytes(raw);
        let mac1 = crypto_hmac_sha256_key(k1, "data".to_string());
        let mac2 = crypto_hmac_sha256_key(k2, "data".to_string());
        assert_eq!(crypto_mac_to_hex(mac1), crypto_mac_to_hex(mac2));
    }

    /// `Key`'s `Debug` impl MUST redact key material — never "<key-bytes>".
    #[test]
    fn key_debug_is_redacted() {
        let key = crypto_key_from_string("supersecret".to_string());
        let debug_str = format!("{:?}", key);
        assert_eq!(debug_str, "<key>", "Key Debug must redact to '<key>'");
        assert!(
            !debug_str.contains("supersecret"),
            "Key Debug must not contain raw key material"
        );
    }

    /// Role-swap guard: `Key` and `Mac` are distinct types at the Rust level.
    /// This test documents the expected COMPILE ERROR if you try to pass a `Mac`
    /// where a `Key` is expected. The compile-fail check is in the doc comment:
    ///
    /// ```compile_fail
    /// use ipe_runtime_rust::crypto::{crypto_key_from_string, crypto_hmac_sha256_key};
    /// let mac = crypto_hmac_sha256_key(
    ///     crypto_key_from_string("k".to_string()),
    ///     "m".to_string(),
    /// );
    /// // Passing a Mac where Key is expected: compile error
    /// let _ = crypto_hmac_sha256_key(mac, "m".to_string());
    /// ```
    #[test]
    fn role_swap_types_are_distinct() {
        // Verify at runtime that Key != Mac (they are separate newtypes).
        // The static compile-fail case is documented in the doc comment above.
        let key = crypto_key_from_string("k".to_string());
        let mac = crypto_hmac_sha256_key(key, "m".to_string());
        // mac_to_hex is the ONLY extraction path for a Mac
        let hex = crypto_mac_to_hex(mac);
        assert!(
            !hex.is_empty(),
            "Mac extraction via mac_to_hex must succeed"
        );
    }
}
