// SPDX-License-Identifier: EPL-2.0

//! `GET work` (spec §10.4): a batch of one division, verification and probation work first
//! for verifiers, the rest sampled from the division's plan, signed into a batch token. The
//! [`WorkServer`] holds what an `api` Lambda instance keeps in memory: the plans with their
//! samplers (whose quotas the instance uses up) and the "queue empty" hints.

use crate::api::{
    ApiError, BattleKind, ErrorCode, Rules, WorkBattle, WorkBot, WorkQuery, WorkResponse,
};
use crate::config::{Config, DivisionConfig};
use crate::ids::Division;
use crate::plan::{MeleeSampler, Plan, QuotaSampler};
use crate::results::Random;
use crate::store::{Store, StoreResult, User, VerifyKind, rfc3339};
use crate::token::{self, BatchBattle, BatchPayload, Keys};
use crate::verify::{self, is_verifier};
use std::collections::{BTreeMap, HashMap};

/// How long an instance skips the verification queue of a division after finding nothing
/// to claim in it (§9.3).
pub const QUEUE_EMPTY_SECONDS: u64 = 30;

/// A division's plan with the instance's samplers.
#[derive(Clone, Debug)]
pub struct DivisionPlan {
    pub plan: Plan,
    pairs: QuotaSampler<(u32, u32)>,
    melee: MeleeSampler,
    /// vid → index in `plan.bots`.
    bots: HashMap<u32, usize>,
}

impl DivisionPlan {
    pub fn new(mut plan: Plan) -> DivisionPlan {
        plan.top.sort_unstable();
        plan.probation.sort_unstable();
        let bots = plan
            .bots
            .iter()
            .enumerate()
            .map(|(i, row)| (row.0, i))
            .collect();
        DivisionPlan {
            pairs: plan.pair_sampler(),
            melee: plan.melee_sampler(),
            bots,
            plan,
        }
    }

    /// Whether the plan has battles: at least two active bots.
    pub fn has_work(&self) -> bool {
        match self.plan.division {
            Division::OneVsOne => !self.plan.pairs.is_empty(),
            Division::Melee => !self.plan.anchors.is_empty(),
        }
    }

    /// Whether a version is in the plan's `top` (slot eligibility, §12.5).
    pub fn is_top(&self, vid: u32) -> bool {
        self.plan.top.binary_search(&vid).is_ok()
    }

    /// A bot of the plan as a work item.
    pub fn bot(&self, vid: u32) -> Option<WorkBot> {
        let (vid, id, version, sha256, size) = self.plan.bots.get(*self.bots.get(&vid)?)?.clone();
        Some(work_bot(vid, id, version, sha256, size))
    }
}

fn work_bot(vid: u32, id: String, version: String, sha256: String, size: u64) -> WorkBot {
    WorkBot {
        url: format!("/jars/{sha256}.jar"),
        id,
        version,
        vid,
        sha256,
        size,
        data: None,
    }
}

/// The rules of a division's battles with `rounds` rounds.
pub fn rules(division: &DivisionConfig, rounds: u32) -> Rules {
    Rules {
        rounds,
        width: division.width,
        height: division.height,
        gun_cooling_rate: division.gun_cooling_rate,
        inactivity_time: division.inactivity_time,
        sentry_border: division.sentry_border,
        hide_enemy_names: division.hide_enemy_names,
    }
}

/// A batch id: `b-` and a ULID (48 bits of milliseconds, 80 random bits, Crockford base32),
/// so ids sort by time.
pub fn batch_id(now_ms: u64, random: &mut Random) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let high = u128::from(now_ms & ((1 << 48) - 1)) << 80;
    let low = (u128::from(random.next_u64()) << 16 | u128::from(random.next_u64() & 0xffff))
        & ((1 << 80) - 1);
    let value = high | low;
    let mut id = String::from("b-");
    for i in (0..26).rev() {
        id.push(ALPHABET[((value >> (5 * i)) & 31) as usize] as char);
    }
    id
}

