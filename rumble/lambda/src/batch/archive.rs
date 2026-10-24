// SPDX-License-Identifier: EPL-2.0

//! `data/v1/archive.json` (spec §15 step 7), from the registry `publish` already read.

use super::{Batch, json_bytes};
use crate::s3::Bucket;
use rumble_core::archive::{ARCHIVE_KEY, archive};
use rumble_core::publish::DATA_CACHE;
use rumble_core::store::{BotItem, VersionItem};
use std::collections::BTreeMap;

/// Writes the archive file; returns the number of bots listed.
pub fn write(
    batch: &Batch,
    versions: &[VersionItem],
    bots: &BTreeMap<String, BotItem>,
    generated_at: &str,
) -> Result<usize, String> {
    let file = archive(versions, bots, generated_at);
    batch.objects().put(
        Bucket::Site,
        ARCHIVE_KEY,
        &json_bytes(&file),
        "application/json",
        Some(DATA_CACHE),
    )?;
    Ok(file.bots.len())
}
