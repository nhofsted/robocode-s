// SPDX-License-Identifier: EPL-2.0

//! The work planner (spec §10.3-§10.5, §12.3): the priority of one more battle of a pairing,
//! the hourly quotas by water-filling, the `plan.json.gz` file, the samplers the `api` Lambda
//! draws battles with, and the version transitions after folding. Pure: callers pass random
//! numbers (uniform in `[0, 1)`) and the time in.

use crate::config::Config;
use crate::ids::{Division, WeightClass};
use crate::state::State;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::io::{Read, Write};

/// Partners kept per bot in a melee plan (§10.3).
pub const PARTNERS_PER_BOT: usize = 40;
/// Partners of a melee anchor in a battle (the battle has 10 robots).
const BATTLE_PARTNERS: usize = 9;
/// The window of recent battles that deactivates an active version (§12.3).
const WINDOW: u32 = 20;
/// Distinct reporters needed to deactivate an active version (§12.3).
const MIN_REPORTERS: u32 = 3;
/// Weight classes, index = class (Nano .. Mega): rumble `r` has the bots of class `<= r`.
const CLASSES: usize = 4;
/// Overflow battles planned per hourly battle, for volunteers beyond `H_D` (§10.3).
pub const OVERFLOW: u32 = 3;

/// The tie-break seed of a plan: FNV-1a of its `generatedAt`, so plans are deterministic but
/// equal priorities fall to different pairs every hour.
fn seed_of(generated_at: &str) -> u64 {
    generated_at.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// splitmix64 of `seed` and `key`: the tie-break of equal priorities. Without it, ties (every
/// unfought pairing of a class) go to the lowest vids and an hour's battles pile onto a few bots.
fn tie(seed: u64, key: u64) -> u64 {
    let mut z = seed ^ key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_133e_11eb);
    z ^ (z >> 31)
}

fn class_index(class: WeightClass) -> usize {
    match class {
        WeightClass::Nano => 0,
        WeightClass::Micro => 1,
        WeightClass::Mini => 2,
        WeightClass::Mega => 3,
    }
}

/// A bot version as the planner needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanBot {
    pub vid: u32,
    pub id: String,
    pub version: String,
    pub sha256: String,
    /// Jar size in bytes.
    pub size: u64,
    /// Weight class from the codesize; a bot is in every rumble of its class and above.
    pub class: WeightClass,
}

/// What the planner needs besides the state and the configuration.
#[derive(Clone, Debug)]
pub struct PlanInput<'a> {
    /// The active versions of the division: the ones that get planned battles.
    pub bots: &'a [PlanBot],
    /// Versions on probation: listed in the plan's `bots` and `probation`, not planned.
    pub probation: &'a [PlanBot],
    /// Versions ranked in the top `slots.top` of any rumble of the division (§10.5).
    pub top: &'a [u32],
    /// 1v1: matchups `(lo, hi)` outside `top` that get recording slots (§12.5).
    pub slot_pairs: &'a [(u32, u32)],
    /// The division's counted battles in the last full hour.
    pub last_hour_battles: u64,
    pub generated_at: &'a str,
}

/// `H_D = max(plan.minHourly_D, plan.headroom · last hour's battles)` (§10.3).
pub fn hourly_target(config: &Config, division: Division, last_hour_battles: u64) -> u32 {
    let wanted = (config.plan.headroom * last_hour_battles as f64).ceil();
    let wanted = if wanted.is_finite() && wanted > 0.0 {
        wanted.min(f64::from(u32::MAX)) as u32
    } else {
        0
    };
    wanted.max(*config.plan.min_hourly.get(division))
}

/// `1 / ((n + ½)(n + 1½))`: the shape of the variance reduction of one more battle.
fn gain_shape(n: f64) -> f64 {
    1.0 / ((n + 0.5) * (n + 1.5))
}

#[derive(Clone, Copy, Default)]
struct Acc {
    pairings: u32,
    sum_mean: f64,
    sum_mean_sq: f64,
    /// Sum of `σ² / n` over the pairings.
    sum_var: f64,
}

impl Acc {
    fn add(&mut self, mean: f64, var_over_n: f64) {
        self.pairings += 1;
        self.sum_mean += mean;
        self.sum_mean_sq += mean * mean;
        self.sum_var += var_over_n;
    }

    /// The standard error of the APS: the pairings' own noise, plus the uncertainty of which
    /// opponents were met, from the spread of the pairing means (shrunk toward
    /// `opponent_prior` with weight `weight`) with the finite-population correction for
    /// `opponents` possible ones. A bot that met a few opponents has an uncertain APS, however
    /// many battles those pairings have; one that met them all has only the noise.
    fn se(&self, opponents: usize, opponent_prior: f64, weight: f64) -> f64 {
        let p = f64::from(self.pairings);
        let mean = self.sum_mean / p;
        let spread = if self.pairings >= 2 {
            let sample = ((self.sum_mean_sq - p * mean * mean) / (p - 1.0)).max(0.0);
            (weight * opponent_prior + (p - 1.0) * sample) / (weight + p - 1.0)
        } else {
            opponent_prior
        };
        let unmet = (1.0 - p / opponents.max(1) as f64).max(0.0);
        (self.sum_var / (p * p) + spread / p * unmet).sqrt()
    }
}

/// The priorities of one more battle for every pair of a set of bots (§10.3). The bots are
/// indexed in vid order; `base` holds `close · σ² · mean_r ½(contested·coverage)` of each pair
/// over the rumbles holding both (the part of `V` that doesn't depend on `n`).
pub struct Priorities {
    vids: Vec<u32>,
    classes: Vec<usize>,
    /// `contested_{X,r}`, for the rumbles `r >= class` of the bot (else 0).
    contested: Vec<[f64; CLASSES]>,
    /// `coverage_{X,r}`.
    coverage: Vec<[f64; CLASSES]>,
    /// Pairs `(i, j)`, i < j, in row order.
    base: Vec<f64>,
    n: Vec<u32>,
}

