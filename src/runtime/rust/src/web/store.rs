//! Session stores — the `SessionStore` abstraction + backends, mirroring Go's
//! `runtime-go/rt/live_store.go`.
//!
//! A session's LIVE state (the tokio driver, SSE channel, rebuilt `HandlerIndex`)
//! is always per-process. A persistent backend additionally keeps a serialized
//! **checkpoint** of the model (+ metadata) so a returning cookie / a restart can
//! reconstruct the session. `get` therefore returns either a `Web` handle (the
//! in-process session, owns its driver) or a `Cold` model (decoded from the
//! checkpoint; the caller spawns a fresh driver seeded with it).

use super::SessionEntry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Wire-format epoch for the Model schema tag (H24). Must equal the
/// backend's `emit_model_schema::WIRE_EPOCH` — the epoch is folded into the
/// compile-time `IPE_LIVE_MODEL_SCHEMA_TAG` each generated Ipe.Web binary
/// carries. Bumped ONLY when the tag framing / blob encoding itself changes
/// shape (domain-separation convention), never for a Model change — the
/// Model's own shape is covered by the structural half of the hash.
pub const LIVE_MODEL_SCHEMA_WIRE_VERSION: &str = "ipe-live-model-schema-v1";

/// Encode one Model checkpoint as `base64(schema_tag(32) ++ bincode(model))`
/// — self-contained (tag travels inside the blob), TEXT-column-safe on every
/// backend (base64 never emits NUL / invalid UTF-8, so no `ALTER TABLE` or
/// BYTEA migration is ever needed). `None` when serialization fails (the
/// caller skips the checkpoint write, same as the old JSON path's `if let Ok`).
#[cfg(any(feature = "db", feature = "redis_store"))]
fn encode_checkpoint<Model: serde::Serialize>(
    schema_tag: &[u8; 32],
    model: &Model,
) -> Option<String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    let body = bincode::serialize(model).ok()?;
    let mut framed = Vec::with_capacity(32 + body.len());
    framed.extend_from_slice(schema_tag);
    framed.extend_from_slice(&body);
    Some(B64.encode(framed))
}

/// Decode one persisted checkpoint: base64 → split the leading 32-byte tag →
/// reject on mismatch BEFORE deserializing (H24) → bincode-decode the body.
/// EVERY failure (bad base64 — including a pre-Stage-C JSON row —, short
/// blob, foreign tag, corrupt body) is `None`: the same fail-soft
/// drop-session/fresh-`init` path H22 guarantees, never a panic.
#[cfg(any(feature = "db", feature = "redis_store"))]
fn decode_checkpoint<Model: serde::de::DeserializeOwned>(
    schema_tag: &[u8; 32],
    blob: &str,
) -> Option<Model> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    let framed = B64.decode(blob.as_bytes()).ok()?;
    let tag = framed.get(..32)?;
    if tag != schema_tag {
        return None;
    }
    let body = framed.get(32..)?;
    bincode::deserialize(body).ok()
}

/// The in-process live session (owns its driver goroutine + SSE channel).
pub type SessionHandle<Model, Msg> = Arc<Mutex<SessionEntry<Model, Msg>>>;

/// Result of a store lookup. `Web` = the in-process session (reuse it). `Cold`
/// = a model decoded from a persistent checkpoint (the caller hydrates: spawn a
/// fresh driver seeded with this model). Memory stores only ever return `Web`.
pub enum StoreHit<Model, Msg> {
    Live(SessionHandle<Model, Msg>),
    Cold(Model),
}

/// Async so persistent backends (sqlite/postgres via sqlx, redis) can do I/O;
/// memory impls have sync bodies. The driver + axum handlers are already async,
/// so call sites just `.await`.
#[async_trait]
pub trait SessionStore<Model, Msg>: Send + Sync {
    /// Look up a session by sid. `None` = unknown (caller creates a new one).
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>>;
    /// Insert/refresh the live handle (and, for persistent backends, checkpoint
    /// the model). Called on session create and write-through on every commit.
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>);
    /// Drop a session.
    async fn delete(&self, sid: &str);
    /// Evict idle-expired sessions (called periodically by the eviction task).
    async fn sweep(&self) {}

    /// Every session handle THIS PROCESS currently holds live (i.e. has an
    /// in-memory driver + possibly an open SSE connection). Deliberately
    /// scoped to the LOCAL mem-cache, never the full persisted table: a
    /// `Cold` row on disk (another replica's session, or one this process
    /// simply hasn't touched yet) has no SSE connection in THIS process to
    /// push anything to, so it is out of scope for what this method is for.
    /// Returns handles directly (not bare sids) — the caller
    /// (`push_reload_to_live_sessions`) needs each handle's `sse_tx` and
    /// would otherwise have to re-`get()` every id, opening a TOCTOU-ish gap
    /// where a session evicted between the enumerate and the re-fetch is
    /// silently skipped OR (worse) touches its TTL a second time for no
    /// reason. No default body (unlike `sweep`) — every backend has an
    /// opinion; a future backend without an in-process cache must make an
    /// explicit, reviewed choice, not silently inherit a possibly-wrong one.
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>>;
}

// ─── Memory store — default; in-process, lost on restart (Go memoryStore) ────

/// In-process store with idle-TTL eviction. `get` touches the entry's last-seen
/// so active sessions don't expire.
/// In-process session table: sid → (live handle, last-seen instant).
type SessionMap<Model, Msg> = HashMap<String, (SessionHandle<Model, Msg>, Instant)>;

