//! Shape-agnostic TEA debugger core — recorder, re-fold, export, and import.
//!
//! Enabled only when the `debugger` feature is active (`ipe build/run --debugger`).
//! A non-`--debugger` build carries zero code from this module.
//!
//! ## Data model
//!
//! The history is a ring buffer of `Msg` values plus a rolling base `Model`.
//! The `Model` at step N is computed by applying `update` over the retained
//! messages from the base, discarding every `Cmd` produced — no effect is
//! re-fired during reconstruction.
//!
//! Memory is `1 Model + N Msgs` where N ≤ `cap`. On overflow the oldest `Msg`
//! is dropped and the base `Model` is advanced one step so re-fold always starts
//! from a consistent checkpoint.
//!
//! ## Integration shapes
//!
//! - **[`History`]** (fn-pointer variant): stores the `update` fn pointer
//!   alongside the log. Self-contained and unit-testable. Used for tests and as
//!   the reference implementation.
//! - **[`RecordBuffer`]** (data-only variant): stores only the log and base.
//!   The caller supplies the `update` closure at overflow/reconstruct time.
//!   Used by the WASM TEA driver, which already owns the update closure in the
//!   `App` struct and should not duplicate it.
//!
//! ## Security
//!
//! - `Secret`-bearing `Msg` types are not seal-legal, so export is unavailable
//!   for them while live recording still works (in-memory values need no codec).
//! - Import runs through the total, fail-closed seal decoder: a malformed or
//!   oversized blob is dropped whole — no partial value, no panic.
//! - The `debugger` feature is absent from `ipe release`, so no recorder code
//!   can ship in a production artifact.

use std::collections::VecDeque;

use crate::tea::IpeCmd;

// Server-driven TEA debugger: session-scoped history and overlay HTML.
// All items gated on `feature = "debugger"` via the inner `#![cfg(...)]`.
pub mod server;

// Terminal (TUI) time-travel debugger.
// Gated on both `feature = "debugger"` AND `not(target_arch = "wasm32")`
// via the inner `#![cfg(...)]` in tui.rs — zero code on wasm32.
pub mod tui;

/// The default message-log capacity when none is configured.
pub const DEFAULT_HISTORY_CAP: usize = 512;

// ── Internal shared step type ──────────────────────────────────────────────

/// One step: the dispatched message and the model that followed it.
///
/// Storing `model_after` lets the driver advance the rolling base on overflow
/// without re-calling `update` on the dropped step's message.
struct Step<Msg, Model> {
    msg: Msg,
    model_after: Model,
}

// ── RecordBuffer — data-only, driver-integrated variant ───────────────────

/// A bounded, rolling TEA message log with a rolling base `Model`.
///
/// Unlike [`History`], `RecordBuffer` does NOT store the `update` function.
/// The caller supplies it at overflow (via [`RecordBuffer::record`]) and at
/// reconstruction time (via [`RecordBuffer::reconstruct`]). This avoids
/// duplicating the update closure in drivers (such as the WASM TEA sink) that
/// already own it.
///
/// Invariant (by construction): `log.len() <= cap` always holds.
pub struct RecordBuffer<Msg, Model> {
    /// The model that precedes the oldest retained `Msg`. Advanced on overflow.
    base: Model,
    /// Retained steps, newest at the back.
    log: VecDeque<Step<Msg, Model>>,
    /// Maximum retained messages. At least 1.
    cap: usize,
}

