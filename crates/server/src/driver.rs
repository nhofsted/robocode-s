// SPDX-License-Identifier: EPL-2.0

//! The engine and what the server keeps per robot to feed it: commands that arrived for
//! the next turn and the parts of the command state the engine doesn't use. Every input
//! goes through [`Driver::input`], which records it (design §11) and applies it, so a
//! replay of a recording takes exactly the server's path.

use crate::convert::{self, Passthrough};
use crate::recording::Recorder;
use robocode_s_engine::{Battle, Commands};
use robocode_s_protocol::v1::{self, recording_entry::Entry};
use std::io;

#[derive(Clone)]
pub struct Driver {
    pub battle: Battle,
    /// Commands received for the next turn.
    pending: Vec<Option<Commands>>,
    pub passthrough: Vec<Passthrough>,
}

impl Driver {
    pub fn new(battle: Battle) -> Self {
        let n = battle.robots.len();
        Self {
            battle,
            pending: vec![None; n],
            passthrough: vec![Passthrough::default(); n],
        }
    }

    /// Applies one input to the engine.
    pub fn apply(&mut self, entry: &Entry) {
        match entry {
            Entry::StartRound(_) => {
                self.battle.start_round();
                for (pending, passthrough) in self.pending.iter_mut().zip(&mut self.passthrough) {
                    *pending = None;
                    // Classic starts each round with new `ExecCommands` that only keep the
                    // colors. Whether `setAdjustRadarForRobotTurn` was called is forgotten
                    // too, so that `setAdjustRadarForGunTurn` again sets the radar's body
                    // adjustment (engine-spec §8).
                    *passthrough = Passthrough {
                        colors: passthrough.colors.take(),
                        ..Passthrough::default()
                    };
                }
            }
            Entry::FinishRound(_) => self.battle.finish_round(),
            Entry::Step(_) => {
                for i in 0..self.pending.len() {
                    if let Some(commands) = self.pending[i].take() {
                        self.battle.set_commands(i, commands);
                    }
                }
                self.battle.step();
            }
            Entry::Commands(c) => {
                if let (Some(slot), Some(commands)) =
                    (usize::try_from(c.robot).ok(), c.commands.as_ref())
                    && slot < self.pending.len()
                {
                    let (commands, passthrough) = convert::commands(commands);
                    self.pending[slot] = Some(commands);
                    self.passthrough[slot] = passthrough;
                }
            }
            Entry::DiscardCommands(i) => {
                if let Some(p) = robot(*i).and_then(|i| self.pending.get_mut(i)) {
                    *p = None;
                }
            }
            Entry::DrainEnergy(i) => {
                if let Some(i) = self.valid(*i) {
                    self.battle.drain_energy(i);
                }
            }
            Entry::Disable(i) => {
                if let Some(i) = self.valid(*i) {
                    self.battle.disable(i);
                }
            }
            Entry::Stop(i) => {
                if let Some(i) = self.valid(*i) {
                    self.battle.stop(i);
                }
            }
            Entry::Abort(_) => self.battle.abort(),
            Entry::Kill(i) => {
                if let Some(i) = self.valid(*i) {
                    self.battle.kill_robot(i);
                }
            }
            // Robots get observers' mouse and keyboard events from the server; the engine
            // never sees them, and what robots do about them comes as their commands.
            Entry::Header(_) | Entry::Snapshot(_) | Entry::Input(_) => {}
        }
    }

    fn valid(&self, i: i32) -> Option<usize> {
        robot(i).filter(|&i| i < self.battle.robots.len())
    }

    /// The current turn as observers see it. `output` per robot (may be shorter).
    pub fn snapshot(&self, output: &mut dyn FnMut(usize) -> String) -> v1::TurnSnapshot {
        v1::TurnSnapshot {
            round: self.battle.round,
            turn: self.battle.time,
            robots: self
                .battle
                .robots
                .iter()
                .zip(&self.passthrough)
                .enumerate()
                .map(|(i, (r, p))| convert::robot_snapshot(r, p, output(i)))
                .collect(),
            bullets: self
                .battle
                .bullets
                .iter()
                .map(convert::bullet_snapshot)
                .collect(),
        }
    }
}

fn robot(i: i32) -> Option<usize> {
    usize::try_from(i).ok()
}

/// The engine as the server drives it: inputs are recorded, then applied.
pub struct RecordingDriver {
    pub driver: Driver,
    recorder: Option<Recorder>,
    error: Option<io::Error>,
}

impl RecordingDriver {
    pub fn new(battle: Battle, recorder: Option<Recorder>) -> Self {
        Self {
            driver: Driver::new(battle),
            recorder,
            error: None,
        }
    }

    pub fn input(&mut self, entry: Entry) {
        self.record(&entry);
        self.driver.apply(&entry);
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Records an entry the engine doesn't take (a snapshot marker).
    pub fn record(&mut self, entry: &Entry) {
        if let Some(r) = &mut self.recorder
            && let Err(e) = r.write(entry)
        {
            self.error.get_or_insert(e);
            self.recorder = None;
        }
    }

    /// Ends the recording; the first write error, if any.
    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(r) = self.recorder.take() {
            r.finish()?;
        }
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