pub struct MemoryStore<Model, Msg> {
    sessions: RwLock<SessionMap<Model, Msg>>,
    ttl: Duration,
}

impl<Model, Msg> MemoryStore<Model, Msg> {
    pub fn new(ttl: Duration) -> Self {
        MemoryStore {
            sessions: RwLock::new(HashMap::new()),
            ttl,
        }
    }
}

#[async_trait]
impl<Model: Send + 'static, Msg: Send + 'static> SessionStore<Model, Msg>
    for MemoryStore<Model, Msg>
{
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>> {
        let mut w = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        w.get_mut(sid).map(|(h, seen)| {
            *seen = Instant::now(); // touch — keep active sessions alive
            StoreHit::Live(h.clone())
        })
    }
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>) {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sid.to_string(), (handle, Instant::now()));
    }
    async fn delete(&self, sid: &str) {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sid);
    }
    async fn sweep(&self) {
        let now = Instant::now();
        let ttl = self.ttl;
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, seen)| now.duration_since(*seen) <= ttl);
    }
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// Used only by the sqlx-backed Sqlite/Postgres session stores (all
// `#[cfg(feature = "db")]`); the memory + redis stores don't call it, so a
// memory-only live build (no db) would orphan it.
#[cfg(feature = "db")]
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─── SQLite store — persistent model checkpoint + live mem-cache (Go sqliteStore)

/// Persistent store: keeps a `mem_cache` of live handles (same-process, owns the
/// driver) AND a `ipe_sessions(sid, blob, last_seen)` table holding the
/// serde-JSON model checkpoint. `get` returns the live handle on a cache hit,
/// else a `Cold` model decoded from the blob (the caller hydrates a fresh
/// driver). Requires `Model: Serialize + DeserializeOwned` (the codegen derives
/// it). Mirrors Go's `sqliteStore`.
#[cfg(feature = "db")]
pub struct SqliteStore<Model, Msg> {
    pool: sqlx::SqlitePool,
    mem_cache: RwLock<SessionMap<Model, Msg>>,
    ttl: Duration,
    /// The live process's Model schema tag (H24): a checkpoint row whose
    /// stored tag differs is rejected BEFORE deserialization — treated
    /// identically to "no row" (fail-soft to a fresh `init`).
    schema_tag: [u8; 32],
}

#[cfg(feature = "db")]
impl<Model, Msg> SqliteStore<Model, Msg> {
    pub async fn new(path: &str, ttl: Duration, schema_tag: [u8; 32]) -> Result<Self, sqlx::Error> {
        let url = format!("sqlite:{path}?mode=rwc");
        let pool = sqlx::SqlitePool::connect(&url).await?;
        // A pre-existing table from before the schema-tag column existed is
        // left as-is by IF NOT EXISTS; statements referencing the missing
        // column then error and are swallowed by the callers' existing
        // best-effort handling — the store degrades fail-soft (sessions
        // restart fresh), never crashes and never mis-decodes.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ipe_sessions (\
             sid TEXT PRIMARY KEY, blob TEXT NOT NULL, last_seen INTEGER NOT NULL, \
             schema_tag TEXT NOT NULL)",
        )
        .execute(&pool)
        .await?;
        Ok(SqliteStore {
            pool,
            mem_cache: RwLock::new(HashMap::new()),
            ttl,
            schema_tag,
        })
    }
}

#[cfg(feature = "db")]
#[async_trait]
impl<Model, Msg> SessionStore<Model, Msg> for SqliteStore<Model, Msg>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>> {
        // Same-process live handle wins (owns the running driver).
        let cached = {
            let mut w = self.mem_cache.write().unwrap_or_else(|e| e.into_inner());
            w.get_mut(sid).map(|(h, seen)| {
                *seen = Instant::now(); // touch — keep active sessions in cache
                h.clone()
            })
        };
        if let Some(h) = cached {
            let _ = sqlx::query("UPDATE ipe_sessions SET last_seen = ? WHERE sid = ?")
                .bind(now_secs())
                .bind(sid)
                .execute(&self.pool)
                .await;
            return Some(StoreHit::Live(h));
        }
        // Cold: decode the persisted model checkpoint (post-restart / other
        // replica). The blob is self-contained (base64(tag ++ bincode)); the
        // leading 32-byte tag is compared BEFORE deserialization (H24) — a
        // mismatch, an old-format JSON row, or a corrupt body all take the
        // same fail-soft miss path. The legacy schema_tag COLUMN is still
        // written (NOT NULL) but no longer read.
        let row: Option<(String,)> = sqlx::query_as("SELECT blob FROM ipe_sessions WHERE sid = ?")
            .bind(sid)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        let model: Model = decode_checkpoint(&self.schema_tag, &row?.0)?;
        let _ = sqlx::query("UPDATE ipe_sessions SET last_seen = ? WHERE sid = ?")
            .bind(now_secs())
            .bind(sid)
            .execute(&self.pool)
            .await;
        Some(StoreHit::Cold(model))
    }
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>) {
        let model = handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model
            .clone();
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sid.to_string(), (handle, Instant::now()));
        if let Some(blob) = encode_checkpoint(&self.schema_tag, &model) {
            let _ = sqlx::query(
                "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(sid) DO UPDATE SET blob=excluded.blob, \
                 last_seen=excluded.last_seen, schema_tag=excluded.schema_tag",
            )
            .bind(sid)
            .bind(blob)
            .bind(now_secs())
            .bind(hex::encode(self.schema_tag))
            .execute(&self.pool)
            .await;
        }
    }
    async fn delete(&self, sid: &str) {
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sid);
        let _ = sqlx::query("DELETE FROM ipe_sessions WHERE sid = ?")
            .bind(sid)
            .execute(&self.pool)
            .await;
    }
    async fn sweep(&self) {
        // Total cutoff: an absurd `IPE_LIVE_TTL` (u64 near 2^63) would make a bare
        // `now_secs() - (ttl as i64)` debug-panic / wrap-to-negative (caller-controlled
        // arithmetic). `try_from` → i64::MAX on overflow, then saturating_sub clamps,
        // so an oversized TTL degrades to "never expire" instead of faulting. For all
        // realistic TTLs this is byte-identical to the old expression.
        let cutoff =
            now_secs().saturating_sub(i64::try_from(self.ttl.as_secs()).unwrap_or(i64::MAX));
        let _ = sqlx::query("DELETE FROM ipe_sessions WHERE last_seen < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await;
        // Bound the in-RAM handle cache by idle-TTL too. Without this, every
        // distinct sid ever seen (e.g. a flood of cookie-less requests) leaves a
        // live handle in mem_cache forever → unbounded growth → OOM (session-DoS).
        // An evicted-but-still-valid session simply re-hydrates Cold from the
        // checkpoint blob on its next request.
        let now = Instant::now();
        let ttl = self.ttl;
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, seen)| now.duration_since(*seen) <= ttl);
    }
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// ─── Postgres store — multi-instance deployments (Go postgresStore) ──────────

