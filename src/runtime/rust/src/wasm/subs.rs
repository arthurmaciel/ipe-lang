//! Browser-WASM `Sub` bridge — the wasm analogue of `tea.rs`'s native
//! `SubManager`. Drives `Sub.every`/`Time.every` via `gloo-timers` and any
//! `IpeSub::Source` (currently: `Sub.subscribeTopic`, `wasm::pubsub`) via its
//! own teardown thunk.
//!
//! Same "stop everything, respawn from the new `Sub`" contract as native
//! (one program, one model, re-evaluated each tick) — the
//! scheduler calls [`SubManager::update`] once per `mount`/`flush` cycle with
//! the freshly computed `subscriptions(model)`.

use std::rc::Rc;

use crate::tea::IpeSub;

/// One live `Sub.every` timer. Kept alive for as long as the subscription is
/// active; `gloo_timers::callback::Interval`'s `Drop` cancels the browser
/// timer automatically, so clearing this `Vec` IS the teardown.
struct ActiveEvery {
    _interval: gloo_timers::callback::Interval,
}

pub(crate) struct SubManager<M> {
    everies: Vec<ActiveEvery>,
    teardowns: Vec<Box<dyn FnOnce()>>,
    _marker: std::marker::PhantomData<M>,
}

impl<M: Clone + 'static> SubManager<M> {
    pub(crate) fn new() -> Self {
        SubManager {
            everies: Vec::new(),
            teardowns: Vec::new(),
            _marker: std::marker::PhantomData,
        }
    }

    /// Tear down every active timer/source, then rebuild from `sub`. `emit`
    /// dispatches a produced `Msg` back into the TEA scheduler (the same
    /// `enqueue` callback the delegated DOM listeners use).
    pub(crate) fn update(&mut self, sub: IpeSub<M>, emit: &Rc<dyn Fn(M)>) {
        self.stop_all();
        self.spawn(sub, emit);
    }

    fn stop_all(&mut self) {
        // `Interval::drop` cancels the underlying `clearInterval` — clearing
        // the Vec IS the timer teardown.
        self.everies.clear();
        for teardown in self.teardowns.drain(..) {
            teardown();
        }
    }

    fn spawn(&mut self, sub: IpeSub<M>, emit: &Rc<dyn Fn(M)>) {
        match sub {
            IpeSub::None => {}
            IpeSub::Batch(items) => {
                for it in items {
                    self.spawn(it, emit);
                }
            }
            IpeSub::Every { ms, msg } => {
                // Matches native's `ms <= 0` guard (a non-positive interval
                // never fires, rather than a busy-loop / panic).
                if ms <= 0 {
                    return;
                }
                let emit = Rc::clone(emit);
                // `setInterval` ticks first after `ms`, not at t=0 — the
                // browser analogue of native's sleep-loop first-tick timing.
                let interval = gloo_timers::callback::Interval::new(
                    u32::try_from(ms).unwrap_or(u32::MAX),
                    move || {
                        (emit)(msg.clone());
                    },
                );
                self.everies.push(ActiveEvery {
                    _interval: interval,
                });
            }
            IpeSub::Source(spawn) => {
                let teardown = spawn(Rc::clone(emit));
                self.teardowns.push(teardown);
            }
        }
    }
}
