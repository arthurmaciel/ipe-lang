//! Shape-agnostic dev-loop control wire — the single parent→child transport
//! definition shared by hot-swap (`ipe watch` appearance edits) and the
//! time-travel debugger across the tui/cli/worker/web shapes.
//!
//! ## Why one wire
//!
//! Both features are the same structural problem: a dev-loop capability that
//! `ipe watch` (the parent) must deliver into the spawned child. Rather than two
//! ad-hoc message formats — the web hot-appearance JSON body and a separate
//! debugger command channel — every message rides ONE [`ControlFrame`] so the
//! wire is defined exactly once. The per-shape transport (loopback HTTP for web,
//! a loopback control socket for tui/cli/worker) carries this frame unchanged.
//!
//! ## Availability
//!
//! Compiled only when a dev-loop surface is present: the `web` feature (the
//! existing hot-appearance endpoints) or the `debugger` feature (the recorder).
//! Both imply `serde`, so the frame's derives are unconditional here. A pure
//! `ipe release` artifact carries neither feature, so this module — and every
//! control surface built on it — is absent from production by construction.
//!
//! ## Bounded by construction
//!
//! A decoded frame is length-delimited and capped at [`MAX_FRAME_LEN`]: a length
//! prefix beyond the cap is rejected before a single payload byte is read, so no
//! remote-supplied length can drive an unbounded allocation.
//!
//! ## No type erasure
//!
//! The frame carries serialized, concrete payloads — an [`AppearancePatch`] or a
//! [`DebugCmd`] — never a `dyn Any`. A shape's transport monomorphizes on its own
//! concrete `(Msg, Model)`; the frame is the wire between them, not an erased
//! carrier of them.

use serde::{Deserialize, Serialize};

/// The largest control frame accepted off the wire, in bytes.
///
/// A length prefix exceeding this cap is refused by [`decode_frame`] before the
/// body is read. The dev-loop's frames are small (an appearance patch or a
/// scrub command); the cap is a generous ceiling that still forecloses an
/// unbounded allocation driven by a malformed or hostile length prefix.
pub const MAX_FRAME_LEN: usize = 1 << 20;

/// An appearance-only hot-swap patch, in wire form.
///
/// Mirrors the classifier's `ViewPatch` (`ipe`-cli `hot_classify`): the running
/// app's PREVIOUS baked-defaults signature (the key its compiled `view` passes to
/// `from_defaults`, hence the runtime overlay's match key) plus the
/// `(index, new_value)` deltas to overlay. The app is never recompiled, so it
/// still bakes the old signature — the patch carries the OLD defaults, not the
/// new. This is the runtime-owned single source of truth for the appearance wire;
/// the classifier's `ViewPatch` is a type alias of it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AppearancePatch {
    /// The PREVIOUS baked defaults, in emit order — the overlay's match key.
    /// An absent field decodes as empty (a patch touching nothing), matching the
    /// tolerant body the web hot-appearance endpoint has always accepted.
    #[serde(default)]
    pub defaults: Vec<String>,
    /// The appearance delta: `(index, new_value)` per changed default.
    #[serde(default)]
    pub patch: Vec<(usize, String)>,
}

/// A time-travel debugger command from `ipe watch` (parent) to the child.
///
/// Each variant maps to an operation the recorder already supports on its
/// bounded message ring (`debugger::{History, RecordBuffer}`): stepping the
/// scrub cursor, resetting to the base model, inspecting a step's model, or
/// resuming live tailing. Reconstruction re-folds `update` over the retained
/// messages and re-fires no `Cmd`, so a scrub never perturbs the live model.
#[cfg(feature = "debugger")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugCmd {
    /// Move the scrub cursor to step `n` (reconstruct the model at that step).
    StepTo(usize),
    /// Step the cursor one step toward the base.
    Back,
    /// Step the cursor one step toward the live tail.
    Forward,
    /// Reset the cursor to the base model and resume live mode.
    Reset,
    /// Ask the child to render the model at step `n` and reply with a
    /// [`ControlFrame::ModelSnapshot`].
    InspectModel(usize),
    /// Leave scrub mode and resume tailing live messages.
    LiveTail,
}

