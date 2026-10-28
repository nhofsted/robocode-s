// SPDX-License-Identifier: EPL-2.0

//! The pure part of the `batch` Lambda's `publish` mode (spec §15): the members of each
//! rumble, the data files of `rumble-data.md` (`rumbles.json`, `rankings/<rumble>.json`,
//! `bots/<id>.json` with their history, `contributors.json`) and the snapshots in the state
//! bucket (`state/e<E>/snapshots/<rumble>/<date>.json`, `.../users/<date>.json`).

use crate::api::Tier;
use crate::config::Config;
use crate::data::{
    self, Author, BotDetails, BotVersion, Contributor, Contributors, RumbleIndex, RumbleSummary,
};
use crate::flags::FlagMap;
use crate::ids::{Division, RumbleId};
use crate::metrics::{self, Member, RumbleMetrics};
use crate::state::State;
use crate::store::{BotItem, BotState, DAY, User, UserState, VersionItem, VersionState, rfc3339};
use crate::verify;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Snapshots older than this many days are deleted (§15 step 7).
pub const SNAPSHOT_DAYS: i64 = 14;
/// `Cache-Control` of the data files (§14.6).
pub const DATA_CACHE: &str = "public, max-age=3600, s-maxage=90000";
/// The published data's prefix in the site bucket.
pub const DATA_PREFIX: &str = "data/v1";

/// A rumble's row in a snapshot: `[rank, aps, glicko, version]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRow(pub u32, pub f64, pub f64, pub String);

/// `state/e<E>/snapshots/<rumble>/<date>.json`: bot id → its row.
pub type Snapshot = BTreeMap<String, SnapshotRow>;

/// `state/e<E>/snapshots/users/<date>.json`: uid → the user's battle counter.
pub type UserSnapshot = BTreeMap<String, u64>;

/// `YYYY-MM-DD` of a unix time (UTC).
pub fn date_of(unix: u64) -> String {
    rfc3339(unix)[..10].to_string()
}

/// The date `days` days after `date` (negative: before).
pub fn shift_date(date: &str, days: i64) -> Option<String> {
    let day = data::day_number(date)? + days;
    u64::try_from(day).ok().map(|d| date_of(d * DAY))
}

/// `state/e<E>/snapshots/<name>/<date>.json` (`name` a rumble id or `users`).
pub fn snapshot_key(engine: u32, name: &str, date: &str) -> String {
    format!("state/e{engine}/snapshots/{name}/{date}.json")
}

/// The keys among `keys` (a listing of `state/e<E>/snapshots/`) whose date is more than
/// [`SNAPSHOT_DAYS`] before `today`.
pub fn expired_snapshots(keys: &[String], today: &str) -> Vec<String> {
    let Some(today) = data::day_number(today) else {
        return Vec::new();
    };
    keys.iter()
        .filter(|key| {
            key.rsplit('/')
                .next()
                .and_then(|file| file.strip_suffix(".json"))
                .and_then(data::day_number)
                .is_some_and(|day| today - day > SNAPSHOT_DAYS)
        })
        .cloned()
        .collect()
}

/// What the rankings need from the table.
pub struct Catalog<'a> {
    pub versions: &'a [VersionItem],
    pub bots: &'a BTreeMap<String, BotItem>,
    /// The owners of claimed bots, by uid.
    pub owners: &'a BTreeMap<String, User>,
    /// The RoboWiki's flags by package (§16.7).
    pub flags: &'a FlagMap,
}

fn country(user: Option<&User>) -> Option<String> {
    user.map(|u| u.country.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
}

impl Catalog<'_> {
    fn owner(&self, bot: &BotItem) -> Option<&User> {
        bot.owner.as_ref().and_then(|uid| self.owners.get(uid))
    }

    /// The flag of a bot: the country its owner chose, else the RoboWiki's flag of its
    /// package.
    fn country(&self, bot: &BotItem) -> Option<String> {
        country(self.owner(bot)).or_else(|| self.flags.of(&bot.id).map(str::to_string))
    }

    /// The ranked version of each bot of a division (§15 step 3): its active versions with
    /// the division (probation versions are not members); of several, the bot's `cur` if it
    /// is one of them, else the newest. Bots that are retired, delisted or removed have none.
    pub fn ranked(&self, division: Division) -> Vec<&VersionItem> {
        let mut by_bot: BTreeMap<&str, Vec<&VersionItem>> = BTreeMap::new();
        for v in self.versions {
            if v.state == VersionState::Active && v.divisions.contains(&division) {
                by_bot.entry(v.bot.as_str()).or_default().push(v);
            }
        }
        by_bot
            .into_iter()
            .filter_map(|(id, list)| {
                let bot = self.bots.get(id)?;
                if bot.state != BotState::Active {
                    return None;
                }
                list.iter()
                    .find(|v| Some(v.vid) == bot.current)
                    .or_else(|| list.iter().max_by_key(|v| v.vid))
                    .copied()
            })
            .collect()
    }
}

