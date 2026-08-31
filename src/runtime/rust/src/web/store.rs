//! Session stores — the `SessionStore` abstraction + backends.
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

/// Hard ceiling on a decoded checkpoint body (bincode-side). A persisted blob's
/// length prefix is attacker-influenceable at the storage boundary (a corrupt /
/// crafted at-rest row); an unbounded decoder would `with_capacity(huge)` on an
/// absurd prefix and OOM before erroring. The shared bounded codec turns any
/// length beyond this into a clean decode error (→ `None`, the same fail-soft
/// miss path a corrupt blob always took), never an allocation spike
/// (PRINCIPLES §3: a decode of an absurd length is turned back). 64 MiB is far
/// above any realistic serialized Model yet far below a memory-pressure risk.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
const MAX_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

/// The single bincode codec every `decode_checkpoint` sibling routes through —
/// one bounded decode boundary ([`MAX_CHECKPOINT_BYTES`]). The option chain
/// (`fixint`, little-endian, `allow_trailing_bytes`) is EXACTLY what
/// `bincode::serialize`/`deserialize` use, so it is byte-compatible with every
/// already-persisted blob (and with `encode_checkpoint`'s `bincode::serialize`);
/// the only addition is `.with_limit(N)`, so a body claiming more than the cap
/// errors cleanly BEFORE any `with_capacity` allocation.
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
fn checkpoint_codec() -> impl bincode::Options {
    use bincode::Options as _;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .with_limit(MAX_CHECKPOINT_BYTES)
}

/// Wire-format epoch for the Model schema tag (H24). Must equal the
/// backend's `emit_model_schema::WIRE_EPOCH` — the epoch is folded into the
/// compile-time `IPE_WEB_MODEL_SCHEMA_TAG` each generated Ipe.Web binary
/// carries. Bumped ONLY when the tag framing / blob encoding itself changes
/// shape (domain-separation convention), never for a Model change — the
/// Model's own shape is covered by the structural half of the hash.
pub const WEB_MODEL_SCHEMA_WIRE_VERSION: &str = "ipe-live-model-schema-v1";

/// Encode one Model checkpoint as `base64(schema_tag(32) ++ bincode(model))`
/// — self-contained (tag travels inside the blob), TEXT-column-safe on every
/// backend (base64 never emits NUL / invalid UTF-8, so no `ALTER TABLE` or
/// BYTEA migration is ever needed). `None` when serialization fails (the
/// caller skips the checkpoint write, same as the old JSON path's `if let Ok`).
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
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
#[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
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
    // Bounded decode: a crafted length prefix beyond MAX_CHECKPOINT_BYTES errors
    // here (→ None) rather than pre-allocating a huge buffer.
    use bincode::Options as _;
    checkpoint_codec().deserialize(body).ok()
}

/// The in-process live session (owns its driver goroutine + SSE channel).
pub type SessionHandle<Model, Msg> = Arc<Mutex<SessionEntry<Model, Msg>>>;

/// Result of a store lookup. `Web` = the in-process session (reuse it). `Cold`
/// = a model decoded from a persistent checkpoint (the caller hydrates: spawn a
/// fresh driver seeded with this model). Memory stores only ever return `Web`.
pub enum StoreHit<Model, Msg> {
    Web(SessionHandle<Model, Msg>),
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
    /// (`push_reload_to_web_sessions`) needs each handle's `sse_tx` and
    /// would otherwise have to re-`get()` every id, opening a TOCTOU-ish gap
    /// where a session evicted between the enumerate and the re-fetch is
    /// silently skipped OR (worse) touches its TTL a second time for no
    /// reason. No default body (unlike `sweep`) — every backend has an
    /// opinion; a future backend without an in-process cache must make an
    /// explicit, reviewed choice, not silently inherit a possibly-wrong one.
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>>;
}

