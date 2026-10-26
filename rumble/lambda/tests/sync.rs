// SPDX-License-Identifier: EPL-2.0

//! The `batch` Lambda's `sync` mode (spec §16.5) on the in-memory services: lists and jars
//! from a fake web, the registry in a `MemStore`.

mod common;

use common::{jar, sha_hex};
use rumble_core::store::{BotItem, BotSource, BotState, DAY, MemStore, Store, VersionState};
use rumble_core::sync::{SYNC_KEY, SyncState};
use rumble_core::{Config, Division};
use rumble_lambda::batch::sync::SyncReport;
use rumble_lambda::batch::{Batch, Mode, Services};
use rumble_lambda::cloudfront::MemCdn;
use rumble_lambda::fetch::MemFetcher;
use rumble_lambda::s3::{Bucket, Fetched, MemObjects, Objects};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const T0_MS: u64 = 1_792_843_200_000;
const LIST_1V1: &str = "http://wiki/1v1";
const LIST_MELEE: &str = "http://wiki/melee";

struct Web {
    store: Arc<MemStore>,
    objects: Arc<MemObjects>,
    fetcher: Arc<MemFetcher>,
    clock: Arc<AtomicU64>,
}

impl Web {
    fn new() -> Web {
        let objects = Arc::new(MemObjects::new());
        let config = json!({"sync": {
            "lists": {"1v1": LIST_1V1, "melee": LIST_MELEE},
            "mirror": "http://mirror/{id}_{version}.jar",
            "minEntries": 1,
            "minListShare": 0.0,
            "maxDelistPerRun": 1
        }});
        objects
            .put(
                Bucket::State,
                "state/config.json",
                &serde_json::to_vec(&config).unwrap(),
                "application/json",
                None,
            )
            .unwrap();
        Web {
            store: Arc::new(MemStore::new()),
            objects,
            fetcher: Arc::new(MemFetcher::new()),
            clock: Arc::new(AtomicU64::new(T0_MS)),
        }
    }

    fn lists(&self, one: &[&str], melee: &[&str]) {
        for (url, entries) in [(LIST_1V1, one), (LIST_MELEE, melee)] {
            let lines: String = entries
                .iter()
                .map(|e| {
                    let (id, version) = e.split_once(' ').unwrap();
                    format!("{id} {version},http://list/{id}_{version}.jar\n")
                })
                .collect();
            self.fetcher
                .set(url, format!("Intro\n<pre>\n{lines}</pre>\nnotes\n"));
        }
    }

    fn serve(&self, url: &str, id: &str, version: &str) {
        self.fetcher.set(url, jar(id, Some(version)));
    }

    fn sync(&self) -> SyncReport {
        let clock = self.clock.clone();
        let batch = Batch::new(Services {
            store: Box::new(self.store.clone()),
            objects: Box::new(self.objects.clone()),
            cdn: Box::new(Arc::new(MemCdn::new())),
            fetcher: Box::new(self.fetcher.clone()),
            clock: Box::new(move || clock.load(Ordering::SeqCst)),
            sleep: Box::new(|_| {}),
            random: Box::new(|| [0; 32]),
        });
        serde_json::from_value(batch.run(Mode::Sync).expect("sync")).expect("sync report")
    }

    fn state(&self) -> SyncState {
        match self.objects.get(Bucket::State, SYNC_KEY, None).unwrap() {
            Fetched::Found { bytes, .. } => serde_json::from_slice(&bytes).unwrap(),
            _ => panic!("no {SYNC_KEY}"),
        }
    }

    fn bot(&self, id: &str) -> BotItem {
        self.store.get_bot(id).unwrap().unwrap()
    }

    fn version_state(&self, vid: u32) -> VersionState {
        self.store.get_version(vid).unwrap().unwrap().state
    }
}

