// SPDX-License-Identifier: EPL-2.0

//! Round and battle hashes (docs/rumble-backend.md §13.4): what verification compares.
//!
//! Every volunteer who runs the same battle (seed, bot versions, rules, engine version) gets
//! the same snapshots, round results and tracks, so the same hashes. They leave out what isn't
//! battle state: console output, debug paint, CPU times and debug properties
//! ([`robocode_s_server::recording::check`]).
//!
//! Byte layouts (all integers little-endian, `‖` concatenation):
//!
//! ```text
//! round hash  = SHA-256( "rsr1"                     4 bytes, ASCII
//!                      ‖ round                      u32, 0-based
//!                      ‖ check(snapshot)            u64 per TurnSnapshot of the round,
//!                                                   in order (turn 0, the round start, first)
//!                      ‖ RoundResult                its prost encoding (cumulative results,
//!                                                   robot order), unprefixed )
//! battle hash = SHA-256( "rsb1"                     4 bytes, ASCII
//!                      ‖ engine                     u32, ENGINE_VERSION
//!                      ‖ seed                       i64
//!                      ‖ round hash ‖ track sha256  32 + 32 bytes per round, in order
//!                      ‖ Results                    prost encoding of each robot's final
//!                                                   results, robot order, unprefixed )
//! ```

use prost::Message;
use robocode_s_protocol::v1;
use robocode_s_server::recording::check;
use sha2::{Digest, Sha256};

/// A SHA-256 digest.
pub type Digest32 = [u8; 32];

/// Builds a round hash from the round's snapshots as they come.
#[derive(Clone)]
pub struct RoundHasher {
    sha: Sha256,
    snapshots: usize,
}

impl RoundHasher {
    pub fn new(round: u32) -> Self {
        let mut sha = Sha256::new();
        sha.update(b"rsr1");
        sha.update(round.to_le_bytes());
        Self { sha, snapshots: 0 }
    }

    pub fn snapshot(&mut self, snapshot: &v1::TurnSnapshot) {
        self.sha.update(check(snapshot).to_le_bytes());
        self.snapshots += 1;
    }

    /// Snapshots added so far.
    pub fn snapshots(&self) -> usize {
        self.snapshots
    }

    pub fn finish(mut self, result: &v1::RoundResult) -> Digest32 {
        self.sha.update(result.encode_to_vec());
        self.sha.finalize().into()
    }
}

/// The round hash of a whole round at once.
pub fn round_hash(
    round: u32,
    snapshots: &[v1::TurnSnapshot],
    result: &v1::RoundResult,
) -> Digest32 {
    let mut h = RoundHasher::new(round);
    for s in snapshots {
        h.snapshot(s);
    }
    h.finish(result)
}

/// The battle hash: `rounds` holds each round's hash and its track's SHA-256, `results` the
/// robots' final results in robot order.
pub fn battle_hash(
    engine: u32,
    seed: i64,
    rounds: &[(Digest32, Digest32)],
    results: &[v1::Results],
) -> Digest32 {
    let mut sha = Sha256::new();
    sha.update(b"rsb1");
    sha.update(engine.to_le_bytes());
    sha.update(seed.to_le_bytes());
    for (round, track) in rounds {
        sha.update(round);
        sha.update(track);
    }
    for r in results {
        sha.update(r.encode_to_vec());
    }
    sha.finalize().into()
}

/// Lowercase hex, as the API carries hashes.
pub fn hex(digest: &[u8]) -> String {
    crate::http::hex(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(turn: i64, x: f64) -> v1::TurnSnapshot {
        v1::TurnSnapshot {
            round: 0,
            turn,
            robots: vec![v1::RobotSnapshot {
                x: Some(x),
                ..Default::default()
            }],
            bullets: vec![],
        }
    }

    #[test]
    fn round_hash_is_the_documented_layout() {
        let snaps = [snapshot(0, 1.0), snapshot(1, 2.0)];
        let result = v1::RoundResult {
            round: 3,
            results: vec![v1::Results {
                name: "a 1".into(),
                score: Some(10.0),
                ..Default::default()
            }],
        };
        let mut bytes = b"rsr1".to_vec();
        bytes.extend(3u32.to_le_bytes());
        for s in &snaps {
            bytes.extend(check(s).to_le_bytes());
        }
        bytes.extend(result.encode_to_vec());
        let expected: Digest32 = Sha256::digest(&bytes).into();
        assert_eq!(round_hash(3, &snaps, &result), expected);
    }

    #[test]
    fn round_hash_ignores_output_paint_and_cpu() {
        let result = v1::RoundResult::default();
        let plain = snapshot(1, 5.0);
        let mut noisy = plain.clone();
        noisy.robots[0].output = "hello".into();
        noisy.robots[0].cpu_nanos = 1234;
        noisy.robots[0].budget_nanos = 99;
        noisy.robots[0].paint.push(v1::PaintOp::default());
        noisy.robots[0]
            .debug_properties
            .push(v1::DebugProperty::default());
        assert_eq!(
            round_hash(0, std::slice::from_ref(&plain), &result),
            round_hash(0, &[noisy], &result)
        );
        assert_ne!(
            round_hash(0, std::slice::from_ref(&plain), &result),
            round_hash(0, &[snapshot(1, 5.5)], &result)
        );
        assert_ne!(
            round_hash(0, std::slice::from_ref(&plain), &result),
            round_hash(1, &[plain], &result)
        );
    }

    #[test]
    fn battle_hash_is_the_documented_layout() {
        let rounds = [([1u8; 32], [2u8; 32]), ([3u8; 32], [4u8; 32])];
        let results = [
            v1::Results {
                name: "a 1".into(),
                rank: 1,
                ..Default::default()
            },
            v1::Results {
                name: "b 2".into(),
                rank: 2,
                ..Default::default()
            },
        ];
        let mut bytes = b"rsb1".to_vec();
        bytes.extend(7u32.to_le_bytes());
        bytes.extend((-5i64).to_le_bytes());
        for (r, t) in &rounds {
            bytes.extend(r);
            bytes.extend(t);
        }
        for r in &results {
            bytes.extend(r.encode_to_vec());
        }
        let expected: Digest32 = Sha256::digest(&bytes).into();
        assert_eq!(battle_hash(7, -5, &rounds, &results), expected);
        assert_ne!(battle_hash(7, -4, &rounds, &results), expected);
        assert_eq!(hex(&expected).len(), 64);
    }
}