/// The one wire message shared by every dev-loop control surface.
///
/// Parent→child carries a hot-swap patch or a debug command; child→parent
/// carries a reply ([`ControlFrame::Ack`] or [`ControlFrame::ModelSnapshot`]).
/// A single enum keeps the wire to one definition — the web loopback endpoints
/// and the tui/cli/worker loopback socket agree by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlFrame {
    /// Parent→child: apply an appearance-only hot-swap (#2749). Re-render the
    /// current model against the patched literals and diff-repaint — never
    /// replay through `update`.
    HotAppearance(AppearancePatch),
    /// Parent→child: a time-travel debugger command (#2587).
    #[cfg(feature = "debugger")]
    Debug(DebugCmd),
    /// Child→parent: the outcome of applying a frame. `ok = false` with a
    /// `detail` lets the parent fall back (e.g. to a full rebuild) rather than
    /// proceed on a silently-dropped command.
    Ack {
        /// Whether the frame was applied.
        ok: bool,
        /// A short, human-readable outcome note (never a secret).
        detail: String,
    },
    /// Child→parent: the rendered model at a scrub step, in reply to
    /// [`DebugCmd::InspectModel`].
    ModelSnapshot {
        /// The step index the snapshot reflects.
        step: usize,
        /// The model rendered via `IpeStringify` (read-only; never re-fires a
        /// `Cmd`).
        rendered: String,
    },
}

/// Why a frame could not be decoded off the wire.
///
/// A typed error channel (never a bare `String`): the transport parses the
/// untrusted byte stream into a `ControlFrame` at exactly one point, and every
/// rejection is one of these named, fail-closed outcomes.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The declared frame length exceeds [`MAX_FRAME_LEN`]; refused before the
    /// body is read.
    TooLong {
        /// The declared length that was refused.
        declared: usize,
    },
    /// The byte slice is shorter than the declared frame length.
    Truncated,
    /// The frame body is not a valid serialized [`ControlFrame`].
    Malformed,
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooLong { declared } => {
                write!(
                    f,
                    "control frame length {declared} exceeds the {MAX_FRAME_LEN}-byte cap"
                )
            }
            FrameError::Truncated => write!(f, "control frame is truncated"),
            FrameError::Malformed => write!(f, "control frame body is malformed"),
        }
    }
}

