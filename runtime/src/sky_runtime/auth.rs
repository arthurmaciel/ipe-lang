//! Std.Auth kernels — authentication helpers.
//!
//! Two tiers (matches the Sky-side `Std.Auth` doc):
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
        bcrypt::hash("sky-login-timing-defence", 12).unwrap_or_else(|_| {
            "$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW".to_string()
        })
    })
}

// ─── Pure crypto kernels ──────────────────────────────────────────────

/// Sky `hashPassword : String -> Result Error String`. Bcrypt with default
/// cost 12 (matches Go runtime).
pub fn auth_hash_password<E: From<String>>(pw: String) -> SkyResult<E, String> {
    auth_hash_password_cost(pw, 12)
}

/// Sky `hashPasswordCost : String -> Int -> Result Error String`. Clamps cost to
/// [4, 15] (4 = fast for tests, 12 = production default, 14–15 = high security).
/// The bcrypt VALID range is [4, 31], but cost is caller-controlled: each +1
/// DOUBLES the work, so cost 31 is a single-call CPU-exhaustion DoS (~years per
/// hash). 15 (~1–2 s/hash) is a generous operational ceiling; higher is always a
/// self-DoS, so it is clamped down rather than honoured.
pub fn auth_hash_password_cost<E: From<String>>(pw: String, cost: i64) -> SkyResult<E, String> {
    if pw.chars().count() < 8 {
        return SkyResult::Err("password must be at least 8 characters".to_string().into());
    }
    if pw.len() > 72 {
        return SkyResult::Err(
            "password longer than 72 bytes (bcrypt limit)"
                .to_string()
                .into(),
        );
    }
    let clamped = cost.clamp(4, 15) as u32;
    match bcrypt::hash(&pw, clamped) {
        Ok(h) => SkyResult::Ok(h),
        Err(e) => SkyResult::Err(format!("bcrypt: {}", e).into()),
    }
}

/// Sky `verifyPassword : String -> String -> Result Error Bool`.
/// `verifyPassword candidate hash` — true if candidate hashes to the same hash.
pub fn auth_verify_password<E: From<String>>(pw: String, hash: String) -> SkyResult<E, bool> {
    match bcrypt::verify(&pw, &hash) {
        Ok(b) => SkyResult::Ok(b),
        Err(e) => SkyResult::Err(format!("bcrypt verify: {}", e).into()),
    }
}