/// One rumble's computed rankings.
pub struct RumbleRun {
    pub id: RumbleId,
    pub metrics: RumbleMetrics,
}

impl RumbleRun {
    /// The member index of a vid.
    pub fn member(&self, vid: u32) -> Option<usize> {
        self.metrics.members.iter().position(|m| m.vid == vid)
    }

    /// Today's snapshot of the rumble.
    pub fn snapshot(&self) -> Snapshot {
        self.metrics
            .order
            .iter()
            .map(|&i| {
                let (member, bot) = (&self.metrics.members[i], &self.metrics.bots[i]);
                (
                    member.id.clone(),
                    SnapshotRow(
                        bot.rank,
                        round2(bot.metrics.aps),
                        (bot.metrics.glicko * 10.0).round() / 10.0,
                        member.version.clone(),
                    ),
                )
            })
            .collect()
    }

    /// The rumble's line in `rumbles.json`. Battles: 1v1 the battles of the members'
    /// pairings; melee the division's counted battles (a melee battle has 45 pairings).
    pub fn summary(&self, config: &Config, state: &State, engine: u32) -> RumbleSummary {
        let division = config.divisions.get(self.id.division);
        let battles = match self.id.division {
            Division::OneVsOne => self.metrics.bots.iter().map(|b| b.battles).sum::<u64>() / 2,
            Division::Melee => state.battles,
        };
        RumbleSummary {
            id: self.id.to_string(),
            division: self.id.division.as_str().to_string(),
            weight_class: self.id.class,
            bots: self.metrics.members.len() as u32,
            battles,
            rounds: division.rounds,
            battlefield: data::Battlefield {
                width: division.width,
                height: division.height,
            },
            engine_version: engine,
        }
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Computes a rumble (§15 step 3) from its division's state: members are the division's
/// ranked versions of the rumble's class or below; `rankWeekAgo` from the snapshot of a
/// week ago.
pub fn compute_rumble(
    id: RumbleId,
    catalog: &Catalog,
    state: &State,
    week_ago: Option<&Snapshot>,
) -> RumbleRun {
    let members = catalog
        .ranked(id.division)
        .into_iter()
        .filter(|v| metrics::is_member(id.division, v.class, id))
        .filter_map(|v| {
            let bot = catalog.bots.get(&v.bot)?;
            Some(Member {
                vid: v.vid,
                id: v.bot.clone(),
                version: v.version.clone(),
                author: bot.author.clone(),
                country: catalog.country(bot),
                rank_week_ago: week_ago.and_then(|s| s.get(&v.bot)).map(|row| row.0),
                colors: v.colors.clone(),
            })
        })
        .collect();
    RumbleRun {
        id,
        metrics: metrics::compute(members, state),
    }
}

/// `rumbles.json`.
pub fn index(summaries: Vec<RumbleSummary>, generated_at: &str) -> RumbleIndex {
    RumbleIndex {
        schema: data::SCHEMA_VERSION,
        generated_at: generated_at.to_string(),
        rumbles: summaries,
    }
}

/// `bots/<id>.json` (§15 step 5): metadata and state, the versions that were ranked (current
/// first, then by release date, else registration, newest first; probation, failed and removed
/// versions are not shown, delisted ones are), and per rumble
/// the bot is ranked in today its numbers with the history extended by today's point (one
/// point a day: a later run of the same day replaces it) and thinned. A bot ranked nowhere
/// today keeps the rumbles of its previous file and gets no point. `None` if the bot has no
/// version to show.
pub fn bot_details(
    bot: &BotItem,
    catalog: &Catalog,
    runs: &[RumbleRun],
    previous: Option<&BotDetails>,
    date: &str,
) -> Option<BotDetails> {
    let by_vid: HashMap<u32, &VersionItem> = catalog.versions.iter().map(|v| (v.vid, v)).collect();
    let mut shown: Vec<&VersionItem> = bot
        .versions
        .iter()
        .filter_map(|vid| by_vid.get(vid).copied())
        .filter(|v| {
            matches!(
                v.state,
                VersionState::Active | VersionState::Retired | VersionState::Delisted
            )
        })
        .collect();
    shown.sort_by_key(|v| {
        (
            Some(v.vid) != bot.current,
            std::cmp::Reverse((release_order(v), v.vid)),
        )
    });
    if shown.is_empty() {
        return None;
    }
    let owner = catalog.owner(bot);
    let mut rumbles = BTreeMap::new();
    for run in runs {
        let Some(member) = run.metrics.members.iter().position(|m| m.id == bot.id) else {
            continue;
        };
        let rumble = run.id.to_string();
        let history = previous
            .and_then(|p| p.rumbles.get(&rumble))
            .map(|r| r.history.clone())
            .unwrap_or_default();
        let history = data::push_history(history, run.metrics.history_point(member, date));
        rumbles.insert(rumble, run.metrics.bot_rumble(member, history));
    }
    if rumbles.is_empty()
        && let Some(previous) = previous
    {
        rumbles = previous.rumbles.clone();
    }
    let text = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    Some(BotDetails {
        schema: data::SCHEMA_VERSION,
        id: bot.id.clone(),
        author: Author {
            name: bot.author.clone(),
            github: owner.map(|u| u.login.clone()),
            country: catalog.country(bot),
        },
        description: text(&bot.description),
        homepage: text(&bot.homepage),
        versions: shown
            .iter()
            .map(|v| BotVersion {
                version: v.version.clone(),
                codesize: v.codesize,
                weight_class: v.class,
                sha256: v.sha256.clone(),
                size: v.size,
                added_at: rfc3339(v.created),
                retired_at: v
                    .retired
                    .filter(|_| v.state == VersionState::Retired)
                    .map(rfc3339),
                colors: v.colors.clone(),
                released_at: v.released.map(|ms| rfc3339(ms / 1000)),
            })
            .collect(),
        rumbles,
        recordings: BTreeMap::new(),
        state: Some(bot.state.as_str().to_string()),
    })
}

/// The time a version is sorted by among a bot's versions (milliseconds): its release date,
/// else its registration.
pub fn release_order(version: &VersionItem) -> u64 {
    version
        .released
        .unwrap_or(version.created.saturating_mul(1000))
}

/// `contributors.json` (§15 step 6): users with battles who haven't hidden themselves and
/// aren't banned, most battles first. `battlesWeek` is the battles since the users snapshot
/// of a week ago (all of them if the user isn't in it; without a snapshot, all of them).
pub fn contributors(
    users: &[User],
    config: &Config,
    engine: u32,
    generated_at: &str,
    week_ago: Option<&UserSnapshot>,
) -> Contributors {
    let mut list: Vec<Contributor> = users
        .iter()
        .filter(|u| u.battles > 0 && !u.hidden && u.reputation.state != UserState::Banned)
        .map(|u| {
            let before = week_ago.and_then(|s| s.get(&u.uid)).copied().unwrap_or(0);
            let tier = match verify::tier(&u.reputation, config) {
                Tier::Verifier => "verifier",
                Tier::Quarantined => "quarantined",
                _ => "new",
            };
            Contributor {
                login: u.login.clone(),
                country: country(Some(u)),
                battles: u.battles,
                battles_week: u.battles.saturating_sub(before),
                tier: tier.to_string(),
                since: rfc3339(u.created),
            }
        })
        .collect();
    list.sort_by(|a, b| {
        b.battles
            .cmp(&a.battles)
            .then_with(|| a.login.cmp(&b.login))
    });
    Contributors {
        schema: data::SCHEMA_VERSION,
        generated_at: generated_at.to_string(),
        engine_version: engine,
        contributors: list,
    }
}

/// Today's users snapshot: every user's battle counter.
pub fn user_snapshot(users: &[User]) -> UserSnapshot {
    users.iter().map(|u| (u.uid.clone(), u.battles)).collect()
}

/// The data file key of a bot (`data/v1/bots/<id>.json`).
pub fn bot_key(id: &str) -> String {
    format!("{DATA_PREFIX}/bots/{id}.json")
}

/// The data file key of a rumble's rankings.
pub fn rankings_key(rumble: &str) -> String {
    format!("{DATA_PREFIX}/rankings/{rumble}.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::record::{Entry, Record};
    use crate::store::{BotSource, Reputation};

    const NOW: u64 = 1_792_843_200; // 2026-10-24T12:00:00Z

    fn version(vid: u32, bot: &str, class: WeightClass, state: VersionState) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: format!("1.{vid}"),
            sha256: format!("{vid:064x}"),
            size: 100,
            codesize: Some(100),
            class,
            divisions: vec![Division::OneVsOne],
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: NOW - 86_400,
            retired: None,
            url: None,
            colors: None,
            released: None,
            nondeterministic: None,
            disagreements: 0,
        }
    }

    fn bot(id: &str, versions: Vec<u32>, current: u32, owner: Option<&str>) -> BotItem {
        BotItem {
            id: id.to_string(),
            author: "Ann".into(),
            owner: owner.map(str::to_string),
            homepage: Some("https://example.org".into()),
            description: Some(String::new()),
            divisions: vec![Division::OneVsOne],
            versions,
            current: Some(current),
            state: BotState::Active,
            created: NOW,
            source: BotSource::Import,
        }
    }

    fn user(uid: &str, battles: u64, rep: u32, state: UserState, hidden: bool) -> User {
        User {
            uid: uid.into(),
            login: format!("l{uid}"),
            name: String::new(),
            country: "BE".into(),
            github_id: 1,
            github_created: None,
            admin: false,
            trusted: false,
            hidden,
            bots: vec![],
            battles,
            last_post: None,
            created: NOW - 10 * 86_400,
            reputation: Reputation {
                rep,
                state,
                ..Reputation::default()
            },
        }
    }

    fn battle(state: &mut State, a: u32, b: u32, score_a: f32) {
        state.fold(
            &Record {
                division: Division::OneVsOne,
                weight: 1,
                uid: "g1".into(),
                contestants: vec![
                    Entry {
                        vid: a,
                        score: score_a,
                        firsts: 5,
                    },
                    Entry {
                        vid: b,
                        score: 100.0 - score_a,
                        firsts: 5,
                    },
                ],
                failed: None,
                trusted: true,
            },
            1,
        );
    }

    #[test]
    fn dates() {
        assert_eq!(date_of(NOW), "2026-10-24");
        assert_eq!(shift_date("2026-10-24", -7).as_deref(), Some("2026-10-17"));
        assert_eq!(shift_date("2026-03-01", -1).as_deref(), Some("2026-02-28"));
        let keys: Vec<String> = ["2026-10-09", "2026-10-10", "2026-10-24", "junk"]
            .iter()
            .map(|d| format!("state/e1/snapshots/1v1/{d}.json"))
            .collect();
        assert_eq!(
            expired_snapshots(&keys, "2026-10-24"),
            vec![keys[0].clone()]
        );
    }

    #[test]
    fn rumbles_bot_files_and_history() {
        let black = crate::data::BotColors {
            body: Some("#000000".into()),
            ..Default::default()
        };
        let versions = vec![
            version(1, "a.A", WeightClass::Nano, VersionState::Retired),
            VersionItem {
                colors: Some(black.clone()),
                ..version(2, "a.A", WeightClass::Nano, VersionState::Active)
            },
            version(3, "b.B", WeightClass::Mega, VersionState::Active),
            version(4, "c.C", WeightClass::Micro, VersionState::Active),
            version(5, "c.C", WeightClass::Micro, VersionState::Probation),
        ];
        let mut bots = BTreeMap::new();
        bots.insert("a.A".into(), bot("a.A", vec![2, 1], 2, Some("g9")));
        bots.insert("b.B".into(), bot("b.B", vec![3], 3, None));
        bots.insert("c.C".into(), bot("c.C", vec![5, 4], 4, None));
        let mut owners = BTreeMap::new();
        owners.insert("g9".into(), user("g9", 0, 0, UserState::New, false));
        // The owner's country wins over the wiki's flag; an unclaimed bot takes the wiki's.
        let flags = FlagMap {
            packages: [("a", "us"), ("b", "nl")]
                .into_iter()
                .map(|(p, f)| (p.to_string(), f.to_string()))
                .collect(),
            at: 0,
        };
        let catalog = Catalog {
            versions: &versions,
            bots: &bots,
            owners: &owners,
            flags: &flags,
        };
        let mut state = State::new(1, Division::OneVsOne, 0);
        battle(&mut state, 2, 3, 70.0);
        battle(&mut state, 2, 4, 60.0);
        battle(&mut state, 3, 4, 55.0);
        battle(&mut state, 1, 3, 10.0); // a retired version: not a member
        let mut week = Snapshot::new();
        week.insert("b.B".into(), SnapshotRow(1, 60.0, 1600.0, "1.3".into()));
        let mega = compute_rumble(
            RumbleId::new(Division::OneVsOne, WeightClass::Mega),
            &catalog,
            &state,
            Some(&week),
        );
        let micro = compute_rumble(
            RumbleId::new(Division::OneVsOne, WeightClass::Micro),
            &catalog,
            &state,
            None,
        );
        assert_eq!(mega.metrics.members.len(), 3);
        assert_eq!(micro.metrics.members.len(), 2);
        let summary = mega.summary(&Config::default(), &state, 1);
        assert_eq!(
            (summary.bots, summary.battles, summary.id.as_str()),
            (3, 3, "1v1")
        );
        let rankings = mega.metrics.rankings("1v1", "t", 1);
        assert_eq!(rankings.bots[0].id, "a.A");
        assert_eq!(rankings.bots[0].country.as_deref(), Some("be"));
        assert_eq!(rankings.bots[0].colors.as_ref(), Some(&black));
        let b_row = rankings.bots.iter().find(|r| r.id == "b.B").unwrap();
        assert_eq!(b_row.rank_week_ago, Some(1));
        assert_eq!(b_row.colors, None);
        assert_eq!(b_row.country.as_deref(), Some("nl"));
        let c_row = rankings.bots.iter().find(|r| r.id == "c.C").unwrap();
        assert_eq!(c_row.country, None);
        let snapshot = mega.snapshot();
        assert_eq!(snapshot["a.A"].0, 1);
        assert_eq!(snapshot["a.A"].3, "1.2");

        let runs = vec![mega, micro];
        let a = bot_details(&bots["a.A"], &catalog, &runs, None, "2026-10-24").unwrap();
        assert_eq!(
            a.versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.2", "1.1"]
        );
        // The current version's colours; the retired one's aren't known.
        assert_eq!(a.versions[0].colors.as_ref(), Some(&black));
        assert_eq!(a.versions[1].colors, None);
        assert_eq!(a.author.github.as_deref(), Some("lg9"));
        assert_eq!(a.author.country.as_deref(), Some("be"));
        let b = bot_details(&bots["b.B"], &catalog, &runs, None, "2026-10-24").unwrap();
        assert_eq!(b.author.country.as_deref(), Some("nl"));
        assert_eq!(a.description, None);
        assert_eq!(a.rumbles.len(), 2);
        assert_eq!(a.rumbles["1v1"].history.len(), 1);
        // A later run on the same day replaces the point; the next day adds one.
        let again = bot_details(&bots["a.A"], &catalog, &runs, Some(&a), "2026-10-24").unwrap();
        assert_eq!(again, a);
        let next = bot_details(&bots["a.A"], &catalog, &runs, Some(&a), "2026-10-25").unwrap();
        assert_eq!(next.rumbles["1v1"].history.len(), 2);
        // c.C: its probation version isn't shown.
        let c = bot_details(&bots["c.C"], &catalog, &runs, None, "2026-10-24").unwrap();
        assert_eq!(c.versions.len(), 1);
        // Not ranked today: the previous rumbles stay.
        let a_gone = bot_details(&bots["a.A"], &catalog, &[], Some(&a), "2026-10-25").unwrap();
        assert_eq!(a_gone.rumbles, a.rumbles);
        // JSON round trip through the data types.
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<BotDetails>(&json).unwrap(), a);
    }