/// Encode a frame as a length-delimited record: a 4-byte big-endian length
/// prefix followed by the JSON body.
///
/// Returns `None` only when the serialized body would exceed [`MAX_FRAME_LEN`]
/// — the same ceiling the decoder enforces, so a sender never emits a record the
/// receiver would refuse.
pub fn encode_frame(frame: &ControlFrame) -> Option<Vec<u8>> {
    let body = serde_json::to_vec(frame).ok()?;
    if body.len() > MAX_FRAME_LEN {
        return None;
    }
    // `body.len() <= MAX_FRAME_LEN` (1 MiB) fits a u32 on every target.
    let len = u32::try_from(body.len()).ok()?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

/// Decode one length-delimited frame from the front of `bytes`, returning the
/// frame and the number of bytes consumed.
///
/// Fail-closed at the boundary: a length prefix beyond [`MAX_FRAME_LEN`] is
/// refused before the body is touched, a short slice is `Truncated`, and a body
/// that is not a valid frame is `Malformed` — no partial value, no panic.
pub fn decode_frame(bytes: &[u8]) -> Result<(ControlFrame, usize), FrameError> {
    let Some(len_prefix) = bytes.get(..4) else {
        return Err(FrameError::Truncated);
    };
    // The slice is exactly 4 bytes, so the array conversion cannot fail.
    let Ok(len_arr) = <[u8; 4]>::try_from(len_prefix) else {
        return Err(FrameError::Malformed);
    };
    // u32 → usize is a widening conversion on every supported target.
    let Ok(declared) = usize::try_from(u32::from_be_bytes(len_arr)) else {
        return Err(FrameError::Malformed);
    };
    if declared > MAX_FRAME_LEN {
        return Err(FrameError::TooLong { declared });
    }
    let end = 4usize.checked_add(declared).ok_or(FrameError::Truncated)?;
    let Some(body) = bytes.get(4..end) else {
        return Err(FrameError::Truncated);
    };
    let frame = serde_json::from_slice(body).map_err(|_| FrameError::Malformed)?;
    Ok((frame, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance() -> AppearancePatch {
        AppearancePatch {
            defaults: vec!["padding: 12px".to_string()],
            patch: vec![(0, "padding: 16px".to_string())],
        }
    }

    // Encode a frame and decode it back, asserting a lossless round-trip that
    // consumes exactly the encoded record.
    #[allow(clippy::expect_used)] // test helper — a failed round-trip is a test failure
    fn assert_round_trip(frame: &ControlFrame) {
        let bytes = encode_frame(frame).expect("a small frame must encode");
        let (decoded, consumed) = decode_frame(&bytes).expect("a self-encoded frame must decode");
        assert_eq!(&decoded, frame, "round-trip preserves the frame");
        assert_eq!(consumed, bytes.len(), "one frame consumes its whole record");
    }

    #[test]
    fn hot_appearance_frame_round_trips() {
        assert_round_trip(&ControlFrame::HotAppearance(appearance()));
    }

    #[test]
    fn ack_and_snapshot_round_trip() {
        assert_round_trip(&ControlFrame::Ack {
            ok: true,
            detail: "applied".to_string(),
        });
        assert_round_trip(&ControlFrame::ModelSnapshot {
            step: 7,
            rendered: "Model { count = 7 }".to_string(),
        });
    }

    #[cfg(feature = "debugger")]
    #[test]
    fn debug_cmd_frames_round_trip() {
        for cmd in [
            DebugCmd::StepTo(3),
            DebugCmd::Back,
            DebugCmd::Forward,
            DebugCmd::Reset,
            DebugCmd::InspectModel(9),
            DebugCmd::LiveTail,
        ] {
            assert_round_trip(&ControlFrame::Debug(cmd));
        }
    }

    #[test]
    fn oversized_length_prefix_is_refused_before_the_body() {
        // A hostile length prefix claiming far more than the cap must be turned
        // back before any body byte is read — no allocation the prefix dictates.
        let declared = MAX_FRAME_LEN + 1;
        #[allow(clippy::expect_used)] // test — the cap fits u32 on every target
        let declared_u32 = u32::try_from(declared).expect("cap + 1 fits u32");
        let mut wire = declared_u32.to_be_bytes().to_vec();
        // Only one real body byte follows: a naive reader that trusts the prefix
        // would try to read `declared` bytes.
        wire.push(b'x');
        assert_eq!(
            decode_frame(&wire),
            Err(FrameError::TooLong { declared }),
            "an over-cap length is refused before the body"
        );
    }

    #[test]
    fn truncated_slice_is_rejected_not_panicked() {
        // Fewer than 4 length bytes.
        assert_eq!(decode_frame(&[0, 0, 1]), Err(FrameError::Truncated));
        // A valid length prefix but a body shorter than declared.
        let mut wire = 8u32.to_be_bytes().to_vec();
        wire.extend_from_slice(b"ab");
        assert_eq!(decode_frame(&wire), Err(FrameError::Truncated));
    }

    #[test]
    fn appearance_patch_matches_the_classifiers_wire_shape() {
        // `ipe watch` (`push_appearance_patches`) POSTs an appearance edit as
        // `{"defaults":[...],"patch":[[i,"v"],...]}` — the classifier's
        // `ViewPatch` shape. `AppearancePatch` is the single runtime-owned
        // definition of that wire; pin that the exact bytes the sender emits
        // deserialize into it, so the two forms cannot drift.
        let wire = r#"{"defaults":["padding: 12px"],"patch":[[0,"padding: 16px"]]}"#;
        #[allow(clippy::expect_used)] // test — a shape mismatch is a test failure
        let decoded: AppearancePatch =
            serde_json::from_str(wire).expect("the classifier wire shape decodes");
        assert_eq!(decoded, appearance(), "the runtime wire matches the sender");
        #[allow(clippy::expect_used)] // test — a shape mismatch is a test failure
        let reencoded = serde_json::to_string(&decoded).expect("it re-serializes");
        assert_eq!(reencoded, wire, "the shape re-serializes byte-identically");
    }

    #[test]
    fn malformed_body_is_rejected() {
        let mut wire = 3u32.to_be_bytes().to_vec();
        wire.extend_from_slice(b"{ [");
        assert_eq!(decode_frame(&wire), Err(FrameError::Malformed));
    }

    #[test]
    fn encode_refuses_a_body_over_the_cap() {
        // A patch whose serialized body exceeds the cap is refused at encode
        // time, so the sender never emits a record the decoder would reject.
        let huge = "v".repeat(MAX_FRAME_LEN);
        let frame = ControlFrame::HotAppearance(AppearancePatch {
            defaults: vec![huge],
            patch: vec![],
        });
        assert!(
            encode_frame(&frame).is_none(),
            "a body past the cap does not encode"
        );
    }
}
