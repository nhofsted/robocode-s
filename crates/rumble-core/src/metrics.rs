// SPDX-License-Identifier: EPL-2.0

//! The ranking metrics of one rumble (spec §15 step 3), computed from the pairing state of its
//! division, and the data files built from them. Pure: no I/O, the caller passes the members'
//! metadata in.

use crate::data::{self, BotRumble, HistoryPoint, Metrics, Pairing, RankingRow, Rankings};
use crate::ids::{Division, RumbleId, WeightClass};
use crate::state::State;
use std::cmp::Ordering;

/// Glicko-2 system constant τ.
pub const GLICKO_TAU: f64 = 0.5;
/// Glicko-2 convergence tolerance ε.
pub const GLICKO_EPSILON: f64 = 1e-6;
/// Initial rating, deviation and volatility.
pub const GLICKO_START: (f64, f64, f64) = (1500.0, 350.0, 0.06);
/// Rating periods run over the same games.
pub const GLICKO_PERIODS: usize = 20;

/// The Glicko-2 scale factor between ratings and the internal μ, φ.
const SCALE: f64 = 173.7178;
/// A pairing score within this of 50 counts as exactly 50 for PWIN.
const HALF_EPS: f64 = 1e-9;

/// Whether a bot version of this division and weight class takes part in a rumble: the same
/// division and a class at or below the rumble's (a nano bot is also in the micro, mini and
/// mega rumbles). The caller only offers active versions; probation versions are not members.
pub fn is_member(division: Division, class: WeightClass, rumble: RumbleId) -> bool {
    division == rumble.division && class <= rumble.class
}

/// A member of the rumble: the ranked version of a bot and the metadata of its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub vid: u32,
    /// Bot id (class name without version).
    pub id: String,
    pub version: String,
    pub author: String,
    pub country: Option<String>,
    /// Rank one week ago (from the snapshot), if the bot was ranked then.
    pub rank_week_ago: Option<u32>,
    /// The version's colours, where known.
    pub colors: Option<crate::data::BotColors>,
}

/// One pairing of a bot, from its side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairingMetrics {
    /// Index of the opponent in `RumbleMetrics::members`.
    pub opponent: usize,
    pub battles: u32,
    pub score: f64,
    pub survival: f64,
    pub normalized: f64,
    pub pbi: f64,
}

/// The numbers of one member.
#[derive(Clone, Debug, PartialEq)]
pub struct BotMetrics {
    pub metrics: Metrics,
    /// 1-based rank by APS (ties: more battles, then bot id).
    pub rank: u32,
    /// Opponents with at least one battle.
    pub pairing_count: u32,
    pub battles: u64,
    /// Ordered by the opponent's rank.
    pub pairings: Vec<PairingMetrics>,
}

/// The metrics of a whole rumble.
#[derive(Clone, Debug, PartialEq)]
pub struct RumbleMetrics {
    pub members: Vec<Member>,
    /// In member order.
    pub bots: Vec<BotMetrics>,
    /// Member indices, best rank first.
    pub order: Vec<usize>,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    n: u32,
    /// The row bot's mean pairing score against the column bot.
    mean: f64,
    /// The row bot's mean pairing survival.
    survival: f64,
}

