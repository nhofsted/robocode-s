// SPDX-License-Identifier: EPL-2.0

//! The per-battle record (spec §6.1): what the rankings need of one battle, its compact binary
//! form for the result log items (§9.2), and the pairing score and survival definitions (§6.2).

use crate::api::{BattleResult, BattleStatus, FailureReason};
use crate::ids::Division;

/// Why a robot failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    /// The robot couldn't be described or never connected.
    Load,
    /// Its process ended during the battle.
    Crash,
}

impl FailReason {
    fn code(self) -> u8 {
        match self {
            FailReason::Load => 0,
            FailReason::Crash => 1,
        }
    }

    fn from_code(code: u8) -> Result<FailReason, String> {
        match code {
            0 => Ok(FailReason::Load),
            1 => Ok(FailReason::Crash),
            _ => Err(format!("bad failure reason {code}")),
        }
    }
}

impl From<FailureReason> for FailReason {
    fn from(reason: FailureReason) -> FailReason {
        match reason {
            FailureReason::Load => FailReason::Load,
            FailureReason::Crash => FailReason::Crash,
        }
    }
}

/// One contestant of a battle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub vid: u32,
    pub score: f32,
    pub firsts: u16,
}

/// The result of one battle as far as the rankings need it.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub division: Division,
    /// +1 counts; -1 cancels an earlier record (§11.3).
    pub weight: i8,
    /// Who ran it (for removal), "system" for corrections. Not part of the binary form: the
    /// log item carries it.
    pub uid: String,
    /// Battle robot order. For a failed battle the scores and firsts are 0.
    pub contestants: Vec<Entry>,
    /// The robot (index into `contestants`) that failed and why.
    pub failed: Option<(u8, FailReason)>,
    /// Flags bit1: the reporter's reputation is at least `rep.verifierMin`. Only trusted failed
    /// records enter the failure tallies (§12.3).
    pub trusted: bool,
}

const FLAG_FAILED: u8 = 1;
const FLAG_TRUSTED: u8 = 2;
const ENTRY_BYTES: usize = 10;

/// The division's byte in the binary forms of records and `pairings.bin`.
pub fn division_code(division: Division) -> u8 {
    match division {
        Division::OneVsOne => 0,
        Division::Melee => 1,
    }
}

pub fn division_from_code(code: u8) -> Result<Division, String> {
    match code {
        0 => Ok(Division::OneVsOne),
        1 => Ok(Division::Melee),
        _ => Err(format!("bad division code {code}")),
    }
}

impl Record {
    /// The size of the binary form: 24 bytes for a 1v1 battle, 104 for a 10-bot melee.
    pub fn encoded_len(&self) -> usize {
        4 + ENTRY_BYTES * self.contestants.len() + if self.failed.is_some() { 2 } else { 0 }
    }

    /// Appends the binary form (little-endian):
    /// `u8 division | i8 weight | u8 flags | u8 n | n x (u32 vid, f32 score, u16 firsts) |
    /// [u8 failedRobot, u8 reason]`. Panics if there are more than 255 contestants.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let n = u8::try_from(self.contestants.len()).expect("at most 255 contestants");
        let flags = if self.failed.is_some() {
            FLAG_FAILED
        } else {
            0
        } | if self.trusted { FLAG_TRUSTED } else { 0 };
        out.reserve(self.encoded_len());
        out.push(division_code(self.division));
        out.push(self.weight as u8);
        out.push(flags);
        out.push(n);
        for entry in &self.contestants {
            out.extend_from_slice(&entry.vid.to_le_bytes());
            out.extend_from_slice(&entry.score.to_le_bytes());
            out.extend_from_slice(&entry.firsts.to_le_bytes());
        }
        if let Some((robot, reason)) = self.failed {
            out.push(robot);
            out.push(reason.code());
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode(&mut out);
        out
    }

    /// Decodes one record from the front of `bytes`, returning it and the bytes used. `uid` is
    /// taken from the log item.
    pub fn decode(bytes: &[u8], uid: &str) -> Result<(Record, usize), String> {
        let header = bytes.get(..4).ok_or("truncated record header")?;
        let division = division_from_code(header[0])?;
        let weight = header[1] as i8;
        if weight != 1 && weight != -1 {
            return Err(format!("bad record weight {weight}"));
        }
        let flags = header[2];
        if flags & !(FLAG_FAILED | FLAG_TRUSTED) != 0 {
            return Err(format!("bad record flags {flags:#x}"));
        }
        let n = header[3] as usize;
        if n < 2 {
            return Err(format!("a record needs at least 2 contestants, has {n}"));
        }
        let failed_flag = flags & FLAG_FAILED != 0;
        let len = 4 + n * ENTRY_BYTES + if failed_flag { 2 } else { 0 };
        let body = bytes.get(..len).ok_or("truncated record")?;
        let mut contestants = Vec::with_capacity(n);
        for chunk in body[4..4 + n * ENTRY_BYTES].chunks_exact(ENTRY_BYTES) {
            let score = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
            if !score.is_finite() || score < 0.0 {
                return Err(format!("bad score {score}"));
            }
            contestants.push(Entry {
                vid: u32::from_le_bytes(chunk[0..4].try_into().unwrap()),
                score,
                firsts: u16::from_le_bytes(chunk[8..10].try_into().unwrap()),
            });
        }
        let failed = if failed_flag {
            let robot = body[len - 2];
            if robot as usize >= n {
                return Err(format!("failed robot {robot} of {n}"));
            }
            Some((robot, FailReason::from_code(body[len - 1])?))
        } else {
            None
        };
        let record = Record {
            division,
            weight,
            uid: uid.to_string(),
            contestants,
            failed,
            trusted: flags & FLAG_TRUSTED != 0,
        };
        Ok((record, len))
    }

