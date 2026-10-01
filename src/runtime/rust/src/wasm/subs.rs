//! Browser-WASM `Sub` bridge — the wasm analogue of `tea.rs`'s native
//! `SubRuntime`. Drives `Sub.every`/`Time.every` via `gloo-timers` and any
//! `IpeSub::Source` (currently: `Sub.subscribeTopic`, `wasm::pubsub`) via its
//! own teardown thunk.
//!
//! Same contract as native (one program, one model, re-evaluated each
//! update): the scheduler calls [`SubManager::update`] once per
//! `mount`/`flush` cycle with the freshly computed `subscriptions(model)`.
//! `Sub.every` timers go through the shared keyed reconcile
//! ([`EveryTimers::reconcile`]), so a still-requested interval keeps its
//! browser timer and phase; sources have no identity and are torn down and
//! respawned.

use std::cell::RefCell;
use std::rc::Rc;

use crate::tea::{EveryTimer, EveryTimers, IpeSub, SubPlan};

/// One live `Sub.every` browser timer and the messages each tick delivers.
///
/// `gloo_timers::callback::Interval`'s `Drop` cancels the browser timer, so
/// dropping this IS the teardown.
struct BrowserEvery<M> {
    msgs: Rc<RefCell<Vec<M>>>,
    _interval: gloo_timers::callback::Interval,
}

impl<M> EveryTimer<M> for BrowserEvery<M> {
    fn retarget(&self, msgs: Vec<M>) {
        *self.msgs.borrow_mut() = msgs;
    }
}

pub(crate) struct SubManager<M> {
    every: EveryTimers<BrowserEvery<M>>,
    teardowns: Vec<Box<dyn FnOnce()>>,
}

impl<M: Clone + 'static> SubManager<M> {
    pub(crate) fn new() -> Self {
        SubManager {
            every: EveryTimers::default(),
            teardowns: Vec::new(),
        }
    }

    /// Re-evaluate against `sub`, dispatching every produced `Msg` through `emit`.
    ///
    /// `emit` is the TEA scheduler's `enqueue` callback, the same one the
    /// delegated DOM listeners use.
    pub(crate) fn update(&mut self, sub: IpeSub<M>, emit: &Rc<dyn Fn(M)>) {
        let SubPlan { every, sources } = SubPlan::of(sub);
        for teardown in self.teardowns.drain(..) {
            teardown();
        }
        self.every.reconcile(every, |interval, msgs| {
            let msgs = Rc::new(RefCell::new(msgs));
            let tick_msgs = Rc::clone(&msgs);
            let emit = Rc::clone(emit);
            // `setInterval` ticks first after one period, not at t=0 — the
            // browser analogue of native's sleep-loop first-tick timing.
            let interval =
                gloo_timers::callback::Interval::new(interval.browser_delay_ms(), move || {
                    // Copy out before emitting: `emit` may re-enter `update`,
                    // which retargets this timer's messages.
                    let batch = tick_msgs.borrow().clone();
                    for msg in batch {
                        (emit)(msg);
                    }
                });
            BrowserEvery {
                msgs,
                _interval: interval,
            }
        });
        for spawn in sources {
            self.teardowns.push(spawn(Rc::clone(emit)));
        }
    }
}