impl Priorities {
    /// Computes the priorities of every pair of `bots` (members of the division) from `state`.
    /// Pairings never fought (`n = 0`) are generated implicitly: the state only keeps `n > 0`.
    pub fn new(state: &State, config: &Config, bots: &[PlanBot]) -> Priorities {
        let division = state.division;
        let prior = *config.plan.prior_variance.get(division);
        let weight = config.plan.prior_weight;
        let opponent_prior = *config.plan.opponent_variance.get(division);
        let close_factor = config.plan.close_factor;
        let rank_factor = config.plan.rank_factor;

        let mut order: Vec<&PlanBot> = bots.iter().collect();
        order.sort_unstable_by_key(|b| b.vid);
        order.dedup_by_key(|b| b.vid);
        let count = order.len();
        let vids: Vec<u32> = order.iter().map(|b| b.vid).collect();
        let classes: Vec<usize> = order.iter().map(|b| class_index(b.class)).collect();
        let mut members = [0usize; CLASSES];
        for &c in &classes {
            for m in &mut members[c..] {
                *m += 1;
            }
        }

        let size = count * count.saturating_sub(1) / 2;
        let mut n = vec![0u32; size];
        let mut variance = vec![prior; size];
        // An unfought pairing counts as close.
        let mut close = vec![close_factor; size];
        let mut acc = vec![[Acc::default(); CLASSES]; count];
        let mut at = 0;
        for i in 0..count {
            for j in i + 1..count {
                if let Some(p) = state.pairing(vids[i], vids[j]).filter(|p| p.n > 0) {
                    let st = p.stats(prior, weight);
                    n[at] = p.n;
                    variance[at] = st.shrunk_variance;
                    close[at] = if (st.mean - 50.0).abs() <= st.ci95 {
                        close_factor
                    } else {
                        1.0
                    };
                    let var_over_n = st.shrunk_variance / f64::from(p.n);
                    // The state keeps the lower vid's side, i < j in vid order.
                    let from = classes[i].max(classes[j]);
                    for a in &mut acc[i][from..] {
                        a.add(st.mean, var_over_n);
                    }
                    for b in &mut acc[j][from..] {
                        b.add(100.0 - st.mean, var_over_n);
                    }
                }
                at += 1;
            }
        }

        let mut contested = vec![[0.0; CLASSES]; count];
        let mut coverage = vec![[0.0; CLASSES]; count];
        for r in 0..CLASSES {
            // APS and its standard error of the members with at least one pairing.
            let mut ranked: Vec<(f64, f64, usize)> = (0..count)
                .filter(|&x| classes[x] <= r && acc[x][r].pairings > 0)
                .map(|x| {
                    let a = &acc[x][r];
                    let se = a.se(members[r] - 1, opponent_prior, weight);
                    (a.sum_mean / f64::from(a.pairings), se, x)
                })
                .collect();
            ranked.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
            for x in (0..count).filter(|&x| classes[x] <= r) {
                contested[x][r] = 1.0 + rank_factor;
                let missing = members[r] - 1 - acc[x][r].pairings as usize;
                coverage[x][r] = 1.0 + missing as f64 / members[r] as f64;
            }
            for (k, &(aps, se, x)) in ranked.iter().enumerate() {
                if acc[x][r].pairings < 2 {
                    continue;
                }
                let below = k.checked_sub(1).map(|p| ranked[p]);
                let above = ranked.get(k + 1).copied();
                let nearer = match (below, above) {
                    (Some(b), Some(a)) => Some(if aps - b.0 <= a.0 - aps { b } else { a }),
                    (b, a) => b.or(a),
                };
                contested[x][r] = match nearer {
                    Some((other_aps, other_se, _)) => {
                        let sd = (se * se + other_se * other_se).sqrt();
                        let z = (aps - other_aps).abs() / sd;
                        1.0 + rank_factor * (-z * z / 2.0).exp()
                    }
                    None => 1.0,
                };
            }
        }

        let mut priorities = Priorities {
            vids,
            classes,
            contested,
            coverage,
            base: Vec::new(),
            n,
        };
        let suffix: Vec<[f64; CLASSES]> = (0..count)
            .map(|x| {
                let mut s = [0.0; CLASSES];
                let mut total = 0.0;
                for r in (priorities.classes[x]..CLASSES).rev() {
                    total += priorities.contested[x][r] * priorities.coverage[x][r];
                    s[r] = total;
                }
                s
            })
            .collect();
        let mut base = Vec::with_capacity(size);
        let mut at = 0;
        for i in 0..count {
            for j in i + 1..count {
                let k = priorities.classes[i].max(priorities.classes[j]);
                // The mean over the pair's rumbles, not the sum: a sum made a nano pairing
                // worth 4 mega ones, and unfought mega bots waited for every nano pairing.
                let rumbles = (CLASSES - k) as f64;
                let both = 0.5 * (suffix[i][k] + suffix[j][k]) / rumbles;
                base.push(close[at] * variance[at] * both);
                at += 1;
            }
        }
        priorities.base = base;
        priorities
    }

    pub fn len(&self) -> usize {
        self.vids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vids.is_empty()
    }

    /// The vid of bot `i` (bots are in vid order).
    pub fn vid(&self, i: usize) -> u32 {
        self.vids[i]
    }

    /// The index of a vid.
    pub fn index_of(&self, vid: u32) -> Option<usize> {
        self.vids.binary_search(&vid).ok()
    }

    /// `contested_{X,r}` of bot `i` in the rumble of `class` (0 if the bot isn't a member).
    pub fn contested(&self, i: usize, class: WeightClass) -> f64 {
        self.contested[i][class_index(class)]
    }

    /// `coverage_{X,r}` of bot `i` in the rumble of `class` (0 if the bot isn't a member).
    pub fn coverage(&self, i: usize, class: WeightClass) -> f64 {
        self.coverage[i][class_index(class)]
    }

    fn tri(&self, i: usize, j: usize) -> usize {
        debug_assert!(i < j && j < self.vids.len());
        i * (2 * self.vids.len() - i - 1) / 2 + (j - i - 1)
    }

    fn slot(&self, a: usize, b: usize) -> usize {
        self.tri(a.min(b), a.max(b))
    }

    /// Battles the pairing has in the state.
    pub fn battles(&self, a: usize, b: usize) -> u32 {
        self.n[self.slot(a, b)]
    }

    /// `V(A, B)` for a pairing that had `n` battles (`n` may be fractional in melee).
    pub fn priority(&self, a: usize, b: usize, n: f64) -> f64 {
        self.base[self.slot(a, b)] * gain_shape(n)
    }

    /// `V(A, B)` at the pairing's battles in the state.
    pub fn current(&self, a: usize, b: usize) -> f64 {
        let at = self.slot(a, b);
        self.base[at] * gain_shape(f64::from(self.n[at]))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Item {
    value: f64,
    tie: u64,
    index: u32,
}

impl Eq for Item {}

impl Ord for Item {
    fn cmp(&self, other: &Item) -> Ordering {
        // Max-heap on the value; ties go by the seeded tie-break, then the lower index.
        self.value
            .total_cmp(&other.value)
            .then(self.tie.cmp(&other.tie))
            .then(other.index.cmp(&self.index))
    }
}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Item) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Water-filling over all pairs (1v1): `h` times give the best pair one battle, then
/// `extra` more times for the overflow. Returns the battles per pair in `Priorities` order:
/// the first `h` and the overflow.
fn pair_quotas(pr: &Priorities, h: u32, extra: u32, seed: u64) -> (Vec<u32>, Vec<u32>) {
    let mut quotas = vec![0u32; pr.base.len()];
    let mut overflow = vec![0u32; pr.base.len()];
    let items: Vec<Item> = pr
        .base
        .iter()
        .zip(&pr.n)
        .enumerate()
        .map(|(index, (&base, &n))| Item {
            value: base * gain_shape(f64::from(n)),
            tie: tie(seed, index as u64),
            index: index as u32,
        })
        .collect();
    let mut heap = BinaryHeap::from(items);
    for pop in 0..u64::from(h) + u64::from(extra) {
        let Some(mut top) = heap.peek_mut() else {
            break;
        };
        let at = top.index as usize;
        if pop < u64::from(h) {
            quotas[at] += 1;
        } else {
            overflow[at] += 1;
        }
        let n = pr.n[at] + quotas[at] + overflow[at];
        top.value = pr.base[at] * gain_shape(f64::from(n));
    }
    (quotas, overflow)
}

/// Melee water-filling over bots with `V_X = Σ_O V(X, O)`: each pop gives X an anchor battle
/// and adds 1/9 battle to its 9 best partners; `h` pops, then `extra` more for the overflow.
/// Returns the anchor quotas, the overflow anchor quotas and each bot's best
/// `PARTNERS_PER_BOT` partners `(index, V)` at the state's battle counts.
#[allow(clippy::type_complexity)]
fn melee_quotas(
    pr: &Priorities,
    h: u32,
    extra: u32,
    seed: u64,
) -> (Vec<u32>, Vec<u32>, Vec<Vec<(usize, f64)>>) {
    let count = pr.len();
    let mut quotas = vec![0u32; count];
    let mut overflow = vec![0u32; count];
    let mut partners = vec![Vec::new(); count];
    if count < 2 {
        return (quotas, overflow, partners);
    }
    // Ties between partners of a bot (every unfought one) go by the pair's seeded key.
    let pair_tie = |x: usize, o: usize| tie(seed, pr.slot(x, o) as u64);
    // Each bot's own battle counts against every other bot.
    let mut own = vec![0.0f64; count * count];
    let mut total = vec![0.0f64; count];
    for x in 0..count {
        let mut list = Vec::with_capacity(count - 1);
        for o in (0..count).filter(|&o| o != x) {
            let n = f64::from(pr.battles(x, o));
            own[x * count + o] = n;
            let v = pr.priority(x, o, n);
            total[x] += v;
            list.push((o, v));
        }
        list.sort_unstable_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then(pair_tie(x, b.0).cmp(&pair_tie(x, a.0)))
                .then(a.0.cmp(&b.0))
        });
        list.truncate(PARTNERS_PER_BOT);
        partners[x] = list;
    }
    let mut heap: BinaryHeap<Item> = (0..count)
        .map(|x| Item {
            value: total[x],
            tie: tie(seed, x as u64),
            index: x as u32,
        })
        .collect();
    let mut scratch: Vec<(f64, u64, usize)> = Vec::with_capacity(count);
    for pop in 0..u64::from(h) + u64::from(extra) {
        let Some(mut top) = heap.peek_mut() else {
            break;
        };
        let x = top.index as usize;
        if pop < u64::from(h) {
            quotas[x] += 1;
        } else {
            overflow[x] += 1;
        }
        scratch.clear();
        scratch.extend(
            (0..count)
                .filter(|&o| o != x)
                .map(|o| (pr.priority(x, o, own[x * count + o]), pair_tie(x, o), o)),
        );
        let take = BATTLE_PARTNERS.min(scratch.len());
        let better = |a: &(f64, u64, usize), b: &(f64, u64, usize)| {
            b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2))
        };
        if take < scratch.len() {
            scratch.select_nth_unstable_by(take - 1, better);
        }
        for &(_, _, o) in &scratch[..take] {
            own[x * count + o] += 1.0 / BATTLE_PARTNERS as f64;
        }
        top.value = (0..count)
            .filter(|&o| o != x)
            .map(|o| pr.priority(x, o, own[x * count + o]))
            .sum();
    }
    (quotas, overflow, partners)
}