/// Computes all metrics of a rumble whose members (unique vids) are `members`, from the
/// pairings of `state` between them.
pub fn compute(members: Vec<Member>, state: &State) -> RumbleMetrics {
    let n = members.len();
    let mut cells = vec![Cell::default(); n * n];
    for i in 0..n {
        for j in i + 1..n {
            let (vi, vj) = (members[i].vid, members[j].vid);
            let Some(p) = state.pairing(vi, vj).filter(|p| p.n > 0) else {
                continue;
            };
            let count = f64::from(p.n);
            let (mean, survival) = (p.s / count, p.v / count);
            // The state keeps the lower vid's side; the other side is 100 minus it.
            let (low, high) = if vi < vj {
                ((mean, survival), (100.0 - mean, 100.0 - survival))
            } else {
                ((100.0 - mean, 100.0 - survival), (mean, survival))
            };
            cells[i * n + j] = Cell {
                n: p.n,
                mean: low.0,
                survival: low.1,
            };
            cells[j * n + i] = Cell {
                n: p.n,
                mean: high.0,
                survival: high.1,
            };
        }
    }
    let cell = |i: usize, j: usize| &cells[i * n + j];

    // APS, PWIN, survival, battles, pairing counts.
    let mut aps = vec![0.0; n];
    let mut pwin = vec![0.0; n];
    let mut survival = vec![0.0; n];
    let mut pairing_count = vec![0u32; n];
    let mut battles = vec![0u64; n];
    for i in 0..n {
        let (mut sum, mut wins, mut surv) = (0.0, 0.0, 0.0);
        for j in 0..n {
            let c = cell(i, j);
            if c.n == 0 {
                continue;
            }
            pairing_count[i] += 1;
            battles[i] += u64::from(c.n);
            sum += c.mean;
            surv += c.survival;
            wins += if (c.mean - 50.0).abs() <= HALF_EPS {
                0.5
            } else {
                f64::from(u8::from(c.mean > 50.0))
            };
        }
        if pairing_count[i] > 0 {
            let count = f64::from(pairing_count[i]);
            aps[i] = sum / count;
            pwin[i] = 100.0 * wins / count;
            survival[i] = surv / count;
        }
        // A melee battle is in nine pairings of a bot: its own tally counts it once.
        if state.division == Division::Melee {
            battles[i] = state
                .versions
                .get(&members[i].vid)
                .map_or(0, |tally| u64::from(tally.battles));
        }
    }

    // Rank by APS, ties: more battles, then bot id.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        aps[b]
            .total_cmp(&aps[a])
            .then(battles[b].cmp(&battles[a]))
            .then_with(|| members[a].id.cmp(&members[b].id))
    });
    let mut position = vec![0usize; n];
    for (pos, &i) in order.iter().enumerate() {
        position[i] = pos;
    }

    // ANPP: per opponent, the score range over all bots that fought it.
    let mut normalized = vec![0.0; n * n];
    let mut anpp = vec![0.0; n];
    for o in 0..n {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for y in 0..n {
            let c = cell(y, o);
            if c.n > 0 {
                lo = lo.min(c.mean);
                hi = hi.max(c.mean);
            }
        }
        for x in 0..n {
            let c = cell(x, o);
            if c.n > 0 {
                normalized[x * n + o] = if hi > lo {
                    100.0 * (c.mean - lo) / (hi - lo)
                } else {
                    50.0
                };
            }
        }
    }
    for i in 0..n {
        if pairing_count[i] > 0 {
            let sum: f64 = (0..n)
                .filter(|&o| cell(i, o).n > 0)
                .map(|o| normalized[i * n + o])
                .sum();
            anpp[i] = sum / f64::from(pairing_count[i]);
        }
    }

    // Vote: every bot gives one vote to the opponent that scores best against it.
    let mut votes = vec![0.0; n];
    for o in 0..n {
        let mut best = f64::NEG_INFINITY;
        let mut winners: Vec<usize> = Vec::new();
        for x in 0..n {
            let c = cell(x, o);
            if c.n == 0 {
                continue;
            }
            match c.mean.partial_cmp(&best) {
                Some(Ordering::Greater) => {
                    best = c.mean;
                    winners.clear();
                    winners.push(x);
                }
                Some(Ordering::Equal) => winners.push(x),
                _ => {}
            }
        }
        for &x in &winners {
            votes[x] += 1.0 / winners.len() as f64;
        }
    }
    let vote: Vec<f64> = votes.iter().map(|v| 100.0 * v / n as f64).collect();

    // Glicko-2.
    let ratings = glicko(n, |i| {
        (0..n)
            .filter(|&j| cell(i, j).n > 0)
            .map(|j| (j, cell(i, j).mean / 100.0))
            .collect()
    });

    // KNN-PBI and the pairing lists.
    let k = ((n as f64).sqrt() / 2.0).ceil() as usize;
    let mut bots: Vec<BotMetrics> = Vec::with_capacity(n);
    for x in 0..n {
        let mut pairings = Vec::with_capacity(pairing_count[x] as usize);
        for &o in &order {
            let c = cell(x, o);
            if c.n == 0 {
                continue;
            }
            // The k bots on either side in APS order that fought O.
            let (mut sum, mut neighbours) = (0.0, 0usize);
            let mut near = |ys: &mut dyn Iterator<Item = &usize>| {
                for &y in ys.filter(|&&y| cell(y, o).n > 0).take(k) {
                    sum += cell(y, o).mean;
                    neighbours += 1;
                }
            };
            near(&mut order[..position[x]].iter().rev());
            near(&mut order[position[x] + 1..].iter());
            let pbi = if neighbours > 0 {
                c.mean - sum / neighbours as f64
            } else {
                0.0
            };
            pairings.push(PairingMetrics {
                opponent: o,
                battles: c.n,
                score: c.mean,
                survival: c.survival,
                normalized: normalized[x * n + o],
                pbi,
            });
        }
        bots.push(BotMetrics {
            metrics: Metrics {
                aps: aps[x],
                pwin: pwin[x],
                survival: survival[x],
                glicko: ratings[x].0,
                glicko_rd: ratings[x].1,
                anpp: anpp[x],
                vote: vote[x],
            },
            rank: position[x] as u32 + 1,
            pairing_count: pairing_count[x],
            battles: battles[x],
            pairings,
        });
    }

    RumbleMetrics {
        members,
        bots,
        order,
    }
}