impl<Msg: Clone, Model: Clone> RecordBuffer<Msg, Model> {
    /// Create a new, empty buffer starting from `initial_model`.
    ///
    /// `cap` is clamped to a minimum of 1.
    #[must_use]
    pub fn new(initial_model: Model, cap: usize) -> Self {
        Self {
            base: initial_model,
            log: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Record one live-pass step.
    ///
    /// `update` is called exactly once on overflow to advance the base over the
    /// evicted step; the resulting `Cmd` is discarded. On a non-overflow call
    /// `update` is never called.
    pub fn record<F>(&mut self, msg: Msg, model_after: Model, update: &F)
    where
        F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
    {
        if self.log.len() >= self.cap
            && let Some(oldest) = self.log.pop_front()
        {
            let (advanced, _cmd) = update(oldest.msg, self.base.clone());
            self.base = advanced;
        }
        self.log.push_back(Step { msg, model_after });
    }

    /// Reconstruct the `Model` at retained-log step `n` (0-indexed).
    ///
    /// Replays `update` from the base over the first `n + 1` retained messages,
    /// discarding every `Cmd`. Returns `None` when `n` is out of range.
    ///
    /// No effect is re-fired: this is a pure fold over the message log.
    #[must_use]
    pub fn reconstruct<F>(&self, n: usize, update: &F) -> Option<Model>
    where
        F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
    {
        if n >= self.log.len() {
            return None;
        }
        let mut model = self.base.clone();
        for step in self.log.iter().take(n + 1) {
            let (next, _cmd) = update(step.msg.clone(), model);
            model = next;
        }
        Some(model)
    }

    /// Number of steps retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.log.len()
    }

    /// `true` when no steps have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.log.is_empty()
    }

    /// The configured cap.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Reference to the rolling base model.
    #[must_use]
    pub fn base(&self) -> &Model {
        &self.base
    }

    /// Iterator over retained messages, oldest first.
    pub fn msgs(&self) -> impl Iterator<Item = &Msg> {
        self.log.iter().map(|s| &s.msg)
    }
}

// ── History — fn-pointer variant, self-contained ──────────────────────────

/// A bounded, rolling TEA session history with a stored `update` fn pointer.
///
/// Self-contained: stores the `update` function so callers need not supply it
/// on every operation. Use this in unit tests and wherever a fn pointer (not a
/// closure) drives the TEA loop.
///
/// For closure-based drivers (the WASM TEA sink), use [`RecordBuffer`] instead.
pub struct History<Msg, Model> {
    inner: RecordBuffer<Msg, Model>,
    update: fn(Msg, Model) -> (Model, IpeCmd<Msg>),
}

impl<Msg: Clone, Model: Clone> History<Msg, Model> {
    /// Create a new history seeded from `initial_model`.
    #[must_use]
    pub fn new(
        initial_model: Model,
        update: fn(Msg, Model) -> (Model, IpeCmd<Msg>),
        cap: usize,
    ) -> Self {
        Self {
            inner: RecordBuffer::new(initial_model, cap),
            update,
        }
    }

    /// Record one live-pass step.
    pub fn record(&mut self, msg: Msg, model_after: Model) {
        let update = self.update;
        self.inner
            .record(msg, model_after, &move |m, mdl| update(m, mdl));
    }

    /// Reconstruct the model at step `n` (0-indexed in the retained window).
    #[must_use]
    pub fn reconstruct(&self, n: usize) -> Option<Model> {
        let update = self.update;
        self.inner.reconstruct(n, &move |m, mdl| update(m, mdl))
    }

    /// Number of steps retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` when no steps have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// The configured cap.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.inner.cap()
    }

    /// Reference to the rolling base model.
    #[must_use]
    pub fn base(&self) -> &Model {
        self.inner.base()
    }

    /// Iterator over retained messages, oldest first.
    pub fn msgs(&self) -> impl Iterator<Item = &Msg> {
        self.inner.msgs()
    }
}

// ── Export / Import ────────────────────────────────────────────────────────

/// Why export failed.
#[derive(Debug)]
pub enum ExportError {
    /// JSON serialization failed.
    Encode(String),
}

impl core::fmt::Display for ExportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExportError::Encode(detail) => write!(f, "debugger export failed: {detail}"),
        }
    }
}