/// The row of a bot in `plan.json`: `[vid, id, version, sha256, size]`.
pub type PlanBotRow = (u32, String, String, String, u64);

/// A division's plan for the next hour: the `plan.json.gz` file of §10.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub engine: u32,
    pub division: Division,
    pub generated_at: String,
    /// `H_D`, the battles planned.
    pub hourly: u32,
    /// The planned and probation versions, by vid.
    pub bots: Vec<PlanBotRow>,
    /// 1v1: `[vidA, vidB, quota]` with A < B, quota > 0, sorted.
    pub pairs: Vec<(u32, u32, u32)>,
    /// Melee: `[vid, quota]`, quota > 0, sorted.
    pub anchors: Vec<(u32, u32)>,
    /// 1v1: the overflow `[vidA, vidB, quota]` (`OVERFLOW · hourly` battles past `pairs`, for
    /// volunteers beyond the target), like `pairs`. Absent in older plans.
    #[serde(default)]
    pub overflow_pairs: Vec<(u32, u32, u32)>,
    /// Melee: the overflow `[vid, quota]`, like `anchors`. Absent in older plans.
    #[serde(default)]
    pub overflow_anchors: Vec<(u32, u32)>,
    /// Melee: a bot's best partners `[vid, V]`, best first.
    pub partners: BTreeMap<u32, Vec<(u32, f64)>>,
    pub top: Vec<u32>,
    /// 1v1: `[vidLo, vidHi]` matchups with recording slots besides those with a `top` bot,
    /// sorted (§12.5). Absent in older plans.
    #[serde(default)]
    pub slot_pairs: Vec<(u32, u32)>,
    pub probation: Vec<u32>,
}

/// Plans the next hour of `state.division` (§10.3).
pub fn plan(state: &State, config: &Config, input: &PlanInput) -> Plan {
    let division = state.division;
    let hourly = hourly_target(config, division, input.last_hour_battles);
    let pr = Priorities::new(state, config, input.bots);
    let seed = seed_of(input.generated_at);
    let extra = hourly.saturating_mul(OVERFLOW);
    let mut pairs = Vec::new();
    let mut anchors = Vec::new();
    let mut overflow_pairs = Vec::new();
    let mut overflow_anchors = Vec::new();
    let mut partners = BTreeMap::new();
    match division {
        Division::OneVsOne => {
            let (quotas, overflow) = pair_quotas(&pr, hourly, extra, seed);
            let mut at = 0;
            for i in 0..pr.len() {
                for j in i + 1..pr.len() {
                    if quotas[at] > 0 {
                        pairs.push((pr.vid(i), pr.vid(j), quotas[at]));
                    }
                    if overflow[at] > 0 {
                        overflow_pairs.push((pr.vid(i), pr.vid(j), overflow[at]));
                    }
                    at += 1;
                }
            }
        }
        Division::Melee => {
            let (quotas, overflow, lists) = melee_quotas(&pr, hourly, extra, seed);
            for (x, &quota) in quotas.iter().enumerate() {
                if quota > 0 {
                    anchors.push((pr.vid(x), quota));
                }
                if overflow[x] > 0 {
                    overflow_anchors.push((pr.vid(x), overflow[x]));
                }
            }
            for (x, list) in lists.iter().enumerate() {
                if !list.is_empty() {
                    partners.insert(
                        pr.vid(x),
                        list.iter().map(|&(o, v)| (pr.vid(o), v)).collect(),
                    );
                }
            }
        }
    }
    let mut rows: Vec<&PlanBot> = input.bots.iter().chain(input.probation).collect();
    rows.sort_unstable_by_key(|b| b.vid);
    rows.dedup_by_key(|b| b.vid);
    let mut probation: Vec<u32> = input.probation.iter().map(|b| b.vid).collect();
    probation.sort_unstable();
    probation.dedup();
    let mut top = input.top.to_vec();
    top.sort_unstable();
    top.dedup();
    let mut slot_pairs: Vec<(u32, u32)> = input
        .slot_pairs
        .iter()
        .map(|&(a, b)| (a.min(b), a.max(b)))
        .filter(|(a, b)| a != b)
        .collect();
    slot_pairs.sort_unstable();
    slot_pairs.dedup();
    Plan {
        engine: state.engine,
        division,
        generated_at: input.generated_at.to_string(),
        hourly,
        bots: rows
            .iter()
            .map(|b| {
                (
                    b.vid,
                    b.id.clone(),
                    b.version.clone(),
                    b.sha256.clone(),
                    b.size,
                )
            })
            .collect(),
        pairs,
        anchors,
        overflow_pairs,
        overflow_anchors,
        partners,
        top,
        slot_pairs,
        probation,
    }
}

impl Plan {
    /// `plan.json.gz`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let json = serde_json::to_vec(self).expect("a plan serializes");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&json).expect("writing to a Vec");
        encoder.finish().expect("writing to a Vec")
    }

    /// Parses `plan.json.gz`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Plan, String> {
        let mut json = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut json)
            .map_err(|e| format!("plan.json.gz: bad gzip: {e}"))?;
        serde_json::from_slice(&json).map_err(|e| format!("plan.json.gz: {e}"))
    }

    /// The sampler of 1v1 battles (§10.4 step 3).
    pub fn pair_sampler(&self) -> QuotaSampler<(u32, u32)> {
        QuotaSampler::with_overflow(
            self.pairs.iter().map(|&(a, b, q)| ((a, b), q)).collect(),
            self.overflow_pairs
                .iter()
                .map(|&(a, b, q)| ((a, b), q))
                .collect(),
        )
    }

    /// The assembler of melee battles (§10.4 step 3).
    pub fn melee_sampler(&self) -> MeleeSampler {
        let planned: Vec<u32> = self
            .bots
            .iter()
            .map(|b| b.0)
            .filter(|vid| self.probation.binary_search(vid).is_err())
            .collect();
        MeleeSampler {
            anchors: QuotaSampler::with_overflow(
                self.anchors.clone(),
                self.overflow_anchors.clone(),
            ),
            partners: self.partners.clone(),
            planned,
        }
    }
}

/// Walker's alias method as built by Vose: O(n) to build, O(1) per draw.
#[derive(Clone, Debug)]
pub struct Alias {
    probability: Vec<f64>,
    alias: Vec<u32>,
}

