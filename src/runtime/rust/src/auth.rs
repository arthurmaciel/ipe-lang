//! Ipe.Auth kernels — authentication helpers.
//!
//! Two tiers (matches the Ipê-side `Ipe.Auth` doc):
//!   - Pure crypto (Result Error _): hashPassword/Cost, verifyPassword,
//!     passwordStrength, signToken, verifyToken.
//!   - DB flows (Task Error _): register, login, setRole.
//!
//! Backed by `bcrypt` for password hashing and `jsonwebtoken` for
//! JWT HS256. DB kernels reuse the sqlx pool from the `db` module.

use super::*;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A fixed, valid cost-12 bcrypt hash used ONLY to make the unknown-email login
/// path do the same KDF work as the known-email path (anti-enumeration timing
/// defence). Computed once; the verify result is always discarded. Cost 12
/// matches the register default so both paths cost the same.
fn dummy_bcrypt_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        // bcrypt::hash is infallible for a fixed valid input + cost; on the
        // structurally-unreachable Err, fall back to a static valid cost-12
        // hash literal so the verify still runs the KDF.
        bcrypt::hash("ipe-login-timing-defence", 12).unwrap_or_else(|_| {
            "$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW".to_string()
        })
    })
}

// ─── Pure crypto kernels ──────────────────────────────────────────────

/// Ipê `hashPassword : String -> Result Error String`. Bcrypt with default
/// cost 12 (matches Go runtime).
pub fn auth_hash_password<E: From<String>>(pw: String) -> IpeResult<E, String> {
    auth_hash_password_cost(pw, 12)
}

/// Ipê `hashPasswordCost : String -> Int -> Result Error String`. Clamps cost to
/// [4, 15] (4 = fast for tests, 12 = production default, 14–15 = high security).
/// The bcrypt VALID range is [4, 31], but cost is caller-controlled: each +1
/// DOUBLES the work, so cost 31 is a single-call CPU-exhaustion DoS (~years per
/// hash). 15 (~1–2 s/hash) is a generous operational ceiling; higher is always a
/// self-DoS, so it is clamped down rather than honoured.
pub fn auth_hash_password_cost<E: From<String>>(pw: String, cost: i64) -> IpeResult<E, String> {
    if pw.chars().count() < 8 {
        return IpeResult::Err("password must be at least 8 characters".to_string().into());
    }
    if pw.len() > 72 {
        return IpeResult::Err(
            "password longer than 72 bytes (bcrypt limit)"
                .to_string()
                .into(),
        );
    }
    let clamped = cost.clamp(4, 15) as u32;
    match bcrypt::hash(&pw, clamped) {
        Ok(h) => IpeResult::Ok(h),
        Err(e) => IpeResult::Err(format!("bcrypt: {}", e).into()),
    }
}

/// Ipê `verifyPassword : String -> String -> Result Error Bool`.
/// `verifyPassword candidate hash` — true if candidate hashes to the same hash.
pub fn auth_verify_password<E: From<String>>(pw: String, hash: String) -> IpeResult<E, bool> {
    match bcrypt::verify(&pw, &hash) {
        Ok(b) => IpeResult::Ok(b),
        Err(e) => IpeResult::Err(format!("bcrypt verify: {}", e).into()),
    }
}

/// Ipê `passwordStrength : String -> Result Error String`. Validates length
/// and character variety; returns a strength rating on Ok.
///   <8 chars  → Err "too short"
///   >72 bytes → Err "too long" (bcrypt limit)
/// > all-letters or all-digits → Err "needs both letters and digits"
/// > ≥12 chars + letter + digit + symbol → "strong"
/// > ≥10 chars + letter + digit          → "medium"
/// > otherwise (passes letter+digit check) → "weak"
pub fn auth_password_strength<E: From<String>>(pw: String) -> IpeResult<E, String> {
    if pw.chars().count() < 8 {
        return IpeResult::Err("password must be at least 8 characters".to_string().into());
    }
    if pw.len() > 72 {
        return IpeResult::Err(
            "password longer than 72 bytes (bcrypt limit)"
                .to_string()
                .into(),
        );
    }
    let has_letter = pw.chars().any(|c| c.is_alphabetic());
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    let has_symbol = pw.chars().any(|c| !c.is_alphanumeric());
    if !has_letter || !has_digit {
        return IpeResult::Err(
            "password must contain both letters and digits"
                .to_string()
                .into(),
        );
    }
    let char_count = pw.chars().count();
    let rating = if char_count >= 12 && has_symbol {
        "strong"
    } else if char_count >= 10 {
        "medium"
    } else {
        "weak"
    };
    IpeResult::Ok(rating.to_string())
}

