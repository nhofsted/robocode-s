// SPDX-License-Identifier: EPL-2.0

//! What happens to a robot that answered late, when it wakes: classic's
//! `RobotPeer.checkSkippedTurn` (engine-spec §11.3, ADR 0018). Shared by the server and
//! the difftest's engine replay, which checks it against classic's recordings.

use std::ops::Range;

/// More consecutive skipped turns disable a robot (240 for robots that did file I/O).
pub const MAX_SKIPPED_TURNS: i64 = 30;
pub const MAX_SKIPPED_TURNS_WITH_IO: i64 = 240;

/// A robot's skipped turns, as it learns of them when it wakes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The turns it gets a `SkippedTurnEvent` for: those after the one it answered, before
    /// the one it wakes in; none if it is dead by then. Its other events but the critical
    /// ones (`isCriticalEvent`) are dropped.
    pub events: Range<i64>,
    /// It skipped more turns than it may: it is disabled, its pending commands dropped and
    /// its thread stopped.
    pub disable: bool,
}

/// A robot whose last `Execute` answered the turn `answered` wakes in turn `turn`, alive or
/// not; `io_robot`: it did file I/O.
pub fn skipped(answered: i64, turn: i64, alive: bool, io_robot: bool) -> Skipped {
    let max = if io_robot {
        MAX_SKIPPED_TURNS_WITH_IO
    } else {
        MAX_SKIPPED_TURNS
    };
    let first = answered + 1;
    Skipped {
        events: if alive { first..turn } else { first..first },
        disable: turn - answered - 1 > max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_classics_check_skipped_turn() {
        assert_eq!(
            skipped(10, 14, true, false),
            Skipped {
                events: 11..14,
                disable: false
            }
        );
        // Dead: no events, the limit all the same.
        assert!(skipped(10, 14, false, false).events.is_empty());
        assert!(
            !skipped(10, 41, true, false).disable,
            "30 skipped: still in"
        );
        assert!(skipped(10, 42, true, false).disable, "31");
        assert!(
            !skipped(10, 42, true, true).disable,
            "I/O robots may skip 240"
        );
        assert!(skipped(10, 252, false, true).disable);
    }
}