// ─── Memory store — default; in-process, lost on restart ────────────────────

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
            StoreHit::Web(h.clone())
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
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// ─── File store — persistent checkpoint with NO sqlx, for the dev handoff ────

/// A dependency-light persistent store: a `mem_cache` of live handles (same
/// process, owns the driver) PLUS a single on-disk JSON map (`sid → framed
/// checkpoint blob`) so a checkpoint survives a process swap WITHOUT pulling in
/// sqlx/redis. This is what makes `ipe watch`'s blue-green Model handoff work
/// for a plain `Web.app` — such an app reaches no DB kernel, so the emitted
/// crate carries no `db` feature and the sqlite store compiles out; this store
/// rides the `web` feature every web build already has (`base64` + `bincode` +
/// `serde`), reusing the SAME `encode_checkpoint`/`decode_checkpoint` codec and
/// the SAME H24 schema-tag reject-before-deserialize gate as the sqlite/redis
/// backends. A structurally different (Model-type-changed) checkpoint is
/// rejected before deserialize → fail-soft fresh `init`, never a torn Model.
///
/// The on-disk map is the whole file rewritten on each mutation — bounded and
/// simple, matched to a DEV loop's low session count, not a production store.
#[cfg(feature = "web")]
pub struct FileStore<Model, Msg> {
    /// Path to the JSON map file (`sid → blob`).
    path: std::path::PathBuf,
    /// The persisted `sid → framed-checkpoint-blob` map, mirrored in memory and
    /// rewritten to `path` on every mutation. `last_seen` (unix secs) rides
    /// alongside for idle-TTL eviction.
    disk: Mutex<HashMap<String, (String, i64)>>,
    /// Live same-process handles (own the running driver + SSE channel).
    mem_cache: RwLock<SessionMap<Model, Msg>>,
    ttl: Duration,
    /// The live process's Model schema tag (H24) — a stored blob whose leading
    /// tag differs is rejected identically to "no row" (fresh `init`).
    schema_tag: [u8; 32],
}

