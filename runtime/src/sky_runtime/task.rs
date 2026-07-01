// Task combinators — generic over error type E.
use super::*;
use std::future::ready;

pub fn block_on<E, A>(future: SkyTask<E, A>) -> SkyResult<E, A>
where
    E: From<String> + Send + 'static,
    A: Send + 'static,
{
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => return SkyResult::Err(format!("tokio runtime init failed: {}", e).into()),
    };
    match std::thread::spawn(move || rt.block_on(future)).join() {
        Ok(r) => r,
        Err(_) => SkyResult::Err("async task panicked".to_string().into()),
    }
}

// Main-thread driver for the Sky.Webview entry shape.
//
// `block_on` (above) drives the entry future on a SPAWNED OS thread (so a
// panic inside the future can be `.join()`-mapped to an `Err` instead of
// aborting the process). That spawn is fatal for Sky.Webview: tao/winit's
// `EventLoop` and Cocoa's `NSApplication` MUST be created and run on the
// process's TRUE main thread on macOS (a hard Cocoa requirement — there is no
// any-thread escape hatch), and Windows likewise expects the main thread. The
// webview `event_loop.run(...)` lives inside the entry Task's future, so the
// future itself has to be polled on the main thread.
//
// This driver runs the future on the CURRENT (main) thread via a
// `current_thread` tokio runtime — no `std::thread::spawn`, so `event_loop.run`
// constructs and runs on the main thread on every OS. The current-thread
// runtime still drives any async work the webview Task chain does BEFORE it
// hands the thread to `event_loop.run` (pre-webview `andThen` I/O, etc.),
// because `block_on` on a `current_thread` runtime cooperatively polls the
// whole future tree on this one thread. `enable_all()` keeps timers + I/O
// drivers available.
//
// TOTALITY: runtime-init failure returns `Err` (no unwrap/expect/panic). There
// is no spawn here, so there is no `.join()` panic-catch — a panic inside the
// webview future would propagate (the synchronous-panic gate at the entry
// boundary classifies it). That is acceptable for the webview shape: the
// webview path itself is total (window/webview construction failure returns
// `SkyResult::Err`), so a panic would be a genuine compiler/runtime bug, not a
// well-typed-Sky-reachable abort.
pub fn block_on_current_thread<E, A>(future: SkyTask<E, A>) -> SkyResult<E, A>
where
    E: From<String> + Send + 'static,
    A: Send + 'static,
{
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return SkyResult::Err(format!("tokio runtime init failed: {}", e).into()),
    };
    rt.block_on(future)
}

pub fn task_succeed<E: Send + 'static, A: Send + 'static>(a: A) -> SkyTask<E, A> {
    Box::pin(ready(ok_res::<E, A>(a)))
}

pub fn task_map<E, A, B>(
    f: impl FnOnce(A) -> B + Send + 'static,
    task: SkyTask<E, A>,
) -> SkyTask<E, B>
where
    E: Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
{
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(a) => ok_res(f(a)),
            SkyResult::Err(e) => SkyResult::Err(e),
        }
    })
}

pub fn task_and_then<E, A, B>(
    task: SkyTask<E, A>,
    f: impl FnOnce(A) -> SkyTask<E, B> + Send + 'static,
) -> SkyTask<E, B>
where
    E: Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
{
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(a) => f(a).await,
            SkyResult::Err(e) => SkyResult::Err(e),
        }
    })
}

pub fn task_map_error<E1, E2, A>(
    f: impl FnOnce(E1) -> E2 + Send + 'static,
    task: SkyTask<E1, A>,
) -> SkyTask<E2, A>
where
    E1: Send + 'static,
    E2: Send + 'static,
    A: Send + 'static,
{
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(a) => ok_res(a),
            SkyResult::Err(e) => SkyResult::Err(f(e)),
        }
    })
}

pub fn task_lazy<E: Send + 'static, A: Send + 'static>(
    f: impl FnOnce() -> SkyTask<E, A> + Send + 'static,
) -> SkyTask<E, A> {
    Box::pin(async move { f().await })
}

pub fn task_from_result<E: Send + 'static, A: Send + 'static>(r: SkyResult<E, A>) -> SkyTask<E, A> {
    Box::pin(ready(r))
}

pub fn task_and_then_result<E, A, B>(
    f: impl FnOnce(A) -> SkyResult<E, B> + Send + 'static,
    task: SkyTask<E, A>,
) -> SkyTask<E, B>
where
    E: Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
{
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(a) => f(a),
            SkyResult::Err(e) => SkyResult::Err(e),
        }
    })
}

