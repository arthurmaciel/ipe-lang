#![allow(clippy::ptr_arg)]
// Ipê Runtime — Core types (always included)
// Generic over E (error type).  Builder.hs emits `use ipe_runtime::*;`
// and thin wrappers that instantiate E = IpeError.
//
// This module is the home for the core TYPES (IpeMaybe / IpeResult / IpeTask)
// and their combinators, plus the byte-sequence FFI coercion. The String and
// List kernels live in their named Ipê-module homes — `string.rs` and
// `list.rs` — re-exported through `mod.rs`'s glob so call sites are unaffected.

use std::future::Future;
use std::pin::Pin;

// ===========================================
// Task type (generic over error type E)
// ===========================================
// The `Send` bound backs tokio `spawn`/`block_on` on native hosts. On wasm32
// the runtime is single-threaded and browser futures (`JsFuture`, DOM-touching
// async) are `!Send`, so the bound is relaxed there — one type, cfg-split,
// never a fork or a `MaybeSend` trait (the native assertion in
// `tests/wasm_floor_scope.rs` pins the native half).
#[cfg(not(target_arch = "wasm32"))]
pub type IpeTask<E, A> = Pin<Box<dyn Future<Output = IpeResult<E, A>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type IpeTask<E, A> = Pin<Box<dyn Future<Output = IpeResult<E, A>> + 'static>>;

/// Construct Ok with generic error type.  Use `ok_res::<IpeError>` to
/// instantiate with the project's concrete error type.
pub fn ok_res<E, A>(a: A) -> IpeResult<E, A> {
    IpeResult::Ok(a)
}

/// Construct an error value from a string.  Requires `E: From<String>`.
/// When E = `IpeCoreErrorError`, the generated code provides the impl.
#[must_use]
pub fn str_err<E: From<String>>(s: &str) -> E {
    s.to_string().into()
}

/// Convert a foreign FFI error into the project's error type — REDACTED.
///
/// Used by the fallible(-async) FFI wrapper bodies to flatten a foreign
/// `Result<T, E>` Err arm into a Ipê-compatible error:
///
///   `Ok(Err(e)) => IpeResult::Err(ipe_error_from_foreign(e))`
///
/// C5: `tokio::task::spawn(...).await` already catches panics via `JoinError`;
/// this fn handles the non-panic `Err(e)` arm. Any `Debug`-able foreign error
/// type is accepted — `Debug` is universal and always available.
///
/// [B8 SECURITY — load-bearing] The foreign error's raw `Debug` is NEVER
/// surfaced to the Ipê side. A real network/auth client error (a reqwest/hyper
/// transport failure, a stripe API error) can echo the request URL, request
/// headers, a bearer token, or an API key in its `Debug`. So we follow Go's
/// two-level error pattern: the raw `Debug` detail is logged SERVER-SIDE under a
/// fresh correlation id (operators can trace it), and ONLY a fixed generic
/// message carrying that id is returned to Ipê (`Error.toString` shows
/// `external operation failed (ref <id>)`, never the secret-bearing detail).
///
/// Same generic `E: From<String>` contract as `str_err` — the project provides
/// `impl From<String> for IpeCoreErrorError` so both arms of the fallible match
/// resolve to the same `IpeResult<E, A>`. Total — no unwrap/index/panic.
pub fn ipe_error_from_foreign<ForeignE: std::fmt::Debug, E: From<String>>(e: ForeignE) -> E {
    let err_id = short_err_id();
    log_foreign_error(&err_id, &format!("{e:?}"));
    format!("external operation failed (ref {err_id})").into()
}

/// [B8] Server-log a foreign FFI error's raw `Debug` detail under a correlation
/// id, honouring `IPE_LOG_FORMAT=json`. The detail is for OPERATORS ONLY — it can
/// carry secrets / PII / internal paths from a transport error — so it goes to
/// the SERVER LOG (stderr), never to the Ipê-visible message. Mirrors the
/// `classify_and_log_panic` log shape (kind `ForeignError`). Total — no
/// unwrap/index/panic.
fn log_foreign_error(err_id: &str, detail: &str) {
    let json = crate::system::read_env_var("IPE_LOG_FORMAT")
        .is_ok_and(|v| v.eq_ignore_ascii_case("json"));
    if json {
        eprintln!(
            "{{\"level\":\"error\",\"kind\":\"ForeignError\",\"errId\":\"{}\",\"message\":\"{}\"}}",
            err_id,
            crate::telemetry::json_escape(detail)
        );
    } else {
        eprintln!(
            "[error] ForeignError (ref {err_id}): {}",
            scrub_log_controls(detail)
        );
    }
}