/// Sky `passwordStrength : String -> Result Error String`. Validates length
/// and character variety; returns a strength rating on Ok.
///   <8 chars  → Err "too short"
///   >72 bytes → Err "too long" (bcrypt limit)
/// > all-letters or all-digits → Err "needs both letters and digits"
/// > ≥12 chars + letter + digit + symbol → "strong"
/// > ≥10 chars + letter + digit          → "medium"
/// > otherwise (passes letter+digit check) → "weak"
pub fn auth_password_strength<E: From<String>>(pw: String) -> SkyResult<E, String> {
    if pw.chars().count() < 8 {
        return SkyResult::Err("password must be at least 8 characters".to_string().into());
    }
    if pw.len() > 72 {
        return SkyResult::Err(
            "password longer than 72 bytes (bcrypt limit)"
                .to_string()
                .into(),
        );
    }
    let has_letter = pw.chars().any(|c| c.is_alphabetic());
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    let has_symbol = pw.chars().any(|c| !c.is_alphanumeric());
    if !has_letter || !has_digit {
        return SkyResult::Err(
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
    SkyResult::Ok(rating.to_string())
}

// ─── JWT kernels (HS256) ──────────────────────────────────────────────

/// Sky `signToken : String -> a -> Int -> Result Error String`.
/// `signToken secret claims expirySeconds`. `claims` is a string-keyed map of
/// string values at the runtime level (Sky's polymorphic `a` resolves to
/// HashMap<String,String> at the FFI boundary). Adds `exp` (now + expirySeconds)
/// and `iat` (now) claims, mirroring Go's Auth_signToken. Secret must be ≥32
/// bytes (matches Go's production gate).
pub fn auth_sign_token<E: From<String>>(
    secret: String,
    claims: HashMap<String, String>,
    expiry_seconds: i64,
) -> SkyResult<E, String> {
    if secret.len() < 32 {
        return SkyResult::Err(
            "auth.signToken: secret must be ≥32 bytes"
                .to_string()
                .into(),
        );
    }
    // A negative TTL must NOT mint a token. Without this guard a negative
    // `expiry_seconds` (e.g. i64::MIN) underflows `now + expiry_seconds`, and
    // the `unwrap_or(i64::MAX)` fallback would invert intent into a
    // never-expiring token. Reject up front so the safe outcome is the only
    // reachable one.
    if expiry_seconds < 0 {
        return SkyResult::Err(
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
        Ok(t) => SkyResult::Ok(t),
        Err(e) => SkyResult::Err(format!("jwt encode: {}", e).into()),
    }
}

/// Sky `verifyToken : String -> String -> Result Error a`. Verifies signature
/// and `exp`. Returns the claims as a `HashMap<String, String>` (Sky-side
/// resolves polymorphic `a` to this shape at the FFI boundary).
pub fn auth_verify_token<E: From<String>>(
    secret: String,
    token: String,
) -> SkyResult<E, HashMap<String, String>> {
    if secret.len() < 32 {
        return SkyResult::Err(
            "auth.verifyToken: secret must be ≥32 bytes"
                .to_string()
                .into(),
        );
    }
    let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    // Enforce the standard not-before window too. jsonwebtoken defaults
    // validate_nbf = false, so a token carrying a future `nbf` (e.g. a
    // scheduled/delayed-grant token minted elsewhere under the same secret)
    // would otherwise be accepted before its valid-from time. Matches the
    // documented Sky.Core.Jwt contract (signature + exp + nbf checked).
    validation.validate_nbf = true;
    let parsed = match jsonwebtoken::decode::<serde_json::Value>(&token, &key, &validation) {
        Ok(d) => d,
        Err(e) => return SkyResult::Err(format!("jwt verify: {}", e).into()),
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
    SkyResult::Ok(out)
}

// ─── DB-touching kernels ──────────────────────────────────────────────

/// Idempotent `CREATE TABLE IF NOT EXISTS users (...)`. Runs at the start of
/// register/login/setRole so the schema is always available without users
/// having to call a separate migration. The id-column DDL is per-driver
/// — `db_auto_id_column()` returns the right fragment for sqlite
/// (`INTEGER PRIMARY KEY AUTOINCREMENT`), mysql (`BIGINT NOT NULL
/// AUTO_INCREMENT PRIMARY KEY`), or postgres (`BIGSERIAL PRIMARY KEY`).
async fn ensure_users_schema<E: From<String> + Send>(conn: &Db) -> SkyResult<E, ()> {
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
        Ok(_) => SkyResult::Ok(()),
        Err(e) => SkyResult::Err(format!("auth.users schema: {}", e).into()),
    }
}

/// Sky `register : Db -> String -> String -> Task Error Int`.
/// Creates a new user. Returns the new user id.
pub fn auth_register<E: Send + From<String> + 'static>(
    conn: Db,
    email: String,
    password: String,
) -> SkyTask<E, i64> {
    Box::pin(async move {
        // Normalize the email (trim + lowercase) so a case/whitespace variant can't
        // create a DUPLICATE account that bypasses the UNIQUE constraint, and so
        // login matches regardless of the case the user types. Applied identically
        // in auth_login, so auth BEHAVIOUR is unchanged (login still succeeds); only
        // the stored case is canonical. (Email local-parts are technically case-
        // sensitive per RFC 5321, but every real provider treats them case-
        // insensitively; canonical-lowercase is the universal practice.)
        let email = email.trim().to_lowercase();
        if let SkyResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return SkyResult::Err(e);
        }
        // bcrypt is CPU-bound and BLOCKING (~250 ms at cost 12). Running it on a
        // tokio worker thread starves the async runtime (every concurrent register
        // ties up a core worker). Offload to the blocking pool.
        let hash =
            match tokio::task::spawn_blocking(move || auth_hash_password::<E>(password)).await {
                Ok(SkyResult::Ok(h)) => h,
                Ok(SkyResult::Err(e)) => return SkyResult::Err(e),
                Err(_) => {
                    return SkyResult::Err(
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
            Ok(res) => SkyResult::Ok(db_last_insert_id(&res)),
            Err(sqlx::Error::Database(de)) if de.is_unique_violation() => {
                SkyResult::Err("auth.register: email already registered".to_string().into())
            }
            Err(e) => SkyResult::Err(format!("auth.register: {}", e).into()),
        }
    })
}

/// Sky `login : Db -> String -> String -> Task Error Int`.
/// Authenticates the user. Returns user id on success. Does NOT leak whether
/// the email exists vs. password was wrong — both paths return the same
/// generic "invalid credentials" error.
pub fn auth_login<E: Send + From<String> + 'static>(
    conn: Db,
    email: String,
    password: String,
) -> SkyTask<E, i64> {
    Box::pin(async move {
        // Same canonicalisation as auth_register so a case/whitespace variant of a
        // registered email still logs in (and can't be used to probe the store).
        let email = email.trim().to_lowercase();
        if let SkyResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return SkyResult::Err(e);
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
                        return SkyResult::Err(
                            "auth.login: invalid credentials".to_string().into(),
                        )
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
                    SkyResult::Ok(id)
                } else {
                    SkyResult::Err("auth.login: invalid credentials".to_string().into())
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
                SkyResult::Err("auth.login: invalid credentials".to_string().into())
            }
            Err(e) => SkyResult::Err(format!("auth.login: {}", e).into()),
        }
    })
}