    /// Decodes a concatenation of records (the log item's `recs`).
    pub fn decode_all(mut bytes: &[u8], uid: &str) -> Result<Vec<Record>, String> {
        let mut records = Vec::new();
        while !bytes.is_empty() {
            let (record, used) = Record::decode(bytes, uid)?;
            records.push(record);
            bytes = &bytes[used..];
        }
        Ok(records)
    }

    /// The record of an accepted battle of a results post. `vids` come from the batch token, in
    /// the battle's robot order. `ok` battles carry scores and firsts; `robotFailed` battles
    /// carry the failure. `trusted` is the reporter's flag (§12.3). Other statuses are not
    /// recorded.
    pub fn from_battle(
        division: Division,
        uid: &str,
        vids: &[u32],
        battle: &BattleResult,
        trusted: bool,
    ) -> Result<Record, String> {
        if vids.len() < 2 || vids.len() > 255 {
            return Err(format!("a battle needs 2 to 255 bots, has {}", vids.len()));
        }
        let mut contestants: Vec<Entry> = vids
            .iter()
            .map(|&vid| Entry {
                vid,
                score: 0.0,
                firsts: 0,
            })
            .collect();
        let mut failed = None;
        match battle.status {
            BattleStatus::Ok => {
                let robots = battle.robots.as_ref().ok_or("ok battle without robots")?;
                if robots.len() != vids.len() {
                    return Err(format!("{} robots for {} bots", robots.len(), vids.len()));
                }
                for (entry, robot) in contestants.iter_mut().zip(robots) {
                    if !robot.score.is_finite() || robot.score < 0.0 {
                        return Err(format!("bad score {}", robot.score));
                    }
                    entry.score = robot.score as f32;
                    entry.firsts = u16::try_from(robot.firsts)
                        .map_err(|_| format!("firsts {} too large", robot.firsts))?;
                }
            }
            BattleStatus::RobotFailed => {
                let failure = battle
                    .failure
                    .as_ref()
                    .ok_or("robotFailed battle without failure")?;
                if failure.robot as usize >= vids.len() {
                    return Err(format!("failed robot {} of {}", failure.robot, vids.len()));
                }
                failed = Some((failure.robot as u8, failure.reason.into()));
            }
            BattleStatus::NotRun | BattleStatus::Error => {
                return Err(format!("a {:?} battle has no record", battle.status));
            }
        }
        Ok(Record {
            division,
            weight: 1,
            uid: uid.to_string(),
            contestants,
            failed,
            trusted,
        })
    }

    /// The same record with the opposite weight, for removing it again (§11.3).
    pub fn negated(&self) -> Record {
        Record {
            weight: -self.weight,
            ..self.clone()
        }
    }
}

/// Pairing score of A against B (§6.2): `100 * a / (a + b)`, 50 if both are 0.
pub fn pairing_score(a: f32, b: f32) -> f64 {
    share(f64::from(a), f64::from(b))
}

/// Survival of A against B (§6.2): `100 * firsts_a / (firsts_a + firsts_b)`, 50 if both are 0.
pub fn pairing_survival(a: u16, b: u16) -> f64 {
    share(f64::from(a), f64::from(b))
}