/// Replace every control character (CR/LF, ESC, other C0/C1) with a space so an
/// attacker-influenced foreign-error `Debug` or panic payload cannot inject forged
/// log records (CR/LF) or terminal escape sequences into the plain-format server
/// log. The JSON branches already route through `telemetry::json_escape`; this is
/// the plain-branch counterpart, shared by `log_foreign_error` and
/// `classify_and_log_panic`, plus `Trace.attr`/`event`/`span` output. Total — no
/// unwrap/index/panic.
pub(crate) fn scrub_log_controls(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Bake a config-derived default for an env var: set `key=val` ONLY when the
/// var is unset, so shell env / `.env` still win (precedence: process env >
/// baked default). Go parity: the generated `init()`'s `rt.SetPortDefault` +
/// `tomlIpeEnv` family. Routed through the process-global env lock
/// (`locked_set_var_if_absent`) so it is sound by construction even if a thread
/// is already reading the environment.
pub fn set_env_default(key: &str, val: &str) {
    crate::system::locked_set_var_if_absent(key, val);
}

// ===========================================
// Disconnected-store placeholders (closure-Model `Default`)
// ===========================================
// A Ipe.Live Model with function-typed fields (`Arc<dyn Fn(..) -> IpeTask<..>>`,
// e.g. the console's `store`) can't be serialized, so the codegen serde-skips
// those fields and reconstructs them via `Default` from these helpers. Each is a
// closure of the right arity that yields a STRUCTURED `Task` error (never a
// panic / unwrap) — a closure-Model whose session is restored gets a disconnected
// store and the app re-fetches. Closure-Models are memory-store-only (the memory
// store never serialises, so these are never instantiated at runtime there); the
// codegen makes persisting a closure-Model a hard compile error.
const DISCONNECTED_MSG: &str = "disconnected store: a closure-Model session was restored — closure-Models require [live] store = memory";

#[must_use]
pub fn disconnected_fn0<T: Send + 'static, E: From<String> + Send + 'static>()
-> std::sync::Arc<dyn Fn() -> IpeTask<E, T> + Send + Sync> {
    std::sync::Arc::new(|| -> IpeTask<E, T> {
        Box::pin(std::future::ready(IpeResult::Err(str_err::<E>(
            DISCONNECTED_MSG,
        ))))
    })
}
#[must_use]
pub fn disconnected_fn1<A: 'static, T: Send + 'static, E: From<String> + Send + 'static>()
-> std::sync::Arc<dyn Fn(A) -> IpeTask<E, T> + Send + Sync> {
    std::sync::Arc::new(|_a| -> IpeTask<E, T> {
        Box::pin(std::future::ready(IpeResult::Err(str_err::<E>(
            DISCONNECTED_MSG,
        ))))
    })
}
#[must_use]
pub fn disconnected_fn2<
    A1: 'static,
    A2: 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
>() -> std::sync::Arc<dyn Fn(A1, A2) -> IpeTask<E, T> + Send + Sync> {
    std::sync::Arc::new(|_a1, _a2| -> IpeTask<E, T> {
        Box::pin(std::future::ready(IpeResult::Err(str_err::<E>(
            DISCONNECTED_MSG,
        ))))
    })
}
#[must_use]
pub fn disconnected_fn3<
    A1: 'static,
    A2: 'static,
    A3: 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
>() -> std::sync::Arc<dyn Fn(A1, A2, A3) -> IpeTask<E, T> + Send + Sync> {
    std::sync::Arc::new(|_a1, _a2, _a3| -> IpeTask<E, T> {
        Box::pin(std::future::ready(IpeResult::Err(str_err::<E>(
            DISCONNECTED_MSG,
        ))))
    })
}

// ===========================================
// Byte-sequence FFI coercion (Ipê List Int <-> Rust bytes)
// ===========================================

/// Ipê `List Int` (Vec<i64>) -> owned bytes. Each element is narrowed `as u8`,
/// mirroring the numeric param narrowing the FFI codegen already emits.
/// Used for `&[u8]` and `Vec<u8>` parameters.
#[must_use]
pub fn to_u8_vec(xs: &[i64]) -> Vec<u8> {
    xs.iter().map(|&x| x as u8).collect()
}

/// Owned/borrowed bytes -> Ipê `List Int` (Vec<i64>). Used for byte results.
#[must_use]
pub fn from_u8_slice(bs: &[u8]) -> Vec<i64> {
    bs.iter().map(|&b| i64::from(b)).collect()
}

/// Ipê `List Int` -> `[u8; N]`. A length mismatch returns `Err` and never
/// panics (honours "no runtime panic from well-typed Ipê code"). Used for
/// `[u8; N]` / `&[u8; N]` parameters; the generated wrapper instantiates
/// `E = IpeError` and the concrete `N`.
#[must_use]
pub fn to_u8_array<E: From<String>, const N: usize>(xs: &[i64]) -> IpeResult<E, [u8; N]> {
    if xs.len() != N {
        return IpeResult::Err(format!("expected {} bytes, got {}", N, xs.len()).into());
    }
    let mut a = [0u8; N];
    // len == N checked above; zip is total (no indexing).
    for (slot, &x) in a.iter_mut().zip(xs.iter()) {
        *slot = x as u8;
    }
    ok_res(a)
}

/// Ipê `List T` (Rust `&[T]`) -> fixed-size `[T; N]` with length check.
/// Mirrors `to_u8_array`'s never-panic discipline: returns `IpeResult::Err`
/// with a clear message on length mismatch. T: Clone is sufficient — the
/// elements are cloned out into the array.
pub fn to_array<E: From<String>, T: Clone, const N: usize>(xs: &[T]) -> IpeResult<E, [T; N]> {
    if xs.len() != N {
        return IpeResult::Err(format!("expected array of length {}, got {}", N, xs.len()).into());
    }
    let v: Vec<T> = xs.to_vec();
    match v.try_into() {
        Ok(a) => ok_res(a),
        Err(_) => IpeResult::Err("array length conversion failed".to_string().into()),
    }
}

// ===========================================
// Maybe
// ===========================================
// The serde derive is UNCONDITIONAL but its impls are generic-BOUND (the macro
// emits `impl<T: Serialize> … for IpeMaybe<T>`), so a `IpeMaybe<NonSerde>` is
// unaffected — yet a Ipe.Live model carrying a `Maybe X` field (X serde-able)
// serialises for the session store. Without this, any model with a `Maybe`/
// `Result` field failed E0277. NOTE: `serde` is therefore a NON-OPTIONAL dep in
// the runtime crate (core.rs is always compiled) — do NOT re-add `optional = true`.
//
// `Deserialize` is NOT derived — we use a manual impl (see below) that accepts
// BOTH the externally-tagged repr (session-store round-trip) AND a bare value
// (form data: `note=hello` → `Just("hello")`). `Serialize` stays derived (tagged)
// so the session store writes `{"Just":"x"}` / `"Nothing"` and the manual
// `Deserialize` reads those back correctly.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum IpeMaybe<T> {
    Nothing,
    Just(T),
}