/// Same shape as `SqliteStore` (mem-cache of live handles + a `ipe_sessions`
/// blob table + idle-TTL sweep) but over a `PgPool`, for horizontally-scaled
/// deployments (Cloud Run / ECS / k8s) where a returning request can land on a
/// different replica than the one that created the session. `connStr` is a
/// `postgres://user:pass@host/db` URL. Mirrors Go's `postgresStore`.
#[cfg(feature = "db")]
pub struct PostgresStore<Model, Msg> {
    pool: sqlx::PgPool,
    mem_cache: RwLock<SessionMap<Model, Msg>>,
    ttl: Duration,
    /// See [`SqliteStore::schema_tag`] — same H24 reject-before-deserialize gate.
    schema_tag: [u8; 32],
}

#[cfg(feature = "db")]
impl<Model, Msg> PostgresStore<Model, Msg> {
    pub async fn new(
        conn_str: &str,
        ttl: Duration,
        schema_tag: [u8; 32],
    ) -> Result<Self, sqlx::Error> {
        let pool = sqlx::PgPool::connect(conn_str).await?;
        // Pre-existing tables keep their old column set (IF NOT EXISTS) —
        // same fail-soft degradation as SqliteStore::new.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ipe_sessions (\
             sid TEXT PRIMARY KEY, blob TEXT NOT NULL, last_seen BIGINT NOT NULL, \
             schema_tag TEXT NOT NULL)",
        )
        .execute(&pool)
        .await?;
        Ok(PostgresStore {
            pool,
            mem_cache: RwLock::new(HashMap::new()),
            ttl,
            schema_tag,
        })
    }
}

#[cfg(feature = "db")]
#[async_trait]
impl<Model, Msg> SessionStore<Model, Msg> for PostgresStore<Model, Msg>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>> {
        let cached = {
            let mut w = self.mem_cache.write().unwrap_or_else(|e| e.into_inner());
            w.get_mut(sid).map(|(h, seen)| {
                *seen = Instant::now(); // touch — keep active sessions in cache
                h.clone()
            })
        };
        if let Some(h) = cached {
            let _ = sqlx::query("UPDATE ipe_sessions SET last_seen = $1 WHERE sid = $2")
                .bind(now_secs())
                .bind(sid)
                .execute(&self.pool)
                .await;
            return Some(StoreHit::Live(h));
        }
        // Self-contained framed blob — see SqliteStore::get. The legacy
        // schema_tag COLUMN is still written (NOT NULL) but no longer read.
        let row: Option<(String,)> = sqlx::query_as("SELECT blob FROM ipe_sessions WHERE sid = $1")
            .bind(sid)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
        let model: Model = decode_checkpoint(&self.schema_tag, &row?.0)?;
        let _ = sqlx::query("UPDATE ipe_sessions SET last_seen = $1 WHERE sid = $2")
            .bind(now_secs())
            .bind(sid)
            .execute(&self.pool)
            .await;
        Some(StoreHit::Cold(model))
    }
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>) {
        let model = handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model
            .clone();
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sid.to_string(), (handle, Instant::now()));
        if let Some(blob) = encode_checkpoint(&self.schema_tag, &model) {
            let _ = sqlx::query(
                "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (sid) DO UPDATE SET blob = EXCLUDED.blob, \
                 last_seen = EXCLUDED.last_seen, schema_tag = EXCLUDED.schema_tag",
            )
            .bind(sid)
            .bind(blob)
            .bind(now_secs())
            .bind(hex::encode(self.schema_tag))
            .execute(&self.pool)
            .await;
        }
    }
    async fn delete(&self, sid: &str) {
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sid);
        let _ = sqlx::query("DELETE FROM ipe_sessions WHERE sid = $1")
            .bind(sid)
            .execute(&self.pool)
            .await;
    }
    async fn sweep(&self) {
        // Total cutoff: an absurd `IPE_LIVE_TTL` (u64 near 2^63) would make a bare
        // `now_secs() - (ttl as i64)` debug-panic / wrap-to-negative (caller-controlled
        // arithmetic). `try_from` → i64::MAX on overflow, then saturating_sub clamps,
        // so an oversized TTL degrades to "never expire" instead of faulting. For all
        // realistic TTLs this is byte-identical to the old expression.
        let cutoff =
            now_secs().saturating_sub(i64::try_from(self.ttl.as_secs()).unwrap_or(i64::MAX));
        let _ = sqlx::query("DELETE FROM ipe_sessions WHERE last_seen < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await;
        // Bound the in-RAM handle cache by idle-TTL too (see SqliteStore::sweep)
        // — otherwise a cookie-less request flood grows mem_cache without bound.
        let now = Instant::now();
        let ttl = self.ttl;
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, seen)| now.duration_since(*seen) <= ttl);
    }
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// ─── Redis store — multi-instance, native TTL, no sweep (Go redisStore) ───────

