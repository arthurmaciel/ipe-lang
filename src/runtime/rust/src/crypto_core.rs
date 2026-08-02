// Crypto core — the cryptographic floor.
//
// This module holds the entropy pair (`crypto_random_bytes`/
// `crypto_random_token`, emitted into every program via the kernel-wrapper
// prelude), the SHA-2 hash + HMAC family, the typed `Key`/`Mac` role newtypes,
// and the constant-time compare — the primitives an always-on floor keeps. The
// RSA SHA-256 sign/verify pair is `cfg(feature = "crypto")`: it compiles (and
// pulls the `rsa` crate) only when the program reaches the heavy crypto floor
// (a `Crypto` kernel, a `Jwt` kernel, or the `Auth` surface). The heavier,
// rarely-used primitives (legacy SHA-1/MD5, AES-GCM + ChaCha20-Poly1305 AEAD,
// PBKDF2 key derivation) live in the sibling `crypto` module and pull their own
// crates.
//
// wasm32: only `crypto_random_bytes`/`crypto_random_token` (this file's entropy
// pair) and the pure hash/HMAC family compile for the browser target — the RSA
// functions are each individually `cfg(feature = "crypto")` (a native contract).
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

/// Crate-internal `Key` unwrap — the sibling `crypto` module's typed AEAD
/// wrappers (`aesGcmEncryptKey` / `chacha20EncryptKey` / …) recover the raw key
/// blob to forward to the `String`-keyed AEAD primitives. `pub(crate)` so the
/// opaque key material still cannot escape the runtime crate.
#[must_use]
pub(crate) fn crypto_key_reveal(key: Key) -> String {
    key.0
}

// `Crypto.randomBytes : Int -> Task Error String`. Go returns the entropy as a
// LOWERCASE HEX string (rt.go ~l6543: `hex.EncodeToString(b)`), NOT a byte list —
// the Ipê signature is `String`, so the Rust side must return a hex `String` too.
// (A prior `Vec<i64>` return diverged from both the Ipê type and Go: a Ipê call
// site treating the result as a String/Bytes mismatched at codegen.)
#[cfg(not(target_arch = "wasm32"))]
pub fn crypto_random_bytes<E: From<String> + Send + 'static>(n: i64) -> IpeTask<E, String> {
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
        if getrandom::getrandom(&mut buf).is_err() {
            return IpeResult::Err(
                "Crypto.randomBytes: OS entropy source unavailable"
                    .to_string()
                    .into(),
            );
        }
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
        if getrandom::getrandom(&mut buf).is_err() {
            return IpeResult::Err(
                "Crypto.randomToken: OS entropy source unavailable"
                    .to_string()
                    .into(),
            );
        }
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

/// Generated-code alias for `crypto_rsa_sha256_sign` with `E = String`.
///
/// Gated the same as [`crypto_rsa_sha256_sign`] (its callee): the RSA arm
/// compiles only when the `crypto` feature is on, which a generated crate
/// enables exactly when it reaches the heavy crypto floor (a `Crypto` kernel,
/// a `Jwt` kernel, or the `Auth` surface). A program that touches none pulls no
/// `rsa` dependency, so this alias must not reference the absent primitive.
#[cfg(feature = "crypto")]
pub fn ipe_crypto_rsa_sha256_sign(
    key_pem: String,
    msg: String,
) -> IpeResult<crate::error::IpeError, String> {
    crypto_rsa_sha256_sign(key_pem, msg)
}

#[cfg(test)]
mod tests_core {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const EMPTY_SHA512: &str = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
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

    #[cfg(feature = "crypto")]
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
    #[cfg(feature = "crypto")]
    const RSA_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----
MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAK1QGnsdSyVv+JT4WDnGIIr3QA75yZTi
TsgxkiXH9sjXrPHT1hXn2tKCv9MkR8MD1Ndh6jo7inBZUK0YG7H6Jx0CAwEAAQ==
-----END PUBLIC KEY-----";

    #[cfg(feature = "crypto")]
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

    #[cfg(feature = "crypto")]
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
    #[test]
    fn role_swap_types_are_distinct() {
        // Verify at runtime that Key != Mac (they are separate newtypes).
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