#[cfg(feature = "web")]
impl<Model, Msg> FileStore<Model, Msg> {
    /// Open (or create) the map at `path`, loading any existing checkpoints.
    /// A missing / unreadable / malformed file starts empty — never an error,
    /// never a panic: a dev handoff that cannot read a stale map simply begins
    /// fresh (the same fail-soft posture the whole store family keeps).
    #[must_use]
    pub fn new(path: &str, ttl: Duration, schema_tag: [u8; 32]) -> Self {
        let path = std::path::PathBuf::from(path);
        let disk = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, (String, i64)>>(&s).ok())
            .unwrap_or_default();
        FileStore {
            path,
            disk: Mutex::new(disk),
            mem_cache: RwLock::new(HashMap::new()),
            ttl,
            schema_tag,
        }
    }

    /// Atomically rewrite the on-disk map from `disk`. Best-effort: a write
    /// failure (e.g. a transient FS error) leaves the in-memory map as the
    /// source of truth for this process and is never fatal — the next mutation
    /// retries the whole map. Writes to a sibling temp file then renames, so a
    /// crash mid-write never leaves a truncated map a later `new` would fail to
    /// parse (and thus silently drop every session).
    ///
    /// On unix the temp file is created `0600` (owner-only) BEFORE any bytes are
    /// written, so the checkpoint map — which may hold Model secrets — is never
    /// world-readable, not even momentarily. The rename carries the mode to the
    /// final path (rename preserves the inode's permissions).
    fn persist(&self, disk: &HashMap<String, (String, i64)>) {
        use std::io::Write as _;
        let Ok(json) = serde_json::to_string(disk) else {
            return;
        };
        let tmp = self.path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let Ok(mut file) = opts.open(&tmp) else {
            return;
        };
        // `OpenOptions::mode` applies only when the file is freshly created; a
        // pre-existing temp (a prior crashed write) keeps its old mode. Set it
        // explicitly so the map is ALWAYS 0600 before it holds any secret.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        if file.write_all(json.as_bytes()).is_ok() && file.flush().is_ok() {
            drop(file);
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

#[cfg(feature = "web")]
#[async_trait]
impl<Model, Msg> SessionStore<Model, Msg> for FileStore<Model, Msg>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    async fn get(&self, sid: &str) -> Option<StoreHit<Model, Msg>> {
        // Same-process live handle wins (owns the running driver).
        let cached = {
            let mut w = self.mem_cache.write().unwrap_or_else(|e| e.into_inner());
            w.get_mut(sid).map(|(h, seen)| {
                *seen = Instant::now();
                h.clone()
            })
        };
        if let Some(h) = cached {
            return Some(StoreHit::Web(h));
        }
        // Cold: decode the persisted checkpoint. The leading 32-byte tag is
        // compared BEFORE deserialization (H24) — a mismatch (Model-type
        // change), a corrupt blob, or a missing entry all take the same
        // fail-soft miss path.
        let blob = {
            let disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
            disk.get(sid).map(|(b, _)| b.clone())
        }?;
        let model: Model = decode_checkpoint(&self.schema_tag, &blob)?;
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
            let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
            disk.insert(sid.to_string(), (blob, file_now_secs()));
            self.persist(&disk);
        }
    }
    async fn delete(&self, sid: &str) {
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sid);
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        if disk.remove(sid).is_some() {
            self.persist(&disk);
        }
    }
    async fn sweep(&self) {
        let cutoff =
            file_now_secs().saturating_sub(i64::try_from(self.ttl.as_secs()).unwrap_or(i64::MAX));
        {
            let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
            let before = disk.len();
            disk.retain(|_, (_, seen)| *seen >= cutoff);
            if disk.len() != before {
                self.persist(&disk);
            }
        }
        // Bound the in-RAM handle cache by idle-TTL too (session-DoS guard).
        let now = Instant::now();
        let ttl = self.ttl;
        self.mem_cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (_, seen)| now.duration_since(*seen) <= ttl);
    }
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

/// Current unix time in whole seconds, saturating (never panics on a clock
/// before the epoch). The `web`-feature file store's own time helper — the
/// sqlx stores' `now_secs` is `#[cfg(feature = "db")]` and unavailable here.
#[cfg(feature = "web")]
fn file_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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

// ─── SQLite store — persistent model checkpoint + live mem-cache ─────────────

/// Persistent store: keeps a `mem_cache` of live handles (same-process, owns the
/// driver) AND a `ipe_sessions(sid, blob, last_seen)` table holding the
/// serde-JSON model checkpoint. `get` returns the live handle on a cache hit,
/// else a `Cold` model decoded from the blob (the caller hydrates a fresh
/// driver). Requires `Model: Serialize + DeserializeOwned` (the codegen derives
/// it). Implements `sqliteStore`.
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
            return Some(StoreHit::Web(h));
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
        // Total cutoff: an absurd `IPE_WEB_TTL` (u64 near 2^63) would make a bare
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
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// ─── Postgres store — multi-instance deployments ─────────────────────────────

/// Same shape as `SqliteStore` (mem-cache of live handles + a `ipe_sessions`
/// blob table + idle-TTL sweep) but over a `PgPool`, for horizontally-scaled
/// deployments (Cloud Run / ECS / k8s) where a returning request can land on a
/// different replica than the one that created the session. `connStr` is a
/// `postgres://user:pass@host/db` URL. Implements `postgresStore`.
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
            return Some(StoreHit::Web(h));
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
        // Total cutoff: an absurd `IPE_WEB_TTL` (u64 near 2^63) would make a bare
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
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

// ─── Redis store — multi-instance, native TTL, no sweep ─────────────────────