/// Export the message log as JSON bytes.
///
/// Only available when `Msg: serde::Serialize` (seal-legal). A `Secret`-bearing
/// `Msg` cannot implement `serde::Serialize`, so this function is unavailable
/// for such types at compile time — consistent with the seal-legality gate.
///
/// The exported form is a JSON array of messages. The base model is not
/// included; an import always reseeds from the caller-supplied initial model.
#[cfg(feature = "json")]
pub fn export_msgs<Msg, Model>(buf: &RecordBuffer<Msg, Model>) -> Result<Vec<u8>, ExportError>
where
    Msg: Clone + serde::Serialize,
    Model: Clone,
{
    let msgs: Vec<&Msg> = buf.log.iter().map(|s| &s.msg).collect();
    serde_json::to_vec(&msgs).map_err(|e| ExportError::Encode(e.to_string()))
}

/// Export the message log of a [`History`] as JSON bytes.
#[cfg(feature = "json")]
pub fn export_history_msgs<Msg, Model>(
    history: &History<Msg, Model>,
) -> Result<Vec<u8>, ExportError>
where
    Msg: Clone + serde::Serialize,
    Model: Clone,
{
    export_msgs(&history.inner)
}

/// Import a message log from bytes produced by [`export_msgs`], seeding the
/// history from `initial_model`.
///
/// Fail-closed: a malformed, oversized, or type-mismatching blob yields `None`
/// — no partial value, no panic. Limits mirror the seal codec defaults
/// (5 MiB byte budget, depth 128).
#[cfg(feature = "json")]
pub fn import_msgs<Msg, Model, F>(
    bytes: &[u8],
    initial_model: Model,
    update: F,
    cap: usize,
) -> Option<RecordBuffer<Msg, Model>>
where
    Msg: Clone + serde::de::DeserializeOwned,
    Model: Clone,
    F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
{
    use crate::seal_codec::DEFAULT_SEAL_MAX_INPUT_BYTES;

    // Byte budget, before any allocation.
    if bytes.len() > DEFAULT_SEAL_MAX_INPUT_BYTES {
        return None;
    }

    // Depth-bounded parse: reject malformed / deeply-nested JSON.
    let s = core::str::from_utf8(bytes).ok()?;

    let msgs: Vec<Msg> = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let effective_cap = cap.max(1);
    let mut buf = RecordBuffer::new(initial_model, effective_cap);

    for msg in msgs {
        // Compute the post-update model so `Step::model_after` is correct.
        let current = if buf.log.is_empty() {
            buf.base.clone()
        } else {
            buf.log
                .back()
                .map(|s| s.model_after.clone())
                .unwrap_or_else(|| buf.base.clone())
        };
        let (model_after, _cmd) = update(msg.clone(), current);
        buf.record(msg, model_after, &update);
    }
    Some(buf)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum TestMsg {
        Add(i64),
    }

    #[derive(Clone, Debug, PartialEq)]
    struct TestModel {
        count: i64,
    }

    fn test_update(msg: TestMsg, model: TestModel) -> (TestModel, IpeCmd<TestMsg>) {
        let TestMsg::Add(n) = msg;
        (
            TestModel {
                count: model.count + n,
            },
            IpeCmd::None,
        )
    }

    // record → reconstruct equals the live model at each step.
    #[test]
    fn reconstruct_matches_live_model() {
        let mut history = History::new(TestModel { count: 0 }, test_update, 16);
        let msgs = [TestMsg::Add(1), TestMsg::Add(2), TestMsg::Add(3)];
        let mut live = TestModel { count: 0 };
        for (i, msg) in msgs.iter().enumerate() {
            let (next, _) = test_update(msg.clone(), live.clone());
            live = next.clone();
            history.record(msg.clone(), next);

            let reconstructed = history.reconstruct(i).expect("step must be in range");
            assert_eq!(
                reconstructed, live,
                "reconstruct at step {i} must equal live model"
            );
        }
    }

    // A Cmd-carrying step reconstructs WITHOUT re-firing any effect.
    #[test]
    fn reconstruct_does_not_refire_cmd() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let effect_count = Arc::new(AtomicUsize::new(0));

        #[derive(Clone, Debug, PartialEq)]
        enum Msg {
            Trigger,
        }
        #[derive(Clone, Debug, PartialEq)]
        struct Model {
            triggered: bool,
        }

        // The live pass fires one effect (simulated by incrementing the counter).
        let counter = Arc::clone(&effect_count);
        counter.fetch_add(1, Ordering::SeqCst);

        // The stored update fn does NOT touch the counter — only the live pass does.
        let update_fn: fn(Msg, Model) -> (Model, IpeCmd<Msg>) =
            |_msg, _m| (Model { triggered: true }, IpeCmd::None);

        let post_model = Model { triggered: true };
        let mut history = History::new(Model { triggered: false }, update_fn, 8);
        history.record(Msg::Trigger, post_model.clone());

        let before = effect_count.load(Ordering::SeqCst);
        let r = history.reconstruct(0).expect("step 0 must exist");
        let after = effect_count.load(Ordering::SeqCst);

        assert_eq!(r, post_model);
        assert_eq!(
            before, after,
            "reconstruct must not fire any additional effects"
        );
    }

    // Ring buffer caps memory; rolling base keeps reconstruct correct after overflow.
    #[test]
    fn ring_buffer_caps_and_rolling_base_correct() {
        let cap = 3usize;
        let mut history = History::new(TestModel { count: 0 }, test_update, cap);

        for i in 1..=6i64 {
            let current = if history.inner.log.is_empty() {
                history.inner.base.clone()
            } else {
                history
                    .inner
                    .log
                    .back()
                    .map(|s| s.model_after.clone())
                    .unwrap()
            };
            let (next, _) = test_update(TestMsg::Add(i), current);
            history.record(TestMsg::Add(i), next);
        }

        assert_eq!(history.len(), cap, "log must not exceed cap");

        // msgs 4, 5, 6 retained; base is count after 1+2+3 = 6.
        // After msg 4: 6+4=10; after msg 5: 15; after msg 6: 21.
        let last = history.reconstruct(cap - 1).expect("last step must exist");
        assert_eq!(last.count, 21, "rolling base must keep reconstruct correct");
    }

    // Export/import round-trip for a seal-legal Msg.
    #[cfg(feature = "json")]
    #[test]
    fn export_import_round_trip() {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        enum RtMsg {
            Add(i64),
            Reset,
        }
        #[derive(Clone, Debug, PartialEq)]
        struct RtModel {
            count: i64,
        }

        fn rt_update(msg: RtMsg, m: RtModel) -> (RtModel, IpeCmd<RtMsg>) {
            let next = match msg {
                RtMsg::Add(n) => RtModel { count: m.count + n },
                RtMsg::Reset => RtModel { count: 0 },
            };
            (next, IpeCmd::None)
        }

        let mut history = History::new(RtModel { count: 0 }, rt_update, 16);
        let (m1, _) = rt_update(RtMsg::Add(5), history.inner.base.clone());
        history.record(RtMsg::Add(5), m1.clone());
        let (m2, _) = rt_update(RtMsg::Add(3), m1);
        history.record(RtMsg::Add(3), m2);

        let bytes = export_history_msgs(&history).expect("export must succeed");

        let imported =
            import_msgs::<RtMsg, RtModel, _>(&bytes, RtModel { count: 0 }, rt_update, 16)
                .expect("import must succeed");

        let orig_final = history.reconstruct(1).expect("step 1");
        let imp_final = imported
            .reconstruct(1, &rt_update)
            .expect("step 1 after import");
        assert_eq!(
            orig_final, imp_final,
            "round-trip must preserve model state"
        );
    }

    // Malformed / oversized import blobs are dropped fail-closed (no panic, no partial value).
    #[cfg(feature = "json")]
    #[test]
    fn import_malformed_blob_dropped_fail_closed() {
        #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
        enum Msg {
            Noop,
        }
        #[derive(Clone)]
        struct Model;
        fn upd(_msg: Msg, _m: Model) -> (Model, IpeCmd<Msg>) {
            (Model, IpeCmd::None)
        }

        // Malformed JSON.
        assert!(
            import_msgs::<Msg, Model, _>(b"{not valid json}", Model, upd, 8).is_none(),
            "malformed blob must yield None"
        );

        // Oversized blob.
        let oversized = vec![b' '; crate::seal_codec::DEFAULT_SEAL_MAX_INPUT_BYTES + 1];
        assert!(
            import_msgs::<Msg, Model, _>(&oversized, Model, upd, 8).is_none(),
            "oversized blob must yield None"
        );

        // Type mismatch — valid JSON but wrong shape.
        assert!(
            import_msgs::<Msg, Model, _>(b"42", Model, upd, 8).is_none(),
            "type-mismatch blob must yield None"
        );
    }

    // ── Overlay scrub wiring (unit-level) ──────────────────────────────────

    // Selecting step N via `reconstruct(N)` returns the model at that step;
    // no additional effects fire beyond the original live pass.
    #[test]
    fn scrub_reconstruct_wiring() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let effect_count = Arc::new(AtomicUsize::new(0));

        // update: increment count; live pass also bumps the effect counter.
        let counter = Arc::clone(&effect_count);
        let live_update = move |msg: TestMsg, model: TestModel| -> (TestModel, IpeCmd<TestMsg>) {
            counter.fetch_add(1, Ordering::SeqCst);
            let TestMsg::Add(n) = msg;
            (
                TestModel {
                    count: model.count + n,
                },
                IpeCmd::None,
            )
        };

        // The fn-pointer used for reconstruct does NOT touch the counter.
        let reconstruct_update: fn(TestMsg, TestModel) -> (TestModel, IpeCmd<TestMsg>) =
            test_update;

        // Simulate live pass: record 3 steps.
        let mut buf = RecordBuffer::new(TestModel { count: 0 }, 16);
        let mut live = TestModel { count: 0 };
        let msgs_in = [TestMsg::Add(10), TestMsg::Add(5), TestMsg::Add(3)];
        for msg in &msgs_in {
            let (next, _cmd) = live_update(msg.clone(), live.clone());
            buf.record(msg.clone(), next.clone(), &|m, mdl| {
                // The record-time closure is the one used for base advancement
                // on overflow — use the non-counting fn here.
                reconstruct_update(m, mdl)
            });
            live = next;
        }

        let live_effects = effect_count.load(Ordering::SeqCst);
        assert_eq!(live_effects, 3, "live pass must fire exactly 3 effects");

        // Scrub to step 1 (after msgs[0]+msgs[1]): count = 15.
        let at_step_1 = buf.reconstruct(1, &reconstruct_update).expect("step 1");
        assert_eq!(
            at_step_1.count, 15,
            "reconstruct at step 1 must yield count=15"
        );

        // Scrub to step 0 (after msgs[0]): count = 10.
        let at_step_0 = buf.reconstruct(0, &reconstruct_update).expect("step 0");
        assert_eq!(
            at_step_0.count, 10,
            "reconstruct at step 0 must yield count=10"
        );

        // No additional effects fired during scrubbing.
        let after_scrub_effects = effect_count.load(Ordering::SeqCst);
        assert_eq!(
            live_effects, after_scrub_effects,
            "reconstruct must not fire any additional effects"
        );

        // Returning to live (no reconstruct call needed — caller uses live model).
        assert_eq!(live.count, 18, "live model is count=18 after 3 steps");
    }

    // `Secret`-bearing values are redacted in the message label rendered by
    // `IpeStringify::ipe_show` — the same path the overlay's `label_fn` uses.
    #[cfg(feature = "secret")]
    #[test]
    fn secret_redacted_in_label() {
        use crate::secret::secret_from_string;
        use crate::stringify::IpeStringify;

        let secret = secret_from_string("super-secret-token".to_owned());
        let rendered = secret.ipe_show();
        assert!(
            !rendered.contains("super-secret-token"),
            "Secret must not appear in rendered label; got: {rendered:?}"
        );
        assert_eq!(
            rendered, "<redacted>",
            "Secret must stringify to the fixed redacted placeholder"
        );
    }
}