/// The 20 rating periods over the same games: `games(i)` lists the opponents of bot `i` and
/// its outcomes in 0..=1. Every period uses the previous period's ratings of all bots and
/// starts from the previous period's volatility. Returns the last period's (rating, RD).
fn glicko(n: usize, games: impl Fn(usize) -> Vec<(usize, f64)>) -> Vec<(f64, f64)> {
    let games: Vec<Vec<(usize, f64)>> = (0..n).map(games).collect();
    let mut state = vec![
        (
            (GLICKO_START.0 - 1500.0) / SCALE,
            GLICKO_START.1 / SCALE,
            GLICKO_START.2
        );
        n
    ];
    for _ in 0..GLICKO_PERIODS {
        let previous = state.clone();
        for i in 0..n {
            let opponents: Vec<(f64, f64, f64)> = games[i]
                .iter()
                .map(|&(j, s)| (previous[j].0, previous[j].1, s))
                .collect();
            state[i] = glicko2_update(previous[i], &opponents, GLICKO_TAU);
        }
    }
    state
        .iter()
        .map(|&(mu, phi, _)| (SCALE * mu + 1500.0, SCALE * phi))
        .collect()
}

/// One Glicko-2 rating period for one player (Glickman 2012, steps 3 to 8), on the internal
/// scale: `player` is (μ, φ, σ), `games` are the opponents' (μ_j, φ_j) and the outcome s_j
/// (1 win, 0.5 draw, 0 loss). Returns the new (μ, φ, σ). Without games only φ grows.
pub fn glicko2_update(
    player: (f64, f64, f64),
    games: &[(f64, f64, f64)],
    tau: f64,
) -> (f64, f64, f64) {
    let (mu, phi, sigma) = player;
    if games.is_empty() {
        return (mu, (phi * phi + sigma * sigma).sqrt(), sigma);
    }
    let g = |phi: f64| 1.0 / (1.0 + 3.0 * phi * phi / std::f64::consts::PI.powi(2)).sqrt();
    let e = |mu_j: f64, phi_j: f64| 1.0 / (1.0 + (-g(phi_j) * (mu - mu_j)).exp());
    // Step 3 and 4: the estimated variance v and the improvement delta.
    let (mut inv_v, mut sum) = (0.0, 0.0);
    for &(mu_j, phi_j, s) in games {
        let (gj, ej) = (g(phi_j), e(mu_j, phi_j));
        inv_v += gj * gj * ej * (1.0 - ej);
        sum += gj * (s - ej);
    }
    let v = 1.0 / inv_v;
    let delta = v * sum;
    // Step 5: the new volatility, by the Illinois algorithm.
    let a = (sigma * sigma).ln();
    let f = |x: f64| {
        let ex = x.exp();
        let d = phi * phi + v + ex;
        ex * (delta * delta - phi * phi - v - ex) / (2.0 * d * d) - (x - a) / (tau * tau)
    };
    let mut big_a = a;
    let mut big_b = if delta * delta > phi * phi + v {
        (delta * delta - phi * phi - v).ln()
    } else {
        let mut k = 1.0;
        while f(a - k * tau) < 0.0 {
            k += 1.0;
        }
        a - k * tau
    };
    let (mut fa, mut fb) = (f(big_a), f(big_b));
    while (big_b - big_a).abs() > GLICKO_EPSILON {
        let c = big_a + (big_a - big_b) * fa / (fb - fa);
        let fc = f(c);
        if fc * fb <= 0.0 {
            big_a = big_b;
            fa = fb;
        } else {
            fa /= 2.0;
        }
        big_b = c;
        fb = fc;
    }
    let sigma_new = (big_a / 2.0).exp();
    // Steps 6 to 8.
    let phi_star = (phi * phi + sigma_new * sigma_new).sqrt();
    let phi_new = 1.0 / (1.0 / (phi_star * phi_star) + 1.0 / v).sqrt();
    let mu_new = mu + phi_new * phi_new * sum;
    (mu_new, phi_new, sigma_new)
}