#[test]
fn the_lists_drive_the_unclaimed_imported_bots() {
    let web = Web::new();
    // A claimed bot that is listed with another version: never touched.
    web.store
        .put_bot(&BotItem {
            id: "c.C".into(),
            author: "Cy".into(),
            owner: Some("g1".into()),
            homepage: None,
            description: None,
            divisions: vec![Division::Melee],
            versions: vec![],
            current: None,
            state: BotState::Active,
            created: 0,
            source: BotSource::Import,
        })
        .unwrap();

    // Day 1: a.A in both lists, n.New only from the mirror, f.Fail nowhere.
    web.lists(&["a.A 1", "n.New 1.0", "f.Fail 1"], &["a.A 1", "c.C 9"]);
    web.serve("http://list/a.A_1.jar", "a.A", "1");
    web.serve("http://mirror/n.New_1.0.jar", "n.New", "1.0");
    let report = web.sync();
    assert_eq!(
        (report.wanted, report.fetched, report.failed),
        (3, 2, 1),
        "{report:?}"
    );
    assert_eq!((report.counts.imported, report.counts.new_bots), (2, 2));
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].id, "c.C");
    let a = web.bot("a.A");
    assert_eq!(
        (a.divisions.clone(), a.source, a.owner.clone()),
        (
            vec![Division::OneVsOne, Division::Melee],
            BotSource::Import,
            None
        )
    );
    let v = web.store.get_version(a.current.unwrap()).unwrap().unwrap();
    assert_eq!((v.state, v.released), (VersionState::Active, Some(T0_MS)));
    let n = web
        .store
        .get_version(web.bot("n.New").current.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(n.url.as_deref(), Some("http://mirror/n.New_1.0.jar"));
    for sha in [&v.sha256, &n.sha256] {
        assert!(
            web.objects
                .object(Bucket::Jars, &format!("jars/{sha}.jar"))
                .is_some()
        );
    }
    assert_eq!(v.sha256, sha_hex(&jar("a.A", Some("1"))));
    let state = web.state();
    assert_eq!(state.divisions["1v1"].entries, 3);
    assert_eq!(
        state.failures["f.Fail 1"].error,
        "download (download failed: HTTP 404)"
    );
    assert!(web.store.get_bot("f.Fail").unwrap().is_none());
    assert!(
        web.store
            .get_bot("c.C")
            .unwrap()
            .unwrap()
            .versions
            .is_empty()
    );

    // Day 2: a.A has a new version in 1v1 and left melee; its new jar fails validation (a
    // mismatch): nothing changes for it. The 1v1 list fails to load: nothing changes there.
    web.clock.fetch_add(DAY * 1000, Ordering::SeqCst);
    web.lists(&["a.A 2", "n.New 1.0", "f.Fail 1"], &[]);
    web.serve("http://list/a.A_2.jar", "a.A", "3");
    let report = web.sync();
    assert_eq!(report.counts.imported, 0);
    assert!(
        report.lists["melee"].trusted.eq(&false),
        "an empty list is not trusted"
    );
    assert!(report.log.iter().any(|l| l.starts_with("ALERT sync melee")));
    assert_eq!(web.bot("a.A").current, a.current);
    assert!(web.state().failures["a.A 2"].error.starts_with("mismatch"));
    assert_eq!(
        web.bot("a.A").divisions,
        [Division::OneVsOne, Division::Melee]
    );

    // Day 3: the right jar; a.A 2 replaces a.A 1, in 1v1 only (melee is back, without a.A).
    web.clock.fetch_add(DAY * 1000, Ordering::SeqCst);
    web.lists(&["a.A 2", "n.New 1.0", "f.Fail 1"], &["n.New 1.0"]);
    web.serve("http://list/a.A_2.jar", "a.A", "2");
    let report = web.sync();
    assert_eq!(
        (report.counts.imported, report.counts.retired),
        (1, 1),
        "{report:?}"
    );
    let a2 = web.bot("a.A");
    assert_eq!(a2.divisions, [Division::OneVsOne]);
    assert_eq!(web.version_state(a.current.unwrap()), VersionState::Retired);
    assert!(!web.state().failures.contains_key("a.A 2"));
    assert_eq!(
        web.bot("n.New").divisions,
        [Division::OneVsOne, Division::Melee]
    );

    // Day 4: two bots leave both lists, more than maxDelistPerRun (1): nothing is delisted.
    web.clock.fetch_add(DAY * 1000, Ordering::SeqCst);
    web.lists(&["f.Fail 1"], &["z.Z 1"]);
    let report = web.sync();
    assert!(report.delist_guard);
    assert_eq!(report.counts.delisted, 0);
    assert_eq!(web.bot("a.A").state, BotState::Active);

    // Day 5: only a.A leaves: delisted, its version too; `cur` stays.
    web.clock.fetch_add(DAY * 1000, Ordering::SeqCst);
    web.lists(&["n.New 1.0", "f.Fail 1"], &["n.New 1.0"]);
    let report = web.sync();
    assert_eq!(report.counts.delisted, 1, "{report:?}");
    let delisted = web.bot("a.A");
    assert_eq!(
        (delisted.state, delisted.current),
        (BotState::Delisted, a2.current)
    );
    assert_eq!(
        web.version_state(a2.current.unwrap()),
        VersionState::Delisted
    );

    // f.Fail has failed for a week: one alert, once.
    web.clock.fetch_add(3 * DAY * 1000, Ordering::SeqCst);
    let report = web.sync();
    let alerts: Vec<&String> = report
        .log
        .iter()
        .filter(|l| l.starts_with("ALERT sync f.Fail 1"))
        .collect();
    assert_eq!(alerts.len(), 1, "{:?}", report.log);
    assert!(
        !web.sync()
            .log
            .iter()
            .any(|l| l.starts_with("ALERT sync f.Fail"))
    );

    // Day 9: a.A 2 is listed again: active with its pairings, no new version.
    web.lists(&["a.A 2", "n.New 1.0", "f.Fail 1"], &["n.New 1.0"]);
    let report = web.sync();
    assert_eq!((report.counts.relisted, report.counts.imported), (1, 0));
    let back = web.bot("a.A");
    assert_eq!((back.state, back.versions.len()), (BotState::Active, 2));
    assert_eq!(web.version_state(a2.current.unwrap()), VersionState::Active);

    // Disabled: nothing is read.
    let calls = web.fetcher.calls().len();
    web.objects
        .put(
            Bucket::State,
            "state/config.json",
            br#"{"sync": {"enabled": false}}"#,
            "application/json",
            None,
        )
        .unwrap();
    assert!(!web.sync().enabled);
    assert_eq!(web.fetcher.calls().len(), calls);
    assert!(Config::default().sync.enabled);
}