// ─── JWT kernels (HS256) ──────────────────────────────────────────────

/// Ipê `signToken : String -> a -> Int -> Result Error String`.
/// `signToken secret claims expirySeconds`. `claims` is a string-keyed map of
/// string values at the runtime level (Ipê's polymorphic `a` resolves to
/// HashMap<String,String> at the FFI boundary). Adds `exp` (now + expirySeconds)
/// and `iat` (now) claims, mirroring Go's Auth_signToken. Secret must be ≥32
/// bytes (matches Go's production gate).
pub fn auth_sign_token<E: From<String>>(
    secret: String,
    claims: HashMap<String, String>,
    expiry_seconds: i64,
) -> IpeResult<E, String> {
    if secret.len() < crate::jwt::HS256_MIN_SECRET_BYTES {
        return IpeResult::Err(
            crate::jwt::hs256_short_secret_msg("auth.signToken", secret.len()).into(),
        );
    }
    // A negative TTL must NOT mint a token. Without this guard a negative
    // `expiry_seconds` (e.g. i64::MIN) underflows `now + expiry_seconds`, and
    // the `unwrap_or(i64::MAX)` fallback would invert intent into a
    // never-expiring token. Reject up front so the safe outcome is the only
    // reachable one.
    if expiry_seconds < 0 {
        return IpeResult::Err(
            "auth.signToken: expiry_seconds must be non-negative"
                .to_string()
                .into(),
        );
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // Saturate to i64::MAX on overflow so a caller-controlled large
    // expiry_seconds never panics under debug overflow-checks.
    // i64::MAX is a far-future timestamp (~292 billion years) — a safe floor.
    let exp = now.checked_add(expiry_seconds).unwrap_or(i64::MAX);
    let iat = now; // issued-at = current unix seconds (mirrors Go's Auth_signToken)
    // Build a JSON object with string claims + exp + iat.
    let mut payload = serde_json::Map::new();
    for (k, v) in claims {
        payload.insert(k, serde_json::Value::String(v));
    }
    payload.insert("exp".to_string(), serde_json::Value::Number(exp.into()));
    payload.insert("iat".to_string(), serde_json::Value::Number(iat.into()));
    let value = serde_json::Value::Object(payload);
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());
    match jsonwebtoken::encode(&header, &value, &key) {
        Ok(t) => IpeResult::Ok(t),
        Err(e) => IpeResult::Err(format!("jwt encode: {}", e).into()),
    }
}

/// Ipê `verifyToken : String -> String -> Result Error a`. Verifies signature
/// and `exp`. Returns the claims as a `HashMap<String, String>` (Ipê-side
/// resolves polymorphic `a` to this shape at the FFI boundary).
pub fn auth_verify_token<E: From<String>>(
    secret: String,
    token: String,
) -> IpeResult<E, HashMap<String, String>> {
    if secret.len() < crate::jwt::HS256_MIN_SECRET_BYTES {
        return IpeResult::Err(
            crate::jwt::hs256_short_secret_msg("auth.verifyToken", secret.len()).into(),
        );
    }
    // Pre-reject on the full RFC 7519 NumericDate domain (negative, fractional,
    // integer) before jsonwebtoken's `exp - 1` u64 subtraction can underflow.
    // Mirrors jwt.rs's `jwt_decode_hs256` pre-reject; see that function's
    // comment for the detailed rationale.
    if let Some(payload) = crate::jwt::decode_payload(&token) {
        let now = crate::jwt::now_unix_seconds();
        if let Some(exp) = crate::jwt::numeric_date(&payload, "exp")
            && now >= exp
        {
            return IpeResult::Err("auth.verifyToken: token has expired".to_string().into());
        }
        if let Some(nbf) = crate::jwt::numeric_date(&payload, "nbf")
            && now < nbf
        {
            return IpeResult::Err(
                "auth.verifyToken: token is not yet valid"
                    .to_string()
                    .into(),
            );
        }
    }
    let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    // Go's oracle rejects at `now >= exp` with zero clock skew. jsonwebtoken's
    // native boundary with leeway = 0 is `exp < now` (accepts at the exact
    // instant now == exp); reject_tokens_expiring_in_less_than = 1 shifts the
    // reject condition to `exp - 1 < now` (≡ `now >= exp`), restoring parity.
    // The pre-reject above guards the underflow site for the full NumericDate
    // domain. nbf parity needs no shift (already identical at leeway 0). See
    // jwt.rs's longer comment on this exact mechanism.
    validation.leeway = 0;
    validation.reject_tokens_expiring_in_less_than = 1;
    validation.validate_exp = true;
    // Enforce the standard not-before window too. jsonwebtoken defaults
    // validate_nbf = false, so a token carrying a future `nbf` (e.g. a
    // scheduled/delayed-grant token minted elsewhere under the same secret)
    // would otherwise be accepted before its valid-from time. Matches the
    // documented Ipe.Jwt contract (signature + exp + nbf checked).
    validation.validate_nbf = true;
    // Auth.signToken accepts arbitrary claims (including an `aud` key) with
    // no expected-audience argument on this generic decoder. jsonwebtoken's
    // default `validate_aud = true` would then REJECT any token that merely
    // CARRIES an `aud` claim (InvalidAudience) — breaking a clean
    // sign-then-verify roundtrip of aud-bearing claims. Mirrors jwt.rs's
    // identical rationale.
    validation.validate_aud = false;
    let parsed = match jsonwebtoken::decode::<serde_json::Value>(&token, &key, &validation) {
        Ok(d) => d,
        Err(e) => return IpeResult::Err(format!("jwt verify: {}", e).into()),
    };
    let mut out = HashMap::new();
    if let serde_json::Value::Object(m) = parsed.claims {
        for (k, v) in m {
            // Coerce each claim value to a string. Numbers/booleans get
            // their JSON-text representation; nested objects/arrays get their
            // JSON serialisation (matches Go runtime's fmt.Sprintf behaviour).
            let s = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            out.insert(k, s);
        }
    }
    IpeResult::Ok(out)
}