fn uniform(random: &mut Random) -> f64 {
    (random.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

fn shuffle<T>(items: &mut [T], random: &mut Random) {
    for i in (1..items.len()).rev() {
        let j = (random.next_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// What an `api` instance keeps for `GET work`.
#[derive(Clone, Debug, Default)]
pub struct WorkServer {
    plans: BTreeMap<Division, DivisionPlan>,
    queue_empty_until: BTreeMap<Division, u64>,
}

impl WorkServer {
    pub fn new() -> WorkServer {
        WorkServer::default()
    }

    /// Installs a (re)loaded plan; the division's samplers start from its quotas again.
    pub fn set_plan(&mut self, plan: Plan) {
        self.plans.insert(plan.division, DivisionPlan::new(plan));
    }

    pub fn remove_plan(&mut self, division: Division) {
        self.plans.remove(&division);
    }

    pub fn plan(&self, division: Division) -> Option<&DivisionPlan> {
        self.plans.get(&division)
    }

    /// Whether a version is in the `top` of the division's current plan.
    pub fn is_top(&self, division: Division, vid: u32) -> bool {
        self.plans.get(&division).is_some_and(|p| p.is_top(vid))
    }

    /// Serves `GET work` to `user` (§10.4) at `now_ms` with 32 fresh random bytes. `None`
    /// means 204: the rumble is paused, or no division the client offers has two active
    /// bots. 409 `engine_mismatch` if the client's engine isn't the configured one.
    #[allow(clippy::too_many_arguments)]
    pub fn serve<S: Store + ?Sized>(
        &mut self,
        store: &S,
        config: &Config,
        keys: &Keys,
        user: &User,
        query: &WorkQuery,
        now_ms: u64,
        random: &[u8; 32],
    ) -> Result<Option<WorkResponse>, ApiError> {
        let engine = config.engine_version;
        if query.engine != engine {
            return Err(ApiError::new(
                ErrorCode::EngineMismatch,
                format!(
                    "the rumble ranks engine version {engine}; this client has engine {}",
                    query.engine
                ),
            ));
        }
        if config.paused {
            return Ok(None);
        }
        let now = now_ms / 1000;
        let mut random = Random::new(random);

        // 1. One division, with probability ∝ its share among those offered with work.
        let mut offered: Vec<Division> = Vec::new();
        for &division in &query.divisions {
            if !offered.contains(&division)
                && self.plans.get(&division).is_some_and(|p| p.has_work())
            {
                offered.push(division);
            }
        }
        let Some(division) = choose(&offered, config, &mut random) else {
            return Ok(None);
        };
        let division_config = config.divisions.get(division);
        let wanted = query
            .max
            .min(*config.batch.max_battles.get(division))
            .max(1) as usize;
        let deadline = now + u64::from(config.batch.deadline_minutes) * 60;

        let mut battles: Vec<(BatchBattle, Vec<WorkBot>)> = Vec::new();

        // 2. Verification and probation work for verifiers, up to half the batch.
        if is_verifier(&user.reputation, config)
            && self
                .queue_empty_until
                .get(&division)
                .is_none_or(|&until| until <= now)
        {
            let items = verify::take_work_in(
                store,
                engine,
                &user.uid,
                Some(division),
                wanted.div_ceil(2),
                deadline,
                now,
            )?;
            if items.is_empty() {
                self.queue_empty_until
                    .insert(division, now + QUEUE_EMPTY_SECONDS);
            }
            for item in items {
                let bots = match self.bots_of(store, division, &item.bots)? {
                    Some(bots) => bots,
                    None => {
                        // A version vanished: give the item back rather than serve it.
                        verify::release_claim(store, engine, &item.sort, &user.uid)?;
                        continue;
                    }
                };
                let kind = match item.kind {
                    VerifyKind::Probation => BattleKind::Probation,
                    VerifyKind::Check | VerifyKind::Recheck => BattleKind::Verify,
                };
                battles.push((
                    BatchBattle {
                        division,
                        kind,
                        seed: item.seed,
                        rounds: item.rounds,
                        vids: item.bots.clone(),
                        item: Some(item.sort.clone()),
                    },
                    bots,
                ));
            }
        }

        // 3. The rest from the plan.
        let plan = self
            .plans
            .get_mut(&division)
            .expect("offered divisions have plans");
        while battles.len() < wanted {
            let mut vids = match division {
                Division::OneVsOne => {
                    let (u1, u2) = (uniform(&mut random), uniform(&mut random));
                    match plan.pairs.draw(u1, u2) {
                        Some((a, b)) => vec![a, b],
                        None => break,
                    }
                }
                Division::Melee => {
                    let size = division_config.robots.max(2) as usize;
                    plan.melee.battle(size, &mut || uniform(&mut random))
                }
            };
            if vids.len() < 2 {
                break;
            }
            // 4. Robot order: shuffled; seeds: 64 random bits.
            shuffle(&mut vids, &mut random);
            let Some(bots) = vids
                .iter()
                .map(|&vid| plan.bot(vid))
                .collect::<Option<Vec<_>>>()
            else {
                break;
            };
            battles.push((
                BatchBattle {
                    division,
                    kind: BattleKind::Normal,
                    seed: random.next_u64() as i64,
                    rounds: division_config.rounds,
                    vids,
                    item: None,
                },
                bots,
            ));
        }
        if battles.is_empty() {
            return Ok(None);
        }

        // 5. Sign; nothing is stored.
        let id = batch_id(now_ms, &mut random);
        let payload = BatchPayload {
            v: 1,
            id: id.clone(),
            uid: user.uid.clone(),
            e: engine,
            iat: now,
            exp: deadline,
            b: battles.iter().map(|(b, _)| b.clone()).collect(),
            k: String::new(),
        };
        let batch = token::sign(&payload, keys)
            .map_err(|e| ApiError::new(ErrorCode::Busy, format!("signing: {e}")))?;
        let battles = battles
            .into_iter()
            .enumerate()
            .map(|(index, (battle, bots))| WorkBattle {
                index: index as u32,
                kind: battle.kind,
                division,
                seed: battle.seed,
                rules: rules(division_config, battle.rounds),
                bots,
            })
            .collect();
        Ok(Some(WorkResponse {
            batch,
            batch_id: id,
            engine_version: engine,
            deadline: rfc3339(deadline),
            battles,
        }))
    }

    /// The work items of a verification item's versions: from the plan, else the store.
    fn bots_of<S: Store + ?Sized>(
        &self,
        store: &S,
        division: Division,
        vids: &[u32],
    ) -> StoreResult<Option<Vec<WorkBot>>> {
        let plan = self.plans.get(&division);
        let mut bots = Vec::new();
        for &vid in vids {
            if let Some(bot) = plan.and_then(|p| p.bot(vid)) {
                bots.push(bot);
                continue;
            }
            match store.get_version(vid)? {
                Some(v) => bots.push(work_bot(v.vid, v.bot, v.version, v.sha256, v.size)),
                None => return Ok(None),
            }
        }
        Ok(Some(bots))
    }
}

fn choose(offered: &[Division], config: &Config, random: &mut Random) -> Option<Division> {
    let weights: Vec<f64> = offered
        .iter()
        .map(|&d| config.divisions.get(d).share.max(0.0))
        .collect();
    let total: f64 = weights.iter().sum();
    if offered.is_empty() {
        return None;
    }
    if total <= 0.0 || !total.is_finite() {
        let at = (uniform(random) * offered.len() as f64) as usize;
        return offered.get(at.min(offered.len() - 1)).copied();
    }
    let mut target = uniform(random) * total;
    for (&division, &weight) in offered.iter().zip(&weights) {
        if target < weight {
            return Some(division);
        }
        target -= weight;
    }
    offered.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::store::{
        MemStore, Reputation, Run, VERIFY_TTL, VerifyItem, VersionItem, VersionState,
    };

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const NOW_MS: u64 = 1_792_843_200_000;
    const NOW: u64 = NOW_MS / 1000;

    fn keys() -> Keys {
        Keys::parse(&format!("a:{KEY}")).unwrap()
    }

    fn row(vid: u32) -> (u32, String, String, String, u64) {
        (
            vid,
            format!("a.Bot{vid}"),
            "1.0".into(),
            format!("{vid:064x}"),
            1000 + u64::from(vid),
        )
    }

    fn plan_1v1(pairs: Vec<(u32, u32, u32)>) -> Plan {
        Plan {
            engine: 1,
            division: Division::OneVsOne,
            generated_at: String::new(),
            hourly: pairs.iter().map(|p| p.2).sum(),
            bots: (1..=4).map(row).collect(),
            pairs,
            anchors: vec![],
            partners: BTreeMap::new(),
            top: vec![2],
            probation: vec![],
        }
    }

    fn plan_melee() -> Plan {
        let mut partners = BTreeMap::new();
        for vid in 1..=12u32 {
            partners.insert(
                vid,
                (1..=12).filter(|&o| o != vid).map(|o| (o, 1.0)).collect(),
            );
        }
        Plan {
            engine: 1,
            division: Division::Melee,
            generated_at: String::new(),
            hourly: 3,
            bots: (1..=12).map(row).collect(),
            pairs: vec![],
            anchors: vec![(1, 2), (5, 1)],
            partners,
            top: vec![],
            probation: vec![],
        }
    }

    fn user(rep: u32) -> User {
        User {
            uid: "g1".into(),
            reputation: Reputation {
                rep,
                ..Reputation::default()
            },
            ..User::default()
        }
    }

    fn query(max: u32, divisions: Vec<Division>) -> WorkQuery {
        WorkQuery {
            engine: 1,
            max,
            divisions,
            slots: 4,
        }
    }

    #[test]
    fn serves_a_signed_1v1_batch_from_the_plan() {
        let store = MemStore::new();
        let mut server = WorkServer::new();
        server.set_plan(plan_1v1(vec![(1, 2, 3), (3, 4, 1)]));
        let config = Config::default();
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(0),
                &query(6, vec![Division::OneVsOne]),
                NOW_MS,
                &[1; 32],
            )
            .unwrap()
            .unwrap();
        assert_eq!(work.battles.len(), 6);
        assert!(work.batch_id.starts_with("b-") && work.batch_id.len() == 28);
        assert_eq!(work.deadline, rfc3339(NOW + 180 * 60));
        let payload = token::verify(&work.batch, &keys(), NOW).unwrap();
        assert_eq!(payload.id, work.batch_id);
        assert_eq!(payload.uid, "g1");
        assert_eq!(payload.exp, NOW + 180 * 60);
        // The plan's 4 battles of quota are used first: 3 × (1,2) and 1 × (3,4).
        let mut first: Vec<Vec<u32>> = payload.b[..4]
            .iter()
            .map(|b| {
                let mut v = b.vids.clone();
                v.sort_unstable();
                v
            })
            .collect();
        first.sort();
        assert_eq!(first, vec![vec![1, 2], vec![1, 2], vec![1, 2], vec![3, 4]]);
        for (battle, signed) in work.battles.iter().zip(&payload.b) {
            assert_eq!(battle.kind, BattleKind::Normal);
            assert_eq!(battle.seed, signed.seed);
            assert_eq!(battle.rules.rounds, 35);
            assert_eq!(battle.rules.width, 800);
            let vids: Vec<u32> = battle.bots.iter().map(|b| b.vid).collect();
            assert_eq!(vids, signed.vids);
            assert_eq!(
                battle.bots[0].url,
                format!("/jars/{}.jar", battle.bots[0].sha256)
            );
            assert_eq!(battle.bots[0].data, None);
        }
        // Robot order is shuffled: both orders appear over many batches.
        let mut orders = std::collections::HashSet::new();
        for i in 0..40u8 {
            let w = server
                .serve(
                    &store,
                    &config,
                    &keys(),
                    &user(0),
                    &query(1, vec![Division::OneVsOne]),
                    NOW_MS,
                    &[i; 32],
                )
                .unwrap()
                .unwrap();
            orders.insert(w.battles[0].bots[0].vid < w.battles[0].bots[1].vid);
        }
        assert_eq!(orders.len(), 2);
        assert!(server.is_top(Division::OneVsOne, 2));
        assert!(!server.is_top(Division::OneVsOne, 1));
        assert!(!server.is_top(Division::Melee, 2));
    }

    #[test]
    fn batch_size_is_capped_by_the_division() {
        let store = MemStore::new();
        let mut server = WorkServer::new();
        server.set_plan(plan_melee());
        let config = Config::default();
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(0),
                &query(50, vec![Division::Melee]),
                NOW_MS,
                &[2; 32],
            )
            .unwrap()
            .unwrap();
        assert_eq!(work.battles.len(), 10);
        for battle in &work.battles {
            assert_eq!(battle.division, Division::Melee);
            assert_eq!(battle.bots.len(), 10);
            let mut vids: Vec<u32> = battle.bots.iter().map(|b| b.vid).collect();
            vids.sort_unstable();
            vids.dedup();
            assert_eq!(vids.len(), 10, "distinct bots");
            assert_eq!(battle.rules.width, 1000);
        }
    }

    #[test]
    fn no_work_paused_and_engine_mismatch() {
        let store = MemStore::new();
        let mut server = WorkServer::new();
        let config = Config::default();
        let ask = |server: &mut WorkServer, config: &Config, q: &WorkQuery| {
            server.serve(&store, config, &keys(), &user(0), q, NOW_MS, &[3; 32])
        };
        // No plan at all.
        assert_eq!(
            ask(&mut server, &config, &query(5, Division::ALL.to_vec())),
            Ok(None)
        );
        // A plan with one bot has no pairs.
        server.set_plan(plan_1v1(vec![]));
        assert_eq!(
            ask(&mut server, &config, &query(5, Division::ALL.to_vec())),
            Ok(None)
        );
        server.set_plan(plan_1v1(vec![(1, 2, 1)]));
        // Only melee offered.
        assert_eq!(
            ask(&mut server, &config, &query(5, vec![Division::Melee])),
            Ok(None)
        );
        assert!(
            ask(&mut server, &config, &query(5, Division::ALL.to_vec()))
                .unwrap()
                .is_some()
        );
        let paused = Config {
            paused: true,
            ..Config::default()
        };
        assert_eq!(
            ask(&mut server, &paused, &query(5, Division::ALL.to_vec())),
            Ok(None)
        );
        let mut other = query(5, Division::ALL.to_vec());
        other.engine = 2;
        assert_eq!(
            ask(&mut server, &config, &other).unwrap_err().code,
            ErrorCode::EngineMismatch
        );
    }

    #[test]
    fn divisions_follow_their_shares() {
        let store = MemStore::new();
        let mut server = WorkServer::new();
        server.set_plan(plan_1v1(vec![(1, 2, 1000)]));
        server.set_plan(plan_melee());
        let config = Config::default();
        let mut melee = 0;
        for i in 0..1000u32 {
            let mut random = [0u8; 32];
            random[..4].copy_from_slice(&i.to_le_bytes());
            let work = server
                .serve(
                    &store,
                    &config,
                    &keys(),
                    &user(0),
                    &query(1, Division::ALL.to_vec()),
                    NOW_MS,
                    &random,
                )
                .unwrap()
                .unwrap();
            if work.battles[0].division == Division::Melee {
                melee += 1;
            }
        }
        assert!((240..360).contains(&melee), "{melee} of 1000 melee batches");
    }

    fn verify_item(sort: &str, division: Division, bots: Vec<u32>, kind: VerifyKind) -> VerifyItem {
        VerifyItem {
            engine: 1,
            sort: sort.into(),
            division,
            seed: 77,
            bots,
            rounds: 35,
            runs: vec![Run {
                uid: "g9".into(),
                hash: Some("h".into()),
                record: vec![],
                held: false,
            }],
            uids: vec!["g9".into()],
            stage: 1,
            claimant: None,
            claim_until: None,
            kind,
            ttl: NOW + VERIFY_TTL,
        }
    }

    #[test]
    fn verifiers_get_queue_work_first() {
        let store = MemStore::new();
        for (i, kind) in [
            VerifyKind::Check,
            VerifyKind::Probation,
            VerifyKind::Recheck,
            VerifyKind::Check,
        ]
        .into_iter()
        .enumerate()
        {
            store
                .put_verify(&verify_item(
                    &format!("000{i}#x"),
                    Division::OneVsOne,
                    vec![2, 1],
                    kind,
                ))
                .unwrap();
        }
        store
            .put_verify(&verify_item(
                "0000#melee",
                Division::Melee,
                (1..=10).collect(),
                VerifyKind::Check,
            ))
            .unwrap();
        // A version that isn't in the plan comes from the store.
        store
            .put_version(&VersionItem {
                vid: 9,
                bot: "x.New".into(),
                version: "2".into(),
                sha256: "ab".repeat(32),
                size: 5,
                codesize: Some(100),
                class: WeightClass::Nano,
                divisions: vec![Division::OneVsOne],
                state: VersionState::Probation,
                probation_battles: 0,
                probation_failures: 0,
                class_name: "x.New".into(),
                created: NOW,
                retired: None,
                url: None,
            })
            .unwrap();
        store
            .put_verify(&verify_item(
                "0005#p",
                Division::OneVsOne,
                vec![9, 1],
                VerifyKind::Probation,
            ))
            .unwrap();
        let mut server = WorkServer::new();
        server.set_plan(plan_1v1(vec![(1, 2, 5)]));
        let config = Config::default();
        let one_v_one = query(5, vec![Division::OneVsOne]);

        // Not a verifier: plan work only.
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(49),
                &one_v_one,
                NOW_MS,
                &[4; 32],
            )
            .unwrap()
            .unwrap();
        assert!(work.battles.iter().all(|b| b.kind == BattleKind::Normal));

        // A verifier: up to ⌈5/2⌉ = 3 queue battles first, oldest first, in their order.
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(50),
                &one_v_one,
                NOW_MS,
                &[5; 32],
            )
            .unwrap()
            .unwrap();
        let kinds: Vec<BattleKind> = work.battles.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                BattleKind::Verify,
                BattleKind::Probation,
                BattleKind::Verify,
                BattleKind::Normal,
                BattleKind::Normal
            ]
        );
        assert_eq!(work.battles[0].seed, 77);
        let vids: Vec<u32> = work.battles[0].bots.iter().map(|b| b.vid).collect();
        assert_eq!(vids, [2, 1], "the original robot order");
        let payload = token::verify(&work.batch, &keys(), NOW).unwrap();
        assert_eq!(payload.b[0].item.as_deref(), Some("0000#x"));
        assert_eq!(payload.b[1].item.as_deref(), Some("0001#x"));
        let claimed = store.get_verify(1, "0000#x").unwrap().unwrap();
        assert_eq!(claimed.claimant.as_deref(), Some("g1"));
        assert_eq!(claimed.claim_until, Some(NOW + 180 * 60));
        assert!(
            store
                .get_verify(1, "0000#melee")
                .unwrap()
                .unwrap()
                .claimant
                .is_none()
        );

        // The next batch gets the rest: the 4th check and the probation of vid 9.
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(50),
                &one_v_one,
                NOW_MS,
                &[6; 32],
            )
            .unwrap()
            .unwrap();
        assert_eq!(work.battles[0].kind, BattleKind::Verify);
        assert_eq!(work.battles[1].kind, BattleKind::Probation);
        assert_eq!(work.battles[1].bots[0].id, "x.New");
        assert_eq!(work.battles[2].kind, BattleKind::Normal);

        // Then the queue is empty for this verifier: remembered for 30 s.
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(50),
                &one_v_one,
                NOW_MS,
                &[7; 32],
            )
            .unwrap()
            .unwrap();
        assert!(work.battles.iter().all(|b| b.kind == BattleKind::Normal));
        store
            .put_verify(&verify_item(
                "0009#late",
                Division::OneVsOne,
                vec![1, 2],
                VerifyKind::Check,
            ))
            .unwrap();
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(50),
                &one_v_one,
                NOW_MS + 29_000,
                &[8; 32],
            )
            .unwrap()
            .unwrap();
        assert!(work.battles.iter().all(|b| b.kind == BattleKind::Normal));
        let work = server
            .serve(
                &store,
                &config,
                &keys(),
                &user(50),
                &one_v_one,
                NOW_MS + 30_000,
                &[9; 32],
            )
            .unwrap()
            .unwrap();
        assert_eq!(work.battles[0].kind, BattleKind::Verify);
    }

    #[test]
    fn batch_ids_sort_by_time() {
        let mut random = Random::new(&[0; 32]);
        let a = batch_id(1_000, &mut random);
        let b = batch_id(2_000, &mut random);
        assert!(a < b);
        assert_ne!(batch_id(1_000, &mut random), a);
        assert!(a[2..].bytes().all(|c| c.is_ascii_alphanumeric()));
    }
}
