// SPDX-License-Identifier: EPL-2.0

//! Battle recordings (design §11): the header, then every input the engine received, as
//! a gzip stream of `RecordingEntry` messages. [`Replay`] feeds them to a new engine and
//! yields the same snapshots observers saw, each verified against its recorded hash.

use crate::convert;
use crate::driver::Driver;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use prost::Message;
use robocode_s_engine::Battle;
use robocode_s_protocol::v1::{self, recording_entry::Entry};
use robocode_s_protocol::{read_message, write_message};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

/// Version of the recording format (`RecordingHeader.format`).
pub const FORMAT: u32 = 1;

/// Writes a recording.
pub struct Recorder {
    out: GzEncoder<BufWriter<File>>,
}

impl Recorder {
    /// Creates `path` and writes the header.
    pub fn create(path: &Path, header: v1::RecordingHeader) -> io::Result<Self> {
        let file = File::create(path)?;
        let mut recorder = Self {
            out: GzEncoder::new(BufWriter::new(file), Compression::default()),
        };
        recorder.write(&Entry::Header(header))?;
        Ok(recorder)
    }

    pub fn write(&mut self, entry: &Entry) -> io::Result<()> {
        let message = v1::RecordingEntry {
            entry: Some(entry.clone()),
        };
        write_message(&mut self.out, &message)
    }

    pub fn finish(self) -> io::Result<()> {
        self.out.finish()?.flush()
    }
}

/// The hash a snapshot is verified by: FNV-1a over its encoding without output, paint and
/// CPU times (timing, not battle state).
pub fn check(snapshot: &v1::TurnSnapshot) -> u64 {
    let mut s = snapshot.clone();
    for r in &mut s.robots {
        r.output.clear();
        r.paint.clear();
        r.cpu_nanos = 0;
        r.budget_nanos = 0;
    }
    s.encode_to_vec()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// A recording read into memory.
#[derive(Debug, Clone)]
pub struct Recording {
    pub header: v1::RecordingHeader,
    pub entries: Arc<[Entry]>,
}

impl Recording {
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut input = BufReader::new(GzDecoder::new(BufReader::new(File::open(path)?)));
        let invalid = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
        let header = match read_message::<v1::RecordingEntry>(&mut input)? {
            Some(v1::RecordingEntry {
                entry: Some(Entry::Header(h)),
            }) => h,
            _ => return Err(invalid("not a robocode-s recording")),
        };
        if header.format != FORMAT {
            return Err(invalid(&format!(
                "recording format {} (this version reads {FORMAT})",
                header.format
            )));
        }
        let mut entries = Vec::new();
        while let Some(message) = read_message::<v1::RecordingEntry>(&mut input)? {
            if let Some(entry) = message.entry {
                entries.push(entry);
            }
        }
        Ok(Self {
            header,
            entries: entries.into(),
        })
    }

    /// Number of rounds (from the rules).
    pub fn rounds(&self) -> i32 {
        self.header.rules.as_ref().map_or(0, |r| r.num_rounds)
    }

    pub fn replay(&self) -> Replay {
        let rules = convert::rules_in(&self.header.rules.unwrap_or_default());
        let setups = self
            .header
            .robots
            .iter()
            .map(convert::robot_setup)
            .collect();
        Replay {
            entries: self.entries.clone(),
            next: 0,
            driver: Driver::new(Battle::new(rules, setups, self.header.seed)),
        }
    }
}

/// What a replay produces, in the order observers received it.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A turn snapshot with the recorded output and paint; `verified` when it matches
    /// the recorded hash.
    Snapshot {
        snapshot: v1::TurnSnapshot,
        verified: bool,
    },
    /// A round ended, with the battle's results so far.
    RoundOver(v1::RoundResult),
}

/// A recording being played back. Cheap to clone: clones are keyframes to jump back to.
#[derive(Clone)]
pub struct Replay {
    entries: Arc<[Entry]>,
    next: usize,
    driver: Driver,
}

impl Replay {
    /// The engine at this point of the replay.
    pub fn battle(&self) -> &Battle {
        &self.driver.battle
    }

    /// Position in the recording's entries.
    pub fn position(&self) -> usize {
        self.next
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.entries.len()
    }
}

impl Iterator for Replay {
    type Item = Frame;

    fn next(&mut self) -> Option<Frame> {
        while let Some(entry) = self.entries.get(self.next) {
            self.next += 1;
            match entry {
                Entry::Snapshot(s) => {
                    let mut snapshot = self
                        .driver
                        .snapshot(&mut |i| s.output.get(i).cloned().unwrap_or_default());
                    let verified = check(&snapshot) == s.check;
                    for (i, r) in snapshot.robots.iter_mut().enumerate() {
                        // Older recordings have no CPU times: 0.
                        r.cpu_nanos = s.cpu_nanos.get(i).copied().unwrap_or_default();
                        r.budget_nanos = s.budget_nanos;
                    }
                    for p in &s.paint {
                        if let Some(r) = usize::try_from(p.robot)
                            .ok()
                            .and_then(|i| snapshot.robots.get_mut(i))
                        {
                            r.paint = p.ops.clone();
                        }
                    }
                    return Some(Frame::Snapshot { snapshot, verified });
                }
                Entry::FinishRound(_) => {
                    // The server reports the round's results before finishing it.
                    let result = round_result(&self.driver.battle);
                    self.driver.apply(entry);
                    return Some(Frame::RoundOver(result));
                }
                Entry::Step(_) => {
                    self.driver.apply(entry);
                    // The server hands these to the robots; a replay drops them.
                    let battle = &mut self.driver.battle;
                    for i in 0..battle.robots.len() {
                        battle.take_events(i);
                        battle.take_bullet_updates(i);
                        battle.take_team_messages(i);
                    }
                }
                _ => self.driver.apply(entry),
            }
        }
        None
    }
}

/// The battle's results so far, as the server reports them at the end of a round.
pub fn round_result(battle: &Battle) -> v1::RoundResult {
    v1::RoundResult {
        round: battle.round,
        results: battle
            .robots
            .iter()
            .zip(battle.results())
            .map(|(r, results)| convert::results(&r.setup.name, &results))
            .collect(),
    }
}