// ─── DB-touching kernels ──────────────────────────────────────────────
// All three functions (`register`, `login`, `setRole`) take a `Db` connection
// and use `sqlx` directly. They are gated on `#[cfg(feature = "db")]` so a
// non-db project that imports `auth` for the pure-crypto tier (hashPassword,
// verifyPassword, signToken, verifyToken, passwordStrength) still compiles.
// When `db` is disabled `Db = ()` (from config.rs's non-db branch), so the
// call sites never have a real connection to pass — the generated code for
// `AuthRegister/Login/SetRole` is only emitted when the lowerer detects those
// kernel calls, which implies `uses_db = true` and `db` in default features.

#[cfg(feature = "db")]
/// Idempotent `CREATE TABLE IF NOT EXISTS users (...)`. Runs at the start of
/// register/login/setRole so the schema is always available without users
/// having to call a separate migration. The id-column DDL is per-driver
/// — `db_auto_id_column()` returns the right fragment for sqlite
/// (`INTEGER PRIMARY KEY AUTOINCREMENT`), mysql (`BIGINT NOT NULL
/// AUTO_INCREMENT PRIMARY KEY`), or postgres (`BIGSERIAL PRIMARY KEY`).
async fn ensure_users_schema<E: From<String> + Send>(conn: &Db) -> IpeResult<E, ()> {
    let schema = format!(
        "CREATE TABLE IF NOT EXISTS users (
            {},
            email TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'user',
            created_at BIGINT NOT NULL
        )",
        db_auto_id_column()
    );
    match sqlx::query(&schema).execute(conn).await {
        Ok(_) => IpeResult::Ok(()),
        Err(e) => IpeResult::Err(format!("auth.users schema: {}", e).into()),
    }
}