/// Namespace session ids under a fixed prefix.
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
/// `redis://[:pass@]host:port/db` URL or a bare `host:port`. Mirrors
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
            return Some(StoreHit::Web(h));
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
    async fn web_sessions(&self) -> Vec<SessionHandle<Model, Msg>> {
        self.mem_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

/// The session-store backend an operator asked for, parsed ONCE from the raw
/// `IPE_WEB_STORE` string into a closed set. Parsing is where the
/// prod-fail-closed decision lives: a backend whose feature this build lacks is
/// a [`StoreConfigError`], never a silent swap to a different backend. Making
/// "operator asked for sqlite, silently got file" unrepresentable means there is
/// no variant that stands for "asked-for-X-serving-Y"; a request this build
/// cannot honour does not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreBackend {
    Memory,
    /// The sqlx-free on-disk checkpoint map (the `ipe watch` dev-handoff store).
    File,
    Sqlite,
    Postgres,
    Redis,
}

/// A requested backend this build cannot serve — a fail-closed startup error,
/// carrying the operator-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfigError(pub String);

impl std::fmt::Display for StoreConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreConfigError {}

impl StoreBackend {
    /// Parse `IPE_WEB_STORE`. `sqlite`/`postgres` require the `db` feature;
    /// `redis` requires `redis_store`. A request this build cannot honour is a
    /// hard error (fail-closed), NEVER a silent downgrade — the operator asked
    /// for a persisted, possibly-remote store; quietly serving a different one
    /// (or losing state) is the failure mode this closes. An unrecognised value
    /// falls back to `memory` (the historical default for a typo / unset).
    ///
    /// Dev (`ipe watch`) is unaffected: it sets `IPE_WEB_STORE=file` explicitly,
    /// which every web build honours; it never asks for `sqlite`.
    fn parse(kind: &str) -> Result<StoreBackend, StoreConfigError> {
        match kind {
            "memory" => Ok(StoreBackend::Memory),
            "file" => Ok(StoreBackend::File),
            "sqlite" => {
                if cfg!(feature = "db") {
                    Ok(StoreBackend::Sqlite)
                } else {
                    Err(StoreConfigError(
                        "IPE_WEB_STORE=sqlite requested but this build has no `db` \
                         feature; build with `db` or set IPE_WEB_STORE=file|memory"
                            .to_string(),
                    ))
                }
            }
            "postgres" => {
                if cfg!(feature = "db") {
                    Ok(StoreBackend::Postgres)
                } else {
                    Err(StoreConfigError(
                        "IPE_WEB_STORE=postgres requested but this build has no `db` \
                         feature; build with `db` or set IPE_WEB_STORE=file|memory"
                            .to_string(),
                    ))
                }
            }
            "redis" => {
                if cfg!(feature = "redis_store") {
                    Ok(StoreBackend::Redis)
                } else {
                    Err(StoreConfigError(
                        "IPE_WEB_STORE=redis requested but this build has no \
                         `redis_store` feature; build with `redis_store` or set \
                         IPE_WEB_STORE=file|memory"
                            .to_string(),
                    ))
                }
            }
            _ => Ok(StoreBackend::Memory),
        }
    }
}

