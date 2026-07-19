//! Async-FFI bridge runtime contract:
//!
//! 1. `block_on` drives every entry on ONE process-global runtime, so a
//!    reactor-registered handle (FFI client, listener, timer) constructed in
//!    one `block_on` stays usable in a later one — a fresh runtime per entry
//!    would leave the handle on a dead reactor.
//! 2. `AbortOnDrop` — the guard the emitted async FFI wrapper arms around its
//!    spawned foreign task — aborts the inner task when the wrapper future is
//!    dropped before completion (`Task.parallel` early-cancel), so a
//!    cancelled foreign call cannot keep producing side effects.
#![cfg(feature = "tokio")]

use ipe_runtime_rust::{AbortOnDrop, IpeResult, IpeTask, block_on, ok_res};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
fn reactor_handle_survives_across_two_block_on_entries() -> Result<(), String> {
    let make: IpeTask<String, tokio::net::TcpListener> = Box::pin(async {
        match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => ok_res(l),
            Err(e) => IpeResult::Err(format!("bind failed: {e}")),
        }
    });
    let listener = match block_on(make) {
        IpeResult::Ok(l) => l,
        IpeResult::Err(e) => return Err(format!("first block_on failed: {e}")),
    };
    // The listener is registered with the reactor of the runtime that bound
    // it; accepting inside a LATER entry only works if both entries share one
    // runtime.
    let roundtrip: IpeTask<String, bool> = Box::pin(async move {
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(e) => return IpeResult::Err(format!("local_addr failed: {e}")),
        };
        let (accepted, connected) =
            tokio::join!(listener.accept(), tokio::net::TcpStream::connect(addr));
        match (accepted, connected) {
            (Ok(_), Ok(_)) => ok_res(true),
            (a, c) => IpeResult::Err(format!("accept={a:?} connect={c:?}")),
        }
    });
    match block_on(roundtrip) {
        IpeResult::Ok(true) => Ok(()),
        other => Err(format!(
            "listener bound in entry 1 must accept in entry 2 (shared reactor): {other:?}"
        )),
    }
}

#[test]
fn abort_on_drop_cancels_the_spawned_foreign_task() {
    let side_effect = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&side_effect);
    // The emitted async wrapper shape: spawn, arm the guard, await, defuse.
    let wrapper: IpeTask<String, i64> = Box::pin(async move {
        let handle = tokio::task::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            side_effect.store(true, Ordering::SeqCst);
            1_i64
        });
        let guard = AbortOnDrop::new(handle.abort_handle());
        let joined = handle.await;
        guard.defuse();
        match joined {
            Ok(v) => ok_res(v),
            Err(e) => IpeResult::Err(format!("{e:?}")),
        }
    });
    let outcome: IpeResult<String, bool> = block_on(Box::pin(async move {
        // Cancel the wrapper mid-flight: the timeout drops the wrapper future
        // before the inner sleep completes, so the guard must abort the spawn.
        let cancelled = tokio::time::timeout(Duration::from_millis(5), wrapper).await;
        if cancelled.is_ok() {
            return IpeResult::Err("wrapper completed before the cancel window".to_owned());
        }
        // Give a leaked (non-aborted) inner task time to run its side effect.
        tokio::time::sleep(Duration::from_millis(150)).await;
        ok_res(observed.load(Ordering::SeqCst))
    }));
    assert!(
        matches!(outcome, IpeResult::Ok(false)),
        "the aborted foreign task must not produce its side effect: {outcome:?}"
    );
}

#[test]
fn defused_guard_lets_the_join_outcome_through() {
    let wrapper: IpeTask<String, i64> = Box::pin(async move {
        let handle = tokio::task::spawn(async move { 7_i64 });
        let guard = AbortOnDrop::new(handle.abort_handle());
        let joined = handle.await;
        guard.defuse();
        match joined {
            Ok(v) => ok_res(v),
            Err(e) => IpeResult::Err(format!("{e:?}")),
        }
    });
    assert!(matches!(block_on(wrapper), IpeResult::Ok(7)));
}