fn share(a: f64, b: f64) -> f64 {
    if a + b == 0.0 {
        50.0
    } else {
        100.0 * a / (a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Failure, RobotResult};

    fn entry(vid: u32, score: f32, firsts: u16) -> Entry {
        Entry { vid, score, firsts }
    }

    fn duel() -> Record {
        Record {
            division: Division::OneVsOne,
            weight: 1,
            uid: "g1".into(),
            contestants: vec![entry(812, 4123.5, 22), entry(77, 3000.0, 13)],
            failed: None,
            trusted: false,
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(duel().to_bytes().len(), 24);
        let melee = Record {
            division: Division::Melee,
            contestants: (0..10).map(|i| entry(i, 1.0, 0)).collect(),
            ..duel()
        };
        assert_eq!(melee.to_bytes().len(), 104);
        assert_eq!(melee.encoded_len(), 104);
    }

    #[test]
    fn round_trip() {
        let mut failed = duel();
        failed.weight = -1;
        failed.trusted = true;
        failed.failed = Some((1, FailReason::Crash));
        let mut bytes = Vec::new();
        for record in [duel(), failed.clone(), duel()] {
            record.encode(&mut bytes);
        }
        assert_eq!(bytes.len(), 24 + 26 + 24);
        let records = Record::decode_all(&bytes, "g1").unwrap();
        assert_eq!(records, vec![duel(), failed, duel()]);
        assert_eq!(Record::decode_all(&[], "x").unwrap(), vec![]);
    }

    #[test]
    fn layout() {
        let mut failed = duel();
        failed.trusted = true;
        failed.failed = Some((1, FailReason::Load));
        let bytes = failed.to_bytes();
        assert_eq!(&bytes[..4], &[0, 1, 3, 2]);
        assert_eq!(&bytes[4..8], &812u32.to_le_bytes());
        assert_eq!(&bytes[8..12], &4123.5f32.to_le_bytes());
        assert_eq!(&bytes[12..14], &22u16.to_le_bytes());
        assert_eq!(&bytes[24..], &[1, 0]);
    }

    #[test]
    fn invalid_input() {
        let bytes = duel().to_bytes();
        for cut in 1..bytes.len() {
            assert!(Record::decode_all(&bytes[..cut], "x").is_err());
        }
        let broken = |edit: &dyn Fn(&mut Vec<u8>), base: &Record| {
            let mut bytes = base.to_bytes();
            edit(&mut bytes);
            assert!(Record::decode_all(&bytes, "x").is_err());
        };
        broken(&|b| b[0] = 9, &duel());
        broken(&|b| b[1] = 0, &duel());
        broken(&|b| b[2] = 4, &duel());
        broken(&|b| b[3] = 1, &duel());
        broken(
            &|b| b[8..12].copy_from_slice(&f32::NAN.to_le_bytes()),
            &duel(),
        );
        broken(&|b| b[2] = 1, &duel()); // failed flag without the trailing bytes
        let mut failed = duel();
        failed.failed = Some((1, FailReason::Load));
        broken(&|b| b[24] = 2, &failed);
        broken(&|b| b[25] = 7, &failed);
    }

    fn robot(score: f64, firsts: u32) -> RobotResult {
        RobotResult {
            score,
            survival: 0.0,
            last_survivor_bonus: 0.0,
            bullet_damage: 0.0,
            bullet_damage_bonus: 0.0,
            ram_damage: 0.0,
            ram_damage_bonus: 0.0,
            firsts,
            seconds: 0,
            thirds: 0,
            rank: 1,
            colors: None,
            threads: false,
            hard_limit: false,
        }
    }

    fn battle(status: BattleStatus) -> BattleResult {
        BattleResult {
            index: 0,
            status,
            duration_ms: None,
            battle_hash: None,
            robots: None,
            rounds: None,
            failure: None,
        }
    }

    #[test]
    fn from_battle() {
        let mut ok = battle(BattleStatus::Ok);
        ok.robots = Some(vec![robot(4123.5, 22), robot(3000.0, 13)]);
        let record = Record::from_battle(Division::OneVsOne, "g1", &[812, 77], &ok, true).unwrap();
        assert_eq!(
            record.contestants,
            vec![entry(812, 4123.5, 22), entry(77, 3000.0, 13)]
        );
        assert!(record.trusted && record.failed.is_none() && record.weight == 1);
        assert!(Record::from_battle(Division::OneVsOne, "g1", &[812], &ok, true).is_err());

        let mut bad = ok.clone();
        bad.robots.as_mut().unwrap()[0].score = f64::NAN;
        assert!(Record::from_battle(Division::OneVsOne, "g1", &[1, 2], &bad, true).is_err());

        let mut failed = battle(BattleStatus::RobotFailed);
        failed.failure = Some(Failure {
            robot: 1,
            reason: FailureReason::Crash,
            detail: None,
        });
        let record =
            Record::from_battle(Division::OneVsOne, "g1", &[812, 77], &failed, false).unwrap();
        assert_eq!(record.failed, Some((1, FailReason::Crash)));
        assert_eq!(record.contestants.len(), 2);
        failed.failure.as_mut().unwrap().robot = 2;
        assert!(Record::from_battle(Division::OneVsOne, "g1", &[812, 77], &failed, false).is_err());

        let not_run = battle(BattleStatus::NotRun);
        assert!(Record::from_battle(Division::OneVsOne, "g1", &[1, 2], &not_run, true).is_err());
    }

    #[test]
    fn definitions() {
        assert_eq!(pairing_score(75.0, 25.0), 75.0);
        assert_eq!(pairing_score(0.0, 0.0), 50.0);
        assert_eq!(pairing_score(0.0, 10.0), 0.0);
        assert_eq!(pairing_score(10.0, 0.0), 100.0);
        assert_eq!(pairing_survival(21, 14), 60.0);
        assert_eq!(pairing_survival(0, 0), 50.0);
        assert_eq!(pairing_survival(0, 3), 0.0);
        // B's value is 100 minus A's.
        let (a, b) = (1234.5f32, 987.25f32);
        assert!((pairing_score(a, b) + pairing_score(b, a) - 100.0).abs() < 1e-12);
    }
}