// Custom `Deserialize` for `IpeMaybe<T>`.
//
// Accepted input shapes:
//   - Externally-tagged map `{"Just": v}` → `Just(T::deserialize(v))`.
//     This is what `Serialize` emits → session-store round-trip is preserved.
//   - Externally-tagged string `"Nothing"` (unit variant) → `Nothing`.
//   - Bare non-null value `v` → `Just(T::deserialize(v))`.
//     This is the form-data case: `note=hello` → the urlencoded deserialiser
//     presents a bare string, which the tagged derive rejects.
//   - `null` / absent (handled by `Default` + `#[serde(default)]` at the struct
//     field level) → `Nothing`.
//
// The "Nothing" string-variant sentinel is accepted for backward compat with
// any stored sessions, even though the serialiser now no longer writes bare
// `"Nothing"` (it stays externally-tagged as the unit variant string).
//
// Edge case: a bare string value of exactly `"Nothing"` in form data decodes as
// `Nothing`, not `Just("Nothing")`. This is the same trade-off as the tagged
// derive and is acceptable — form fields named `note` with the literal value
// "Nothing" are pathological; real user notes should not hit this.
impl<'de, T: serde::de::Deserialize<'de>> serde::de::Deserialize<'de> for IpeMaybe<T> {
    fn deserialize<D: serde::de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(IpeMaybeVisitor(std::marker::PhantomData))
    }
}

struct IpeMaybeVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: serde::de::Deserialize<'de>> serde::de::Visitor<'de> for IpeMaybeVisitor<T> {
    type Value = IpeMaybe<T>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a IpeMaybe value: `{\"Just\": v}`, `\"Nothing\"`, or a bare value")
    }

    // --- bare unit / null → Nothing ---
    fn visit_unit<E: serde::de::Error>(self) -> Result<IpeMaybe<T>, E> {
        Ok(IpeMaybe::Nothing)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<IpeMaybe<T>, E> {
        Ok(IpeMaybe::Nothing)
    }
    fn visit_some<D: serde::de::Deserializer<'de>>(self, d: D) -> Result<IpeMaybe<T>, D::Error> {
        T::deserialize(d).map(IpeMaybe::Just)
    }

    // --- bare string → "Nothing" sentinel OR Just(T) ---
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<IpeMaybe<T>, E> {
        if v == "Nothing" {
            return Ok(IpeMaybe::Nothing);
        }
        // Bare non-sentinel string: deserialize T from it.
        T::deserialize(serde::de::value::StrDeserializer::new(v)).map(IpeMaybe::Just)
    }

    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<IpeMaybe<T>, E> {
        if v == "Nothing" {
            return Ok(IpeMaybe::Nothing);
        }
        T::deserialize(serde::de::value::StringDeserializer::new(v)).map(IpeMaybe::Just)
    }

    // --- externally-tagged map `{"Just": v}` → Just(T) ---
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<IpeMaybe<T>, A::Error> {
        use serde::de::Error as _;
        let key: Option<String> = map.next_key()?;
        match key.as_deref() {
            Some("Just") => {
                let val: T = map.next_value()?;
                // Consume any remaining entries (defensive; tagged enums have one).
                while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    let _: serde::de::IgnoredAny = map.next_value()?;
                }
                Ok(IpeMaybe::Just(val))
            }
            Some("Nothing") => {
                // Unit variant as a map key (edge case from some serialisers).
                while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    let _: serde::de::IgnoredAny = map.next_value()?;
                }
                Ok(IpeMaybe::Nothing)
            }
            Some(other) => Err(A::Error::unknown_variant(other, &["Just", "Nothing"])),
            None => Err(A::Error::missing_field("Just")),
        }
    }

    // --- bare numerics / bool → Just(T) ---
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<IpeMaybe<T>, E> {
        T::deserialize(serde::de::value::BoolDeserializer::new(v)).map(IpeMaybe::Just)
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<IpeMaybe<T>, E> {
        T::deserialize(serde::de::value::I64Deserializer::new(v)).map(IpeMaybe::Just)
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<IpeMaybe<T>, E> {
        T::deserialize(serde::de::value::U64Deserializer::new(v)).map(IpeMaybe::Just)
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<IpeMaybe<T>, E> {
        T::deserialize(serde::de::value::F64Deserializer::new(v)).map(IpeMaybe::Just)
    }
}

impl<T> IpeMaybe<T> {
    pub fn with_default(self, def: T) -> T {
        match self {
            IpeMaybe::Just(v) => v,
            IpeMaybe::Nothing => def,
        }
    }
    pub fn is_just(&self) -> bool {
        matches!(self, IpeMaybe::Just(_))
    }
    pub fn is_nothing(&self) -> bool {
        matches!(self, IpeMaybe::Nothing)
    }
}

// `Nothing` is the natural zero of an absent `Maybe`, mirroring Go's
// `json.Unmarshal` decoding a missing nullable field to nil. This MANUAL impl
// (the derive would demand `T: Default`, which a `IpeMaybe<NonDefault>` field
// cannot satisfy) lets a form-target record carrying a `Maybe X` field qualify
// for the lenient `#[serde(default)]` form-decode stamp without an E0277.
// Deliberately NOT provided for `IpeResult`: an absent `Result` has no canonical
// zero (`Ok` vs `Err` is undecidable), so a Result-typed form field keeps the
// strict (non-Default) emission instead — see Emitter.hs `allFieldsDefaultable`.
//
// NOT `#[derive(Default)]` (clippy::derivable_impls): the derive stamps a
// `T: Default` bound on EVERY type param, which would defeat the point — a
// `IpeMaybe<NonDefault>` field must still have a default (its inner `T` is never
// constructed in the `Nothing` zero). This MANUAL impl is unbounded in `T`.
#[allow(clippy::derivable_impls)]
impl<T> Default for IpeMaybe<T> {
    fn default() -> Self {
        IpeMaybe::Nothing
    }
}

pub fn ipe_maybe_map<T, U>(m: IpeMaybe<T>, f: impl FnOnce(T) -> U) -> IpeMaybe<U> {
    match m {
        IpeMaybe::Just(v) => IpeMaybe::Just(f(v)),
        IpeMaybe::Nothing => IpeMaybe::Nothing,
    }
}

pub fn ipe_maybe_and_then<T, U>(m: IpeMaybe<T>, f: impl FnOnce(T) -> IpeMaybe<U>) -> IpeMaybe<U> {
    match m {
        IpeMaybe::Just(v) => f(v),
        IpeMaybe::Nothing => IpeMaybe::Nothing,
    }
}

/// `IpeMaybe<T>` -> `Option<T>` for FFI parameter coercion: a Ipê `Maybe X`
/// argument reaches the wrapper as `IpeMaybe<X>` but the underlying crate fn
/// takes `Option<…>`. The generated wrapper calls this then adapts the inner
/// value (`.as_deref()` for `Option<&str>`, `.map(|x| x as u16)` for narrowed
/// numerics, identity otherwise). Total: `Just -> Some`, `Nothing -> None`.
pub fn ipe_maybe_to_option<T>(m: IpeMaybe<T>) -> Option<T> {
    match m {
        IpeMaybe::Just(v) => Some(v),
        IpeMaybe::Nothing => None,
    }
}

// ===========================================
// Result (generic over error type E)
// ===========================================
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IpeResult<E, A> {
    Ok(A),
    Err(E),
}

impl<E, A> IpeResult<E, A> {
    pub fn is_ok(&self) -> bool {
        matches!(self, IpeResult::Ok(_))
    }
    pub fn is_err(&self) -> bool {
        matches!(self, IpeResult::Err(_))
    }
    pub fn with_default(self, def: A) -> A {
        match self {
            IpeResult::Ok(v) => v,
            IpeResult::Err(_) => def,
        }
    }
}

pub fn ipe_result_map<E, A, B>(r: IpeResult<E, A>, f: impl FnOnce(A) -> B) -> IpeResult<E, B> {
    match r {
        IpeResult::Ok(v) => IpeResult::Ok(f(v)),
        IpeResult::Err(e) => IpeResult::Err(e),
    }
}

pub fn ipe_result_and_then<E, A, B>(
    r: IpeResult<E, A>,
    f: impl FnOnce(A) -> IpeResult<E, B>,
) -> IpeResult<E, B> {
    match r {
        IpeResult::Ok(v) => f(v),
        IpeResult::Err(e) => IpeResult::Err(e),
    }
}

/// `Result.mapError : (e -> f) -> Result e a -> Result f a`. Container-first in
/// the runtime (matching `ipe_result_map` / `ipe_result_and_then`); the emitter
/// reverses the Ipê `(fn, result)` order via `kernel_swaps_first_two`. Maps the
/// `Err` channel and leaves the `Ok` value untouched — total, no panic path.
pub fn ipe_result_map_error<E, F, A>(
    r: IpeResult<E, A>,
    f: impl FnOnce(E) -> F,
) -> IpeResult<F, A> {
    match r {
        IpeResult::Ok(v) => IpeResult::Ok(v),
        IpeResult::Err(e) => IpeResult::Err(f(e)),
    }
}

/// `Result.toMaybe : Result e a -> Maybe a` — `Ok v` → `Just v`, `Err _` →
/// `Nothing` (the error is discarded). Total.
pub fn ipe_result_to_maybe<E, A>(r: IpeResult<E, A>) -> IpeMaybe<A> {
    match r {
        IpeResult::Ok(v) => IpeMaybe::Just(v),
        IpeResult::Err(_) => IpeMaybe::Nothing,
    }
}

/// `Result.fromMaybe : e -> Maybe a -> Result e a` — `Just v` → `Ok v`,
/// `Nothing` → `Err err` (the supplied error fills the missing case). Total.
/// Takes its arguments in Ipê order (`err`, then `maybe`), so no emitter arg
/// swap is needed.
pub fn ipe_result_from_maybe<E, A>(err: E, m: IpeMaybe<A>) -> IpeResult<E, A> {
    match m {
        IpeMaybe::Just(v) => IpeResult::Ok(v),
        IpeMaybe::Nothing => IpeResult::Err(err),
    }
}

// ===========================================
// Maybe / Result default + traverse helpers
// ===========================================
pub fn result_with_default<E, A>(def: A, r: IpeResult<E, A>) -> A {
    match r {
        IpeResult::Ok(v) => v,
        IpeResult::Err(_) => def,
    }
}

pub fn maybe_with_default<A>(def: A, m: IpeMaybe<A>) -> A {
    match m {
        IpeMaybe::Just(v) => v,
        IpeMaybe::Nothing => def,
    }
}

// `Result.traverse : (a -> Result e b) -> List a -> Result e (List b)`. Maps
// `f` across the list, collecting the `Ok` values; the FIRST `Err` (in list
// order) short-circuits with the real error. No `Clone` bound — each element is
// MOVED into `f`, matching the Ipê pure-Ipê one-pass definition. Total: no
// unwrap/index/panic (`Vec::push` grows, never indexes).
pub fn result_traverse<T0, T1, E>(
    f: impl Fn(T0) -> IpeResult<E, T1>,
    items: Vec<T0>,
) -> IpeResult<E, Vec<T1>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match f(item) {
            IpeResult::Ok(v) => out.push(v),
            IpeResult::Err(e) => return IpeResult::Err(e),
        }
    }
    IpeResult::Ok(out)
}

