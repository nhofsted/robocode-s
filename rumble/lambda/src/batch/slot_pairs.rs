// SPDX-License-Identifier: EPL-2.0

//! The 1v1 matchups that get recording slots besides those with a `top` bot (spec §12.5).
//! `publish` computes them from the general 1v1 rumble's metrics (`rumble_core::slots::
//! slot_pairs`) and writes `state/e<E>/1v1/slot-pairs.json`; `plan` reads it into the plan's
//! `slotPairs`. Only I/O lives here.

use super::Batch;
use crate::s3::Bucket;
use rumble_core::ids::{Division, RumbleId, WeightClass};
use rumble_core::publish::RumbleRun;
use rumble_core::slots::slot_pairs;
use serde::{Deserialize, Serialize};

/// `slot-pairs.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotPairsFile {
    /// `[vidLo, vidHi]`, sorted.
    pub pairs: Vec<(u32, u32)>,
}

/// The state bucket key of the file.
pub fn key(engine: u32) -> String {
    format!("state/e{engine}/1v1/slot-pairs.json")
}

/// Computes and writes the pairs from the general 1v1 rumble among `runs` (nothing is written
/// if it wasn't ranked). Returns the number of pairs.
pub fn write(batch: &Batch, engine: u32, runs: &[RumbleRun]) -> Result<usize, String> {
    let general = RumbleId::new(Division::OneVsOne, WeightClass::Mega);
    let Some(run) = runs.iter().find(|r| r.id == general) else {
        return Ok(0);
    };
    let pairs = slot_pairs(&run.metrics);
    let count = pairs.len();
    batch.write_state_json(&key(engine), &SlotPairsFile { pairs })?;
    Ok(count)
}

/// The pairs of the last publish (none yet: empty).
pub fn read(batch: &Batch, engine: u32) -> Result<Vec<(u32, u32)>, String> {
    let file: Option<SlotPairsFile> = batch.read_json(Bucket::State, &key(engine))?;
    Ok(file.unwrap_or_default().pairs)
}