pub fn task_on_error<E, A>(
    f: impl FnOnce(E) -> SkyTask<E, A> + Send + 'static,
    task: SkyTask<E, A>,
) -> SkyTask<E, A>
where
    E: Send + 'static,
    A: Send + 'static,
{
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(a) => ok_res(a),
            SkyResult::Err(e) => f(e).await,
        }
    })
}

pub fn task_fail<E: Send + 'static, A: Send + 'static>(e: E) -> SkyTask<E, A> {
    Box::pin(ready(SkyResult::Err(e)))
}

pub fn task_perform<E: Send + 'static, A: Send + 'static>(task: SkyTask<E, A>) -> SkyTask<E, ()> {
    Box::pin(async move {
        match task.await {
            SkyResult::Ok(_) => ok_res(()),
            SkyResult::Err(e) => SkyResult::Err(e),
        }
    })
}

pub fn task_sequence<E: Send + 'static, A: Send + 'static>(
    tasks: Vec<SkyTask<E, A>>,
) -> SkyTask<E, Vec<A>> {
    Box::pin(async move {
        let mut out = Vec::with_capacity(tasks.len());
        for t in tasks {
            match t.await {
                SkyResult::Ok(a) => out.push(a),
                SkyResult::Err(e) => return SkyResult::Err(e),
            }
        }
        ok_res(out)
    })
}

pub fn task_run<E: From<String> + Send + 'static, A: Send + 'static>(
    task: SkyTask<E, A>,
) -> SkyResult<E, A> {
    block_on(task)
}

pub fn task_parallel<E: From<String> + Send + 'static, A: Send + 'static>(
    tasks: Vec<SkyTask<E, A>>,
) -> SkyTask<E, Vec<A>> {
    Box::pin(async move {
        let handles: Vec<tokio::task::JoinHandle<SkyResult<E, A>>> =
            tasks.into_iter().map(tokio::spawn).collect();
        let mut out = Vec::with_capacity(handles.len());
        for h in handles {
            let result = match h.await {
                Ok(r) => r,
                Err(_) => SkyResult::Err("parallel task panicked".to_string().into()),
            };
            match result {
                SkyResult::Ok(a) => out.push(a),
                SkyResult::Err(e) => return SkyResult::Err(e),
            }
        }
        ok_res(out)
    })
}

// Task.retryWith : RetryPolicy e -> Task e a -> Task e a
//
// A real retry loop, faithful to runtime-go/rt/task_retry.go. The two things
// Rust could not give the old run-once stub — re-running the one-shot
// `SkyTask` future, and reading the generated, runtime-unnameable `RetryPolicy`
// / `ShouldRetry` ADT fields — are both supplied by CODEGEN now:
//   * The policy is DESTRUCTURED at the call site into the primitive fields
//     (`max_attempts` / `base_ms` / `jitter` / `kind`) plus a `should_retry`
//     closure lowered from the `ShouldRetry e` ADT (`RetryAlways` → `|_| true`,
//     `RetryWhen f` → `move |e| f(e.clone())`).
//   * The task argument is wrapped in a re-runnable `make_task : impl Fn() ->
//     SkyTask<E, A>` closure, so each attempt rebuilds a fresh future (the
//     side effects re-fire per attempt, exactly as Go re-invokes its thunk).
//
// Semantics (mirror Go's `Task_retryWith` loop):
//   attempt 1..=max_attempts:
//     run make_task().await
//       Ok(a)  → return Ok(a)
//       Err(e) → if attempt == max_attempts → return Err(e)   (last attempt)
//                else if !should_retry(&e)  → return Err(e)   (short-circuit)
//                else sleep(compute_delay(...)) and loop
// The final Err is the LAST attempt's error (so the caller still sees a real
// error). `max_attempts` is clamped to ≥ 1 (0 / 1 both mean "run once").
//
// TOTALITY: no unwrap / expect / panic / indexing. Jitter randomness comes from
// the runtime's existing total `lcg_next()` LCG (same source as Random.*),
// never `thread_rng` (which could panic on a poisoned global).
pub fn task_retry_with<E, A>(
    max_attempts: i64,
    base_ms: i64,
    jitter: bool,
    kind: i64,
    should_retry: impl Fn(&E) -> bool + Send + 'static,
    make_task: impl Fn() -> SkyTask<E, A> + Send + 'static,
) -> SkyTask<E, A>
where
    E: Send + 'static,
    A: Send + 'static,
{
    Box::pin(async move {
        let attempts = if max_attempts < 1 { 1 } else { max_attempts };
        let base = if base_ms < 0 { 0 } else { base_ms };
        let mut attempt: i64 = 1;
        loop {
            match make_task().await {
                SkyResult::Ok(a) => return ok_res(a),
                SkyResult::Err(e) => {
                    if attempt >= attempts {
                        return SkyResult::Err(e);
                    }
                    if !should_retry(&e) {
                        return SkyResult::Err(e);
                    }
                    let delay = retry_compute_delay(kind, base, attempt, jitter);
                    if delay > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
                    }
                    attempt += 1;
                }
            }
        }
    })
}