/// Select a backend from the parsed `IPE_WEB_STORE`. A backend this build cannot
/// serve is a fail-closed [`StoreConfigError`] (surfaced by the caller as a task
/// error → stderr + exit 1, or a 503 mount route), NEVER a silent swap. A
/// persistent backend that parses but fails to *connect* at runtime (a dead DB
/// / bad URL) still degrades to memory — that is an environment fault, not an
/// operator mis-request, and matches the prior best-effort posture. The
/// `Model: Serialize` bound is for the persistent backends; memory needs none,
/// but a single signature keeps the codegen call uniform (it derives serde on
/// the model when emitting this). `schema_tag` (the compile-time Model schema
/// fingerprint, H24) is forwarded to the persistent backends only — memory
/// never round-trips through bytes.
pub async fn choose_store<Model, Msg>(
    kind: &str,
    path: &str,
    ttl: Duration,
    schema_tag: [u8; 32],
) -> Result<Arc<dyn SessionStore<Model, Msg>>, StoreConfigError>
where
    Model: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    Msg: Send + Sync + 'static,
{
    let backend = StoreBackend::parse(kind)?;
    match backend {
        #[cfg(feature = "db")]
        StoreBackend::Sqlite => match SqliteStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: sqlite @ {path}");
                return Ok(Arc::new(s));
            }
            Err(e) => {
                eprintln!("[ipe.live] sqlite store unavailable ({e}); falling back to memory")
            }
        },
        #[cfg(feature = "db")]
        StoreBackend::Postgres => match PostgresStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: postgres");
                return Ok(Arc::new(s));
            }
            Err(e) => {
                eprintln!("[ipe.live] postgres store unavailable ({e}); falling back to memory")
            }
        },
        #[cfg(feature = "redis_store")]
        StoreBackend::Redis => match RedisStore::new(path, ttl, schema_tag).await {
            Ok(s) => {
                eprintln!("[ipe.live] session store: redis");
                return Ok(Arc::new(s));
            }
            Err(e) => {
                eprintln!("[ipe.live] redis store unavailable ({e}); falling back to memory")
            }
        },
        #[cfg(feature = "web")]
        StoreBackend::File => {
            eprintln!("[ipe.live] session store: file @ {path}");
            return Ok(Arc::new(FileStore::new(path, ttl, schema_tag)));
        }
        // A parsed-but-feature-absent persistent backend is impossible
        // (`parse` fail-closes it); this arm catches `Memory` and — in a build
        // that parsed a persistent backend but whose feature cfg is off for
        // THIS arm's match (e.g. `File` without `web`) — falls to memory.
        _ => {}
    }
    let _ = (path, schema_tag);
    // Memory store logs with a timestamp + human-readable TTL duration;
    // persistent backends log bare lines above (no duration needed).
    eprintln!("{}", memory_store_log_line(ttl));
    Ok(Arc::new(MemoryStore::new(ttl)))
}

/// Produces the `[ipe.live] session store: memory (ttl=…)` startup log line,
/// with a `YYYY/MM/DD HH:MM:SS` timestamp prefix and a human-readable TTL.
/// Shared so the in-process console sub-app mount emits the same line format.
pub(crate) fn memory_store_log_line(ttl: Duration) -> String {
    format!(
        "{} [ipe.live] session store: memory (ttl={})",
        go_log_timestamp(),
        go_duration_string(ttl)
    )
}

/// Render the current local time as `YYYY/MM/DD HH:MM:SS`.
fn go_log_timestamp() -> String {
    chrono::Local::now().format("%Y/%m/%d %H:%M:%S").to_string()
}

/// Render a whole-second `Duration` as `1h0m0s`, `30m0s`, `45s`, `0s`.
/// Sub-second remainder is dropped (TTLs are whole seconds — `IPE_WEB_TTL`
/// parses to `u64` seconds).
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
mod format_tests {
    use super::{go_duration_string, memory_store_log_line};
    use std::time::Duration;

    #[test]
    fn duration_string_format() {
        assert_eq!(go_duration_string(Duration::from_secs(3600)), "1h0m0s");
        assert_eq!(go_duration_string(Duration::from_secs(1800)), "30m0s");
        assert_eq!(go_duration_string(Duration::from_secs(90)), "1m30s");
        assert_eq!(go_duration_string(Duration::from_secs(45)), "45s");
        assert_eq!(go_duration_string(Duration::from_secs(0)), "0s");
        // Sub-second remainder dropped (whole-second TTL granularity).
        assert_eq!(go_duration_string(Duration::from_millis(1500)), "1s");
    }

