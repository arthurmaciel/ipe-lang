//! Std.Cache — a bounded LRU cache with optional TTL + running stats.
//!
//! Handle-based: `cache_new_raw` returns an `i64` handle wrapped in the opaque
//! `SkyCacheHandle` (the Sky `Cache k v` lowers to this non-generic enum — the
//! handle carries no type args; `k`/`v` live only on the kernel calls). The
//! other kernels take the unwrapped `i64`.
//!
//! Each handle holds a `K`-typed `Vec<CacheEntry<K>>` whose entries carry a
//! value-erased `Box<dyn Any + Send>`, downcast to `V` only on `get` (where the
//! Sky `getRaw : … -> Task Error (Maybe v)` return makes `V` available). Keys
//! are matched by `PartialEq` (already in the codegen's standard generic bounds)
//! via a linear scan — no `Eq`/`Hash` needed, so the generic stdlib wrappers
//! type-check without any bound-threading. O(n) per op, fine for the small caches
//! Sky uses; a future codegen `Eq+Hash` bound would allow an O(1) `HashMap`.
//!
//! Both the `Vec<CacheEntry<K>>` downcast (by `K`) and the value downcast (by
//! `V`) are **correct by construction** — every op on a handle uses the same
//! `(K, V)`, enforced by Sky's opaque `Cache k v` — so neither can fail; a
//! mismatch / missing handle degrades to a miss / no-op, never a panic. The same
//! sanctioned-seam discipline as the pub/sub broker; strictly safer than Go's reflect
//! cache (the cast cannot fail). See the README `dyn Any` register.

use super::*;
use std::any::Any;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The opaque Sky `Cache k v` — a non-generic handle wrapper. The variant name
/// `Cache` matches the Sky constructor so the codegen lowers `Cache.Cache raw`
/// to `SkyCacheHandle::Cache(raw)` and `case c of Cache raw -> …` to a match.
#[derive(Clone, Debug, PartialEq)]
pub enum SkyCacheHandle {
    Cache(i64),
}

/// Mirrors Sky's `CacheCfg` record (field names match → the codegen maps
/// `Std.Cache.CacheCfg` to this struct, EmailMessage-style).
#[allow(non_snake_case)]
#[derive(Clone)]
pub struct CacheCfg {
    pub maxEntries: i64,
    pub ttlMs: i64,
    pub maxBytes: i64,
}

/// Mirrors Sky's `stats` return record `{ hits, misses, evictions }`.
#[allow(non_snake_case)]
#[derive(Clone)]
pub struct CacheStats {
    pub hits: i64,
    pub misses: i64,
    pub evictions: i64,
}

struct CacheEntry<K> {
    key: K,
    // SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — Cache_remove carries no V; value erased, downcast to V only on get (per-handle V-consistent); miss → Nothing [ledger #4]
    value: Box<dyn Any + Send>, // the cache value `V`, downcast on get
    expires_at: Option<Instant>,
    last_seq: u64, // for LRU eviction
}

struct Slot {
    cfg: CacheCfg,
    hits: i64,
    misses: i64,
    evictions: i64,
    entries: i64, // tracked here so sizeRaw needs neither K nor V
    seq: u64,     // monotonic access counter
    // SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — Cache_size/clear carry no V; per-handle store downcast by K (every op uses same K); mismatch → no-op [ledger #4]
    store: Option<Box<dyn Any + Send>>, // Vec<CacheEntry<K>>, created lazily on first K-bearing op
}

// type_complexity (accepted, cosmetic): the `(next_handle, Vec<(handle, Slot)>)`
// tuple is this registry's one-off internal store shape — a type alias would
// hide it rather than clarify. Not a soundness concern.
#[allow(clippy::type_complexity)]
fn registry() -> &'static Mutex<(i64, Vec<(i64, Slot)>)> {
    static R: OnceLock<Mutex<(i64, Vec<(i64, Slot)>)>> = OnceLock::new();
    R.get_or_init(|| Mutex::new((0, Vec::new())))
}

fn with_slot<R>(handle: i64, default: R, f: impl FnOnce(&mut Slot) -> R) -> R {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    match g.1.iter_mut().find(|(h, _)| *h == handle) {
        Some((_, slot)) => f(slot),
        None => default,
    }
}