// Backoff cap (ms). Mirrors Go's `retryDelayCapMs` — exponential growth and the
// post-jitter delay are both clamped here so a huge attempt count or base can't
// produce an unbounded sleep.
const RETRY_DELAY_CAP_MS: i64 = 30_000;
const RETRY_KIND_EXPONENTIAL: i64 = 1;

// Port of Go's `computeDelay`. Wait before attempt n+1 (1-indexed: attempt 1
// runs, then sleep compute_delay(1), then attempt 2, ...). Linear → `base`
// every time; exponential → `base * 2^(attempt-1)` capped at 30 s. Jitter
// multiplies by a uniform factor in [0.5, 1.5). Total: saturating arithmetic,
// no overflow panic, result clamped to [0, RETRY_DELAY_CAP_MS].
fn retry_compute_delay(kind: i64, base_ms: i64, attempt: i64, jitter: bool) -> i64 {
    let mut d = base_ms;
    if kind == RETRY_KIND_EXPONENTIAL {
        // base * 2^(attempt-1). Guard the shift (and the multiply) against
        // overflow on large attempt counts — saturate to the cap instead.
        if (1..=30).contains(&attempt) {
            let factor: i64 = 1i64 << (attempt - 1);
            d = base_ms.saturating_mul(factor);
        } else {
            d = RETRY_DELAY_CAP_MS;
        }
    }
    if d > RETRY_DELAY_CAP_MS {
        d = RETRY_DELAY_CAP_MS;
    }
    if jitter && d > 0 {
        // Uniform in [0.5*d, 1.5*d). lcg_next() is the runtime's total LCG;
        // map its top 53 bits to a float in [0, 1) like random_float does.
        super::random::lcg_init();
        let unit = (super::random::lcg_next() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0);
        let scaled = (d as f64) * (0.5 + unit);
        // round-to-nearest, then re-clamp.
        d = scaled.round() as i64;
        if d > RETRY_DELAY_CAP_MS {
            d = RETRY_DELAY_CAP_MS;
        }
    }
    if d < 0 {
        d = 0;
    }
    d
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    // ── compute_delay: linear, exponential, cap, jitter-bounds ──

    #[test]
    fn delay_linear_is_constant() {
        // kind=0 (linear): always `base`, ignoring attempt.
        assert_eq!(retry_compute_delay(0, 100, 1, false), 100);
        assert_eq!(retry_compute_delay(0, 100, 5, false), 100);
    }

    #[test]
    fn delay_exponential_doubles() {
        // kind=1: base * 2^(attempt-1).
        assert_eq!(retry_compute_delay(1, 100, 1, false), 100);
        assert_eq!(retry_compute_delay(1, 100, 2, false), 200);
        assert_eq!(retry_compute_delay(1, 100, 3, false), 400);
        assert_eq!(retry_compute_delay(1, 100, 4, false), 800);
    }

    #[test]
    fn delay_capped_at_30s() {
        // A large exponential must clamp to RETRY_DELAY_CAP_MS, never overflow.
        assert_eq!(retry_compute_delay(1, 1000, 20, false), RETRY_DELAY_CAP_MS);
        assert_eq!(retry_compute_delay(1, 1000, 99, false), RETRY_DELAY_CAP_MS);
        // Even a huge base saturates rather than panicking.
        assert_eq!(
            retry_compute_delay(1, i64::MAX, 5, false),
            RETRY_DELAY_CAP_MS
        );
    }

    #[test]
    fn delay_zero_base_is_zero() {
        assert_eq!(retry_compute_delay(0, 0, 3, false), 0);
        assert_eq!(retry_compute_delay(1, 0, 3, false), 0);
        // jitter on a zero delay stays zero (guarded by `d > 0`).
        assert_eq!(retry_compute_delay(1, 0, 3, true), 0);
    }

    #[test]
    fn delay_jitter_stays_in_bounds() {
        // Jitter multiplies by a uniform factor in [0.5, 1.5); result must land
        // in [0.5*d, 1.5*d] and never exceed the cap. Probe many draws.
        let base = 1000;
        for _ in 0..1000 {
            let d = retry_compute_delay(0, base, 1, true);
            assert!(d >= 500, "jitter delay {} below 0.5*base", d);
            assert!(d <= 1500, "jitter delay {} above 1.5*base", d);
            assert!(d <= RETRY_DELAY_CAP_MS);
        }
    }

    // ── task_retry_with loop semantics ──

    // A re-runnable task factory backed by a shared counter: increments on every
    // attempt, fails until the counter reaches `threshold`, then succeeds.
    fn transient_factory(
        counter: Arc<AtomicI64>,
        threshold: i64,
    ) -> impl Fn() -> SkyTask<String, i64> + Send + Sync + 'static {
        move || {
            let counter = counter.clone();
            Box::pin(async move {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= threshold {
                    ok_res::<String, i64>(n)
                } else {
                    SkyResult::Err(format!("boom-{}", n))
                }
            })
        }
    }

    #[test]
    fn retry_transient_succeeds() {
        // Fails attempts 1-2, succeeds on attempt 3; maxAttempts=5 → Ok(3).
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            5,
            0,
            false,
            0,
            |_e: &String| true,
            transient_factory(counter.clone(), 3),
        );
        match block_on(task) {
            SkyResult::Ok(n) => assert_eq!(n, 3),
            SkyResult::Err(e) => panic!("expected Ok(3), got Err({})", e),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3, "task ran 3 times");
    }

    #[test]
    fn retry_always_fails_returns_last_err_after_max() {
        // threshold unreachable; maxAttempts=4 → Err after exactly 4 runs.
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            4,
            0,
            false,
            0,
            |_e: &String| true,
            transient_factory(counter.clone(), 999),
        );
        match block_on(task) {
            SkyResult::Ok(n) => panic!("expected Err, got Ok({})", n),
            SkyResult::Err(e) => assert_eq!(e, "boom-4", "last attempt's err"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 4, "ran exactly maxAttempts");
    }

    #[test]
    fn retry_short_circuits_when_should_retry_false() {
        // should_retry → false: stop after the first Err (1 run), maxAttempts=5.
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            5,
            0,
            false,
            0,
            |_e: &String| false,
            transient_factory(counter.clone(), 999),
        );
        match block_on(task) {
            SkyResult::Ok(n) => panic!("expected Err, got Ok({})", n),
            SkyResult::Err(e) => assert_eq!(e, "boom-1", "first attempt's err"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1, "short-circuited after 1");
    }

    #[test]
    fn retry_should_retry_predicate_consulted_on_err() {
        // Retry only while the err is "boom-1"; once it's "boom-2", stop.
        // threshold high so it never succeeds; predicate gates the loop.
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            10,
            0,
            false,
            0,
            |e: &String| e == "boom-1",
            transient_factory(counter.clone(), 999),
        );
        match block_on(task) {
            SkyResult::Ok(n) => panic!("expected Err, got Ok({})", n),
            // attempt1 → boom-1 (retry), attempt2 → boom-2 (predicate false → stop).
            SkyResult::Err(e) => assert_eq!(e, "boom-2"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retry_max_attempts_clamped_to_one() {
        // maxAttempts=0 means "run once" (clamped to 1), no retry.
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            0,
            0,
            false,
            0,
            |_e: &String| true,
            transient_factory(counter.clone(), 999),
        );
        match block_on(task) {
            SkyResult::Ok(n) => panic!("expected Err, got Ok({})", n),
            SkyResult::Err(e) => assert_eq!(e, "boom-1"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1, "clamped to a single run");
    }

    #[test]
    fn retry_succeeds_first_try_runs_once() {
        // Threshold 1: succeeds on the first attempt; no further runs.
        let counter = Arc::new(AtomicI64::new(0));
        let task = task_retry_with(
            5,
            0,
            false,
            0,
            |_e: &String| true,
            transient_factory(counter.clone(), 1),
        );
        match block_on(task) {
            SkyResult::Ok(n) => assert_eq!(n, 1),
            SkyResult::Err(e) => panic!("expected Ok(1), got Err({})", e),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1, "ran once on success");
    }
}