/// Sky `setRole : Db -> Int -> String -> Task Error ()`.
/// Sets the user's role. No-op if the user doesn't exist (returns Ok).
pub fn auth_set_role<E: Send + From<String> + 'static>(
    conn: Db,
    user_id: i64,
    role: String,
) -> SkyTask<E, ()> {
    Box::pin(async move {
        if let SkyResult::Err(e) = ensure_users_schema::<E>(&conn).await {
            return SkyResult::Err(e);
        }
        let sql = db_format_sql("UPDATE users SET role = ? WHERE id = ?".to_string());
        match sqlx::query(&sql)
            .bind(&role)
            .bind(user_id)
            .execute(&conn)
            .await
        {
            Ok(_) => SkyResult::Ok(()),
            Err(e) => SkyResult::Err(format!("auth.setRole: {}", e).into()),
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
        let hash: SkyResult<String, String> =
            auth_hash_password_cost("password123".into(), TEST_COST);
        let h = match hash {
            SkyResult::Ok(h) => h,
            _ => panic!("hash"),
        };
        let ok: SkyResult<String, bool> = auth_verify_password("password123".into(), h.clone());
        assert!(matches!(ok, SkyResult::Ok(true)));
        let bad: SkyResult<String, bool> = auth_verify_password("wrongpass".into(), h);
        assert!(matches!(bad, SkyResult::Ok(false)));
    }

    #[test]
    fn test_hash_too_short() {
        let r: SkyResult<String, String> = auth_hash_password("short".into());
        assert!(matches!(r, SkyResult::Err(_)));
    }

    #[test]
    fn test_password_strength() {
        // <8 chars → Err
        let r: SkyResult<String, String> = auth_password_strength("short".into());
        assert!(matches!(r, SkyResult::Err(_)));
        // All letters → Err
        let r: SkyResult<String, String> = auth_password_strength("abcdefghij".into());
        assert!(matches!(r, SkyResult::Err(_)));
        // All digits → Err
        let r: SkyResult<String, String> = auth_password_strength("1234567890".into());
        assert!(matches!(r, SkyResult::Err(_)));
        // 8 chars, letter+digit → weak
        let r: SkyResult<String, String> = auth_password_strength("abc12345".into());
        assert!(matches!(r, SkyResult::Ok(ref s) if s == "weak"));
        // 10 chars, letter+digit → medium
        let r: SkyResult<String, String> = auth_password_strength("abcde12345".into());
        assert!(matches!(r, SkyResult::Ok(ref s) if s == "medium"));
        // 12 chars + symbol → strong
        let r: SkyResult<String, String> = auth_password_strength("abc12345xyz!".into());
        assert!(matches!(r, SkyResult::Ok(ref s) if s == "strong"));
    }

    #[test]
    fn test_jwt_sign_verify_roundtrip() {
        // Secret must be ≥32 bytes
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let mut claims = HashMap::new();
        claims.insert("sub".to_string(), "user-123".to_string());
        claims.insert("role".to_string(), "admin".to_string());
        let token: SkyResult<String, String> = auth_sign_token(secret.clone(), claims, 3600);
        let t = match token {
            SkyResult::Ok(t) => t,
            _ => panic!("sign"),
        };
        let verified: SkyResult<String, HashMap<String, String>> = auth_verify_token(secret, t);
        match verified {
            SkyResult::Ok(m) => {
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
        let token: SkyResult<String, String> =
            auth_sign_token("short".into(), HashMap::new(), 3600);
        assert!(matches!(token, SkyResult::Err(_)));
    }

    #[tokio::test]
    async fn test_email_normalized_case_insensitive() {
        let pool = match DbPool::connect("sqlite::memory:").await {
            Ok(p) => p,
            Err(_) => return, // in-memory connect can't realistically fail; skip if it does
        };
        // Register with mixed case + surrounding whitespace.
        let id: SkyResult<String, i64> = auth_register(
            pool.clone(),
            "  Alice@Example.COM ".into(),
            "hunter2!".into(),
        )
        .await;
        let uid = match id {
            SkyResult::Ok(i) => i,
            SkyResult::Err(_) => 0,
        };
        assert!(uid > 0, "register with mixed-case email should succeed");
        // Login with a DIFFERENT case must resolve to the SAME account.
        let login: SkyResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(
            matches!(login, SkyResult::Ok(u) if u == uid),
            "login must be case-insensitive"
        );
        // A case-variant re-register must hit the UNIQUE constraint (no dup account).
        let dup: SkyResult<String, i64> =
            auth_register(pool.clone(), "ALICE@example.com".into(), "hunter2!".into()).await;
        assert!(
            matches!(dup, SkyResult::Err(_)),
            "case-variant must not create a duplicate account"
        );
    }

    #[tokio::test]
    async fn test_register_login_flow() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        // register
        let id: SkyResult<String, i64> =
            auth_register(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        let user_id = match id {
            SkyResult::Ok(i) => i,
            SkyResult::Err(e) => panic!("{}", e),
        };
        assert!(user_id > 0);
        // login correct
        let login_ok: SkyResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(matches!(login_ok, SkyResult::Ok(uid) if uid == user_id));
        // login wrong password
        let login_bad: SkyResult<String, i64> =
            auth_login(pool.clone(), "alice@example.com".into(), "wrong".into()).await;
        assert!(matches!(login_bad, SkyResult::Err(_)));
        // login non-existent email
        let login_noexist: SkyResult<String, i64> =
            auth_login(pool.clone(), "nobody@example.com".into(), "anything".into()).await;
        assert!(matches!(login_noexist, SkyResult::Err(_)));
        // duplicate register
        let dup: SkyResult<String, i64> =
            auth_register(pool.clone(), "alice@example.com".into(), "hunter2!".into()).await;
        assert!(matches!(dup, SkyResult::Err(_)));
        // set role
        let role: SkyResult<String, ()> = auth_set_role(pool, user_id, "admin".into()).await;
        assert!(matches!(role, SkyResult::Ok(())));
    }

    #[test]
    fn test_sign_token_negative_expiry_rejected() {
        // A negative TTL must NOT mint a token (it would otherwise underflow
        // into a never-expiring token). Expect Err.
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let token: SkyResult<String, String> =
            auth_sign_token(secret, HashMap::new(), -1);
        assert!(
            matches!(token, SkyResult::Err(_)),
            "negative expiry must be rejected"
        );
        // i64::MIN (the pathological underflow case) must also be rejected.
        let secret = "a-test-secret-of-32-bytes-padding".to_string();
        let token2: SkyResult<String, String> =
            auth_sign_token(secret, HashMap::new(), i64::MIN);
        assert!(matches!(token2, SkyResult::Err(_)));
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
        let login: SkyResult<String, i64> =
            auth_login(pool, "badid@example.com".into(), "whatever".into()).await;
        assert!(
            matches!(login, SkyResult::Err(_)),
            "a failed id-column decode must yield Err, never Ok(0)"
        );
    }
}
