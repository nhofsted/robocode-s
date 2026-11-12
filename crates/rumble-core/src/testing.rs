// SPDX-License-Identifier: EPL-2.0

//! Plain registry items for tests (this crate's, and the Lambda's with the `testing`
//! feature): a test states only the fields it cares about and takes the rest with `..`.

use crate::ids::{Division, WeightClass};
use crate::store::{BotItem, BotSource, BotState, VersionItem, VersionState};

/// An active 1v1 version `1.<vid>` of `bot`, with a SHA-256 made of `vid` (so versions'
/// recording directories differ), created at 0.
pub fn version(vid: u32, bot: &str) -> VersionItem {
    VersionItem {
        vid,
        bot: bot.to_string(),
        version: format!("1.{vid}"),
        sha256: format!("{vid:064x}"),
        size: 100,
        codesize: Some(100),
        class: WeightClass::Mega,
        divisions: vec![Division::OneVsOne],
        state: VersionState::Active,
        probation_battles: 0,
        probation_failures: 0,
        class_name: bot.to_string(),
        created: 0,
        retired: None,
        url: None,
        colors: None,
        released: None,
        nondeterministic: None,
        disagreements: 0,
        llm: None,
    }
}

/// An active, imported, unclaimed 1v1 bot by `a`, without versions, created at 0.
pub fn bot(id: &str) -> BotItem {
    BotItem {
        id: id.to_string(),
        author: "a".into(),
        owner: None,
        homepage: None,
        description: None,
        divisions: vec![Division::OneVsOne],
        versions: vec![],
        current: None,
        state: BotState::Active,
        created: 0,
        source: BotSource::Import,
    }
}
