//! The harness's one wall-clock budget and the per-phase caps derived from it.
//!
//! Every child the harness starts gets a wall no larger than what is left of
//! the budget, minus [`RESERVE`], so every child is gone before the budget
//! runs out, even one the harness can no longer kill directly.

use std::num::NonZeroU64;
use std::time::{Duration, Instant};

/// The smallest harness wall `run --wall` accepts.
pub const MIN_WALL_SECS: u64 = 10;

/// The largest wall any budget or phase may carry.
pub const MAX_WALL_SECS: u64 = 600;

/// Budget kept back from every phase.
///
/// It covers the jail wrapper's `timeout --kill-after=5s` escalation plus a
/// margin for the harness to report and clean up.
pub const RESERVE: Duration = Duration::from_secs(6);

/// A wall-clock cap in whole seconds, in `1..=MAX_WALL_SECS`.
///
/// Zero is unrepresentable: `timeout 0` means "no limit", so a zero wall would
/// silently disable the kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WallSecs(NonZeroU64);

/// Why a `--wall` value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallError {
    /// The value is not a decimal integer.
    NotANumber,
    /// The value is outside `MIN_WALL_SECS..=MAX_WALL_SECS`.
    OutOfRange {
        /// The refused value.
        secs: u64,
    },
}

impl std::fmt::Display for WallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotANumber => write!(f, "--wall must be a whole number of seconds"),
            Self::OutOfRange { secs } => write!(
                f,
                "--wall {secs} is outside {MIN_WALL_SECS}..={MAX_WALL_SECS} seconds"
            ),
        }
    }
}

impl WallSecs {
    /// A cap of `secs` seconds; `None` when `secs` is zero or above [`MAX_WALL_SECS`].
    #[must_use]
    pub const fn new(secs: u64) -> Option<Self> {
        if secs > MAX_WALL_SECS {
            return None;
        }
        match NonZeroU64::new(secs) {
            Some(secs) => Some(Self(secs)),
            None => None,
        }
    }

    /// Parse a harness `--wall` value, in `MIN_WALL_SECS..=MAX_WALL_SECS`.
    ///
    /// # Errors
    ///
    /// [`WallError`] for a non-integer or out-of-range value.
    pub fn parse_harness(raw: &str) -> Result<Self, WallError> {
        let secs = raw.parse::<u64>().map_err(|_| WallError::NotANumber)?;
        if secs < MIN_WALL_SECS {
            return Err(WallError::OutOfRange { secs });
        }
        Self::new(secs).ok_or(WallError::OutOfRange { secs })
    }

    /// The cap in seconds, never zero.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// The cap as a duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        Duration::from_secs(self.0.get())
    }
}

/// The run's one wall-clock budget, started when the harness starts.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    start: Instant,
    total: WallSecs,
}

impl Budget {
    /// A budget of `total` starting now.
    #[must_use]
    pub fn start(total: WallSecs) -> Self {
        Self {
            start: Instant::now(),
            total,
        }
    }

    /// The whole budget.
    #[must_use]
    pub const fn total(self) -> WallSecs {
        self.total
    }

    /// The wall for the next phase: `cap`, cut down to what the budget has left.
    ///
    /// `None` when less than one second plus [`RESERVE`] is left: the phase
    /// must not start.
    #[must_use]
    pub fn phase_wall(self, cap: WallSecs) -> Option<WallSecs> {
        phase_wall_after(self.start.elapsed(), self.total, cap)
    }
}

/// The phase wall once `elapsed` of `total` is spent, never above `cap`.
#[must_use]
pub fn phase_wall_after(elapsed: Duration, total: WallSecs, cap: WallSecs) -> Option<WallSecs> {
    let left = total
        .duration()
        .checked_sub(elapsed)?
        .checked_sub(RESERVE)?;
    WallSecs::new(left.as_secs().min(cap.get()))
}

#[cfg(test)]
mod tests {
    use super::{MAX_WALL_SECS, MIN_WALL_SECS, RESERVE, WallError, WallSecs, phase_wall_after};
    use std::time::Duration;

    fn wall(secs: u64) -> WallSecs {
        let parsed = WallSecs::new(secs);
        assert!(parsed.is_some(), "{secs} is a legal wall");
        parsed.unwrap_or(WallSecs(std::num::NonZeroU64::MIN))
    }

    #[test]
    fn the_wall_range_matches_the_server_clamp() {
        let source = include_str!("../../server/src/Runner.ipe");
        assert_eq!(
            crate::ipe_source::constant(source, "minJailWallSecs"),
            Some(MIN_WALL_SECS)
        );
        assert_eq!(
            crate::ipe_source::constant(source, "maxJailWallSecs"),
            Some(MAX_WALL_SECS)
        );
    }

    #[test]
    fn a_zero_or_oversized_wall_is_unrepresentable() {
        assert_eq!(WallSecs::new(0), None);
        assert_eq!(WallSecs::new(MAX_WALL_SECS + 1), None);
        assert_eq!(
            WallSecs::new(MAX_WALL_SECS).map(WallSecs::get),
            Some(MAX_WALL_SECS)
        );
    }

    #[test]
    fn a_harness_wall_outside_its_range_is_refused() {
        assert_eq!(WallSecs::parse_harness("abc"), Err(WallError::NotANumber));
        assert_eq!(WallSecs::parse_harness("-5"), Err(WallError::NotANumber));
        assert_eq!(WallSecs::parse_harness(""), Err(WallError::NotANumber));
        assert_eq!(
            WallSecs::parse_harness("18446744073709551616"),
            Err(WallError::NotANumber)
        );
        let below = MIN_WALL_SECS - 1;
        assert_eq!(
            WallSecs::parse_harness(&below.to_string()),
            Err(WallError::OutOfRange { secs: below })
        );
        let above = MAX_WALL_SECS + 1;
        assert_eq!(
            WallSecs::parse_harness(&above.to_string()),
            Err(WallError::OutOfRange { secs: above })
        );
        assert_eq!(
            WallSecs::parse_harness(&MIN_WALL_SECS.to_string()).map(WallSecs::get),
            Ok(MIN_WALL_SECS)
        );
    }

    #[test]
    fn a_phase_wall_never_exceeds_its_cap_or_the_remaining_budget() {
        let total = wall(60);
        assert_eq!(
            phase_wall_after(Duration::ZERO, total, wall(10)).map(WallSecs::get),
            Some(10)
        );
        assert_eq!(
            phase_wall_after(Duration::ZERO, total, wall(600)).map(WallSecs::get),
            Some(60 - RESERVE.as_secs())
        );
        assert_eq!(
            phase_wall_after(Duration::from_millis(50_500), total, wall(600)).map(WallSecs::get),
            Some(3)
        );
    }

    #[test]
    fn a_phase_with_no_budget_left_is_refused() {
        let total = wall(60);
        let spent = total.duration().saturating_sub(RESERVE);
        assert_eq!(phase_wall_after(spent, total, wall(10)), None);
        assert_eq!(
            phase_wall_after(
                spent.saturating_sub(Duration::from_millis(500)),
                total,
                wall(10)
            ),
            None
        );
        assert_eq!(
            phase_wall_after(Duration::from_secs(61), total, wall(10)),
            None
        );
    }
}