impl Alias {
    /// `None` if there are no weights or they sum to 0. Weights must not be negative.
    pub fn new(weights: &[f64]) -> Option<Alias> {
        let n = weights.len();
        let total: f64 = weights.iter().sum();
        if n == 0 || total <= 0.0 || !total.is_finite() {
            return None;
        }
        let mut scaled: Vec<f64> = weights.iter().map(|w| w * n as f64 / total).collect();
        let mut probability = vec![1.0; n];
        let mut alias: Vec<u32> = (0..n as u32).collect();
        let (mut small, mut large): (Vec<usize>, Vec<usize>) =
            (0..n).partition(|&i| scaled[i] < 1.0);
        while let (Some(&s), Some(&l)) = (small.last(), large.last()) {
            small.pop();
            probability[s] = scaled[s];
            alias[s] = l as u32;
            scaled[l] -= 1.0 - scaled[s];
            if scaled[l] < 1.0 {
                large.pop();
                small.push(l);
            }
        }
        // What is left has probability 1 (up to rounding).
        Some(Alias { probability, alias })
    }

    pub fn len(&self) -> usize {
        self.probability.len()
    }

    pub fn is_empty(&self) -> bool {
        self.probability.is_empty()
    }

    /// An index with probability proportional to its weight, from two uniform numbers in
    /// `[0, 1)`: `u1` picks the column, `u2` decides between it and its alias.
    pub fn sample(&self, u1: f64, u2: f64) -> usize {
        let n = self.probability.len();
        let column = ((u1 * n as f64) as usize).min(n - 1);
        if u2 < self.probability[column] {
            column
        } else {
            self.alias[column] as usize
        }
    }
}

/// Draws items with probability proportional to their remaining quota; once every quota is
/// used up, from the overflow quotas the same way, and once those are used up too,
/// proportional to all the quotas, so extra volunteers still get the most useful work
/// (§10.4 step 3). The instance's copy of the remaining quotas is decremented.
#[derive(Clone, Debug)]
pub struct QuotaSampler<T> {
    items: Vec<T>,
    remaining: Vec<u32>,
    remaining_total: u64,
    /// The overflow quotas, taken into `remaining` once it is used up.
    overflow: Option<Vec<u32>>,
    original: Option<Alias>,
    /// The alias table of `remaining`, rebuilt after draws.
    current: Option<Alias>,
    stale: bool,
}

impl<T: Copy + Ord> QuotaSampler<T> {
    pub fn new(items: Vec<T>, quotas: Vec<u32>) -> QuotaSampler<T> {
        assert_eq!(items.len(), quotas.len());
        let weights: Vec<f64> = quotas.iter().map(|&q| f64::from(q)).collect();
        let original = Alias::new(&weights);
        QuotaSampler {
            items,
            remaining_total: quotas.iter().map(|&q| u64::from(q)).sum(),
            remaining: quotas,
            overflow: None,
            current: original.clone(),
            original,
            stale: false,
        }
    }

    /// A sampler of `quotas`, then of `overflow` (an item may be in both).
    pub fn with_overflow(quotas: Vec<(T, u32)>, overflow: Vec<(T, u32)>) -> QuotaSampler<T> {
        let mut merged: BTreeMap<T, (u32, u32)> = BTreeMap::new();
        for (t, q) in quotas {
            merged.entry(t).or_default().0 += q;
        }
        for (t, q) in overflow {
            merged.entry(t).or_default().1 += q;
        }
        let items: Vec<T> = merged.keys().copied().collect();
        let first: Vec<u32> = merged.values().map(|q| q.0).collect();
        let second: Vec<u32> = merged.values().map(|q| q.1).collect();
        let all: Vec<f64> = merged
            .values()
            .map(|q| f64::from(q.0) + f64::from(q.1))
            .collect();
        let mut sampler = QuotaSampler::new(items, first);
        sampler.original = Alias::new(&all);
        if second.iter().any(|&q| q > 0) {
            sampler.overflow = Some(second);
        }
        sampler
    }

    /// Battles left in the quotas, overflow included.
    pub fn remaining(&self) -> u64 {
        let overflow: u64 = self.overflow.iter().flatten().map(|&q| u64::from(q)).sum();
        self.remaining_total + overflow
    }

    /// Draws an item (`None` if the plan has none), taking one battle off its quota if any is
    /// left.
    pub fn draw(&mut self, u1: f64, u2: f64) -> Option<T> {
        if self.remaining_total == 0
            && let Some(overflow) = self.overflow.take()
        {
            self.remaining_total = overflow.iter().map(|&q| u64::from(q)).sum();
            self.remaining = overflow;
            self.stale = true;
        }
        if self.remaining_total == 0 {
            let at = self.original.as_ref()?.sample(u1, u2);
            return Some(self.items[at]);
        }
        if self.stale {
            let weights: Vec<f64> = self.remaining.iter().map(|&q| f64::from(q)).collect();
            self.current = Alias::new(&weights);
            self.stale = false;
        }
        let mut at = self.current.as_ref()?.sample(u1, u2);
        if self.remaining[at] == 0 {
            // Rounding at a table edge: take the nearest item that has quota left.
            at = (0..self.remaining.len()).find(|&i| self.remaining[i] > 0)?;
        }
        self.remaining[at] -= 1;
        self.remaining_total -= 1;
        self.stale = true;
        Some(self.items[at])
    }
}

/// Assembles melee battles from a plan (§10.4 step 3).
#[derive(Clone, Debug)]
pub struct MeleeSampler {
    anchors: QuotaSampler<u32>,
    partners: BTreeMap<u32, Vec<(u32, f64)>>,
    /// Every planned bot (not on probation), for battles whose partner lists run dry.
    planned: Vec<u32>,
}

impl MeleeSampler {
    /// One battle of `size` distinct bots (fewer if the plan has fewer): an anchor with
    /// probability proportional to its remaining anchor quota, then partners one at a time,
    /// each with probability proportional to `Σ V(chosen, O)` over the partner lists of the bots
    /// already chosen, excluding those. `rand` yields uniform numbers in `[0, 1)`. The order is
    /// the order of choosing; the caller shuffles.
    pub fn battle(&mut self, size: usize, rand: &mut dyn FnMut() -> f64) -> Vec<u32> {
        let (u1, u2) = (rand(), rand());
        let Some(anchor) = self.anchors.draw(u1, u2) else {
            return Vec::new();
        };
        let mut chosen = vec![anchor];
        let mut weights: BTreeMap<u32, f64> = BTreeMap::new();
        self.add_partners(anchor, &chosen, &mut weights);
        while chosen.len() < size {
            let total: f64 = weights.values().sum();
            let next = if total > 0.0 {
                let mut target = rand() * total;
                let mut pick = None;
                for (&vid, &w) in &weights {
                    pick = Some(vid);
                    if target < w {
                        break;
                    }
                    target -= w;
                }
                pick
            } else {
                let left: Vec<u32> = self
                    .planned
                    .iter()
                    .copied()
                    .filter(|v| !chosen.contains(v))
                    .collect();
                (!left.is_empty())
                    .then(|| left[((rand() * left.len() as f64) as usize).min(left.len() - 1)])
            };
            let Some(next) = next else {
                break;
            };
            chosen.push(next);
            weights.remove(&next);
            self.add_partners(next, &chosen, &mut weights);
        }
        chosen
    }

    fn add_partners(&self, vid: u32, chosen: &[u32], weights: &mut BTreeMap<u32, f64>) {
        for &(other, v) in self.partners.get(&vid).into_iter().flatten() {
            if v > 0.0 && !chosen.contains(&other) {
                *weights.entry(other).or_insert(0.0) += v;
            }
        }
    }

    /// Anchor battles left in the quotas.
    pub fn remaining(&self) -> u64 {
        self.anchors.remaining()
    }
}

/// The state of a bot version as the transitions need it (§12.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionStatus {
    Probation,
    Active,
    Failed,
}

impl VersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStatus::Probation => "probation",
            VersionStatus::Active => "active",
            VersionStatus::Failed => "failed",
        }
    }
}