    #[test]
    fn delisted_bots_are_not_ranked_but_keep_their_file() {
        let versions = vec![
            version(1, "a.A", WeightClass::Mega, VersionState::Active),
            version(2, "b.B", WeightClass::Mega, VersionState::Active),
            VersionItem {
                released: Some((NOW - 400 * DAY) * 1000),
                nondeterministic: None,
                disagreements: 0,
                ..version(3, "d.D", WeightClass::Mega, VersionState::Delisted)
            },
            VersionItem {
                released: Some((NOW - 900 * DAY) * 1000),
                nondeterministic: None,
                disagreements: 0,
                retired: Some(NOW - 500 * DAY),
                ..version(4, "d.D", WeightClass::Mega, VersionState::Retired)
            },
            // Registered later, released earlier than 4: sorted by the release date.
            VersionItem {
                released: Some((NOW - 1000 * DAY) * 1000),
                nondeterministic: None,
                disagreements: 0,
                retired: Some(NOW - 900 * DAY),
                ..version(5, "d.D", WeightClass::Mega, VersionState::Retired)
            },
        ];
        let mut bots = BTreeMap::new();
        bots.insert("a.A".into(), bot("a.A", vec![1], 1, None));
        bots.insert("b.B".into(), bot("b.B", vec![2], 2, None));
        bots.insert(
            "d.D".into(),
            BotItem {
                state: BotState::Delisted,
                ..bot("d.D", vec![3, 4, 5], 3, None)
            },
        );
        let owners = BTreeMap::new();
        let flags = FlagMap::default();
        let catalog = Catalog {
            versions: &versions,
            bots: &bots,
            owners: &owners,
            flags: &flags,
        };
        let mut state = State::new(1, Division::OneVsOne, 0);
        battle(&mut state, 1, 2, 60.0);
        // The delisted bot's frozen pairings: a.A lost badly, b.B won.
        for _ in 0..5 {
            battle(&mut state, 1, 3, 0.0);
            battle(&mut state, 2, 3, 100.0);
        }
        let run = compute_rumble(
            RumbleId::new(Division::OneVsOne, WeightClass::Mega),
            &catalog,
            &state,
            None,
        );
        let ids: Vec<&str> = run.metrics.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a.A", "b.B"]);
        let rankings = run.metrics.rankings("1v1", "t", 1);
        assert_eq!(rankings.bots[0].id, "a.A");
        assert_eq!(
            (rankings.bots[0].metrics.aps, rankings.bots[0].pairings),
            (60.0, 1)
        );
        assert_eq!(rankings.bots[0].battles, 1);

        // Its file: written, with the state, the last rumbles kept and the versions current
        // first, then by release date.
        let ranked = bot_details(&bots["a.A"], &catalog, &[run], None, "2026-10-24").unwrap();
        assert_eq!(ranked.state.as_deref(), Some("active"));
        let earlier = BotDetails {
            rumbles: ranked.rumbles.clone(),
            ..ranked
        };
        let d = bot_details(&bots["d.D"], &catalog, &[], Some(&earlier), "2026-10-25").unwrap();
        assert_eq!(d.state.as_deref(), Some("delisted"));
        assert_eq!(d.rumbles, earlier.rumbles);
        let shown: Vec<(&str, Option<&str>, Option<&str>)> = d
            .versions
            .iter()
            .map(|v| {
                (
                    v.version.as_str(),
                    v.released_at.as_deref(),
                    v.retired_at.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("1.3", Some("2025-09-19T12:00:00Z"), None),
                (
                    "1.4",
                    Some("2024-05-07T12:00:00Z"),
                    Some("2025-06-11T12:00:00Z")
                ),
                (
                    "1.5",
                    Some("2024-01-28T12:00:00Z"),
                    Some("2024-05-07T12:00:00Z")
                ),
            ]
        );
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["state"], "delisted");
        assert_eq!(json["versions"][0]["releasedAt"], "2025-09-19T12:00:00Z");
    }

    #[test]
    fn contributor_list() {
        let config = Config::default();
        let users = vec![
            user("g1", 10, 0, UserState::New, false),
            user("g2", 30, 60, UserState::New, false),
            user("g3", 50, 0, UserState::Banned, false),
            user("g4", 40, 0, UserState::New, true),
            user("g5", 0, 0, UserState::New, false),
            user("g6", 5, 0, UserState::Quarantined, false),
        ];
        let mut week = UserSnapshot::new();
        week.insert("g2".into(), 25);
        let list = contributors(&users, &config, 1, "t", Some(&week));
        let rows: Vec<(&str, u64, u64, &str)> = list
            .contributors
            .iter()
            .map(|c| (c.login.as_str(), c.battles, c.battles_week, c.tier.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("lg2", 30, 5, "verifier"),
                ("lg1", 10, 10, "new"),
                ("lg6", 5, 5, "quarantined")
            ]
        );
        assert_eq!(list.contributors[0].since, "2026-10-14T12:00:00Z");
        assert_eq!(user_snapshot(&users)["g3"], 50);
    }
}