fn round(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

/// The metrics as published: percentages and APS to two decimals, the rating to one.
fn rounded(m: &Metrics) -> Metrics {
    Metrics {
        aps: round(m.aps, 2),
        pwin: round(m.pwin, 2),
        survival: round(m.survival, 2),
        glicko: round(m.glicko, 1),
        glicko_rd: round(m.glicko_rd, 1),
        anpp: round(m.anpp, 2),
        vote: round(m.vote, 2),
    }
}

impl RumbleMetrics {
    /// `rankings/<rumble>.json`, sorted by rank.
    pub fn rankings(&self, rumble: &str, generated_at: &str, engine_version: u32) -> Rankings {
        Rankings {
            schema: data::SCHEMA_VERSION,
            rumble: rumble.to_string(),
            generated_at: generated_at.to_string(),
            engine_version,
            bots: self
                .order
                .iter()
                .map(|&i| {
                    let (member, bot) = (&self.members[i], &self.bots[i]);
                    RankingRow {
                        id: member.id.clone(),
                        version: member.version.clone(),
                        author: member.author.clone(),
                        country: member.country.clone(),
                        rank: bot.rank,
                        pairings: bot.pairing_count,
                        battles: bot.battles,
                        rank_week_ago: member.rank_week_ago,
                        colors: member.colors.clone(),
                        metrics: rounded(&bot.metrics),
                    }
                })
                .collect(),
        }
    }

    /// The bot's part of `bots/<id>.json` for this rumble, with the history the caller has
    /// already extended and thinned (`data::push_history`).
    pub fn bot_rumble(&self, member: usize, history: Vec<HistoryPoint>) -> BotRumble {
        let bot = &self.bots[member];
        BotRumble {
            rank: bot.rank,
            battles: bot.battles,
            pairings: bot
                .pairings
                .iter()
                .map(|p| Pairing {
                    opponent: self.members[p.opponent].id.clone(),
                    opponent_version: self.members[p.opponent].version.clone(),
                    battles: p.battles,
                    score: round(p.score, 2),
                    survival: round(p.survival, 2),
                    normalized: round(p.normalized, 2),
                    pbi: round(p.pbi, 2),
                })
                .collect(),
            history,
            metrics: rounded(&bot.metrics),
        }
    }

    /// Today's history point of a member (`data::push_history` appends it).
    pub fn history_point(&self, member: usize, date: &str) -> HistoryPoint {
        let bot = &self.bots[member];
        HistoryPoint {
            date: date.to_string(),
            version: self.members[member].version.clone(),
            rank: bot.rank,
            aps: round(bot.metrics.aps, 2),
            glicko: round(bot.metrics.glicko, 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Entry, Record};

    fn member(vid: u32, id: &str) -> Member {
        Member {
            vid,
            id: id.to_string(),
            version: "1.0".to_string(),
            author: "x".to_string(),
            country: None,
            rank_week_ago: None,
            colors: None,
        }
    }

    /// One battle between two vids: scores and rounds won.
    fn battle(state: &mut State, a: u32, b: u32, score: (f32, f32), firsts: (u16, u16)) {
        let record = Record {
            division: Division::OneVsOne,
            weight: 1,
            uid: "g1".to_string(),
            contestants: vec![
                Entry {
                    vid: a,
                    score: score.0,
                    firsts: firsts.0,
                },
                Entry {
                    vid: b,
                    score: score.1,
                    firsts: firsts.1,
                },
            ],
            failed: None,
            trusted: false,
        };
        state.fold(&record, 1);
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    /// A (vid 1), B (2), C (3): A-B 60:40, A-C 80:20, B-C 55:45; rounds 7:3, 4:1, 1:1.
    fn triangle() -> RumbleMetrics {
        let mut state = State::new(1, Division::OneVsOne, 0);
        battle(&mut state, 1, 2, (60.0, 40.0), (7, 3));
        battle(&mut state, 1, 3, (80.0, 20.0), (4, 1));
        battle(&mut state, 2, 3, (55.0, 45.0), (1, 1));
        compute(
            vec![member(1, "a.A"), member(2, "b.B"), member(3, "c.C")],
            &state,
        )
    }

    #[test]
    fn membership() {
        let mini = RumbleId::new(Division::OneVsOne, WeightClass::Mini);
        assert!(is_member(Division::OneVsOne, WeightClass::Nano, mini));
        assert!(is_member(Division::OneVsOne, WeightClass::Mini, mini));
        assert!(!is_member(Division::OneVsOne, WeightClass::Mega, mini));
        assert!(!is_member(Division::Melee, WeightClass::Nano, mini));
        let mega = RumbleId::new(Division::Melee, WeightClass::Mega);
        assert!(is_member(Division::Melee, WeightClass::Mega, mega));
        assert!(is_member(Division::Melee, WeightClass::Micro, mega));
    }

    #[test]
    fn simple_metrics_on_a_triangle() {
        let m = triangle();
        let [a, b, c] = [&m.bots[0], &m.bots[1], &m.bots[2]];
        // APS: A (60 + 80) / 2, B (40 + 55) / 2, C (20 + 45) / 2.
        close(a.metrics.aps, 70.0);
        close(b.metrics.aps, 47.5);
        close(c.metrics.aps, 32.5);
        // PWIN: B beats C and loses to A: 50.
        close(a.metrics.pwin, 100.0);
        close(b.metrics.pwin, 50.0);
        close(c.metrics.pwin, 0.0);
        // Survival: B's 30 and 50, C's 20 and 50.
        close(a.metrics.survival, 75.0);
        close(b.metrics.survival, 40.0);
        close(c.metrics.survival, 35.0);
        // ANPP: A is the best against B and C, B the best against A, C the worst against all.
        close(a.metrics.anpp, 100.0);
        close(b.metrics.anpp, 50.0);
        close(c.metrics.anpp, 0.0);
        // Vote: A votes B, B votes A, C votes A.
        close(a.metrics.vote, 200.0 / 3.0);
        close(b.metrics.vote, 100.0 / 3.0);
        close(c.metrics.vote, 0.0);
        assert_eq!([a.rank, b.rank, c.rank], [1, 2, 3]);
        assert_eq!(m.order, [0, 1, 2]);
        assert_eq!(
            [a.pairing_count, b.pairing_count, c.pairing_count],
            [2, 2, 2]
        );
        assert_eq!([a.battles, b.battles, c.battles], [2, 2, 2]);
    }

    #[test]
    fn pbi_uses_the_neighbours_in_aps_order() {
        let m = triangle();
        // k = ceil(sqrt(3) / 2) = 1. The opponent itself never counts as a neighbour.
        let pbi = |x: usize, o: usize| {
            m.bots[x]
                .pairings
                .iter()
                .find(|p| p.opponent == o)
                .unwrap()
                .pbi
        };
        close(pbi(0, 1), 60.0 - 45.0); // A vs B: the neighbour below A with a pairing is C
        close(pbi(0, 2), 80.0 - 55.0); // A vs C: B
        close(pbi(1, 0), 40.0 - 20.0); // B vs A: C (above is A itself)
        close(pbi(1, 2), 55.0 - 80.0); // B vs C: A
        close(pbi(2, 0), 20.0 - 40.0); // C vs A: B
        close(pbi(2, 1), 45.0 - 60.0); // C vs B: skipping B, A
        // Normalised scores of the pairing lists.
        let first = &m.bots[1].pairings[0];
        assert_eq!(first.opponent, 0);
        close(first.normalized, 100.0);
        close(first.score, 40.0);
        close(first.survival, 30.0);
    }

    #[test]
    fn partial_pairings_ties_and_pwin_half() {
        // D only fought A; B and C never met; A-B is an exact 50:50 split.
        let mut state = State::new(1, Division::OneVsOne, 0);
        battle(&mut state, 1, 2, (50.0, 50.0), (1, 1));
        battle(&mut state, 1, 3, (90.0, 10.0), (3, 0));
        battle(&mut state, 1, 3, (70.0, 30.0), (1, 0));
        battle(&mut state, 4, 1, (10.0, 90.0), (0, 1));
        let members = vec![
            member(4, "d.D"),
            member(3, "c.C"),
            member(2, "b.B"),
            member(1, "a.A"),
        ];
        let m = compute(members, &state);
        let by_id = |id: &str| m.members.iter().position(|x| x.id == id).unwrap();
        let (a, b, c, d) = (by_id("a.A"), by_id("b.B"), by_id("c.C"), by_id("d.D"));
        // A: pairings B 50, C 80 (two battles), D 90.
        close(m.bots[a].metrics.aps, (50.0 + 80.0 + 90.0) / 3.0);
        close(m.bots[a].metrics.pwin, 100.0 * 2.5 / 3.0);
        assert_eq!(m.bots[a].pairing_count, 3);
        assert_eq!(m.bots[a].battles, 4);
        assert_eq!(m.bots[d].pairing_count, 1);
        close(m.bots[d].metrics.aps, 10.0);
        close(m.bots[d].metrics.pwin, 0.0);
        close(m.bots[b].metrics.pwin, 50.0);
        // ANPP of D: against A, A scores 90 at best, D 10 at worst: D is 0, so is C (20), B 50:
        // the range against A is 10 (D) to 50 (B)... A is not its own opponent.
        close(m.bots[d].metrics.anpp, 0.0);
        // Votes: A votes for the best against it: B 50, C 20, D 10 -> B. B votes A, C votes A,
        // D votes A.
        close(m.bots[b].metrics.vote, 25.0);
        close(m.bots[a].metrics.vote, 75.0);
        // Ranking: A first, then C (80 against A... C's APS is 20), B 50, D 10.
        assert_eq!(m.bots[a].rank, 1);
        assert_eq!(m.bots[b].rank, 2);
        assert_eq!(m.bots[c].rank, 3);
        assert_eq!(m.bots[d].rank, 4);
        // Bots without any pairing have neutral numbers and rank last.
        let loner = compute(
            vec![member(1, "a.A"), member(9, "z.Z")],
            &State::new(1, Division::OneVsOne, 0),
        );
        close(loner.bots[0].metrics.aps, 0.0);
        assert_eq!(loner.bots[0].rank, 1); // equal: id order
        assert_eq!(loner.bots[1].rank, 2);
        close(loner.bots[0].metrics.glicko, 1500.0);
    }

    #[test]
    fn rank_ties_prefer_more_battles_then_id() {
        let mut state = State::new(1, Division::OneVsOne, 0);
        // B and C are both at 50 against A; C has fought twice.
        battle(&mut state, 1, 2, (50.0, 50.0), (1, 1));
        battle(&mut state, 1, 3, (50.0, 50.0), (1, 1));
        battle(&mut state, 1, 3, (50.0, 50.0), (1, 1));
        let m = compute(
            vec![member(1, "b.B"), member(2, "a.A"), member(3, "c.C")],
            &state,
        );
        // All three have APS 50 (the loser's side of an even split); C has 2 battles, A (vid 1)
        // 3 battles: A first, then C, then B.
        let ranks: Vec<u32> = m.bots.iter().map(|b| b.rank).collect();
        assert_eq!(ranks, [1, 3, 2]);
        let mut state = State::new(1, Division::OneVsOne, 0);
        battle(&mut state, 1, 2, (50.0, 50.0), (1, 1));
        let m = compute(vec![member(1, "z.Z"), member(2, "a.A")], &state);
        assert_eq!(m.bots[1].rank, 1);
    }

    #[test]
    fn melee_battles_come_from_the_version_tally() {
        let mut state = State::new(1, Division::Melee, 0);
        let record = Record {
            division: Division::Melee,
            weight: 1,
            uid: "g1".to_string(),
            contestants: (1..=3)
                .map(|vid| Entry {
                    vid,
                    score: vid as f32 * 100.0,
                    firsts: vid as u16,
                })
                .collect(),
            failed: None,
            trusted: false,
        };
        state.fold(&record, 1);
        let m = compute(
            vec![member(1, "a.A"), member(2, "b.B"), member(3, "c.C")],
            &state,
        );
        assert!(
            m.bots
                .iter()
                .all(|b| b.battles == 1 && b.pairing_count == 2)
        );
        // C: 300/(300+100), 300/(300+200): 75 and 60.
        close(m.bots[2].metrics.aps, 67.5);
    }

    #[test]
    fn glicko2_matches_glickmans_example() {
        // Glickman 2012, section 3: r 1500, RD 200, σ 0.06, τ 0.5, three opponents.
        let to_internal = |r: f64, rd: f64| ((r - 1500.0) / SCALE, rd / SCALE);
        let games: Vec<(f64, f64, f64)> = [
            (1400.0, 30.0, 1.0),
            (1550.0, 100.0, 0.0),
            (1700.0, 300.0, 0.0),
        ]
        .iter()
        .map(|&(r, rd, s)| {
            let (mu, phi) = to_internal(r, rd);
            (mu, phi, s)
        })
        .collect();
        let (mu, phi) = to_internal(1500.0, 200.0);
        let (mu_new, phi_new, sigma_new) = glicko2_update((mu, phi, 0.06), &games, 0.5);
        assert!((SCALE * mu_new + 1500.0 - 1464.06).abs() < 0.01);
        assert!((SCALE * phi_new - 151.52).abs() < 0.01);
        assert!((sigma_new - 0.05999).abs() < 1e-5);
        // Intermediate values from the paper: μ' = -0.2069, φ' = 0.8722.
        assert!((mu_new + 0.2069).abs() < 1e-3);
        assert!((phi_new - 0.8722).abs() < 1e-3);
    }

    #[test]
    fn glicko2_without_games_only_widens() {
        let (mu, phi, sigma) = glicko2_update((0.5, 1.0, 0.06), &[], 0.5);
        close(mu, 0.5);
        close(sigma, 0.06);
        close(phi, (1.0f64 + 0.06 * 0.06).sqrt());
    }

    #[test]
    fn glicko_orders_the_bots_and_shrinks_the_deviation() {
        let m = triangle();
        let g: Vec<f64> = m.bots.iter().map(|b| b.metrics.glicko).collect();
        assert!(g[0] > g[1] && g[1] > g[2], "{g:?}");
        assert!(g[0] > 1500.0 && g[2] < 1500.0);
        for bot in &m.bots {
            assert!(bot.metrics.glicko_rd < 350.0 && bot.metrics.glicko_rd > 30.0);
        }
        // Pure dominance in a round robin of four: ratings strictly follow the dominance order.
        let mut state = State::new(1, Division::OneVsOne, 0);
        for a in 1..=4u32 {
            for b in a + 1..=4 {
                let edge = 55.0 + 5.0 * (b - a) as f32;
                battle(&mut state, a, b, (edge, 100.0 - edge), (1, 0));
            }
        }
        let m = compute(
            (1..=4).map(|v| member(v, &format!("p.B{v}"))).collect(),
            &state,
        );
        let g: Vec<f64> = m.bots.iter().map(|b| b.metrics.glicko).collect();
        assert!(g.windows(2).all(|w| w[0] > w[1]), "{g:?}");
        assert!(m.bots.iter().zip(1u32..).all(|(b, rank)| b.rank == rank));
    }

    #[test]
    fn a_dense_rumble_ranks_by_skill() {
        let n = 300u32;
        let mut state = State::new(1, Division::OneVsOne, 0);
        for a in 1..=n {
            for b in a + 1..=n {
                let edge = 50.0 + (b - a) as f32 / n as f32 * 40.0;
                battle(&mut state, a, b, (edge, 100.0 - edge), (1, 0));
            }
        }
        let members = (1..=n).map(|v| member(v, &format!("p.B{v:04}"))).collect();
        let m = compute(members, &state);
        assert!(m.bots.iter().zip(1u32..).all(|(b, rank)| b.rank == rank));
        assert!(
            m.bots
                .windows(2)
                .all(|w| w[0].metrics.glicko > w[1].metrics.glicko)
        );
        let total: f64 = m.bots.iter().map(|b| b.metrics.vote).sum();
        close(total, 100.0);
    }

    #[test]
    fn builds_the_data_files() {
        let m = triangle();
        let rankings = m.rankings("1v1", "2026-10-24T06:00:00Z", 1);
        assert_eq!(rankings.schema, 1);
        assert_eq!(
            rankings
                .bots
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["a.A", "b.B", "c.C"]
        );
        assert_eq!(rankings.bots[0].rank, 1);
        assert_eq!(rankings.bots[0].pairings, 2);
        assert_eq!(rankings.bots[0].metrics.aps, 70.0);
        assert_eq!(rankings.bots[0].metrics.vote, 66.67);
        let point = m.history_point(1, "2026-10-24");
        let history = data::push_history(Vec::new(), point);
        let details = m.bot_rumble(1, history);
        assert_eq!(details.rank, 2);
        assert_eq!(details.pairings.len(), 2);
        assert_eq!(details.pairings[0].opponent, "a.A");
        assert_eq!(details.pairings[0].opponent_version, "1.0");
        assert_eq!(details.pairings[0].pbi, 20.0);
        assert_eq!(details.history[0].rank, 2);
        assert_eq!(details.history[0].aps, 47.5);
        // Serializes in the contract's shape.
        let json = serde_json::to_value(&details).unwrap();
        assert!(json["aps"].is_number() && json["glickoRd"].is_number());
        assert!(json["pairings"][0]["opponentVersion"].is_string());
    }
}