/// Namespace session ids under a fixed prefix (Go `redisKey`).
#[cfg(feature = "redis_store")]
fn redis_key(sid: &str) -> String {
    format!("ipe:sess:{sid}")
}

/// Cross-instance store backed by Redis. Sessions live under `ipe:sess:<sid>`
/// as a HASH (`blob` = the serde-JSON checkpoint, `tag` = the hex Model schema
/// tag — one key, one native Redis TTL, so the tag and blob can never expire
/// out of sync). Expiry is the server's job; there's no sweep loop for the
/// persisted side. A `mem_cache` keeps the same-process live handle (owns the
/// driver) so a hit on the originating replica reuses it. `addr` is a full
/// `redis://[:pass@]host:port/db` URL or a bare `host:port`. Mirrors Go's
/// `redisStore` plus the H24 schema-tag gate.
#[cfg(feature = "redis_store")]
pub struct RedisStore<Model, Msg> {
    conn: redis::aio::MultiplexedConnection,
    mem_cache: RwLock<SessionMap<Model, Msg>>,
    ttl_secs: u64,
    /// See [`SqliteStore::schema_tag`] — same H24 reject-before-deserialize gate.
    schema_tag: [u8; 32],
}

#[cfg(feature = "redis_store")]
impl<Model, Msg> RedisStore<Model, Msg> {
    pub async fn new(
        addr: &str,
        ttl: Duration,
        schema_tag: [u8; 32],
    ) -> Result<Self, redis::RedisError> {
        let client = if addr.contains("://") {
            redis::Client::open(addr)?
        } else {
            redis::Client::open(format!("redis://{addr}"))?
        };
        let mut conn = client.get_multiplexed_async_connection().await?;
        // Ping so a misconfigured URL fails at startup, not on first write.
        redis::cmd("PING").query_async::<()>(&mut conn).await?;
        Ok(RedisStore {
            conn,
            mem_cache: RwLock::new(HashMap::new()),
            ttl_secs: ttl.as_secs().max(1),
            schema_tag,
        })
    }
}

#[cfg(feature = "redis_store")]
#[async_trait]
impl<Model, Msg> SessionStore<Model, Msg> for RedisStore<Model, Msg>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>> {
        use redis::AsyncCommands;
        let cached = {
            let mut w = self.mem_cache.write().unwrap_or_else(|e| e.into_inner());
            w.get_mut(sid).map(|(h, seen)| {
                *seen = Instant::now(); // touch — keep active sessions in cache
                h.clone()
            })
        };
        let mut conn = self.conn.clone();
        if let Some(h) = cached {
            // Touch native TTL so an active session doesn't expire mid-conversation.
            let _: Result<(), _> = conn.expire(redis_key(sid), self.ttl_secs as i64).await;
            return Some(StoreHit::Live(h));
        }
        // The session HASH's blob field is the self-contained framed
        // checkpoint (see SqliteStore::get); a pre-HASH string key errs
        // WRONGTYPE → `.ok()?` → the same fail-soft miss path. The legacy
        // companion `tag` field is retired (no longer written or read).
        let blob: Option<String> = redis::cmd("HGET")
            .arg(redis_key(sid))
            .arg("blob")
            .query_async(&mut conn)
            .await
            .ok()?;
        let model: Model = decode_checkpoint(&self.schema_tag, &blob?)?;
        let _: Result<(), _> = conn.expire(redis_key(sid), self.ttl_secs as i64).await;
        Some(StoreHit::Cold(model))
    }
    async fn set(&self, sid: &str, handle: SessionHandle<Model, Msg>) {
        let model = handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model
            .clone();
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sid.to_string(), (handle, Instant::now()));
        if let Some(blob) = encode_checkpoint(&self.schema_tag, &model) {
            let mut conn = self.conn.clone();
            // HASH per session, one key + one TTL; the tag lives INSIDE the
            // framed blob, so nothing can drift apart.
            let key = redis_key(sid);
            let _: Result<(), _> = redis::pipe()
                .cmd("HSET")
                .arg(&key)
                .arg("blob")
                .arg(blob)
                .ignore()
                .cmd("EXPIRE")
                .arg(&key)
                .arg(self.ttl_secs)
                .ignore()
                .query_async(&mut conn)
                .await;
        }
    }
    async fn delete(&self, sid: &str) {
        use redis::AsyncCommands;
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sid);
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.del(redis_key(sid)).await;
    }
    async fn sweep(&self) {
        // Redis evicts the persisted blob natively, but the in-RAM handle cache
        // still needs idle-TTL eviction — otherwise a cookie-less request flood
        // grows mem_cache without bound → OOM (session-DoS). An evicted-but-valid
        // session re-hydrates Cold from Redis on its next request.
        let now = Instant::now();
        let ttl = Duration::from_secs(self.ttl_secs);
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, seen)| now.duration_since(*seen) <= ttl);
    }
    async fn live_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