/// `Cache.newRaw : CacheCfg -> Task Error Int` — allocate a cache, return its handle.
pub fn cache_new_raw<E: Send + From<String> + 'static>(cfg: CacheCfg) -> SkyTask<E, i64> {
    Box::pin(async move {
        // `maxBytes` is not enforced on the Rust backend: the value is erased to a
        // `Box<dyn Any>`, so per-entry byte accounting isn't available without a
        // size-measuring bound. Warn ONCE so a caller relying on it for a memory
        // bound isn't silently unprotected — `maxEntries` (LRU) is the live bound.
        if cfg.maxBytes > 0 {
            use std::sync::atomic::{AtomicBool, Ordering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[sky.cache] CacheCfg.maxBytes ({}) is not enforced on the Rust backend; \
                     use maxEntries (LRU) to bound memory",
                    cfg.maxBytes
                );
            }
        }
        let h = {
            let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
            g.0 += 1;
            let h = g.0;
            g.1.push((
                h,
                Slot {
                    cfg,
                    hits: 0,
                    misses: 0,
                    evictions: 0,
                    entries: 0,
                    seq: 0,
                    store: None,
                },
            ));
            h
        };
        ok_res(h)
    })
}

/// `Cache.putRaw : Int -> k -> v -> Task Error ()`.
pub fn cache_put<E, K, V>(handle: i64, key: K, value: V) -> SkyTask<E, ()>
where
    E: Send + From<String> + 'static,
    K: PartialEq + Send + 'static,
    V: Send + 'static,
{
    Box::pin(async move {
        with_slot(handle, (), |slot| {
            slot.seq += 1;
            let seq = slot.seq;
            let max = slot.cfg.maxEntries;
            let ttl = slot.cfg.ttlMs;
            let expires_at = if ttl > 0 {
                // Saturate instead of panicking: a Sky caller passing a near-i64::MAX ttl
                // (e.g. `withTTL Int.maxInt`) must not cause "overflow when adding duration
                // to instant". ttl > 0 is already guarded above, so the cast is lossless.
                Some(
                    Instant::now()
                        .checked_add(Duration::from_millis(ttl as u64))
                        .unwrap_or_else(|| {
                            Instant::now() + Duration::from_secs(60 * 60 * 24 * 365 * 30)
                        }),
                )
            } else {
                None // ttl <= 0 → no expiry (existing semantics preserved)
            };
            let (added, evicted) = {
                let store = slot
                    .store
                    .get_or_insert_with(|| Box::new(Vec::<CacheEntry<K>>::new()));
                match store.downcast_mut::<Vec<CacheEntry<K>>>() {
                    None => (0i64, 0i64), // impossible per per-handle (K,V) consistency
                    Some(vec) => {
                        let mut added = 1i64;
                        if let Some(e) = vec.iter_mut().find(|e| e.key == key) {
                            e.value = Box::new(value);
                            e.expires_at = expires_at;
                            e.last_seq = seq;
                            added = 0;
                        } else {
                            vec.push(CacheEntry {
                                key,
                                value: Box::new(value),
                                expires_at,
                                last_seq: seq,
                            });
                        }
                        let mut evicted = 0i64;
                        if max > 0 && vec.len() as i64 > max {
                            // evict the least-recently-used (smallest last_seq)
                            if let Some((idx, _)) =
                                vec.iter().enumerate().min_by_key(|(_, e)| e.last_seq)
                            {
                                vec.remove(idx);
                                evicted = 1;
                            }
                        }
                        (added, evicted)
                    }
                }
            };
            slot.entries += added - evicted;
            slot.evictions += evicted;
        });
        ok_res(())
    })
}

/// `Cache.getRaw : Int -> k -> Task Error (Maybe v)`.
pub fn cache_get<E, K, V>(handle: i64, key: K) -> SkyTask<E, SkyMaybe<V>>
where
    E: Send + From<String> + 'static,
    K: PartialEq + Send + 'static,
    V: Clone + Send + 'static,
{
    Box::pin(async move {
        let out = with_slot(handle, SkyMaybe::Nothing, |slot| {
            slot.seq += 1;
            let seq = slot.seq;
            let now = Instant::now();
            enum Outcome<V> {
                Hit(V),
                Expired,
                Miss,
            }
            let outcome = match slot
                .store
                .as_mut()
                .and_then(|s| s.downcast_mut::<Vec<CacheEntry<K>>>())
            {
                None => Outcome::Miss,
                Some(vec) => match vec.iter().position(|e| e.key == key) {
                    None => Outcome::Miss,
                    Some(idx) => {
                        // index is in-bounds (just found); guard with .get anyway
                        let expired = vec
                            .get(idx)
                            .is_some_and(|e| e.expires_at.is_some_and(|x| now >= x));
                        if expired {
                            vec.remove(idx);
                            Outcome::Expired
                        } else {
                            match vec.get_mut(idx) {
                                Some(e) => {
                                    e.last_seq = seq; // LRU touch
                                    match e.value.downcast_ref::<V>().cloned() {
                                        Some(v) => Outcome::Hit(v),
                                        None => Outcome::Miss, // impossible; total fallback
                                    }
                                }
                                None => Outcome::Miss,
                            }
                        }
                    }
                },
            };
            match outcome {
                Outcome::Hit(v) => {
                    slot.hits += 1;
                    SkyMaybe::Just(v)
                }
                Outcome::Expired => {
                    slot.misses += 1;
                    slot.entries -= 1;
                    SkyMaybe::Nothing
                }
                Outcome::Miss => {
                    slot.misses += 1;
                    SkyMaybe::Nothing
                }
            }
        });
        ok_res(out)
    })
}