#[cfg(feature = "db")]
/// Ipê `register : Db -> String -> String -> Task Error Int`.
/// Creates a new user. Returns the new user id.
pub fn auth_register<E: Send + From<String> + 'static>(
    conn: Db,
    email: String,
    password: String,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        // Normalize the email (trim + lowercase) so a case/whitespace variant can't
        // create a DUPLICATE account that bypasses the UNIQUE constraint, and so
        // login matches regardless of the case the user types. Applied identically
        // in auth_login, so auth BEHAVIOUR is unchanged (login still succeeds); only
        // the stored case is canonical. (Email local-parts are technically case-
        // sensitive per RFC 5321, but every real provider treats them case-
        // insensitively; canonical-lowercase is the universal practice.)
        let email = email.trim().to_lowercase();
        if let IpeResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return IpeResult::Err(e);
        }
        // bcrypt is CPU-bound and BLOCKING (~250 ms at cost 12). Running it on a
        // tokio worker thread starves the async runtime (every concurrent register
        // ties up a core worker). Offload to the blocking pool.
        let hash =
            match tokio::task::spawn_blocking(move || auth_hash_password::<E>(password)).await {
                Ok(IpeResult::Ok(h)) => h,
                Ok(IpeResult::Err(e)) => return IpeResult::Err(e),
                Err(_) => {
                    return IpeResult::Err(
                        "auth.register: password-hash task failed"
                            .to_string()
                            .into(),
                    );
                }
            };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let sql = db_format_sql(
            "INSERT INTO users (email, password_hash, role, created_at) VALUES (?, ?, ?, ?)"
                .to_string(),
        );
        let result = sqlx::query(&sql)
            .bind(&email)
            .bind(&hash)
            .bind("user")
            .bind(now)
            .execute(&conn)
            .await;
        match result {
            Ok(res) => IpeResult::Ok(db_last_insert_id(&res)),
            Err(sqlx::Error::Database(de)) if de.is_unique_violation() => {
                IpeResult::Err("auth.register: email already registered".to_string().into())
            }
            Err(e) => IpeResult::Err(format!("auth.register: {}", e).into()),
        }
    })
}

#[cfg(feature = "db")]
/// Ipê `login : Db -> String -> String -> Task Error Int`.
/// Authenticates the user. Returns user id on success. Does NOT leak whether
/// the email exists vs. password was wrong — both paths return the same
/// generic "invalid credentials" error.
pub fn auth_login<E: Send + From<String> + 'static>(
    conn: Db,
    email: String,
    password: String,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        // Same canonicalisation as auth_register so a case/whitespace variant of a
        // registered email still logs in (and can't be used to probe the store).
        let email = email.trim().to_lowercase();
        if let IpeResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return IpeResult::Err(e);
        }
        let sql = db_format_sql("SELECT id, password_hash FROM users WHERE email = ?".to_string());
        match sqlx::query(&sql).bind(&email).fetch_optional(&conn).await {
            Ok(Some(row)) => {
                use sqlx::Row;
                // A failed id-column read MUST NOT silently default to user 0
                // (authenticating as the wrong/zero user). Fail closed instead.
                let id: i64 = match row.try_get(0) {
                    Ok(id) => id,
                    Err(_) => {
                        return IpeResult::Err(
                            "auth.login: invalid credentials".to_string().into(),
                        );
                    }
                };
                let hash: String = row.try_get(1).unwrap_or_default();
                // bcrypt::verify is CPU-bound + blocking → blocking pool (see register).
                let ok = tokio::task::spawn_blocking(move || {
                    bcrypt::verify(&password, &hash).unwrap_or(false)
                })
                .await
                .unwrap_or(false);
                if ok {
                    IpeResult::Ok(id)
                } else {
                    IpeResult::Err("auth.login: invalid credentials".to_string().into())
                }
            }
            Ok(None) => {
                // TIMING: perform an equal-cost bcrypt verify against a fixed
                // cost-12 hash so the unknown-email path does the same hashing
                // work as the known-email path — removing the email-enumeration
                // timing oracle. The result is discarded.
                let _ = tokio::task::spawn_blocking(move || {
                    bcrypt::verify(&password, dummy_bcrypt_hash())
                })
                .await;
                IpeResult::Err("auth.login: invalid credentials".to_string().into())
            }
            Err(e) => IpeResult::Err(format!("auth.login: {}", e).into()),
        }
    })
}