/// Select a backend from `[live] store` (Go `chooseStore`), falling back to
/// memory on any error — never crash. The `Model: Serialize` bound is for the
/// persistent backends; memory needs none, but a single signature keeps the
/// codegen call uniform (it derives serde on the model when emitting this).
/// `schema_tag` (the compile-time Model schema fingerprint, H24) is forwarded
/// to the persistent backends only — memory never round-trips through bytes.
pub async fn choose_store<Model, Msg>(
    kind: &str,
    path: &str,
    ttl: Duration,
    schema_tag: [u8; 32],
) -> Arc<dyn SessionStore<Model, Msg>>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    #[cfg(feature = "db")]
    if kind == "sqlite" {
        match SqliteStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: sqlite @ {path}");
                return Arc::new(s);
            }
            Err(e) => {
                eprintln!("[ipe.live] sqlite store unavailable ({e}); falling back to memory")
            }
        }
    }
    #[cfg(feature = "db")]
    if kind == "postgres" {
        match PostgresStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: postgres");
                return Arc::new(s);
            }
            Err(e) => {
                eprintln!("[ipe.live] postgres store unavailable ({e}); falling back to memory")
            }
        }
    }
    #[cfg(feature = "redis_store")]
    if kind == "redis" {
        match RedisStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: redis");
                return Arc::new(s);
            }
            Err(e) => eprintln!("[ipe.live] redis store unavailable ({e}); falling back to memory"),
        }
    }
    let _ = (kind, path, schema_tag);
    // Go parity (live_store.go:1032): the memory store logs through Go's `log`
    // package, so the line carries a `log.LstdFlags` timestamp prefix and a
    // Go-`Duration.String()` ttl (`1h0m0s` / `30m0s`). The persistent stores
    // above use a bare `eprintln!` (matching Go's `log.Printf` there too, minus
    // the duration), so only memory needs the duration + timestamp shape.
    eprintln!("{}", memory_store_log_line(ttl));
    Arc::new(MemoryStore::new(ttl))
}

/// The exact `[ipe.live] session store: memory (ttl=…)` line Go emits, with a
/// Go `log.LstdFlags` timestamp prefix and a Go-`Duration.String()` ttl. Shared
/// so the in-process console sub-app mount can emit the SAME second line Go's
/// console sub-app store init produces (Go prints this line TWICE: root + console).
pub(crate) fn memory_store_log_line(ttl: Duration) -> String {
    format!(
        "{} [ipe.live] session store: memory (ttl={})",
        go_log_timestamp(),
        go_duration_string(ttl)
    )
}

/// Render a `chrono::Local` now as Go's `log.LstdFlags` prefix: `2006/01/02 15:04:05`.
fn go_log_timestamp() -> String {
    chrono::Local::now().format("%Y/%m/%d %H:%M:%S").to_string()
}