/// A change of a version's status for the caller to write to `REG`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub vid: u32,
    pub from: VersionStatus,
    pub to: VersionStatus,
    pub reason: String,
}

/// The transitions after folding (§12.3), for the versions given with their status (statuses
/// other than probation and active never change here):
/// - probation to active or failed once the version has `probation.battles` battles, failed if
///   its failures are at least `probation.failShare` of them (the caller retires the bot's
///   previous active version);
/// - active to failed if at least `failShare` of its last 20 battles failed and at least 3
///   distinct users reported failures. Fewer than 20 battles can't reach the share unless
///   `failShare` is low enough.
///
/// Only trusted reports are in the tallies (`State::fold`). Order follows `versions`.
pub fn transitions(
    state: &State,
    config: &Config,
    versions: &[(u32, VersionStatus)],
) -> Vec<Transition> {
    let share = config.probation.fail_share;
    let mut out = Vec::new();
    for &(vid, status) in versions {
        let Some(tally) = state.versions.get(&vid) else {
            continue;
        };
        match status {
            VersionStatus::Probation if tally.battles >= config.probation.battles => {
                let rate = f64::from(tally.failed) / f64::from(tally.battles);
                let (to, reason) = if rate >= share {
                    (
                        VersionStatus::Failed,
                        format!(
                            "probation: {} of {} battles failed",
                            tally.failed, tally.battles
                        ),
                    )
                } else {
                    (
                        VersionStatus::Active,
                        format!(
                            "probation passed: {} of {} battles failed",
                            tally.failed, tally.battles
                        ),
                    )
                };
                out.push(Transition {
                    vid,
                    from: status,
                    to,
                    reason,
                });
            }
            VersionStatus::Active => {
                let failures = tally.recent_failures();
                let needed = (share * f64::from(WINDOW) - 1e-9).ceil().max(1.0) as u32;
                if failures >= needed && tally.distinct_reporters() >= MIN_REPORTERS {
                    out.push(Transition {
                        vid,
                        from: status,
                        to: VersionStatus::Failed,
                        reason: format!(
                            "{failures} of the last {WINDOW} battles failed, reported by {}+ users",
                            tally.distinct_reporters()
                        ),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Entry, FailReason, Record};
    use std::collections::{HashMap, HashSet};

    fn bot(vid: u32, class: WeightClass) -> PlanBot {
        PlanBot {
            vid,
            id: format!("test.Bot{vid}"),
            version: "1.0".into(),
            sha256: format!("{vid:064x}"),
            size: 1000 + u64::from(vid),
            class,
        }
    }

    fn bots(n: u32, class: WeightClass) -> Vec<PlanBot> {
        (1..=n).map(|v| bot(v, class)).collect()
    }

    fn entry(vid: u32, score: f32, firsts: u16) -> Entry {
        Entry { vid, score, firsts }
    }

    fn record(division: Division, entries: Vec<Entry>) -> Record {
        Record {
            division,
            weight: 1,
            uid: "g1".into(),
            contestants: entries,
            failed: None,
            trusted: true,
        }
    }

    /// Folds `times` battles of `a` against `b` where `a` gets `score` of 100 points; the score
    /// alternates by ±1 around it so the variance is the same everywhere.
    fn fight(state: &mut State, a: u32, b: u32, score: f32, times: u32) {
        for t in 0..times {
            let s = if t % 2 == 0 { score - 1.0 } else { score + 1.0 };
            let r = record(
                Division::OneVsOne,
                vec![entry(a, s, 5), entry(b, 100.0 - s, 5)],
            );
            state.fold(&r, 1);
        }
    }

    fn state_1v1() -> State {
        State::new(1, Division::OneVsOne, 0)
    }

    /// splitmix64, uniform in [0, 1).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_133e_11eb);
            z ^= z >> 31;
            (z >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    #[test]
    fn hand_computed_priority() {
        // Two unfought mega bots: gain 1.333·16, close 10, contested 3, coverage 1.5 each.
        let state = state_1v1();
        let config = Config::default();
        let pr = Priorities::new(&state, &config, &bots(2, WeightClass::Mega));
        let expected = 16.0 / 0.75 * 10.0 * 0.5 * (4.5 + 4.5);
        assert!((pr.current(0, 1) - expected).abs() < 1e-9);
        assert!((pr.priority(0, 1, 1.0) - 16.0 / (1.5 * 2.5) * 10.0 * 4.5).abs() < 1e-9);
        assert_eq!(pr.contested(0, WeightClass::Mega), 3.0);
        assert_eq!(pr.coverage(0, WeightClass::Mega), 1.5);
        assert_eq!(pr.contested(0, WeightClass::Nano), 0.0);

        // A nano and a mega bot: only the mega rumble holds both.
        let mixed = [bot(1, WeightClass::Nano), bot(2, WeightClass::Mega)];
        let pr = Priorities::new(&state, &config, &mixed);
        // Nano bot: 4 rumbles; r = Nano has 1 member (missing 0): contested 3, coverage 1.
        // The pair counts r = Mega only: bot 1 (3 · 1.5) and bot 2 (3 · 1.5).
        assert!((pr.current(0, 1) - expected).abs() < 1e-9);
    }

    #[test]
    fn unfought_pairings_come_first() {
        let mut state = state_1v1();
        for a in 1..=4 {
            for b in a + 1..=4 {
                if (a, b) != (1, 4) {
                    fight(&mut state, a, b, 50.0, 6);
                }
            }
        }
        let pr = Priorities::new(&state, &Config::default(), &bots(4, WeightClass::Mega));
        let unfought = pr.current(0, 3);
        for (a, b) in [(0, 1), (0, 2), (1, 2), (1, 3), (2, 3)] {
            assert!(unfought > 5.0 * pr.current(a, b), "{a}-{b}");
        }
        // Planned alone, the first battle goes to the unfought pairing.
        let config = Config::default();
        let p = plan(
            &state,
            &config,
            &PlanInput {
                bots: &bots(4, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "t",
            },
        );
        let quota = |a, b| {
            p.pairs
                .iter()
                .find(|x| (x.0, x.1) == (a, b))
                .map_or(0, |x| x.2)
        };
        assert!(quota(1, 4) > quota(1, 2));
    }

    #[test]
    fn close_pairings_beat_lopsided_ones() {
        let mut state = state_1v1();
        // All pairings n = 10 with the same variance; only (1,2) is close to 50.
        for a in 1..=4 {
            for b in a + 1..=4 {
                let score = if (a, b) == (1, 2) { 50.0 } else { 90.0 };
                fight(&mut state, a, b, score, 10);
            }
        }
        let pr = Priorities::new(&state, &Config::default(), &bots(4, WeightClass::Mega));
        let close = pr.current(0, 1);
        for (a, b) in [(0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            assert!(close > pr.current(a, b), "{a}-{b}");
        }
        // The gain falls with n: the same pair with more battles is worth less.
        assert!(pr.priority(0, 1, 11.0) < pr.priority(0, 1, 10.0));
    }

    #[test]
    fn contested_bots_are_preferred() {
        let mut state = state_1v1();
        // Bots 1-4 are lopsided against 5 and 6; 1 and 2 have almost the same APS, 3 and 4
        // are far from everyone.
        let skill = [
            (1, 70.0),
            (2, 70.4),
            (3, 40.0),
            (4, 10.0),
            (5, 55.0),
            (6, 56.0),
        ];
        for &(a, sa) in &skill {
            for &(b, sb) in &skill {
                if a < b {
                    let score = (50.0f32 + (sa - sb) as f32).clamp(1.0, 99.0);
                    fight(&mut state, a, b, score, 20);
                }
            }
        }
        let pr = Priorities::new(&state, &Config::default(), &bots(6, WeightClass::Mega));
        let mega = WeightClass::Mega;
        assert!(pr.contested(0, mega) > pr.contested(2, mega));
        assert!(pr.contested(1, mega) > pr.contested(3, mega));
        assert!(pr.contested(3, mega) < 1.01);
        // Fully covered: coverage is 1 everywhere.
        assert!((0..6).all(|i| pr.coverage(i, mega) == 1.0));
        // A bot with a single pairing gets the full rank factor.
        let mut thin = state_1v1();
        fight(&mut thin, 1, 2, 60.0, 5);
        let pr = Priorities::new(&thin, &Config::default(), &bots(3, mega));
        assert_eq!(pr.contested(0, mega), 3.0);
        assert!((pr.coverage(0, mega) - (1.0 + 1.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn an_aps_from_few_opponents_stays_contested() {
        // Bots 2-30 met each other; bot 1 beat two weak bots only. Its APS of 70 is above
        // everyone's, but says little: it met 2 of 29 opponents.
        let mut state = state_1v1();
        for a in 2..=30u32 {
            for b in a + 1..=30 {
                fight(&mut state, a, b, (50.0 + (b - a) as f32).min(99.0), 10);
            }
        }
        fight(&mut state, 1, 29, 70.0, 10);
        fight(&mut state, 1, 30, 70.0, 10);
        let mega = WeightClass::Mega;
        let pr = Priorities::new(&state, &Config::default(), &bots(30, mega));
        assert!(pr.contested(0, mega) > 2.0, "{}", pr.contested(0, mega));
        // Once it met them all, only the battles' noise is left: far from everyone, not contested.
        for b in 2..=28 {
            fight(&mut state, 1, b, 70.0, 10);
        }
        let pr = Priorities::new(&state, &Config::default(), &bots(30, mega));
        assert!(pr.contested(0, mega) < 1.01, "{}", pr.contested(0, mega));
    }

    #[test]
    fn hourly_targets() {
        let config = Config::default();
        assert_eq!(hourly_target(&config, Division::OneVsOne, 0), 200);
        assert_eq!(hourly_target(&config, Division::OneVsOne, 100), 200);
        assert_eq!(hourly_target(&config, Division::OneVsOne, 1000), 1500);
        assert_eq!(hourly_target(&config, Division::Melee, 3), 20);
        assert_eq!(hourly_target(&config, Division::Melee, 100), 150);
    }

    fn mixed_state(count: u32) -> State {
        let mut state = state_1v1();
        for a in 1..=count {
            for b in a + 1..=count {
                if (a * 7 + b * 3) % 5 != 0 {
                    let score = 30.0 + ((a * 31 + b * 17) % 40) as f32;
                    fight(&mut state, a, b, score, 1 + (a + b) % 4);
                }
            }
        }
        state
    }

    #[test]
    fn quotas_sum_to_the_hourly_target() {
        let config = Config::default();
        let state = mixed_state(30);
        let all: Vec<PlanBot> = (1..=30)
            .map(|v| {
                bot(
                    v,
                    [WeightClass::Nano, WeightClass::Mini, WeightClass::Mega][v as usize % 3],
                )
            })
            .collect();
        for last in [0u64, 500, 3333] {
            let p = plan(
                &state,
                &config,
                &PlanInput {
                    bots: &all,
                    probation: &[bot(99, WeightClass::Mega)],
                    top: &[3, 1, 3],
                    slot_pairs: &[],
                    last_hour_battles: last,
                    generated_at: "2026-10-24T10:05:00Z",
                },
            );
            let total: u32 = p.pairs.iter().map(|x| x.2).sum();
            assert_eq!(total, p.hourly);
            assert_eq!(p.hourly, hourly_target(&config, Division::OneVsOne, last));
            assert!(p.pairs.iter().all(|x| x.0 < x.1 && x.2 > 0));
            assert!(
                p.pairs
                    .windows(2)
                    .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1))
            );
            assert!(p.anchors.is_empty() && p.partners.is_empty());
            assert_eq!(p.top, [1, 3]);
            assert_eq!(p.probation, [99]);
            assert_eq!(p.bots.len(), 31);
            assert!(p.pairs.iter().all(|x| x.0 != 99 && x.1 != 99));
        }
    }

    #[test]
    fn plan_file_round_trip() {
        let config = Config::default();
        let state = mixed_state(12);
        let p = plan(
            &state,
            &config,
            &PlanInput {
                bots: &bots(12, WeightClass::Mega),
                probation: &[],
                top: &[2],
                slot_pairs: &[(5, 2), (3, 4), (2, 5)],
                last_hour_battles: 0,
                generated_at: "2026-10-24T10:05:00Z",
            },
        );
        let bytes = p.to_bytes();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        assert_eq!(Plan::from_bytes(&bytes).unwrap(), p);
        assert!(Plan::from_bytes(b"nope").is_err());

        let json: serde_json::Value = serde_json::to_value(&p).unwrap();
        assert_eq!(json["engine"], 1);
        assert_eq!(json["division"], "1v1");
        assert_eq!(json["generatedAt"], "2026-10-24T10:05:00Z");
        assert_eq!(json["hourly"], 200);
        assert_eq!(
            json["bots"][0],
            serde_json::json!([1, "test.Bot1", "1.0", format!("{:064x}", 1), 1001])
        );
        assert_eq!(json["pairs"][0].as_array().unwrap().len(), 3);
        assert_eq!(json["top"], serde_json::json!([2]));
        assert_eq!(json["slotPairs"], serde_json::json!([[2, 5], [3, 4]]));
        let old = br#"{"engine":1,"division":"1v1","generatedAt":"","hourly":0,"bots":[],"pairs":[],"anchors":[],"partners":{},"top":[],"probation":[]}"#;
        let older: Plan = serde_json::from_slice(old).unwrap();
        assert!(older.slot_pairs.is_empty());
        assert!(json["partners"].as_object().unwrap().is_empty());
    }

    #[test]
    fn alias_frequencies_match_the_weights() {
        let weights = [5.0, 3.0, 0.0, 1.0, 1.0, 10.0, 0.5];
        let total: f64 = weights.iter().sum();
        let alias = Alias::new(&weights).unwrap();
        let mut rng = Rng(42);
        let mut counts = [0u64; 7];
        let draws = 1_000_000;
        for _ in 0..draws {
            counts[alias.sample(rng.next(), rng.next())] += 1;
        }
        assert_eq!(counts[2], 0);
        for (i, &w) in weights.iter().enumerate() {
            let p = w / total;
            let freq = counts[i] as f64 / draws as f64;
            assert!((freq - p).abs() < 0.002, "{i}: {freq} vs {p}");
            if p >= 0.1 {
                assert!((freq / p - 1.0).abs() < 0.02, "{i}: {freq} vs {p}");
            }
        }
        assert!(Alias::new(&[]).is_none());
        assert!(Alias::new(&[0.0, 0.0]).is_none());
        // Edge numbers stay in range.
        assert!(alias.sample(0.999_999_999_999, 0.999_999_999_999) < 7);
        assert_eq!(Alias::new(&[2.0]).unwrap().sample(0.5, 0.99), 0);
    }

    #[test]
    fn quota_sampler_uses_remaining_then_original() {
        let mut s = QuotaSampler::new(vec![10u32, 20, 30], vec![3, 0, 1]);
        let mut rng = Rng(7);
        let mut counts = HashMap::new();
        for _ in 0..4 {
            *counts
                .entry(s.draw(rng.next(), rng.next()).unwrap())
                .or_insert(0) += 1;
        }
        assert_eq!(counts[&10], 3);
        assert_eq!(counts[&30], 1);
        assert_eq!(s.remaining(), 0);
        // Quotas used up: sampling follows the original quotas 3:0:1.
        let mut counts = [0u32; 3];
        for _ in 0..40_000 {
            let v = s.draw(rng.next(), rng.next()).unwrap();
            counts[(v / 10 - 1) as usize] += 1;
        }
        assert_eq!(counts[1], 0);
        assert!((f64::from(counts[0]) / 40_000.0 - 0.75).abs() < 0.01);
        assert!(
            QuotaSampler::<u32>::new(vec![], vec![])
                .draw(0.5, 0.5)
                .is_none()
        );

        let state = mixed_state(10);
        let p = plan(
            &state,
            &Config::default(),
            &PlanInput {
                bots: &bots(10, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "t",
            },
        );
        let mut sampler = p.pair_sampler();
        let planned = u64::from(p.hourly) * u64::from(1 + OVERFLOW);
        assert_eq!(sampler.remaining(), planned);
        for _ in 0..planned {
            sampler.draw(rng.next(), rng.next());
        }
        assert_eq!(sampler.remaining(), 0);
    }

    #[test]
    fn quota_sampler_uses_the_overflow_before_repeating() {
        let mut s = QuotaSampler::with_overflow(vec![(1u32, 2), (2, 1)], vec![(2, 1), (3, 2)]);
        assert_eq!(s.remaining(), 6);
        let mut rng = Rng(3);
        let mut draws: Vec<u32> = (0..3)
            .map(|_| s.draw(rng.next(), rng.next()).unwrap())
            .collect();
        draws.sort_unstable();
        assert_eq!(draws, [1, 1, 2]);
        let mut draws: Vec<u32> = (0..3)
            .map(|_| s.draw(rng.next(), rng.next()).unwrap())
            .collect();
        draws.sort_unstable();
        assert_eq!(draws, [2, 3, 3]);
        assert_eq!(s.remaining(), 0);
        // Then proportional to all the quotas, 2:2:2.
        let mut counts = [0u32; 3];
        for _ in 0..30_000 {
            counts[s.draw(rng.next(), rng.next()).unwrap() as usize - 1] += 1;
        }
        assert!(
            counts
                .iter()
                .all(|&c| (f64::from(c) / 30_000.0 - 1.0 / 3.0).abs() < 0.02)
        );
        // No overflow (an older plan): straight to the original quotas.
        let mut s = QuotaSampler::with_overflow(vec![(1u32, 1)], vec![]);
        assert_eq!(s.draw(0.5, 0.5), Some(1));
        assert_eq!(s.draw(0.5, 0.5), Some(1));
    }

    #[test]
    fn equal_priorities_are_spread_over_the_bots() {
        // 300 unfought nanos: every pairing has the same priority. An hour's battles must not
        // all go to the lowest vid (prod, 2026-10-24: one bot got all 200).
        let config = Config::default();
        let state = state_1v1();
        let all = bots(300, WeightClass::Nano);
        let input = |at| PlanInput {
            bots: &all,
            probation: &[],
            top: &[],
            slot_pairs: &[],
            last_hour_battles: 0,
            generated_at: at,
        };
        let p = plan(&state, &config, &input("2026-10-24T15:12:00Z"));
        assert_eq!(p.hourly, 200);
        let mut per_bot: HashMap<u32, u32> = HashMap::new();
        for &(a, b, q) in p.pairs.iter().chain(&p.overflow_pairs) {
            assert_eq!(q, 1, "unfought pairings first: no pair twice");
            *per_bot.entry(a).or_default() += 1;
            *per_bot.entry(b).or_default() += 1;
        }
        assert_eq!(p.overflow_pairs.len(), 600);
        assert!(per_bot.len() > 280, "{} bots", per_bot.len());
        assert!(per_bot.values().all(|&n| n <= 15), "{per_bot:?}");
        // Deterministic within an hour, different the next.
        assert_eq!(plan(&state, &config, &input("2026-10-24T15:12:00Z")), p);
        let next = plan(&state, &config, &input("2026-10-24T16:12:00Z"));
        let first: HashSet<(u32, u32)> = p.pairs.iter().map(|x| (x.0, x.1)).collect();
        assert!(
            next.pairs
                .iter()
                .filter(|x| first.contains(&(x.0, x.1)))
                .count()
                < 20
        );
    }

    #[test]
    fn weight_classes_are_averaged_not_summed() {
        // 100 nanos and 100 megas, nothing fought: a nano pairing is in 4 rumbles, a mega one
        // in 1. Summed, every battle of the hour went to the nanos (prod, 2026-10-24).
        let config = Config::default();
        let mut all = bots(100, WeightClass::Nano);
        all.extend((101..=200).map(|v| bot(v, WeightClass::Mega)));
        let p = plan(
            &state_1v1(),
            &config,
            &PlanInput {
                bots: &all,
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "2026-10-24T20:12:00Z",
            },
        );
        let megas = p.pairs.iter().filter(|x| x.0 > 100 && x.1 > 100).count();
        assert!(megas > 20, "{megas} mega pairings of {}", p.pairs.len());
        let pr = Priorities::new(&state_1v1(), &config, &all);
        let ratio = pr.current(0, 1) / pr.current(100, 101);
        assert!((0.8..1.25).contains(&ratio), "nano/mega {ratio}");
    }

    #[test]
    fn equal_melee_priorities_are_spread_over_the_bots() {
        // 400 unfought melee bots: anchors and partner lists must not all be the lowest vids.
        let config = Config::default();
        let state = State::new(1, Division::Melee, 0);
        let p = plan(
            &state,
            &config,
            &PlanInput {
                bots: &bots(400, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "2026-10-24T15:12:00Z",
            },
        );
        assert_eq!(p.anchors.iter().map(|a| a.1).sum::<u32>(), 20);
        assert_eq!(p.overflow_anchors.iter().map(|a| a.1).sum::<u32>(), 60);
        assert!(p.anchors.iter().any(|a| a.0 > 100), "{:?}", p.anchors);
        let partners: HashSet<u32> = p
            .anchors
            .iter()
            .flat_map(|a| p.partners[&a.0].iter().map(|x| x.0))
            .collect();
        assert!(partners.len() > 300, "{} partners", partners.len());
    }

    fn melee_state(count: u32) -> State {
        let mut state = State::new(1, Division::Melee, 0);
        for k in 0..count * 3 {
            // Battles of 10 consecutive-ish bots.
            let entries: Vec<Entry> = (0..10u32)
                .map(|i| {
                    let vid = 1 + (k + i * 3) % count;
                    entry(vid, 10.0 * (1 + (vid * 7 + k) % 9) as f32, (i % 4) as u16)
                })
                .collect();
            let distinct: HashSet<u32> = entries.iter().map(|e| e.vid).collect();
            if distinct.len() == 10 {
                state.fold(&record(Division::Melee, entries), 1);
            }
        }
        state
    }

    #[test]
    fn melee_plan_and_battles() {
        let config = Config::default();
        let state = melee_state(40);
        assert!(state.pairing_count() > 100);
        let p = plan(
            &state,
            &config,
            &PlanInput {
                bots: &bots(40, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 100,
                generated_at: "t",
            },
        );
        assert_eq!(p.division, Division::Melee);
        assert_eq!(p.hourly, 150);
        assert!(p.pairs.is_empty());
        assert_eq!(p.anchors.iter().map(|a| a.1).sum::<u32>(), 150);
        assert_eq!(p.partners.len(), 40);
        for (vid, list) in &p.partners {
            assert_eq!(list.len(), 39, "40 bots: 39 partners");
            assert!(list.iter().all(|x| x.0 != *vid));
            assert!(list.windows(2).all(|w| w[0].1 >= w[1].1));
        }
        assert_eq!(Plan::from_bytes(&p.to_bytes()).unwrap(), p);

        let mut sampler = p.melee_sampler();
        let mut rng = Rng(99);
        let mut seen = HashSet::new();
        for _ in 0..400 {
            let battle = sampler.battle(10, &mut || rng.next());
            let distinct: HashSet<u32> = battle.iter().copied().collect();
            assert_eq!(battle.len(), 10);
            assert_eq!(distinct.len(), 10);
            assert!(battle.iter().all(|v| (1..=40).contains(v)));
            seen.extend(battle);
        }
        assert!(seen.len() > 30);
        // 150 anchors and 450 overflow anchors were available.
        assert_eq!(sampler.remaining(), 200);

        // Many bots: partner lists are capped at 40.
        let big = plan(
            &melee_state(60),
            &config,
            &PlanInput {
                bots: &bots(60, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "t",
            },
        );
        assert!(big.partners.values().all(|l| l.len() == PARTNERS_PER_BOT));
        assert_eq!(big.anchors.iter().map(|a| a.1).sum::<u32>(), 20);
        // No partner lists at all (an empty state with 12 bots has some, but a sampler built
        // from a bare plan falls back to the planned bots).
        let mut bare = big.clone();
        bare.partners.clear();
        let mut sampler = bare.melee_sampler();
        let battle = sampler.battle(10, &mut || rng.next());
        assert_eq!(battle.iter().collect::<HashSet<_>>().len(), 10);
    }

    #[test]
    fn melee_unfought_bots_are_anchored_first() {
        // 12 bots; bot 12 never fought. It has the most to gain.
        let config = Config::default();
        let mut state = State::new(1, Division::Melee, 0);
        for k in 0..30u32 {
            let entries: Vec<Entry> = (0..10u32)
                .map(|i| entry(1 + (k + i) % 11, 10.0 + (i * 5) as f32, 1))
                .collect();
            state.fold(&record(Division::Melee, entries), 1);
        }
        let p = plan(
            &state,
            &config,
            &PlanInput {
                bots: &bots(12, WeightClass::Mega),
                probation: &[],
                top: &[],
                slot_pairs: &[],
                last_hour_battles: 0,
                generated_at: "t",
            },
        );
        let quota = |vid| p.anchors.iter().find(|a| a.0 == vid).map_or(0, |a| a.1);
        assert!((1..=11).all(|v| quota(12) > quota(v)));
    }

    fn tally_state() -> State {
        let mut state = State::new(1, Division::OneVsOne, 0);
        let ok = |a, b| {
            record(
                Division::OneVsOne,
                vec![entry(a, 10.0, 1), entry(b, 10.0, 1)],
            )
        };
        let mut failed = record(Division::OneVsOne, vec![entry(1, 0.0, 0), entry(2, 0.0, 0)]);
        failed.failed = Some((1, FailReason::Crash));
        // Probation versions: 10 battles with 9 and 5 failures (vids 10 and 11).
        for (vid, fails) in [(10, 9), (11, 5)] {
            failed.contestants[1].vid = vid;
            for _ in 0..fails {
                state.fold(&failed, 1);
            }
            for _ in 0..10 - fails {
                state.fold(&ok(vid, 1), 1);
            }
        }
        // Probation version 12 with 9 battles only.
        for _ in 0..9 {
            state.fold(&ok(12, 1), 1);
        }
        state
    }

    #[test]
    fn probation_transitions() {
        let state = tally_state();
        let config = Config::default();
        let versions = [
            (10, VersionStatus::Probation),
            (11, VersionStatus::Probation),
            (12, VersionStatus::Probation),
            (13, VersionStatus::Probation),
            (1, VersionStatus::Failed),
        ];
        let t = transitions(&state, &config, &versions);
        assert_eq!(t.len(), 2);
        assert_eq!(
            (t[0].vid, t[0].from, t[0].to),
            (10, VersionStatus::Probation, VersionStatus::Failed)
        );
        assert_eq!(
            (t[1].vid, t[1].from, t[1].to),
            (11, VersionStatus::Probation, VersionStatus::Active)
        );
        assert!(t[0].reason.contains("9 of 10"));
        assert_eq!(VersionStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn deactivation_needs_three_distinct_reporters() {
        let config = Config::default();
        let mut state = State::new(1, Division::OneVsOne, 0);
        let ok = record(
            Division::OneVsOne,
            vec![entry(2, 10.0, 1), entry(3, 10.0, 1)],
        );
        let mut failed = record(Division::OneVsOne, vec![entry(1, 0.0, 0), entry(2, 0.0, 0)]);
        failed.failed = Some((1, FailReason::Crash));
        for _ in 0..30 {
            state.fold(&ok, 1);
        }
        let active = [(2, VersionStatus::Active)];
        // 16 failures of the last 20 from two users: not enough.
        for i in 0..16 {
            state.fold_from(&failed, 1, if i % 2 == 0 { "g1" } else { "g2" });
        }
        assert_eq!(state.versions[&2].recent_failures(), 16);
        assert!(transitions(&state, &config, &active).is_empty());
        // A third user tips it.
        state.fold_from(&failed, 1, "g3");
        let t = transitions(&state, &config, &active);
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].vid, t[0].to), (2, VersionStatus::Failed));
        assert!(t[0].reason.contains("last 20"));

        // Three users but only 15 of 20 failed: below the share.
        let mut state = State::new(1, Division::OneVsOne, 0);
        for _ in 0..5 {
            state.fold(&ok, 1);
        }
        for i in 0..15 {
            state.fold_from(&failed, 1, ["g1", "g2", "g3"][i % 3]);
        }
        state.fold(&ok, 1);
        for _ in 0..4 {
            state.fold(&ok, 1);
        }
        assert_eq!(state.versions[&2].recent_failures(), 15);
        assert!(transitions(&state, &config, &active).is_empty());

        // Successes after a spree reset the reporters: the next failures need three again.
        let mut state = State::new(1, Division::OneVsOne, 0);
        for i in 0..20 {
            state.fold_from(&failed, 1, ["g1", "g2", "g3"][i % 3]);
        }
        assert_eq!(transitions(&state, &config, &active).len(), 1);
        for _ in 0..12 {
            state.fold(&ok, 1);
        }
        assert_eq!(state.versions[&2].distinct_reporters(), 0);
        for _ in 0..16 {
            state.fold_from(&failed, 1, "g1");
        }
        assert!(transitions(&state, &config, &active).is_empty());
    }

    /// 1,100 bots, 90% of the pairings fought, H = 20,000. Run with
    /// `cargo test -p robocode-s-rumble-core --release -- --ignored big_plan --nocapture`.
    #[test]
    #[ignore]
    fn big_plan() {
        let mut state = state_1v1();
        let count = 1100u32;
        for a in 1..=count {
            for b in a + 1..=count {
                if (a * 13 + b * 7) % 10 != 0 {
                    let score = 20.0 + ((a * 31 + b * 17) % 60) as f32;
                    fight(&mut state, a, b, score, 1 + (a + b) % 3);
                }
            }
        }
        let all: Vec<PlanBot> = (1..=count)
            .map(|v| {
                bot(
                    v,
                    [WeightClass::Mega, WeightClass::Mini][(v % 4 == 0) as usize],
                )
            })
            .collect();
        let config = Config::default();
        let input = PlanInput {
            bots: &all,
            probation: &[],
            top: &[],
            slot_pairs: &[],
            last_hour_battles: 13_334,
            generated_at: "t",
        };
        let started = std::time::Instant::now();
        let p = plan(&state, &config, &input);
        let planned = started.elapsed();
        assert_eq!(p.hourly, 20_001);
        assert_eq!(p.pairs.iter().map(|x| x.2).sum::<u32>(), 20_001);
        let bytes = p.to_bytes();
        eprintln!(
            "plan {planned:?} ({} pairings fought), file {:?} ({} bytes, {} pairs)",
            state.pairing_count(),
            started.elapsed() - planned,
            bytes.len(),
            p.pairs.len()
        );
        assert!(planned.as_secs_f64() < 2.0);

        let mut melee = State::new(1, Division::Melee, 0);
        for k in 0..20_000u32 {
            let entries: Vec<Entry> = (0..10u32)
                .map(|i| entry(1 + (k * 7 + i * 101) % count, 10.0 + (i * 9) as f32, 1))
                .collect();
            if entries.iter().map(|e| e.vid).collect::<HashSet<_>>().len() == 10 {
                melee.fold(&record(Division::Melee, entries), 1);
            }
        }
        let started = std::time::Instant::now();
        let p = plan(
            &melee,
            &config,
            &PlanInput {
                last_hour_battles: 2000,
                ..input
            },
        );
        eprintln!(
            "melee plan {:?} ({} pairings fought, H = {})",
            started.elapsed(),
            melee.pairing_count(),
            p.hourly
        );
    }
}
