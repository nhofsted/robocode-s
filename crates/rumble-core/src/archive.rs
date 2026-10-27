// SPDX-License-Identifier: EPL-2.0

//! `archive.json` (spec §15 step 7): every version of every bot that has not been taken down,
//! with what the archive page needs to link its jar (`/jars/<sha256>.jar`). Probation, active,
//! retired, failed and delisted versions are listed, with their release date where known
//! (§16.5); removed versions and removed bots are not (a
//! takedown deletes the jar). At about 1,500 bots with a few versions each, 100 bytes a
//! version, the file is a few hundred KB uncompressed (about 600 KB at 4,000 versions),
//! small enough for one file.

use crate::data::{Archive, ArchiveBot, ArchiveVersion, SCHEMA_VERSION};
use crate::publish::date_of;
use crate::store::{BotItem, BotState, VersionItem, VersionState};
use std::collections::BTreeMap;

/// The data file's path under the site bucket.
pub const ARCHIVE_KEY: &str = "data/v1/archive.json";

/// Bots sorted by id, versions newest first (by vid, which grows with registration).
pub fn archive(
    versions: &[VersionItem],
    bots: &BTreeMap<String, BotItem>,
    generated_at: &str,
) -> Archive {
    let mut by_bot: BTreeMap<&str, Vec<&VersionItem>> = BTreeMap::new();
    for v in versions {
        if v.state != VersionState::Removed {
            by_bot.entry(v.bot.as_str()).or_default().push(v);
        }
    }
    let bots = by_bot
        .into_iter()
        .filter_map(|(id, mut list)| {
            let bot = bots.get(id).filter(|b| b.state != BotState::Removed)?;
            list.sort_by_key(|v| std::cmp::Reverse(v.vid));
            Some(ArchiveBot {
                id: id.to_string(),
                author: bot.author.clone(),
                versions: list
                    .into_iter()
                    .map(|v| ArchiveVersion {
                        version: v.version.clone(),
                        sha256: v.sha256.clone(),
                        size: v.size,
                        state: v.state.as_str().to_string(),
                        added: date_of(v.created),
                        released: v.released.map(|ms| date_of(ms / 1000)),
                    })
                    .collect(),
            })
        })
        .collect();
    Archive {
        schema: SCHEMA_VERSION,
        generated_at: generated_at.to_string(),
        bots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{Division, WeightClass};
    use crate::store::BotSource;

    fn version(vid: u32, bot: &str, state: VersionState) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: format!("1.{vid}"),
            sha256: format!("{vid:064x}"),
            size: 1000 + u64::from(vid),
            codesize: None,
            class: WeightClass::Mega,
            divisions: vec![Division::OneVsOne],
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: 1_792_843_200,
            retired: None,
            url: None,
            colors: None,
            released: None,
            nondeterministic: None,
            disagreements: 0,
        }
    }

    fn bot(id: &str, state: BotState) -> BotItem {
        BotItem {
            id: id.to_string(),
            author: format!("author of {id}"),
            owner: None,
            homepage: None,
            description: None,
            divisions: vec![Division::OneVsOne],
            versions: vec![],
            current: None,
            state,
            created: 0,
            source: BotSource::Import,
        }
    }

    #[test]
    fn lists_everything_but_takedowns() {
        let versions = vec![
            version(3, "b.Two", VersionState::Active),
            version(1, "a.One", VersionState::Retired),
            version(2, "a.One", VersionState::Active),
            version(4, "a.One", VersionState::Removed),
            version(5, "a.One", VersionState::Probation),
            version(6, "c.Gone", VersionState::Retired),
            version(7, "b.Two", VersionState::Failed),
            version(8, "d.NoItem", VersionState::Active),
            VersionItem {
                released: Some(1_000_000_000_000),
                nondeterministic: None,
                disagreements: 0,
                ..version(9, "e.Gone", VersionState::Delisted)
            },
        ];
        let bots: BTreeMap<String, BotItem> = [
            bot("a.One", BotState::Active),
            bot("b.Two", BotState::Retired),
            bot("c.Gone", BotState::Removed),
            bot("e.Gone", BotState::Delisted),
        ]
        .into_iter()
        .map(|b| (b.id.clone(), b))
        .collect();
        let a = archive(&versions, &bots, "2026-10-24T12:00:00Z");
        let ids: Vec<&str> = a.bots.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["a.One", "b.Two", "e.Gone"]);
        // A delisted bot stays downloadable, with its release date.
        let e = &a.bots[2].versions[0];
        assert_eq!(
            (e.state.as_str(), e.released.as_deref()),
            ("delisted", Some("2001-09-09"))
        );
        assert_eq!(a.bots[0].versions[0].released, None);
        let one: Vec<(&str, &str)> = a.bots[0]
            .versions
            .iter()
            .map(|v| (v.version.as_str(), v.state.as_str()))
            .collect();
        assert_eq!(
            one,
            [("1.5", "probation"), ("1.2", "active"), ("1.1", "retired")]
        );
        assert_eq!(a.bots[1].versions[0].state, "failed");
        assert_eq!(a.bots[0].author, "author of a.One");
        let v = &a.bots[0].versions[1];
        assert_eq!((v.size, v.added.as_str()), (1002, "2026-10-24"));
        assert_eq!(v.sha256.len(), 64);
        assert_eq!(a.schema, 1);
    }
}