/// `Cache.removeRaw : Int -> k -> Task Error ()`.
pub fn cache_remove<E, K>(handle: i64, key: K) -> SkyTask<E, ()>
where
    E: Send + From<String> + 'static,
    K: PartialEq + Send + 'static,
{
    Box::pin(async move {
        with_slot(handle, (), |slot| {
            let before = slot
                .store
                .as_ref()
                .and_then(|s| s.downcast_ref::<Vec<CacheEntry<K>>>())
                .map_or(0, |v| v.len());
            if let Some(vec) = slot
                .store
                .as_mut()
                .and_then(|s| s.downcast_mut::<Vec<CacheEntry<K>>>())
            {
                vec.retain(|e| e.key != key);
                let removed = (before - vec.len()) as i64;
                slot.entries -= removed;
            }
        });
        ok_res(())
    })
}

/// `Cache.clearRaw : Int -> Task Error ()`.
pub fn cache_clear<E: Send + From<String> + 'static>(handle: i64) -> SkyTask<E, ()> {
    Box::pin(async move {
        with_slot(handle, (), |slot| {
            slot.store = None;
            slot.entries = 0;
        });
        ok_res(())
    })
}

/// `Cache.sizeRaw : Int -> Task Error Int`.
pub fn cache_size<E: Send + From<String> + 'static>(handle: i64) -> SkyTask<E, i64> {
    Box::pin(async move { ok_res(with_slot(handle, 0, |slot| slot.entries)) })
}

/// `Cache.statsRaw : Int -> Task Error { hits, misses, evictions }`.
pub fn cache_stats<E: Send + From<String> + 'static>(handle: i64) -> SkyTask<E, CacheStats> {
    Box::pin(async move {
        let s = with_slot(
            handle,
            CacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
            },
            |slot| CacheStats {
                hits: slot.hits,
                misses: slot.misses,
                evictions: slot.evictions,
            },
        );
        ok_res(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T: Send + 'static>(t: SkyTask<SkyError, T>) -> T {
        match crate::sky_runtime::task::block_on(t) {
            SkyResult::Ok(v) => v,
            SkyResult::Err(_) => panic!("cache task failed"),
        }
    }

    #[test]
    fn put_get_size_remove_stats() {
        let h = run(cache_new_raw::<SkyError>(CacheCfg {
            maxEntries: 8,
            ttlMs: 0,
            maxBytes: 0,
        }));
        run(cache_put::<SkyError, String, String>(
            h,
            "a".into(),
            "1".into(),
        ));
        run(cache_put::<SkyError, String, String>(
            h,
            "b".into(),
            "2".into(),
        ));
        assert_eq!(run(cache_size::<SkyError>(h)), 2);
        assert_eq!(
            run(cache_get::<SkyError, String, String>(h, "a".into())),
            SkyMaybe::Just("1".into())
        );
        assert_eq!(
            run(cache_get::<SkyError, String, String>(h, "z".into())),
            SkyMaybe::Nothing
        );
        run(cache_remove::<SkyError, String>(h, "a".into()));
        assert_eq!(
            run(cache_get::<SkyError, String, String>(h, "a".into())),
            SkyMaybe::Nothing
        );
        assert_eq!(run(cache_size::<SkyError>(h)), 1);
        let st = run(cache_stats::<SkyError>(h));
        assert_eq!(st.hits, 1);
        assert_eq!(st.misses, 2);
    }

    #[test]
    fn lru_eviction_over_capacity() {
        let h = run(cache_new_raw::<SkyError>(CacheCfg {
            maxEntries: 2,
            ttlMs: 0,
            maxBytes: 0,
        }));
        run(cache_put::<SkyError, String, i64>(h, "a".into(), 1));
        run(cache_put::<SkyError, String, i64>(h, "b".into(), 2));
        let _ = run(cache_get::<SkyError, String, i64>(h, "a".into())); // touch a (b now LRU)
        run(cache_put::<SkyError, String, i64>(h, "c".into(), 3)); // evicts b
        assert_eq!(run(cache_size::<SkyError>(h)), 2);
        assert_eq!(
            run(cache_get::<SkyError, String, i64>(h, "b".into())),
            SkyMaybe::Nothing
        );
        assert_eq!(
            run(cache_get::<SkyError, String, i64>(h, "a".into())),
            SkyMaybe::Just(1)
        );
        assert_eq!(run(cache_stats::<SkyError>(h)).evictions, 1);
    }
}