#[cfg(feature = "db")]
/// Ipê `setRole : Db -> Int -> String -> Task Error ()`.
/// Sets the user's role. No-op if the user doesn't exist (returns Ok).
pub fn auth_set_role<E: Send + From<String> + 'static>(
    conn: Db,
    user_id: i64,
    role: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        if let IpeResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return IpeResult::Err(e);
        }
        let sql = db_format_sql("UPDATE users SET role = ? WHERE id = ?".to_string());
        match sqlx::query(&sql)
            .bind(&role)
            .bind(user_id)
            .execute(&conn)
            .await
        {
            Ok(_) => IpeResult::Ok(()),
            Err(e) => IpeResult::Err(format!("auth.setRole: {}", e).into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // bcrypt cost 4 for fast tests (production uses 12).
    const TEST_COST: i64 = 4;

    #[test]
    fn test_hash_verify_roundtrip() {
        let hash: IpeResult<String, String> =
            auth_hash_password_cost("password123".into(), TEST_COST);
        let h = match hash {
            IpeResult::Ok(h) => h,
            _ => panic!("hash"),
        };
        let ok: IpeResult<String, bool> = auth_verify_password("password123".into(), h.clone());
        assert!(matches!(ok, IpeResult::Ok(true)));
        let bad: IpeResult<String, bool> = auth_verify_password("wrongpass".into(), h);
        assert!(matches!(bad, IpeResult::Ok(false)));
    }

    #[test]
    fn test_hash_too_short() {
        let r: IpeResult<String, String> = auth_hash_password("short".into());
        assert!(matches!(r, IpeResult::Err(_)));
    }

    #[test]
    fn test_password_strength() {
        // <8 chars → Err
        let r: IpeResult<String, String> = auth_password_strength("short".into());
        assert!(matches!(r, IpeResult::Err(_)));
        // All letters → Err
        let r: IpeResult<String, String> = auth_password_strength("abcdefghij".into());
        assert!(matches!(r, IpeResult::Err(_)));
        // All digits → Err
        let r: IpeResult<String, String> = auth_password_strength("1234567890".into());
        assert!(matches!(r, IpeResult::Err(_)));
        // 8 chars, letter+digit → weak
        let r: IpeResult<String, String> = auth_password_strength("abc12345".into());
        assert!(matches!(r, IpeResult::Ok(ref s) if s == "weak"));
        // 10 chars, letter+digit → medium
        let r: IpeResult<String, String> = auth_password_strength("abcde12345".into());
        assert!(matches!(r, IpeResult::Ok(ref s) if s == "medium"));
        // 12 chars + symbol → strong
        let r: IpeResult<String, String> = auth_password_strength("abc12345xyz!".into());
        assert!(matches!(r, IpeResult::Ok(ref s) if s == "strong"));
    }

    #[test]
    fn test_jwt_sign_verify_roundtrip() {
        // Secret must be ≥32 bytes
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let mut claims = HashMap::new();
        claims.insert("sub".to_string(), "user-123".to_string());
        claims.insert("role".to_string(), "admin".to_string());
        let token: IpeResult<String, String> = auth_sign_token(secret.clone(), claims, 3600);
        let t = match token {
            IpeResult::Ok(t) => t,
            _ => panic!("sign"),
        };
        let verified: IpeResult<String, HashMap<String, String>> = auth_verify_token(secret, t);
        match verified {
            IpeResult::Ok(m) => {
                assert_eq!(m.get("sub").unwrap(), "user-123");
                assert_eq!(m.get("role").unwrap(), "admin");
                assert!(m.contains_key("exp"));
                assert!(m.contains_key("iat")); // parity with Go's Auth_signToken
            }
            _ => panic!("verify"),
        }
    }

    #[test]
    fn test_jwt_short_secret_rejected() {
        let token: IpeResult<String, String> =
            auth_sign_token("short".into(), HashMap::new(), 3600);
        assert!(matches!(token, IpeResult::Err(_)));
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    // AUD-02 regression: mirror `jwt.rs`'s `test_hs256_expired_token_rejected`
    // boundary test for the `Auth` surface. `auth_sign_token` always computes
    // `exp = now + expiry_seconds` with `expiry_seconds >= 0` enforced, so a
    // past-`exp` token can't be minted through the public API — encode one
    // directly (same HS256 + JSON-claims shape `auth_sign_token` uses
    // internally) to exercise `auth_verify_token`'s `leeway = 0` guard.
    #[test]
    fn test_auth_verify_token_expired_30s_ago_rejected() {
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let claims = serde_json::json!({ "sub": "x", "exp": now_unix() - 30 });
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let key = jsonwebtoken::EncodingKey::from_secret(secret.as_bytes());
        let token = jsonwebtoken::encode(&header, &claims, &key).expect("encode");
        let verified: IpeResult<String, HashMap<String, String>> = auth_verify_token(secret, token);
        assert!(
            matches!(verified, IpeResult::Err(_)),
            "an Auth token expired 30s ago must be rejected (no clock-skew leeway)"
        );
    }

    // AUD-02 regression: claims containing an `aud` key must round-trip
    // through sign-then-verify. Pre-fix, `Validation`'s default
    // `validate_aud = true` rejected ANY token merely carrying an `aud`
    // claim (no expected-audience argument exists on this generic decoder),
    // breaking a clean roundtrip of aud-bearing claims.
    #[test]
    fn test_auth_verify_token_accepts_aud_bearing_claims() {
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let mut claims = HashMap::new();
        claims.insert("sub".to_string(), "user-123".to_string());
        claims.insert("aud".to_string(), "my-service".to_string());
        let token: IpeResult<String, String> = auth_sign_token(secret.clone(), claims, 3600);
        let t = match token {
            IpeResult::Ok(t) => t,
            IpeResult::Err(e) => panic!("sign: {e}"),
        };
        let verified: IpeResult<String, HashMap<String, String>> = auth_verify_token(secret, t);
        match verified {
            IpeResult::Ok(m) => {
                assert_eq!(m.get("aud").map(String::as_str), Some("my-service"));
            }
            IpeResult::Err(e) => panic!("verify must accept aud-bearing claims: {e}"),
        }
    }

    #[tokio::test]
    async fn test_email_normalized_case_insensitive() {
        let pool = match DbPool::connect("sqlite::memory:").await {
            Ok(p) => p,
            Err(_) => return, // in-memory connect can't realistically fail; skip if it does
        };
        // Register with mixed case + surrounding whitespace.
        let id: IpeResult<String, i64> = auth_register(
            pool.clone(),
            "  Alice@Example.COM ".into(),
            "hunter2!".into(),
        )
        .await;
        let uid = match id {
            IpeResult::Ok(i) => i,
            IpeResult::Err(_) => 0,
        };
        assert!(uid > 0, "register with mixed-case email should succeed");
        // Login with a DIFFERENT case must resolve to the SAME account.
        let login: IpeResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(
            matches!(login, IpeResult::Ok(u) if u == uid),
            "login must be case-insensitive"
        );
        // A case-variant re-register must hit the UNIQUE constraint (no dup account).
        let dup: IpeResult<String, i64> =
            auth_register(pool.clone(), "ALICE@example.com".into(), "hunter2!".into()).await;
        assert!(
            matches!(dup, IpeResult::Err(_)),
            "case-variant must not create a duplicate account"
        );
    }

    #[tokio::test]
    async fn test_register_login_flow() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        // register
        let id: IpeResult<String, i64> =
            auth_register(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        let user_id = match id {
            IpeResult::Ok(i) => i,
            IpeResult::Err(e) => panic!("{}", e),
        };
        assert!(user_id > 0);
        // login correct
        let login_ok: IpeResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(matches!(login_ok, IpeResult::Ok(uid) if uid == user_id));
        // login wrong password
        let login_bad: IpeResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "wrong".into()).await;
        assert!(matches!(login_bad, IpeResult::Err(_)));
        // login non-existent email
        let login_noexist: IpeResult<String, i64> =
            auth_login(pool.clone(), "nobody@example.com".into(), "anything".into()).await;
        assert!(matches!(login_noexist, IpeResult::Err(_)));
        // duplicate register
        let dup: IpeResult<String, i64> =
            auth_register(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(matches!(dup, IpeResult::Err(_)));
        // set role
        let role: IpeResult<String, ()> = auth_set_role(pool, user_id, "admin".into()).await;
        assert!(matches!(role, IpeResult::Ok(())));
    }

    #[test]
    fn test_sign_token_negative_expiry_rejected() {
        // A negative TTL must NOT mint a token (it would otherwise underflow
        // into a never-expiring token). Expect Err.
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let token: IpeResult<String, String> = auth_sign_token(secret, HashMap::new(), -1);
        assert!(
            matches!(token, IpeResult::Err(_)),
            "negative expiry must be rejected"
        );
        // i64::MIN (the pathological underflow case) must also be rejected.
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let token2: IpeResult<String, String> = auth_sign_token(secret, HashMap::new(), i64::MIN);
        assert!(matches!(token2, IpeResult::Err(_)));
    }

    #[tokio::test]
    async fn test_login_id_decode_failure_yields_err_not_user_zero() {
        let pool = match DbPool::connect("sqlite::memory:").await {
            Ok(p) => p,
            Err(_) => return,
        };
        // Pre-create the users table with a TEXT id so a row's id column will
        // FAIL to decode into i64. ensure_users_schema uses CREATE TABLE IF NOT
        // EXISTS, so it leaves this schema in place.
        sqlx::query(
            "CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create table");
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at) \
             VALUES ('not-a-number', 'badid@example.com', 'x', 'user', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert");
        // The matched-email branch reads id first; a decode failure must fail
        // closed (Err), NOT silently authenticate as user 0.
        let login: IpeResult<String, i64> =
            auth_login(pool, "badid@example.com".into(), "whatever".into()).await;
        assert!(
            matches!(login, IpeResult::Err(_)),
            "a failed id-column decode must yield Err, never Ok(0)"
        );
    }
}