/// Render a whole-second `Duration` the way Go's `time.Duration.String()` does
/// for our TTL granularity: `1h0m0s`, `30m0s`, `45s`, `0s`. Sub-second remainder
/// is dropped (TTLs are whole seconds — `IPE_LIVE_TTL` parses to `u64` seconds).
fn go_duration_string(d: Duration) -> String {
    let total = d.as_secs();
    if total == 0 {
        return "0s".to_string();
    }
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod go_format_tests {
    use super::{go_duration_string, memory_store_log_line};
    use std::time::Duration;

    #[test]
    fn duration_matches_go_string() {
        // Go time.Duration.String() reference values.
        assert_eq!(go_duration_string(Duration::from_secs(3600)), "1h0m0s");
        assert_eq!(go_duration_string(Duration::from_secs(1800)), "30m0s");
        assert_eq!(go_duration_string(Duration::from_secs(90)), "1m30s");
        assert_eq!(go_duration_string(Duration::from_secs(45)), "45s");
        assert_eq!(go_duration_string(Duration::from_secs(0)), "0s");
        // Sub-second remainder dropped (whole-second TTL granularity).
        assert_eq!(go_duration_string(Duration::from_millis(1500)), "1s");
    }

    #[test]
    fn memory_line_shape_matches_go() {
        let line = memory_store_log_line(Duration::from_secs(3600));
        // Trailing message exactly as Go (post-timestamp).
        assert!(
            line.ends_with("[ipe.live] session store: memory (ttl=1h0m0s)"),
            "got {line:?}"
        );
        // A `log.LstdFlags` timestamp prefix `YYYY/MM/DD HH:MM:SS ` precedes it.
        let prefix = line
            .strip_suffix("[ipe.live] session store: memory (ttl=1h0m0s)")
            .unwrap_or("");
        assert_eq!(
            prefix.len(),
            20,
            "timestamp prefix `YYYY/MM/DD HH:MM:SS ` is 20 chars: {prefix:?}"
        );
        assert_eq!(prefix.matches('/').count(), 2);
        assert_eq!(prefix.matches(':').count(), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::{Html, build_index};
    use tokio::sync::mpsc::channel;

    // A minimal SessionEntry<(), ()> for exercising the store's TTL/touch logic.
    fn handle() -> SessionHandle<(), ()> {
        let (tx, _rx) = channel::<()>(1);
        let tree: Html<()> = Html::HText(String::new());
        let index = build_index(&tree);
        Arc::new(Mutex::new(SessionEntry {
            model: (),
            last_view: tree,
            index,
            seq: 0,
            sse_tx: None,
            msg_tx: tx,
        }))
    }

    #[tokio::test]
    async fn memory_store_get_set_delete() {
        let s: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        assert!(s.get("a").await.is_none());
        s.set("a", handle()).await;
        assert!(matches!(s.get("a").await, Some(StoreHit::Live(_))));
        s.delete("a").await;
        assert!(s.get("a").await.is_none());
    }

    // A SessionEntry<i32, ()> with a given model, for the checkpoint tests.
    #[cfg(any(feature = "db", feature = "redis_store"))]
    fn handle_i32(model: i32) -> SessionHandle<i32, ()> {
        let (tx, _rx) = channel::<()>(1);
        let tree: Html<()> = Html::HText(String::new());
        let index = build_index(&tree);
        Arc::new(Mutex::new(SessionEntry {
            model,
            last_view: tree,
            index,
            seq: 0,
            sse_tx: None,
            msg_tx: tx,
        }))
    }

    /// A fixed tag for tests that only exercise same-schema behaviour.
    #[cfg(any(feature = "db", feature = "redis_store"))]
    const TEST_TAG: [u8; 32] = [7u8; 32];

    /// Restart survival: a store writes a checkpoint, a FRESH store over the same
    /// file (no mem-cache) decodes it as a `Cold` model.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_checkpoint_survives_restart() {
        let path = std::env::temp_dir().join(format!("ipetest_p5_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            s.set("s1", handle_i32(42)).await;
            // same-process get is a Live cache hit
            assert!(matches!(s.get("s1").await, Some(StoreHit::Live(_))));
        }
        {
            // "restart": new store, empty mem-cache → decodes the checkpoint
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            match s.get("s1").await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 42),
                _ => panic!("expected Cold(42) after restart"),
            }
        }
        let _ = std::fs::remove_file(p);
    }

    /// H24: a checkpoint written under a DIFFERENT Model schema tag is
    /// rejected BEFORE deserialization — `get()` returns `None` (fresh
    /// `init`), never `Some(Cold(stale_shape))`.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_rejects_a_row_written_by_a_different_schema_tag() {
        let path = std::env::temp_dir().join(format!("ipetest_h24_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), [0xAA; 32])
                .await
                .unwrap();
            s.set("s1", handle_i32(42)).await;
        }
        {
            // "redeploy with a changed Model": same file, different tag.
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), [0xBB; 32])
                .await
                .unwrap();
            assert!(
                s.get("s1").await.is_none(),
                "a foreign-schema checkpoint must be rejected before deserialize"
            );
        }
        let _ = std::fs::remove_file(p);
    }

    /// The gate isn't "always reject": the SAME tag on both sides still
    /// round-trips the checkpoint as `Cold`.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_accepts_a_row_written_by_the_same_schema_tag() {
        let path = std::env::temp_dir().join(format!("ipetest_h24ok_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), [0xAA; 32])
                .await
                .unwrap();
            s.set("s1", handle_i32(42)).await;
        }
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), [0xAA; 32])
                .await
                .unwrap();
            match s.get("s1").await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 42),
                _ => panic!("expected Cold(42) under the SAME schema tag"),
            }
        }
        let _ = std::fs::remove_file(p);
    }

    /// Postgres mirror of the reject test — `IPE_TEST_PG_URL`-gated.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn postgres_store_rejects_a_row_written_by_a_different_schema_tag() {
        let Ok(url) = std::env::var("IPE_TEST_PG_URL") else {
            return;
        };
        let sid = format!("pgtest_h24_{}", std::process::id());
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), [0xAA; 32])
                    .await
                    .unwrap();
            s.delete(&sid).await;
            s.set(&sid, handle_i32(7)).await;
        }
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), [0xBB; 32])
                    .await
                    .unwrap();
            assert!(
                s.get(&sid).await.is_none(),
                "a foreign-schema checkpoint must be rejected before deserialize"
            );
            s.delete(&sid).await;
        }
    }

    /// Redis mirror of the reject test (HASH-per-session shape) —
    /// `IPE_TEST_REDIS_URL`-gated.
    #[cfg(feature = "redis_store")]
    #[tokio::test]
    async fn redis_store_rejects_a_row_written_by_a_different_schema_tag() {
        let Ok(url) = std::env::var("IPE_TEST_REDIS_URL") else {
            return;
        };
        let sid = format!("redistest_h24_{}", std::process::id());
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), [0xAA; 32])
                .await
                .unwrap();
            s.delete(&sid).await;
            s.set(&sid, handle_i32(9)).await;
        }
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), [0xBB; 32])
                .await
                .unwrap();
            assert!(
                s.get(&sid).await.is_none(),
                "a foreign-schema checkpoint must be rejected before deserialize"
            );
            s.delete(&sid).await;
        }
    }

    #[cfg(feature = "redis_store")]
    #[test]
    fn redis_key_is_namespaced() {
        assert_eq!(redis_key("abc"), "ipe:sess:abc");
    }

    /// Postgres restart survival — gated on `IPE_TEST_PG_URL` (a reachable
    /// `postgres://…` URL). Skipped when unset so CI without a PG server stays
    /// green; run locally with the env var to exercise the real round-trip.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn postgres_store_checkpoint_survives_restart() {
        let Ok(url) = std::env::var("IPE_TEST_PG_URL") else {
            return;
        };
        let sid = format!("pgtest_{}", std::process::id());
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
                    .await
                    .unwrap();
            s.delete(&sid).await;
            s.set(&sid, handle_i32(7)).await;
            assert!(matches!(s.get(&sid).await, Some(StoreHit::Live(_))));
        }
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
                    .await
                    .unwrap();
            match s.get(&sid).await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 7),
                _ => panic!("expected Cold(7) after restart"),
            }
            s.delete(&sid).await;
        }
    }

    /// Redis restart survival — gated on `IPE_TEST_REDIS_URL`. Skipped when unset.
    #[cfg(feature = "redis_store")]
    #[tokio::test]
    async fn redis_store_checkpoint_survives_restart() {
        let Ok(url) = std::env::var("IPE_TEST_REDIS_URL") else {
            return;
        };
        let sid = format!("redistest_{}", std::process::id());
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            s.delete(&sid).await;
            s.set(&sid, handle_i32(9)).await;
            assert!(matches!(s.get(&sid).await, Some(StoreHit::Live(_))));
        }
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            match s.get(&sid).await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 9),
                _ => panic!("expected Cold(9) after restart"),
            }
            s.delete(&sid).await;
        }
    }

    /// `live_sessions()` lists exactly the locally-live handles: empty on a
    /// fresh store, grows with `set()`, shrinks with `delete()`.
    #[tokio::test]
    async fn memory_store_live_sessions_lists_only_locally_cached_handles() {
        let s: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        assert!(s.live_sessions().await.is_empty());
        s.set("a", handle()).await;
        s.set("b", handle()).await;
        assert_eq!(s.live_sessions().await.len(), 2);
        s.delete("a").await;
        assert_eq!(s.live_sessions().await.len(), 1);
    }

    /// A persisted row with NO in-process handle (another replica's session,
    /// seeded via raw SQL bypassing `set()`) is NOT a live session — only the
    /// locally-`set()` one is returned.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_live_sessions_excludes_cold_rows() {
        let path = std::env::temp_dir().join(format!("ipetest_lives_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        // Cross-replica cold row: valid framed blob, but no mem_cache entry.
        sqlx::query(
            "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) VALUES (?, ?, ?, ?)",
        )
        .bind("cold_sid")
        .bind(encode_checkpoint(&TEST_TAG, &41_i32).unwrap())
        .bind(now_secs())
        .bind(hex::encode(TEST_TAG))
        .execute(&s.pool)
        .await
        .unwrap();
        s.set("live_sid", handle_i32(42)).await;

        let live = s.live_sessions().await;
        assert_eq!(
            live.len(),
            1,
            "only the locally-set session has a live handle; the cold row \
             (no SSE connection in this process) must be excluded"
        );
        // The cold row is still a valid checkpoint through get().
        assert!(matches!(s.get("cold_sid").await, Some(StoreHit::Cold(41))));
        let _ = std::fs::remove_file(p);
    }

    /// Stage-C wire format: the raw persisted blob is
    /// `base64(schema_tag(32) ++ bincode(model))` — a property ONLY the new
    /// format satisfies (a JSON body would fail the length identity) — and
    /// a fresh store still round-trips it as `Cold`.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_new_format_round_trips_model_through_bincode() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        let path = std::env::temp_dir().join(format!("ipetest_binc_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        let model: i32 = 42;
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            s.set("s1", handle_i32(model)).await;
            // Read the raw column back and assert the format identity.
            let row: (String,) = sqlx::query_as("SELECT blob FROM ipe_sessions WHERE sid = 's1'")
                .fetch_one(&s.pool)
                .await
                .unwrap();
            let framed = B64
                .decode(row.0.as_bytes())
                .expect("the persisted blob must be valid base64");
            let body_len = bincode::serialized_size(&model).unwrap() as usize;
            assert_eq!(
                framed.len(),
                32 + body_len,
                "blob must be exactly schema_tag(32) ++ bincode(model)"
            );
            assert_eq!(framed.get(..32).unwrap(), TEST_TAG);
        }
        {
            let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            match s.get("s1").await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 42),
                _ => panic!("expected Cold(42) through the bincode path"),
            }
        }
        let _ = std::fs::remove_file(p);
    }

    /// A raw pre-Stage-C JSON row (seeded directly, bypassing `set()`) is
    /// rejected cleanly by `get()` — `None`, NEVER a panic: it fails base64
    /// decode (or the tag prefix) and takes the same fail-soft path a
    /// corrupt blob always took.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_old_json_row_is_rejected_not_crashed() {
        let path = std::env::temp_dir().join(format!("ipetest_oldjson_{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        let s: SqliteStore<i32, ()> = SqliteStore::new(p, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) VALUES (?, ?, ?, ?)",
        )
        .bind("old")
        .bind("42") // a pre-Stage-C serde-JSON body
        .bind(now_secs())
        .bind(hex::encode(TEST_TAG))
        .execute(&s.pool)
        .await
        .unwrap();
        assert!(
            s.get("old").await.is_none(),
            "an old-format JSON row ages out via the fail-soft miss path"
        );
        let _ = std::fs::remove_file(p);
    }

    /// Postgres mirrors of the bincode round-trip + old-row fail-soft —
    /// `IPE_TEST_PG_URL`-gated.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn postgres_store_new_format_round_trips_and_rejects_old_json_rows() {
        let Ok(url) = std::env::var("IPE_TEST_PG_URL") else {
            return;
        };
        let sid = format!("pgtest_binc_{}", std::process::id());
        let old_sid = format!("pgtest_oldjson_{}", std::process::id());
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
                    .await
                    .unwrap();
            s.delete(&sid).await;
            s.delete(&old_sid).await;
            s.set(&sid, handle_i32(7)).await;
            sqlx::query(
                "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&old_sid)
            .bind("7")
            .bind(now_secs())
            .bind(hex::encode(TEST_TAG))
            .execute(&s.pool)
            .await
            .unwrap();
        }
        {
            let s: PostgresStore<i32, ()> =
                PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
                    .await
                    .unwrap();
            match s.get(&sid).await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 7),
                _ => panic!("expected Cold(7) through the bincode path"),
            }
            assert!(s.get(&old_sid).await.is_none());
            s.delete(&sid).await;
            s.delete(&old_sid).await;
        }
    }

    /// Redis mirrors of the bincode round-trip + old-row fail-soft —
    /// `IPE_TEST_REDIS_URL`-gated.
    #[cfg(feature = "redis_store")]
    #[tokio::test]
    async fn redis_store_new_format_round_trips_and_rejects_old_json_rows() {
        let Ok(url) = std::env::var("IPE_TEST_REDIS_URL") else {
            return;
        };
        let sid = format!("redistest_binc_{}", std::process::id());
        let old_sid = format!("redistest_oldjson_{}", std::process::id());
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            s.delete(&sid).await;
            s.delete(&old_sid).await;
            s.set(&sid, handle_i32(9)).await;
            // Old-format row: raw JSON in the blob field.
            let mut conn = s.conn.clone();
            let _: () = redis::cmd("HSET")
                .arg(redis_key(&old_sid))
                .arg("blob")
                .arg("9")
                .arg("tag")
                .arg(hex::encode(TEST_TAG))
                .query_async(&mut conn)
                .await
                .unwrap();
        }
        {
            let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
                .await
                .unwrap();
            match s.get(&sid).await {
                Some(StoreHit::Cold(m)) => assert_eq!(m, 9),
                _ => panic!("expected Cold(9) through the bincode path"),
            }
            assert!(s.get(&old_sid).await.is_none());
            s.delete(&sid).await;
            s.delete(&old_sid).await;
        }
    }

    /// Postgres mirror of the cold-row exclusion — `IPE_TEST_PG_URL`-gated.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn postgres_store_live_sessions_excludes_cold_rows() {
        let Ok(url) = std::env::var("IPE_TEST_PG_URL") else {
            return;
        };
        let cold_sid = format!("pgtest_cold_{}", std::process::id());
        let live_sid = format!("pgtest_live_{}", std::process::id());
        let s: PostgresStore<i32, ()> = PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        s.delete(&cold_sid).await;
        s.delete(&live_sid).await;
        sqlx::query(
            "INSERT INTO ipe_sessions (sid, blob, last_seen, schema_tag) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&cold_sid)
        .bind("41")
        .bind(now_secs())
        .bind(hex::encode(TEST_TAG))
        .execute(&s.pool)
        .await
        .unwrap();
        s.set(&live_sid, handle_i32(42)).await;
        assert_eq!(s.live_sessions().await.len(), 1);
        s.delete(&cold_sid).await;
        s.delete(&live_sid).await;
    }

    /// Redis mirror of the cold-row exclusion — `IPE_TEST_REDIS_URL`-gated.
    #[cfg(feature = "redis_store")]
    #[tokio::test]
    async fn redis_store_live_sessions_excludes_cold_rows() {
        let Ok(url) = std::env::var("IPE_TEST_REDIS_URL") else {
            return;
        };
        let cold_sid = format!("redistest_cold_{}", std::process::id());
        let live_sid = format!("redistest_live_{}", std::process::id());
        let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        s.delete(&cold_sid).await;
        s.delete(&live_sid).await;
        // Cross-replica cold row: HASH written directly, no mem_cache entry.
        let mut conn = s.conn.clone();
        let _: () = redis::cmd("HSET")
            .arg(redis_key(&cold_sid))
            .arg("blob")
            .arg("41")
            .arg("tag")
            .arg(hex::encode(TEST_TAG))
            .query_async(&mut conn)
            .await
            .unwrap();
        s.set(&live_sid, handle_i32(42)).await;
        assert_eq!(s.live_sessions().await.len(), 1);
        s.delete(&cold_sid).await;
        s.delete(&live_sid).await;
    }

    #[tokio::test]
    async fn memory_store_ttl_eviction_and_touch() {
        let s: MemoryStore<(), ()> = MemoryStore::new(Duration::from_millis(40));
        s.set("idle", handle()).await;
        s.set("active", handle()).await;
        std::thread::sleep(Duration::from_millis(60));
        // touch "active" so it survives the sweep
        let _ = s.get("active").await;
        s.sweep().await;
        assert!(
            s.get("active").await.is_some(),
            "touched session should survive"
        );
        assert!(
            s.get("idle").await.is_none(),
            "idle session should be evicted"
        );
    }
}
