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

/// A robot that last woke in turn `last` (classic's `currentExecutionTime`) wakes in turn
/// `turn`, alive or not; `io_robot`: it did file I/O. Within a round, `last` is the turn the
/// robot answered. Classic never resets it between rounds, so a robot that answers a round's
/// start late compares with the turn it last woke in the round before ([`carried_last`]).
/// `None` if that finds no skipped turn: classic's check then does nothing, its events stay.
pub fn skipped(last: i64, turn: i64, alive: bool, io_robot: bool) -> Option<Skipped> {
    let max = if io_robot {
        MAX_SKIPPED_TURNS_WITH_IO
    } else {
        MAX_SKIPPED_TURNS
    };
    let count = turn - last - 1;
    if count < 1 {
        return None;
    }
    let first = last + 1;
    Some(Skipped {
        events: if alive { first..turn } else { first..first },
        disable: count > max,
    })
}

/// The `last` of [`skipped`] for a robot that answered `answered` late: the turn it
/// answered, but for the round's start (turn 0) the turn it last woke in before
/// (`previous`, from an earlier round; 0 in the first). Classic's `RobotPeer` sets
/// `currentExecutionTime` only in `checkSkippedTurn`, and `initializeRound` resets only
/// `lastExecutionTime`, which the check overwrites first.
pub fn carried_last(answered: i64, previous: i64) -> i64 {
    if answered == 0 { previous } else { answered }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_classics_check_skipped_turn() {
        assert_eq!(
            skipped(10, 14, true, false),
            Some(Skipped {
                events: 11..14,
                disable: false
            })
        );
        // Dead: no events, the limit all the same.
        assert!(skipped(10, 14, false, false).unwrap().events.is_empty());
        assert!(
            !skipped(10, 41, true, false).unwrap().disable,
            "30 skipped: still in"
        );
        assert!(skipped(10, 42, true, false).unwrap().disable, "31");
        assert!(
            !skipped(10, 42, true, true).unwrap().disable,
            "I/O robots may skip 240"
        );
        assert!(skipped(10, 252, false, true).unwrap().disable);
    }

    #[test]
    fn a_late_round_start_compares_with_the_turn_before() {
        // First round: from 0, as any late answer.
        assert_eq!(carried_last(0, 0), 0);
        assert_eq!(
            skipped(carried_last(0, 0), 3, true, false).unwrap().events,
            1..3
        );
        // A later round: the robot last woke in turn 900 of the round before, so waking in
        // turn 40 finds nothing skipped, and nothing is disabled.
        assert_eq!(skipped(carried_last(0, 900), 40, true, false), None);
        // It died in turn 30 of the round before: turns 31 to 39 count.
        assert_eq!(
            skipped(carried_last(0, 30), 40, true, false)
                .unwrap()
                .events,
            31..40
        );
        // Within a round, the turn it answered.
        assert_eq!(carried_last(12, 900), 12);
    }
}