    #[test]
    fn memory_line_shape() {
        let line = memory_store_log_line(Duration::from_secs(3600));
        assert!(
            line.ends_with("[ipe.live] session store: memory (ttl=1h0m0s)"),
            "got {line:?}"
        );
        // Timestamp prefix `YYYY/MM/DD HH:MM:SS ` precedes it.
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
            #[cfg(feature = "debugger")]
            history: crate::debugger::RecordBuffer::new((), crate::debugger::DEFAULT_HISTORY_CAP),
        }))
    }

    #[tokio::test]
    async fn memory_store_get_set_delete() {
        let s: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        assert!(s.get("a").await.is_none());
        s.set("a", handle()).await;
        assert!(matches!(s.get("a").await, Some(StoreHit::Web(_))));
        s.delete("a").await;
        assert!(s.get("a").await.is_none());
    }

    // A SessionEntry<i32, ()> with a given model, for the checkpoint tests.
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
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
            #[cfg(feature = "debugger")]
            history: crate::debugger::RecordBuffer::new(
                model,
                crate::debugger::DEFAULT_HISTORY_CAP,
            ),
        }))
    }

    /// A fixed tag for tests that only exercise same-schema behaviour.
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
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
            // same-process get is a Web cache hit
            assert!(matches!(s.get("s1").await, Some(StoreHit::Web(_))));
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
            assert!(matches!(s.get(&sid).await, Some(StoreHit::Web(_))));
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
            assert!(matches!(s.get(&sid).await, Some(StoreHit::Web(_))));
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

    /// `web_sessions()` lists exactly the locally-live handles: empty on a
    /// fresh store, grows with `set()`, shrinks with `delete()`.
    #[tokio::test]
    async fn memory_store_web_sessions_lists_only_locally_cached_handles() {
        let s: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        assert!(s.web_sessions().await.is_empty());
        s.set("a", handle()).await;
        s.set("b", handle()).await;
        assert_eq!(s.web_sessions().await.len(), 2);
        s.delete("a").await;
        assert_eq!(s.web_sessions().await.len(), 1);
    }

    /// A persisted row with NO in-process handle (another replica's session,
    /// seeded via raw SQL bypassing `set()`) is NOT a live session — only the
    /// locally-`set()` one is returned.
    #[cfg(feature = "db")]
    #[tokio::test]
    async fn sqlite_store_web_sessions_excludes_cold_rows() {
        let path = std::env::temp_dir().join(format!("ipetest_webs_{}.db", std::process::id()));
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
        s.set("web_sid", handle_i32(42)).await;

        let live = s.web_sessions().await;
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
    async fn postgres_store_web_sessions_excludes_cold_rows() {
        let Ok(url) = std::env::var("IPE_TEST_PG_URL") else {
            return;
        };
        let cold_sid = format!("pgtest_cold_{}", std::process::id());
        let web_sid = format!("pgtest_web_{}", std::process::id());
        let s: PostgresStore<i32, ()> = PostgresStore::new(&url, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        s.delete(&cold_sid).await;
        s.delete(&web_sid).await;
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
        s.set(&web_sid, handle_i32(42)).await;
        assert_eq!(s.web_sessions().await.len(), 1);
        s.delete(&cold_sid).await;
        s.delete(&web_sid).await;
    }

    /// Redis mirror of the cold-row exclusion — `IPE_TEST_REDIS_URL`-gated.
    #[cfg(feature = "redis_store")]
    #[tokio::test]
    async fn redis_store_web_sessions_excludes_cold_rows() {
        let Ok(url) = std::env::var("IPE_TEST_REDIS_URL") else {
            return;
        };
        let cold_sid = format!("redistest_cold_{}", std::process::id());
        let web_sid = format!("redistest_web_{}", std::process::id());
        let s: RedisStore<i32, ()> = RedisStore::new(&url, Duration::from_secs(60), TEST_TAG)
            .await
            .unwrap();
        s.delete(&cold_sid).await;
        s.delete(&web_sid).await;
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
        s.set(&web_sid, handle_i32(42)).await;
        assert_eq!(s.web_sessions().await.len(), 1);
        s.delete(&cold_sid).await;
        s.delete(&web_sid).await;
    }

    /// File-store restart survival: a store writes a checkpoint, a FRESH store
    /// over the same file (empty mem-cache) decodes it as a `Cold` model — the
    /// dev-handoff persistence path, with NO sqlx.
    #[cfg(feature = "web")]
    #[tokio::test]
    async fn file_store_checkpoint_survives_restart() {
        let path = std::env::temp_dir().join(format!("ipetest_file_{}.json", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let s: FileStore<i32, ()> = FileStore::new(p, Duration::from_secs(60), TEST_TAG);
            s.set("s1", handle_i32(42)).await;
            assert!(matches!(s.get("s1").await, Some(StoreHit::Web(_))));
        }
        {
            // "rebuild": a new store, empty mem-cache → decodes the checkpoint.
            let s: FileStore<i32, ()> = FileStore::new(p, Duration::from_secs(60), TEST_TAG);
            let cold = matches!(s.get("s1").await, Some(StoreHit::Cold(42)));
            assert!(
                cold,
                "expected Cold(42) after a rebuild: the checkpoint must survive on disk"
            );
        }
        let _ = std::fs::remove_file(p);
    }

    /// H24 for the file store: a checkpoint written under a DIFFERENT Model
    /// schema tag (a Model-type change across a rebuild) is REJECTED before
    /// deserialize — `get()` returns `None` (fresh `init`), never a torn Model.
    #[cfg(feature = "web")]
    #[tokio::test]
    async fn file_store_rejects_a_row_written_by_a_different_schema_tag() {
        let path =
            std::env::temp_dir().join(format!("ipetest_fileh24_{}.json", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        {
            let s: FileStore<i32, ()> = FileStore::new(p, Duration::from_secs(60), [0xAA; 32]);
            s.set("s1", handle_i32(42)).await;
        }
        {
            // "rebuild with a changed Model": same file, different tag.
            let s: FileStore<i32, ()> = FileStore::new(p, Duration::from_secs(60), [0xBB; 32]);
            assert!(
                s.get("s1").await.is_none(),
                "a foreign-schema checkpoint must be rejected before deserialize"
            );
        }
        let _ = std::fs::remove_file(p);
    }

    /// A malformed / truncated on-disk map does NOT crash `new` — it starts
    /// empty (fail-soft), so a dev handoff over a corrupt map begins fresh
    /// rather than faulting.
    #[cfg(feature = "web")]
    #[tokio::test]
    async fn file_store_starts_empty_on_a_corrupt_map() {
        let path =
            std::env::temp_dir().join(format!("ipetest_filecorrupt_{}.json", std::process::id()));
        let p = path.to_str().unwrap();
        std::fs::write(p, b"{ this is not valid json").unwrap();
        let s: FileStore<i32, ()> = FileStore::new(p, Duration::from_secs(60), TEST_TAG);
        assert!(
            s.get("anything").await.is_none(),
            "a corrupt map must yield an empty store, not a panic"
        );
        let _ = std::fs::remove_file(p);
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

    /// Bounded decode: a framed blob whose bincode length prefix claims a body
    /// far larger than MAX_CHECKPOINT_BYTES decodes to `None` cleanly — the
    /// bounded codec errors on the oversized length instead of `with_capacity`
    /// on an absurd size. A `Vec<u8>` is the shape whose bincode is a bare
    /// 8-byte little-endian length prefix, so a crafted prefix directly probes
    /// the allocation boundary.
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
    #[test]
    fn decode_checkpoint_rejects_an_oversized_length_prefix_without_oom() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        // framed = tag(32) ++ bincode(Vec<u8>): the body is an 8-byte LE length
        // then that many bytes. Claim ~4 GiB but supply no bytes — an unbounded
        // decoder would try to reserve ~4 GiB; the bounded one errors first.
        let mut framed = Vec::new();
        framed.extend_from_slice(&TEST_TAG);
        framed.extend_from_slice(&(4_000_000_000_u64).to_le_bytes());
        let blob = B64.encode(&framed);
        let decoded: Option<Vec<u8>> = decode_checkpoint(&TEST_TAG, &blob);
        assert!(
            decoded.is_none(),
            "an oversized length prefix must decode to None, never OOM"
        );
    }

    /// A within-limit blob still round-trips — the bound rejects only absurd
    /// lengths, not legitimate payloads (guards against an over-tight cap).
    #[cfg(any(feature = "db", feature = "redis_store", feature = "web"))]
    #[test]
    fn decode_checkpoint_accepts_a_within_limit_body() {
        let payload: Vec<u8> = vec![1, 2, 3, 4, 5];
        let blob = encode_checkpoint(&TEST_TAG, &payload);
        assert!(blob.is_some(), "encoding a small Vec<u8> cannot fail");
        if let Some(blob) = blob {
            let decoded: Option<Vec<u8>> = decode_checkpoint(&TEST_TAG, &blob);
            assert_eq!(decoded, Some(payload));
        }
    }

    /// Prod fail-closed: `IPE_WEB_STORE=sqlite` in a build WITHOUT the `db`
    /// feature is a hard config error at store selection — never a silent
    /// `FileStore`. Only meaningful when `db` is absent; a `db` build parses
    /// `sqlite` to the real backend, exercised by the other tests.
    #[cfg(all(feature = "web", not(feature = "db")))]
    #[tokio::test]
    async fn choose_store_fails_closed_on_prod_sqlite_without_db_feature() {
        let r = choose_store::<i32, ()>("sqlite", "", Duration::from_secs(60), TEST_TAG).await;
        let err = r.err();
        assert!(
            err.is_some(),
            "a sqlite request without `db` must fail closed, not serve a store"
        );
        if let Some(StoreConfigError(msg)) = err {
            assert!(
                msg.contains("sqlite") && msg.contains("db"),
                "the error must name the missing feature: {msg:?}"
            );
        }
    }

    /// The dev path is untouched: `file` always parses to a real `FileStore`,
    /// and `memory`/unknown still yield a memory store (never an error).
    #[cfg(feature = "web")]
    #[tokio::test]
    async fn choose_store_honours_file_and_memory_without_error() {
        let path = std::env::temp_dir().join(format!("ipetest_choose_{}.json", std::process::id()));
        let p = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&p);
        assert!(
            choose_store::<i32, ()>("file", &p, Duration::from_secs(60), TEST_TAG)
                .await
                .is_ok(),
            "dev `file` must select a store, never fail closed"
        );
        assert!(
            choose_store::<i32, ()>("memory", "", Duration::from_secs(60), TEST_TAG)
                .await
                .is_ok()
        );
        assert!(
            choose_store::<i32, ()>("totally-unknown", "", Duration::from_secs(60), TEST_TAG)
                .await
                .is_ok(),
            "an unrecognised value falls back to memory, not an error"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// 0600 perms: the file store's on-disk map is owner-only on unix — a
    /// session blob may hold Model secrets and must never be world-readable.
    #[cfg(all(feature = "web", unix))]
    #[tokio::test]
    async fn file_store_map_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("ipetest_perms_{}.json", std::process::id()));
        let p = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&p);
        let s: FileStore<i32, ()> = FileStore::new(&p, Duration::from_secs(60), TEST_TAG);
        s.set("s1", handle_i32(42)).await;
        let meta = std::fs::metadata(&p);
        assert!(
            meta.is_ok(),
            "the map file must exist after a write: {:?}",
            meta.as_ref().err()
        );
        if let Ok(meta) = meta {
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the session map must be 0600 (owner-only)");
        }
        let _ = std::fs::remove_file(&p);
    }
}