// ===========================================
// Result / Maybe applicative combinators (mapN / andMap / combine)
// ===========================================
// FUNCTION-FIRST argument order (matches the Ipê call surface AND the JsonDec
// `decode_mapN` runtime shape), so NO `kernel_swaps_first_two` entry is needed.
// The N-ary function is a MULTI-ARG Rust fn value — a Ipê arity-N function /
// record-alias auto-constructor lowers to `impl Fn(A, .., N) -> V`, so `f(a, b,
// ..)` type-checks. Each combinator is TOTAL: the first `Err` / `Nothing` in
// Ipê evaluation order short-circuits; the `Ok` / `Just` value type is never
// indexed or unwrapped. No panic, no `unsafe`, no allocation beyond the result.

/// `Result.map2 : (a -> b -> v) -> Result e a -> Result e b -> Result e v`.
/// First `Err` in (ra, rb) order wins — matches the nested-case `.ipe` def.
pub fn result_map2<E, A, B, V>(
    f: impl FnOnce(A, B) -> V,
    ra: IpeResult<E, A>,
    rb: IpeResult<E, B>,
) -> IpeResult<E, V> {
    let a = match ra {
        IpeResult::Ok(a) => a,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let b = match rb {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    IpeResult::Ok(f(a, b))
}

/// `Result.map3 : (a -> b -> c -> v) -> Result e a -> Result e b -> Result e c
/// -> Result e v`.
pub fn result_map3<E, A, B, C, V>(
    f: impl FnOnce(A, B, C) -> V,
    ra: IpeResult<E, A>,
    rb: IpeResult<E, B>,
    rc: IpeResult<E, C>,
) -> IpeResult<E, V> {
    let a = match ra {
        IpeResult::Ok(a) => a,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let b = match rb {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let c = match rc {
        IpeResult::Ok(c) => c,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    IpeResult::Ok(f(a, b, c))
}

/// `Result.map4 : (a -> b -> c -> d -> v) -> Result e a -> .. -> Result e v`.
pub fn result_map4<E, A, B, C, D, V>(
    f: impl FnOnce(A, B, C, D) -> V,
    ra: IpeResult<E, A>,
    rb: IpeResult<E, B>,
    rc: IpeResult<E, C>,
    rd: IpeResult<E, D>,
) -> IpeResult<E, V> {
    let a = match ra {
        IpeResult::Ok(a) => a,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let b = match rb {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let c = match rc {
        IpeResult::Ok(c) => c,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let d = match rd {
        IpeResult::Ok(d) => d,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    IpeResult::Ok(f(a, b, c, d))
}

/// `Result.map5 : (a -> b -> c -> d -> e -> v) -> Result er a -> .. -> Result
/// er v`. (`er` is the shared error channel; `e` is the fifth `Ok` value.)
pub fn result_map5<Er, A, B, C, D, E, V>(
    f: impl FnOnce(A, B, C, D, E) -> V,
    ra: IpeResult<Er, A>,
    rb: IpeResult<Er, B>,
    rc: IpeResult<Er, C>,
    rd: IpeResult<Er, D>,
    re: IpeResult<Er, E>,
) -> IpeResult<Er, V> {
    let a = match ra {
        IpeResult::Ok(a) => a,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let b = match rb {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let c = match rc {
        IpeResult::Ok(c) => c,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let d = match rd {
        IpeResult::Ok(d) => d,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    let e_val = match re {
        IpeResult::Ok(v) => v,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    IpeResult::Ok(f(a, b, c, d, e_val))
}

/// `Result.andMap : Result e a -> Result e (a -> b) -> Result e b`. The `.ipe`
/// definition inspects the FUNCTION result first, so its `Err` wins over the
/// value's `Err`. The function is applied only when BOTH are `Ok`.
pub fn result_and_map<E, A, B, F: FnOnce(A) -> B>(
    ra: IpeResult<E, A>,
    rf: IpeResult<E, F>,
) -> IpeResult<E, B> {
    match rf {
        IpeResult::Ok(f) => match ra {
            IpeResult::Ok(a) => IpeResult::Ok(f(a)),
            IpeResult::Err(e) => IpeResult::Err(e),
        },
        IpeResult::Err(e) => IpeResult::Err(e),
    }
}

/// `Result.combine : List (Result e a) -> Result e (List a)`. Collects every
/// `Ok`; the first `Err` in list order short-circuits.
#[must_use]
pub fn result_combine<E, A>(results: Vec<IpeResult<E, A>>) -> IpeResult<E, Vec<A>> {
    let mut out = Vec::with_capacity(results.len());
    for r in results {
        match r {
            IpeResult::Ok(v) => out.push(v),
            IpeResult::Err(e) => return IpeResult::Err(e),
        }
    }
    IpeResult::Ok(out)
}

/// `Maybe.map2 : (a -> b -> v) -> Maybe a -> Maybe b -> Maybe v`.
pub fn maybe_map2<A, B, V>(
    f: impl FnOnce(A, B) -> V,
    ma: IpeMaybe<A>,
    mb: IpeMaybe<B>,
) -> IpeMaybe<V> {
    let a = match ma {
        IpeMaybe::Just(a) => a,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let b = match mb {
        IpeMaybe::Just(b) => b,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    IpeMaybe::Just(f(a, b))
}

/// `Maybe.map3 : (a -> b -> c -> v) -> Maybe a -> Maybe b -> Maybe c -> Maybe v`.
pub fn maybe_map3<A, B, C, V>(
    f: impl FnOnce(A, B, C) -> V,
    ma: IpeMaybe<A>,
    mb: IpeMaybe<B>,
    mc: IpeMaybe<C>,
) -> IpeMaybe<V> {
    let a = match ma {
        IpeMaybe::Just(a) => a,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let b = match mb {
        IpeMaybe::Just(b) => b,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let c = match mc {
        IpeMaybe::Just(c) => c,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    IpeMaybe::Just(f(a, b, c))
}

/// `Maybe.map4 : (a -> b -> c -> d -> v) -> Maybe a -> .. -> Maybe v`.
pub fn maybe_map4<A, B, C, D, V>(
    f: impl FnOnce(A, B, C, D) -> V,
    ma: IpeMaybe<A>,
    mb: IpeMaybe<B>,
    mc: IpeMaybe<C>,
    md: IpeMaybe<D>,
) -> IpeMaybe<V> {
    let a = match ma {
        IpeMaybe::Just(a) => a,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let b = match mb {
        IpeMaybe::Just(b) => b,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let c = match mc {
        IpeMaybe::Just(c) => c,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let d = match md {
        IpeMaybe::Just(d) => d,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    IpeMaybe::Just(f(a, b, c, d))
}

/// `Maybe.map5 : (a -> b -> c -> d -> e -> v) -> Maybe a -> .. -> Maybe v`.
pub fn maybe_map5<A, B, C, D, E, V>(
    f: impl FnOnce(A, B, C, D, E) -> V,
    ma: IpeMaybe<A>,
    mb: IpeMaybe<B>,
    mc: IpeMaybe<C>,
    md: IpeMaybe<D>,
    me: IpeMaybe<E>,
) -> IpeMaybe<V> {
    let a = match ma {
        IpeMaybe::Just(a) => a,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let b = match mb {
        IpeMaybe::Just(b) => b,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let c = match mc {
        IpeMaybe::Just(c) => c,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let d = match md {
        IpeMaybe::Just(d) => d,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    let e = match me {
        IpeMaybe::Just(e) => e,
        IpeMaybe::Nothing => return IpeMaybe::Nothing,
    };
    IpeMaybe::Just(f(a, b, c, d, e))
}

/// `Maybe.andMap : Maybe a -> Maybe (a -> b) -> Maybe b`. Function-Maybe
/// inspected first (matches the `.ipe` definition).
pub fn maybe_and_map<A, B, F: FnOnce(A) -> B>(ma: IpeMaybe<A>, mf: IpeMaybe<F>) -> IpeMaybe<B> {
    match mf {
        IpeMaybe::Just(f) => match ma {
            IpeMaybe::Just(a) => IpeMaybe::Just(f(a)),
            IpeMaybe::Nothing => IpeMaybe::Nothing,
        },
        IpeMaybe::Nothing => IpeMaybe::Nothing,
    }
}

/// `Maybe.combine : List (Maybe a) -> Maybe (List a)`. Collects every `Just`;
/// the first `Nothing` short-circuits.
#[must_use]
pub fn maybe_combine<A>(maybes: Vec<IpeMaybe<A>>) -> IpeMaybe<Vec<A>> {
    let mut out = Vec::with_capacity(maybes.len());
    for m in maybes {
        match m {
            IpeMaybe::Just(v) => out.push(v),
            IpeMaybe::Nothing => return IpeMaybe::Nothing,
        }
    }
    IpeMaybe::Just(out)
}

// ===========================================
// Synchronous-panic gate (Go parity: rt.LogPanicAndExit)
// ===========================================
// The generated `fn main()` installs this FIRST so any panic that escapes the
// synchronous Ipê path — a div-by-zero (`a / 0`), an index-out-of-range, an
// arithmetic overflow, etc. — is CLASSIFIED into a Ipê error kind, logged
// structurally with a short correlation id, and the process exits 1 — instead of
// dumping a raw Rust backtrace. Mirrors Go's `defer rt.LogPanicAndExit()` on
// every emitted `func main()` (AGENTS.md "Synchronous-panic gate"). The hook is
// total (no unwrap/index/panic of its own) and honours `IPE_LOG_FORMAT=json`.

/// Map a Rust panic message to a Ipê error classification (Go's panic-class
/// taxonomy, restricted to the kinds reachable from well-typed Ipê on the typed
/// Rust backend — TypeMismatch/CoerceFailure are Go-runtime-only).
fn classify_panic(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("divide by zero") || m.contains("divisor of zero") {
        "DivisionByZero"
    } else if m.contains("index out of bounds") || m.contains("out of range") {
        "IndexOutOfRange"
    } else if m.contains("overflow") {
        "ArithmeticOverflow"
    } else {
        "Unexpected"
    }
}

/// 8 hex chars (4 bytes) of correlation id — derives from the wall-clock
/// sub-second component so two panics in one process don't collide. Total: a
/// clock read failure falls back to `0`.
fn short_err_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{n:08x}")
}

/// Extract a panic payload's message, classify it, emit the structured/plain
/// server-side log line (honouring `IPE_LOG_FORMAT=json`), and RETURN the 8-hex
/// correlation errId. SHARED by the exit-on-panic hook (`install_panic_classifier`,
/// used for Ipe.Cli/Tui binaries) and the server/live `CatchPanicLayer` responder
/// (`server::panic_response`).
///
/// Two load-bearing properties:
/// 1. **Total** — it runs in a panic-unwinding context; a panic of ITS OWN would
///    abort the process. Downcast falls back to `"panic"`; no unwrap/index.
/// 2. **Does NOT exit** — the raw message goes to the SERVER LOG only; the caller
///    decides the fate (the hook adds `process::exit(1)`; the HTTP layer returns a
///    500 carrying ONLY the errId, never the message — so a panic message that
///    happens to contain a secret / PII / internal path is never sent to a client).
pub fn classify_and_log_panic(payload: &(dyn std::any::Any + Send)) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    };
    let kind = classify_panic(&msg);
    let err_id = short_err_id();
    let json = crate::system::read_env_var("IPE_LOG_FORMAT")
        .is_ok_and(|v| v.eq_ignore_ascii_case("json"));
    if json {
        eprintln!(
            "{{\"level\":\"error\",\"kind\":\"{}\",\"errId\":\"{}\",\"message\":\"{}\"}}",
            kind,
            err_id,
            crate::telemetry::json_escape(&msg)
        );
    } else {
        eprintln!(
            "[error] {kind} (ref {err_id}): {}",
            scrub_log_controls(&msg)
        );
    }
    err_id
}

/// The JSON body for a server `CatchPanicLayer` 500: classifies + logs the panic
/// SERVER-SIDE (errId) via `classify_and_log_panic`, then returns a body carrying
/// ONLY the errId — NEVER the panic message. The SINGLE source of the 500 body
/// shape, shared by Ipe.Http.Server and Ipe.Live (each wraps it in a 500 Response
/// at its own `CatchPanicLayer::custom` site). Axum-free, so it lives in the
/// always-compiled `core` — the generated project includes `server.rs` only for
/// Ipe.Http.Server apps, so a Live-only app can't reference a server-module fn.
///
/// SECURITY: `err_id` (8 lowercase-hex chars) is the ONLY value interpolated; the
/// rest is a fixed literal. A panic message (free-form, may carry secrets / PII /
/// paths) never reaches this body.
pub fn panic_500_body(payload: &(dyn std::any::Any + Send)) -> String {
    let err_id = classify_and_log_panic(payload);
    format!("{{\"error\":\"internal server error\",\"ref\":\"{err_id}\"}}")
}

/// Install the classifying panic hook. Idempotent in effect (re-installing just
/// replaces the hook). Called at the top of generated `fn main()` for non-server
/// shapes (Ipe.Cli/Tui); server/live binaries rely on the per-request
/// `CatchPanicLayer` instead (so a handler panic returns a 500, not exit).
///
/// **Design note — hook logs then RESUMES the unwind (never calls exit).** Calling
/// `process::exit(1)` from the hook would prevent `catch_unwind` anywhere in the
/// process from absorbing panics, which breaks two load-bearing mechanisms:
///
///   1. `tokio::task::spawn(...)` internally uses `catch_unwind` to turn a task
///      panic into a `JoinError`.  The async-FFI binding bodies use this to satisfy
///      C5 (foreign `async fn` panics → `IpeResult::Err`).
///
///   2. `block_on`'s `std::thread::spawn(…).join()` catches a panicking entry
///      future at the OS-thread boundary and maps it to `IpeResult::Err`.
///
/// By resuming the unwind, both mechanisms can absorb the panic after the hook
/// has logged the classified error.  For a truly uncaught panic (nothing catches
/// it), the Rust runtime prints a backtrace and aborts/exits — still a clean
/// non-zero exit. The classified log line always fires first.
pub fn install_panic_classifier() {
    std::panic::set_hook(Box::new(|info| {
        // Log (classified, with errId) — diagnostic fires regardless of whether
        // the panic is subsequently caught by catch_unwind / tokio::task::spawn.
        let _ = classify_and_log_panic(info.payload());
        // Do NOT call process::exit — let the panic unwind propagate so that
        // catch_unwind callers (tokio task spawn, block_on thread join, async-FFI
        // wrappers) can absorb it and map it to a Ipê Err value.
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // IpeMaybe<T> Deserialize regressions — pinned in-tree for every T.
    // -----------------------------------------------------------------------

    // --- IpeMaybe<i64> ---

    #[test]
    fn ipe_maybe_i64_tagged_just() {
        let v: IpeMaybe<i64> = serde_json::from_str(r#"{"Just":5}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(5_i64));
    }

    #[test]
    fn ipe_maybe_i64_bare_int_becomes_just() {
        let v: IpeMaybe<i64> = serde_json::from_str("5").unwrap();
        assert_eq!(v, IpeMaybe::Just(5_i64));
    }

    #[test]
    fn ipe_maybe_i64_nothing_string() {
        let v: IpeMaybe<i64> = serde_json::from_str(r#""Nothing""#).unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_i64_null_becomes_nothing() {
        let v: IpeMaybe<i64> = serde_json::from_str("null").unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_i64_round_trip() {
        let original = IpeMaybe::Just(42_i64);
        let json = serde_json::to_string(&original).unwrap();
        let decoded: IpeMaybe<i64> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);

        let nothing: IpeMaybe<i64> = IpeMaybe::Nothing;
        let json2 = serde_json::to_string(&nothing).unwrap();
        let decoded2: IpeMaybe<i64> = serde_json::from_str(&json2).unwrap();
        assert_eq!(decoded2, nothing);
    }

    // --- IpeMaybe<bool> ---

    #[test]
    fn ipe_maybe_bool_bare_true_becomes_just() {
        let v: IpeMaybe<bool> = serde_json::from_str("true").unwrap();
        assert_eq!(v, IpeMaybe::Just(true));
    }

    #[test]
    fn ipe_maybe_bool_tagged_just() {
        let v: IpeMaybe<bool> = serde_json::from_str(r#"{"Just":false}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(false));
    }

    #[test]
    fn ipe_maybe_bool_null_becomes_nothing() {
        let v: IpeMaybe<bool> = serde_json::from_str("null").unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    // --- IpeMaybe<f64> ---

    #[test]
    fn ipe_maybe_f64_bare_float_becomes_just() {
        let v: IpeMaybe<f64> = serde_json::from_str("1.5").unwrap();
        assert_eq!(v, IpeMaybe::Just(1.5_f64));
    }

    #[test]
    fn ipe_maybe_f64_tagged_just() {
        let v: IpeMaybe<f64> = serde_json::from_str(r#"{"Just":2.5}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(2.5_f64));
    }

    #[test]
    fn ipe_maybe_f64_round_trip() {
        let original = IpeMaybe::Just(0.25_f64);
        let json = serde_json::to_string(&original).unwrap();
        let decoded: IpeMaybe<f64> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    // --- IpeMaybe<SmallStruct> ---
    //
    // The guardian confirmed that a bare map `{...}` is REJECTED (Err), NOT
    // mis-decoded as Just(struct).  A bare object arrives via visit_map; the
    // visitor matches on the first key — if it is not "Just"/"Nothing" it
    // returns unknown_variant Err, which is the correct safe behaviour.

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct SmallPoint {
        x: i32,
        y: i32,
    }

    #[test]
    fn ipe_maybe_struct_tagged_just() {
        let v: IpeMaybe<SmallPoint> = serde_json::from_str(r#"{"Just":{"x":1,"y":2}}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(SmallPoint { x: 1, y: 2 }));
    }

    #[test]
    fn ipe_maybe_struct_bare_map_is_rejected_not_mis_just() {
        // A bare `{"x":1,"y":2}` must NOT decode as Just(SmallPoint{1,2}).
        // The visitor's map arm checks the first key: "x" is not "Just"/"Nothing"
        // → unknown_variant error.  Correct, safe behaviour confirmed by guardian.
        let result: Result<IpeMaybe<SmallPoint>, _> = serde_json::from_str(r#"{"x":1,"y":2}"#);
        assert!(
            result.is_err(),
            "bare struct map must not silently decode as Just"
        );
    }

    #[test]
    fn ipe_maybe_struct_null_becomes_nothing() {
        let v: IpeMaybe<SmallPoint> = serde_json::from_str("null").unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_struct_round_trip() {
        let original = IpeMaybe::Just(SmallPoint { x: 10, y: 20 });
        let json = serde_json::to_string(&original).unwrap();
        let decoded: IpeMaybe<SmallPoint> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    // --- IpeMaybe<Vec<i64>> ---

    #[test]
    fn ipe_maybe_vec_i64_tagged_just() {
        let v: IpeMaybe<Vec<i64>> = serde_json::from_str(r#"{"Just":[1,2,3]}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(vec![1_i64, 2, 3]));
    }

    #[test]
    fn ipe_maybe_vec_i64_null_becomes_nothing() {
        let v: IpeMaybe<Vec<i64>> = serde_json::from_str("null").unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_vec_i64_round_trip() {
        let original = IpeMaybe::Just(vec![10_i64, 20, 30]);
        let json = serde_json::to_string(&original).unwrap();
        let decoded: IpeMaybe<Vec<i64>> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    // --- IpeMaybe<IpeMaybe<i64>> (nested) ---

    #[test]
    fn ipe_maybe_nested_just_just() {
        // {"Just":{"Just":5}} → Just(Just(5))
        let v: IpeMaybe<IpeMaybe<i64>> = serde_json::from_str(r#"{"Just":{"Just":5}}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(IpeMaybe::Just(5_i64)));
    }

    #[test]
    fn ipe_maybe_nested_just_nothing() {
        // {"Just":"Nothing"} → Just(Nothing)
        let v: IpeMaybe<IpeMaybe<i64>> = serde_json::from_str(r#"{"Just":"Nothing"}"#).unwrap();
        assert_eq!(v, IpeMaybe::Just(IpeMaybe::Nothing));
    }

    #[test]
    fn ipe_maybe_nested_nothing() {
        let v: IpeMaybe<IpeMaybe<i64>> = serde_json::from_str(r#""Nothing""#).unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_nested_null_becomes_nothing() {
        let v: IpeMaybe<IpeMaybe<i64>> = serde_json::from_str("null").unwrap();
        assert_eq!(v, IpeMaybe::Nothing);
    }

    #[test]
    fn ipe_maybe_nested_round_trip() {
        let original: IpeMaybe<IpeMaybe<i64>> = IpeMaybe::Just(IpeMaybe::Just(99_i64));
        let json = serde_json::to_string(&original).unwrap();
        let decoded: IpeMaybe<IpeMaybe<i64>> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    // -----------------------------------------------------------------------
    // Existing panic-classifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn classify_and_log_panic_returns_8hex_errid_and_never_panics() {
        // &str, String, and a non-string payload all yield an 8-hex errId — and
        // the call itself never panics (it runs in a panic-unwinding context).
        let s: &str = "divide by zero";
        let e1 = classify_and_log_panic(&s);
        let owned: String = "index out of bounds: the len is 0".to_string();
        let e2 = classify_and_log_panic(&owned);
        let other: i32 = 42; // non-string payload → "panic" fallback
        let e3 = classify_and_log_panic(&other);
        for id in [&e1, &e2, &e3] {
            assert_eq!(id.len(), 8, "errId not 8 chars: {id}");
            // The errId is the ONLY value interpolated into the HTTP 500 body, so
            // its charset MUST be [0-9a-f] — proving the body can carry nothing
            // attacker-influenced.
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "errId not lowercase hex: {id}"
            );
        }
    }

    #[test]
    fn classify_panic_maps_known_kinds() {
        assert_eq!(
            classify_panic("attempt to divide by zero"),
            "DivisionByZero"
        );
        assert_eq!(
            classify_panic("index out of bounds: the len is 3"),
            "IndexOutOfRange"
        );
        assert_eq!(
            classify_panic("attempt to add with overflow"),
            "ArithmeticOverflow"
        );
        assert_eq!(classify_panic("something else entirely"), "Unexpected");
    }

    // [B8] The Ipê-visible message NEVER contains the foreign error's Debug detail
    // (which can carry a bearer token / API key / URL from a transport error). It
    // is a fixed generic message + a correlation id only.
    #[derive(Debug)]
    struct SecretBearingError {
        #[allow(dead_code)]
        bearer: &'static str,
    }

    #[test]
    fn foreign_error_redacts_secret_from_ipe_message() {
        let e = SecretBearingError {
            bearer: "Bearer sk_live_SUPERSECRET_KEY",
        };
        let msg: String = ipe_error_from_foreign(e);
        assert!(
            !msg.contains("SUPERSECRET") && !msg.contains("Bearer") && !msg.contains("bearer"),
            "the foreign Debug detail (with the bearer token) must NOT reach the Ipe-visible \
             message — got: {msg:?}"
        );
        assert!(
            msg.starts_with("external operation failed (ref ") && msg.ends_with(')'),
            "the Ipe-visible message must be the fixed generic message + correlation id — \
             got: {msg:?}"
        );
        // The 8-hex correlation id is present between the fixed prefix and `)`.
        let id = msg
            .trim_start_matches("external operation failed (ref ")
            .trim_end_matches(')');
        assert_eq!(id.len(), 8, "correlation id is 8 hex chars — got: {id:?}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "id must be hex — got: {id:?}"
        );
    }

    // ── Result ↔ Maybe bridges — Elm-matching semantics ───────────────────

    #[test]
    fn result_to_maybe_matches_elm() {
        // Elm: toMaybe (Ok 5) == Just 5; toMaybe (Err e) == Nothing.
        let ok: IpeResult<String, i64> = IpeResult::Ok(5);
        assert_eq!(ipe_result_to_maybe(ok), IpeMaybe::Just(5));
        let err: IpeResult<String, i64> = IpeResult::Err("boom".to_string());
        assert_eq!(ipe_result_to_maybe(err), IpeMaybe::Nothing);
    }

    #[test]
    fn result_from_maybe_matches_elm() {
        // Elm: fromMaybe e (Just v) == Ok v; fromMaybe e Nothing == Err e.
        let just: IpeResult<String, i64> =
            ipe_result_from_maybe("missing".to_string(), IpeMaybe::Just(9));
        assert_eq!(just, IpeResult::Ok(9));
        let nothing: IpeResult<String, i64> =
            ipe_result_from_maybe("missing".to_string(), IpeMaybe::Nothing);
        assert_eq!(nothing, IpeResult::Err("missing".to_string()));
    }
}
