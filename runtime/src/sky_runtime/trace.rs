// Std.Trace — opt-in app-level tracing spans / events / attributes.
//
// Mirrors the contract from the Go runtime: `span name task` runs `task` and
// returns its result UNCHANGED (the value/error flow through untouched), plus a
// span around it; `event` / `attr` mark a point / annotate the active span.
// Output is opt-in via SKY_TRACE (off by default → zero noise), but the wrapped
// task ALWAYS runs regardless, so spans never change program behaviour.
use super::*;
use std::time::Instant;

fn trace_enabled() -> bool {
    std::env::var("SKY_TRACE")
        .map(|v| !v.is_empty() && v != "0" && v != "false")
        .unwrap_or(false)
}

// Trace.span : String -> Task e a -> Task e a
pub fn trace_span<E: Send + 'static, A: Send + 'static>(
    name: String,
    task: SkyTask<E, A>,
) -> SkyTask<E, A> {
    Box::pin(async move {
        let on = trace_enabled();
        let start = Instant::now();
        if on {
            eprintln!("[trace] span start {}", name);
        }
        let result = task.await;
        let elapsed = start.elapsed();
        let ok = matches!(result, SkyResult::Ok(_));
        // Always record the span into the telemetry ring (the Sky Console reads
        // it); the stderr line stays opt-in via SKY_TRACE.
        super::telemetry::record_span(&name, elapsed.as_micros() as u64, ok);
        if on {
            let outcome = if ok { "ok" } else { "err" };
            eprintln!(
                "[trace] span end {} ({} ms, {})",
                name,
                elapsed.as_millis(),
                outcome
            );
        }
        result
    })
}

// Trace.event : String -> Task Error ()
pub fn trace_event<E: Send + 'static>(name: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        if trace_enabled() {
            eprintln!("[trace] event {}", name);
        }
        ok_res(())
    })
}

// Trace.attr : String -> String -> Task Error ()
// Keys are namespaced under `sky.trace.` to match the Go runtime.
pub fn trace_attr<E: Send + 'static>(key: String, value: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        if trace_enabled() {
            eprintln!("[trace] attr sky.trace.{} = {}", key, value);
        }
        ok_res(())
    })
}
