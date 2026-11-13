// SPDX-License-Identifier: EPL-2.0

//! The style map of a rumble (`data/v1/map/<rumble>.json`, docs/rumble-data.md): bots placed
//! so that bots with similar strengths and weaknesses are near each other. Pure: the caller
//! passes the pairings and the previous run's model in.
//!
//! **Model.** Pairing scores are fitted with a fractional logistic model of strength plus a
//! low-rank intransitive part (the skew-symmetric "disc" decomposition of the residuals):
//!
//! `logit(score_XO / 100) ≈ s_X − s_O + Σ_c (a_Xc b_Oc − b_Xc a_Oc)`
//!
//! `s` is a bot's strength; each of the [`RANK`] planes `c` is one rock-paper-scissors pattern,
//! in which a bot's `(a, b)` is its point: X does better against O than their strengths predict
//! when X is ahead of O around the plane's origin. The loss is the binomial cross-entropy
//! weighted by battles, plus an L2 penalty `LAMBDA · |U|²` on the planes. The rank and penalty
//! were chosen by cross-validation on held-out pairings (ADR 0029).
//!
//! **Distance.** Two bots' style distance is the root mean square, over all members as
//! opponents, of the difference of their fitted results once strength is taken out, in logit
//! units; [`POINTS_PER_LOGIT`] turns it into score points at an even match. It is exact and
//! does not depend on the layout.
//!
//! **Layout.** Exact t-SNE of the style profiles (perplexity [`PERPLEXITY`]), started from the
//! previous run's layout so that the map moves little from day to day. The layout keeps
//! neighbourhoods, not distances between groups; the file therefore also lists every bot's
//! nearest neighbours by style distance, and the share of map neighbours that are true
//! neighbours (trustworthiness).
//!
//! Everything is deterministic: initial values come from the bot ids, the optimisers have
//! fixed iteration rules, and there is no threading.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Intransitive planes of the model.
pub const RANK: usize = 8;
/// L2 penalty on the plane coordinates.
pub const LAMBDA: f64 = 1.0;
/// t-SNE perplexity (lower for small rumbles: at most a third of the members).
pub const PERPLEXITY: f64 = 30.0;
/// Nearest neighbours listed per bot, and used for trustworthiness.
pub const NEIGHBOURS: usize = 10;
/// Score points per logit unit at an even match (the logistic's slope at 0 is 1/4).
pub const POINTS_PER_LOGIT: f64 = 25.0;
/// Bots with fewer pairings than this share of the other members are fitted (they inform the
/// others) but not placed on the map: their style is mostly the penalty's.
pub const MIN_PAIRING_SHARE: f64 = 0.25;
/// Rumbles with fewer bots than this on the map get no map.
pub const MIN_BOTS: usize = 20;

/// Iteration limits: a cold start (no previous model) and a warm one.
const FIT_ITERATIONS: (usize, usize) = (400, 150);
const TSNE_ITERATIONS: (usize, usize) = (1000, 400);
/// t-SNE: early exaggeration of a cold start, and its iterations.
const EXAGGERATION: f64 = 12.0;
const EXAGGERATION_ITERATIONS: usize = 250;

/// The pairings of a rumble's members: `score[i * n + j]` is i's mean pairing score (0-100)
/// against j, `battles[i * n + j]` the battles (0: no pairing; symmetric).
pub struct Pairings {
    pub ids: Vec<String>,
    pub score: Vec<f64>,
    pub battles: Vec<u32>,
}

/// What a run keeps for the next one (`state/e<E>/map/<rumble>.json`), by bot id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapState {
    pub rank: usize,
    pub bots: BTreeMap<String, StateBot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateBot {
    /// Strength.
    pub s: f64,
    /// Plane coordinates `[a_1, b_1, a_2, b_2, ...]`.
    pub u: Vec<f64>,
    /// Layout position, if the bot was on the map.
    pub xy: Option<[f64; 2]>,
}

/// `data/v1/map/<rumble>.json` (`StyleMap` in `types.ts`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleMap {
    pub schema: u32,
    pub rumble: String,
    pub generated_at: String,
    pub engine_version: u32,
    /// Intransitive planes of the model.
    pub rank: usize,
    /// Share (0-1) of each bot's map neighbours that are among its true nearest neighbours,
    /// rank-weighted (trustworthiness at [`NEIGHBOURS`]).
    pub trustworthiness: f64,
    /// Members on the map.
    pub bots: Vec<MapBot>,
    /// Members too new for the map (fewer than [`MIN_PAIRING_SHARE`] of the pairings).
    pub unplaced: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapBot {
    pub id: String,
    /// Position, both in -1..1, the longer side of the layout spanning it.
    pub x: f64,
    pub y: f64,
    /// Nearest neighbours by style distance, nearest first: `[index into bots, distance in
    /// score points]`.
    pub near: Vec<(u32, f64)>,
}

/// The result of a run: the published file and the state for the next run.
pub struct MapRun {
    pub map: StyleMap,
    pub state: MapState,
    /// Optimiser iterations (fit, layout), for the logs.
    pub iterations: (usize, usize),
}

/// `data/v1/map/<rumble>.json`.
pub fn map_key(rumble: &str) -> String {
    format!("{}/map/{rumble}.json", crate::publish::DATA_PREFIX)
}

/// The previous run's model: `state/e<E>/map/<rumble>.json`.
pub fn state_key(engine: u32, rumble: &str) -> String {
    format!("state/e{engine}/map/{rumble}.json")
}

/// The pairings between `members` (bot id and ranked vid) in a division's state.
pub fn pairings(state: &crate::state::State, members: &[(String, u32)]) -> Pairings {
    let n = members.len();
    let mut score = vec![0.0; n * n];
    let mut battles = vec![0; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let (vi, vj) = (members[i].1, members[j].1);
            let Some(p) = state.pairing(vi, vj).filter(|p| p.n > 0) else {
                continue;
            };
            // The state keeps the lower vid's side.
            let low = p.s / f64::from(p.n);
            let mean = if vi < vj { low } else { 100.0 - low };
            score[i * n + j] = mean;
            score[j * n + i] = 100.0 - mean;
            battles[i * n + j] = p.n;
            battles[j * n + i] = p.n;
        }
    }
    Pairings {
        ids: members.iter().map(|m| m.0.clone()).collect(),
        score,
        battles,
    }
}

/// Builds the style map of a rumble.`None` when too few members have enough pairings.
pub fn compute(
    pairings: &Pairings,
    previous: Option<&MapState>,
    rumble: &str,
    generated_at: &str,
    engine_version: u32,
) -> Option<MapRun> {
    let n = pairings.ids.len();
    let previous = previous.filter(|p| p.rank == RANK);
    let (s, u, fit_iterations) = fit(pairings, previous);

    // Members on the map.
    let need = ((n.saturating_sub(1)) as f64 * MIN_PAIRING_SHARE).ceil() as usize;
    let mut placed = Vec::new();
    let mut unplaced = Vec::new();
    for i in 0..n {
        let count = (0..n).filter(|&j| pairings.battles[i * n + j] > 0).count();
        if count >= need {
            placed.push(i);
        } else {
            unplaced.push(pairings.ids[i].clone());
        }
    }
    if placed.len() < MIN_BOTS {
        return None;
    }

    let z = profiles(&u, n);
    let m = placed.len();
    let dim = 2 * RANK;
    let zp: Vec<f64> = placed
        .iter()
        .flat_map(|&i| z[i * dim..(i + 1) * dim].iter().copied())
        .collect();
    let d2 = squared_distances(&zp, m, dim);

    // Layout: previous positions, new bots at their known neighbours' mean.
    let start = previous.and_then(|p| warm_layout(p, pairings, &placed, &d2));
    let warm = start.is_some();
    let start = start.unwrap_or_else(|| pca_layout(&zp, m, dim));
    let perplexity = PERPLEXITY.min((m - 1) as f64 / 3.0);
    let (y, tsne_iterations) = tsne(&d2, m, start, perplexity, warm);

    let trust = trustworthiness(&d2, &squared_distances(&y, m, 2), m, NEIGHBOURS);
    let near = nearest(&d2, m, NEIGHBOURS);
    let xy = normalized(&y, m);

    let mut state = MapState {
        rank: RANK,
        bots: BTreeMap::new(),
    };
    for i in 0..n {
        state.bots.insert(
            pairings.ids[i].clone(),
            StateBot {
                s: s[i],
                u: u[i * dim..(i + 1) * dim].to_vec(),
                xy: None,
            },
        );
    }
    let mut bots = Vec::with_capacity(m);
    for (k, &i) in placed.iter().enumerate() {
        if let Some(b) = state.bots.get_mut(&pairings.ids[i]) {
            b.xy = Some([y[2 * k], y[2 * k + 1]]);
        }
        bots.push(MapBot {
            id: pairings.ids[i].clone(),
            x: round(xy[2 * k], 4),
            y: round(xy[2 * k + 1], 4),
            near: near[k]
                .iter()
                .map(|&j| (j as u32, round(d2[k * m + j].sqrt() * POINTS_PER_LOGIT, 2)))
                .collect(),
        });
    }
    Some(MapRun {
        map: StyleMap {
            schema: crate::data::SCHEMA_VERSION,
            rumble: rumble.to_string(),
            generated_at: generated_at.to_string(),
            engine_version,
            rank: RANK,
            trustworthiness: round(trust, 4),
            bots,
            unplaced,
        },
        state,
        iterations: (fit_iterations, tsne_iterations),
    })
}

fn round(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

// ---------------------------------------------------------------------------------------------
// The model fit.

/// An observed pairing, i < j: i's score as a fraction, and the battles as weight.
struct Obs {
    i: u32,
    j: u32,
    p: f64,
    w: f64,
}

fn observations(pairings: &Pairings) -> Vec<Obs> {
    let n = pairings.ids.len();
    let mut obs = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let w = pairings.battles[i * n + j];
            if w > 0 {
                obs.push(Obs {
                    i: i as u32,
                    j: j as u32,
                    p: (pairings.score[i * n + j] / 100.0).clamp(0.0, 1.0),
                    w: f64::from(w),
                });
            }
        }
    }
    obs
}

/// A deterministic pseudo-random sequence from a bot id (FNV-1a, then SplitMix64): small
/// starting plane coordinates (0 is a saddle point of the loss).
fn seeded(id: &str, count: usize) -> Vec<f64> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..count)
        .map(|_| {
            h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = h;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            ((z >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.02
        })
        .collect()
}

/// The loss and its gradient at `x = [s (n), U (n × 2·RANK)]`.
fn loss(obs: &[Obs], n: usize, x: &[f64], grad: &mut [f64]) -> f64 {
    let dim = 2 * RANK;
    let (s, u) = x.split_at(n);
    grad.fill(0.0);
    let (gs, gu) = grad.split_at_mut(n);
    let mut total = 0.0;
    for o in obs {
        let (i, j) = (o.i as usize, o.j as usize);
        let (ui, uj) = (&u[i * dim..(i + 1) * dim], &u[j * dim..(j + 1) * dim]);
        let mut eta = s[i] - s[j];
        for c in 0..RANK {
            eta += ui[2 * c] * uj[2 * c + 1] - ui[2 * c + 1] * uj[2 * c];
        }
        // Cross-entropy of p against sigmoid(eta), stable for large |eta|.
        let softplus = |t: f64| {
            if t > 0.0 {
                t + (-t).exp().ln_1p()
            } else {
                t.exp().ln_1p()
            }
        };
        total += o.w * (o.p * softplus(-eta) + (1.0 - o.p) * softplus(eta));
        let q = 1.0 / (1.0 + (-eta).exp());
        let g = o.w * (q - o.p);
        gs[i] += g;
        gs[j] -= g;
        for c in 0..RANK {
            let (ai, bi, aj, bj) = (ui[2 * c], ui[2 * c + 1], uj[2 * c], uj[2 * c + 1]);
            gu[i * dim + 2 * c] += g * bj;
            gu[i * dim + 2 * c + 1] -= g * aj;
            gu[j * dim + 2 * c] -= g * bi;
            gu[j * dim + 2 * c + 1] += g * ai;
        }
    }
    for (k, &v) in u.iter().enumerate() {
        total += LAMBDA * v * v;
        gu[k] += 2.0 * LAMBDA * v;
    }
    // Strength is defined up to a constant: pin its mean to 0.
    let mean: f64 = s.iter().sum::<f64>();
    total += 1e-6 * mean * mean;
    for g in gs.iter_mut() {
        *g += 2e-6 * mean;
    }
    total
}

/// Fits the model; returns strengths, plane coordinates (n × 2·RANK) and iterations.
fn fit(pairings: &Pairings, previous: Option<&MapState>) -> (Vec<f64>, Vec<f64>, usize) {
    let n = pairings.ids.len();
    let dim = 2 * RANK;
    let obs = observations(pairings);
    let mut x = vec![0.0; n + n * dim];
    let mut known = 0;
    for (i, id) in pairings.ids.iter().enumerate() {
        match previous
            .and_then(|p| p.bots.get(id))
            .filter(|b| b.u.len() == dim)
        {
            Some(b) => {
                x[i] = b.s;
                x[n + i * dim..n + (i + 1) * dim].copy_from_slice(&b.u);
                known += 1;
            }
            None => x[n + i * dim..n + (i + 1) * dim].copy_from_slice(&seeded(id, dim)),
        }
    }
    let warm = known * 2 > n;
    let max = if warm {
        FIT_ITERATIONS.1
    } else {
        FIT_ITERATIONS.0
    };
    let iterations = lbfgs(&mut x, max, |x, g| loss(&obs, n, x, g));
    let u = x.split_off(n);
    (x, u, iterations)
}

/// Limited-memory BFGS (memory 10) with a backtracking Armijo line search. Stops after `max`
/// iterations, or when an iteration lowers the loss by less than 1e-10 of it.
fn lbfgs(x: &mut [f64], max: usize, mut f: impl FnMut(&[f64], &mut [f64]) -> f64) -> usize {
    const MEMORY: usize = 10;
    let len = x.len();
    let mut g = vec![0.0; len];
    let mut fx = f(x, &mut g);
    let mut ss: Vec<Vec<f64>> = Vec::new();
    let mut ys: Vec<Vec<f64>> = Vec::new();
    let mut xn = vec![0.0; len];
    let mut gn = vec![0.0; len];
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(p, q)| p * q).sum::<f64>();
    for it in 0..max {
        // Two-loop recursion: d = -H g.
        let mut d: Vec<f64> = g.iter().map(|v| -v).collect();
        let mut alpha = vec![0.0; ss.len()];
        for k in (0..ss.len()).rev() {
            alpha[k] = dot(&ss[k], &d) / dot(&ys[k], &ss[k]);
            for (dv, yv) in d.iter_mut().zip(&ys[k]) {
                *dv -= alpha[k] * yv;
            }
        }
        let gamma = match (ss.last(), ys.last()) {
            (Some(s), Some(y)) => dot(s, y) / dot(y, y),
            _ => 1.0 / dot(&g, &g).sqrt().max(1e-12),
        };
        for v in &mut d {
            *v *= gamma;
        }
        for k in 0..ss.len() {
            let beta = dot(&ys[k], &d) / dot(&ys[k], &ss[k]);
            for (dv, sv) in d.iter_mut().zip(&ss[k]) {
                *dv += (alpha[k] - beta) * sv;
            }
        }
        let mut slope = dot(&g, &d);
        if slope >= 0.0 {
            // Not a descent direction: restart from steepest descent.
            ss.clear();
            ys.clear();
            for (dv, gv) in d.iter_mut().zip(&g) {
                *dv = -gv / dot(&g, &g).sqrt().max(1e-12);
            }
            slope = dot(&g, &d);
        }
        let mut step = 1.0;
        let mut fnew;
        loop {
            for k in 0..len {
                xn[k] = x[k] + step * d[k];
            }
            fnew = f(&xn, &mut gn);
            if fnew <= fx + 1e-4 * step * slope || step < 1e-10 {
                break;
            }
            step *= 0.5;
        }
        if fnew > fx {
            return it;
        }
        let s: Vec<f64> = (0..len).map(|k| xn[k] - x[k]).collect();
        let y: Vec<f64> = (0..len).map(|k| gn[k] - g[k]).collect();
        if dot(&s, &y) > 1e-12 {
            if ss.len() == MEMORY {
                ss.remove(0);
                ys.remove(0);
            }
            ss.push(s);
            ys.push(y);
        }
        x.copy_from_slice(&xn);
        g.copy_from_slice(&gn);
        let done = fx - fnew < 1e-10 * fx.abs();
        fx = fnew;
        if done {
            return it + 1;
        }
    }
    max
}

// ---------------------------------------------------------------------------------------------
// Style profiles.

/// Coordinates (n × 2·RANK) whose Euclidean distances are the style distances in logits:
/// `d²(i, j) = (u_i − u_j)ᵀ C (u_i − u_j)` with `C = J (UᵀU / n) Jᵀ`, so `z_i = u_i V √W` for
/// `C = V W Vᵀ`.
fn profiles(u: &[f64], n: usize) -> Vec<f64> {
    let dim = 2 * RANK;
    // G = UᵀU / n.
    let mut gram = vec![0.0; dim * dim];
    for i in 0..n {
        let row = &u[i * dim..(i + 1) * dim];
        for a in 0..dim {
            for b in 0..dim {
                gram[a * dim + b] += row[a] * row[b];
            }
        }
    }
    for v in &mut gram {
        *v /= n as f64;
    }
    // (J v)_{2c} = v_{2c+1}, (J v)_{2c+1} = -v_{2c}: C = J G Jᵀ.
    let jidx = |k: usize| {
        if k % 2 == 0 {
            (k + 1, 1.0)
        } else {
            (k - 1, -1.0)
        }
    };
    let mut c = vec![0.0; dim * dim];
    for a in 0..dim {
        for b in 0..dim {
            let ((ka, sa), (kb, sb)) = (jidx(a), jidx(b));
            c[a * dim + b] = sa * sb * gram[ka * dim + kb];
        }
    }
    let (w, v) = jacobi_eigen(c, dim);
    let mut z = vec![0.0; n * dim];
    for i in 0..n {
        for k in 0..dim {
            let scale = w[k].max(0.0).sqrt();
            let mut acc = 0.0;
            for a in 0..dim {
                acc += u[i * dim + a] * v[a * dim + k];
            }
            z[i * dim + k] = acc * scale;
        }
    }
    z
}

/// Eigenvalues and eigenvectors (columns of the row-major result) of a symmetric matrix, by
/// cyclic Jacobi rotations; eigenvalues in descending order.
fn jacobi_eigen(mut a: Vec<f64>, n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![0.0; n * n];
    for k in 0..n {
        v[k * n + k] = 1.0;
    }
    for _sweep in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|p| (0..n).filter(move |&q| q != p).map(move |q| (p, q)))
            .map(|(p, q)| a[p * n + q] * a[p * n + q])
            .sum();
        let scale: f64 = (0..n).map(|p| a[p * n + p] * a[p * n + p]).sum();
        if off <= 1e-30 * scale.max(1e-300) {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let sign = if theta >= 0.0 { 1.0 } else { -1.0 };
                let t = sign / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k * n + p], a[k * n + q]);
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p * n + k], a[q * n + k]);
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| a[y * n + y].total_cmp(&a[x * n + x]));
    let w = order.iter().map(|&k| a[k * n + k]).collect();
    let mut vs = vec![0.0; n * n];
    for (col, &k) in order.iter().enumerate() {
        for r in 0..n {
            vs[r * n + col] = v[r * n + k];
        }
    }
    (w, vs)
}

fn squared_distances(z: &[f64], m: usize, dim: usize) -> Vec<f64> {
    let mut d = vec![0.0; m * m];
    for i in 0..m {
        for j in i + 1..m {
            let mut acc = 0.0;
            for k in 0..dim {
                let t = z[i * dim + k] - z[j * dim + k];
                acc += t * t;
            }
            d[i * m + j] = acc;
            d[j * m + i] = acc;
        }
    }
    d
}

/// Each point's `k` nearest others (ties by index).
fn nearest(d2: &[f64], m: usize, k: usize) -> Vec<Vec<usize>> {
    (0..m)
        .map(|i| {
            let mut idx: Vec<usize> = (0..m).filter(|&j| j != i).collect();
            idx.sort_by(|&a, &b| d2[i * m + a].total_cmp(&d2[i * m + b]).then(a.cmp(&b)));
            idx.truncate(k);
            idx
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Layout.

/// The first two principal components, scaled to a standard deviation of 1e-4 (the usual
/// t-SNE start). Each component's sign is fixed so that its largest coordinate is positive.
fn pca_layout(z: &[f64], m: usize, dim: usize) -> Vec<f64> {
    let mut mean = vec![0.0; dim];
    for i in 0..m {
        for k in 0..dim {
            mean[k] += z[i * dim + k] / m as f64;
        }
    }
    let mut cov = vec![0.0; dim * dim];
    for i in 0..m {
        for a in 0..dim {
            for b in 0..dim {
                cov[a * dim + b] += (z[i * dim + a] - mean[a]) * (z[i * dim + b] - mean[b]);
            }
        }
    }
    let (_, v) = jacobi_eigen(cov, dim);
    let mut y = vec![0.0; 2 * m];
    for c in 0..2 {
        let mut col: Vec<f64> = (0..m)
            .map(|i| {
                (0..dim)
                    .map(|k| (z[i * dim + k] - mean[k]) * v[k * dim + c])
                    .sum()
            })
            .collect();
        let big = col
            .iter()
            .copied()
            .fold(0.0f64, |acc, t| if t.abs() > acc.abs() { t } else { acc });
        let sd = (col.iter().map(|t| t * t).sum::<f64>() / m as f64)
            .sqrt()
            .max(1e-300);
        let sign = if big < 0.0 { -1.0 } else { 1.0 };
        for t in &mut col {
            *t *= sign * 1e-4 / sd;
        }
        for i in 0..m {
            y[2 * i + c] = col[i];
        }
    }
    y
}

/// The previous layout for the bots that were on it; a new bot starts at the mean of its three
/// nearest neighbours that were. `None` if fewer than half the bots were on it.
fn warm_layout(
    previous: &MapState,
    pairings: &Pairings,
    placed: &[usize],
    d2: &[f64],
) -> Option<Vec<f64>> {
    let m = placed.len();
    let old: Vec<Option<[f64; 2]>> = placed
        .iter()
        .map(|&i| previous.bots.get(&pairings.ids[i]).and_then(|b| b.xy))
        .collect();
    if old.iter().filter(|p| p.is_some()).count() * 2 < m {
        return None;
    }
    let mut y = vec![0.0; 2 * m];
    for k in 0..m {
        let p = match old[k] {
            Some(p) => p,
            None => {
                let mut known: Vec<usize> = (0..m).filter(|&j| old[j].is_some()).collect();
                known.sort_by(|&a, &b| d2[k * m + a].total_cmp(&d2[k * m + b]).then(a.cmp(&b)));
                known.truncate(3);
                let mut p = [0.0; 2];
                for &j in &known {
                    let q = old[j].unwrap_or_default();
                    p[0] += q[0] / known.len() as f64;
                    p[1] += q[1] / known.len() as f64;
                }
                // Apart from its neighbours, deterministically.
                p[0] += 1e-3 * (k as f64 * 0.618_034).fract();
                p[1] += 1e-3 * (k as f64 * 0.381_966).fract();
                p
            }
        };
        y[2 * k] = p[0];
        y[2 * k + 1] = p[1];
    }
    Some(y)
}

/// Exact t-SNE (van der Maaten & Hinton 2008) from squared distances, with the usual gains
/// and momentum. A cold start runs early exaggeration first; a warm start does not.
fn tsne(d2: &[f64], m: usize, mut y: Vec<f64>, perplexity: f64, warm: bool) -> (Vec<f64>, usize) {
    let p = affinities(d2, m, perplexity);
    let iterations = if warm {
        TSNE_ITERATIONS.1
    } else {
        TSNE_ITERATIONS.0
    };
    let rate = (m as f64 / EXAGGERATION / 4.0).max(50.0);
    let mut velocity = vec![0.0; 2 * m];
    let mut gains = vec![1.0; 2 * m];
    let mut num = vec![0.0; m * m];
    let mut grad = vec![0.0; 2 * m];
    for it in 0..iterations {
        let exaggeration = if !warm && it < EXAGGERATION_ITERATIONS {
            EXAGGERATION
        } else {
            1.0
        };
        let momentum = if !warm && it < EXAGGERATION_ITERATIONS {
            0.5
        } else {
            0.8
        };
        let mut z = 0.0;
        for i in 0..m {
            for j in i + 1..m {
                let dx = y[2 * i] - y[2 * j];
                let dy = y[2 * i + 1] - y[2 * j + 1];
                let q = 1.0 / (1.0 + dx * dx + dy * dy);
                num[i * m + j] = q;
                num[j * m + i] = q;
                z += 2.0 * q;
            }
        }
        grad.fill(0.0);
        for i in 0..m {
            let (mut gx, mut gy) = (0.0, 0.0);
            for j in 0..m {
                if i == j {
                    continue;
                }
                let q = num[i * m + j];
                let f = (exaggeration * p[i * m + j] - q / z) * q;
                gx += f * (y[2 * i] - y[2 * j]);
                gy += f * (y[2 * i + 1] - y[2 * j + 1]);
            }
            grad[2 * i] = 4.0 * gx;
            grad[2 * i + 1] = 4.0 * gy;
        }
        for k in 0..2 * m {
            gains[k] = if (grad[k] > 0.0) != (velocity[k] > 0.0) {
                gains[k] + 0.2
            } else {
                (gains[k] * 0.8_f64).max(0.01)
            };
            velocity[k] = momentum * velocity[k] - rate * gains[k] * grad[k];
            y[k] += velocity[k];
        }
        // Keep the layout centred.
        for c in 0..2 {
            let mean = (0..m).map(|i| y[2 * i + c]).sum::<f64>() / m as f64;
            for i in 0..m {
                y[2 * i + c] -= mean;
            }
        }
    }
    (y, iterations)
}

/// Symmetric t-SNE affinities: per point a Gaussian whose entropy matches the perplexity
/// (bisection on the precision), symmetrized and normalized.
fn affinities(d2: &[f64], m: usize, perplexity: f64) -> Vec<f64> {
    let target = perplexity.ln();
    let mut p = vec![0.0; m * m];
    let mut row = vec![0.0; m];
    for i in 0..m {
        // Distances relative to the nearest, for range.
        let (mut lo, mut hi) = (0.0f64, f64::INFINITY);
        let mut beta = 1.0;
        let scale = {
            let mut ds: Vec<f64> = (0..m).filter(|&j| j != i).map(|j| d2[i * m + j]).collect();
            ds.sort_by(f64::total_cmp);
            ds[ds.len() / 2].max(1e-12)
        };
        for _ in 0..100 {
            let mut sum = 0.0;
            for j in 0..m {
                row[j] = if j == i {
                    0.0
                } else {
                    (-beta * d2[i * m + j] / scale).exp()
                };
                sum += row[j];
            }
            let sum = sum.max(1e-300);
            let mut h = 0.0;
            for &v in row.iter().filter(|&&v| v > 0.0) {
                let pj = v / sum;
                h -= pj * pj.ln();
            }
            for v in row.iter_mut() {
                *v /= sum;
            }
            if (h - target).abs() < 1e-5 {
                break;
            }
            if h > target {
                lo = beta;
                beta = if hi.is_finite() {
                    (beta + hi) / 2.0
                } else {
                    beta * 2.0
                };
            } else {
                hi = beta;
                beta = (beta + lo) / 2.0;
            }
        }
        p[i * m..(i + 1) * m].copy_from_slice(&row);
    }
    let mut s = vec![0.0; m * m];
    for i in 0..m {
        for j in 0..m {
            s[i * m + j] = ((p[i * m + j] + p[j * m + i]) / (2.0 * m as f64)).max(1e-12);
        }
    }
    s
}

/// Trustworthiness (Venna & Kaski 2001) of a layout at `k` neighbours: 1 when every point's
/// `k` map neighbours are its `k` true neighbours; lower as map neighbours rank further away.
fn trustworthiness(high: &[f64], low: &[f64], m: usize, k: usize) -> f64 {
    let mut penalty = 0.0;
    for i in 0..m {
        let mut by_high: Vec<usize> = (0..m).filter(|&j| j != i).collect();
        by_high.sort_by(|&a, &b| high[i * m + a].total_cmp(&high[i * m + b]).then(a.cmp(&b)));
        let mut rank = vec![0usize; m];
        for (r, &j) in by_high.iter().enumerate() {
            rank[j] = r + 1;
        }
        let mut by_low: Vec<usize> = (0..m).filter(|&j| j != i).collect();
        by_low.sort_by(|&a, &b| low[i * m + a].total_cmp(&low[i * m + b]).then(a.cmp(&b)));
        for &j in by_low.iter().take(k) {
            if rank[j] > k {
                penalty += (rank[j] - k) as f64;
            }
        }
    }
    let (m, k) = (m as f64, k as f64);
    1.0 - 2.0 / (m * k * (2.0 * m - 3.0 * k - 1.0)) * penalty
}

/// Centres the layout and scales it so that its longer side spans -1..1.
fn normalized(y: &[f64], m: usize) -> Vec<f64> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for i in 0..m {
        for c in 0..2 {
            lo[c] = lo[c].min(y[2 * i + c]);
            hi[c] = hi[c].max(y[2 * i + c]);
        }
    }
    let half = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 2.0).max(1e-300);
    let mid = [(hi[0] + lo[0]) / 2.0, (hi[1] + lo[1]) / 2.0];
    (0..2 * m).map(|k| (y[k] - mid[k % 2]) / half).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rumble of `n` bots: strengths spread evenly and three styles in a rock-paper-scissors
    /// cycle, each beating the next by up to `edge` logits more than strength predicts.
    fn synthetic(n: usize, edge: f64) -> Pairings {
        let ids: Vec<String> = (0..n).map(|i| format!("t.Bot{i:03}")).collect();
        let mut score = vec![0.0; n * n];
        let mut battles = vec![0; n * n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let s = |k: usize| k as f64 / n as f64 * 4.0;
                let angle = |k: usize| (k % 3) as f64 * std::f64::consts::TAU / 3.0;
                let eta = s(i) - s(j) + edge * (angle(i) - angle(j)).sin();
                score[i * n + j] = 100.0 / (1.0 + (-eta).exp());
                battles[i * n + j] = 1;
            }
        }
        Pairings {
            ids,
            score,
            battles,
        }
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let p = synthetic(12, 0.8);
        let obs = observations(&p);
        let n = 12;
        let mut x: Vec<f64> = (0..n).map(|i| i as f64 * 0.1).collect();
        for i in 0..n {
            x.extend(seeded(&p.ids[i], 2 * RANK).iter().map(|v| v * 30.0));
        }
        let mut g = vec![0.0; x.len()];
        loss(&obs, n, &x, &mut g);
        let mut scratch = vec![0.0; x.len()];
        for k in [0, 5, n, n + 3, n + 2 * RANK * 7 + 1, x.len() - 1] {
            let h = 1e-6;
            let mut xp = x.clone();
            xp[k] += h;
            let mut xm = x.clone();
            xm[k] -= h;
            let fd =
                (loss(&obs, n, &xp, &mut scratch) - loss(&obs, n, &xm, &mut scratch)) / (2.0 * h);
            assert!(
                (fd - g[k]).abs() < 1e-5 * (1.0 + fd.abs()),
                "{k}: {fd} vs {}",
                g[k]
            );
        }
    }

    #[test]
    fn eigen_decomposes() {
        let a = vec![4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 1.0];
        let (w, v) = jacobi_eigen(a.clone(), 3);
        assert!(w[0] >= w[1] && w[1] >= w[2]);
        for c in 0..3 {
            for r in 0..3 {
                let av: f64 = (0..3).map(|k| a[r * 3 + k] * v[k * 3 + c]).sum();
                assert!((av - w[c] * v[r * 3 + c]).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn styles_separate_and_runs_are_deterministic() {
        let p = synthetic(40, 1.5);
        let a = compute(&p, None, "1v1", "2026-11-13T00:00:00Z", 1).expect("a map");
        let b = compute(&p, None, "1v1", "2026-11-13T00:00:00Z", 1).expect("a map");
        assert_eq!(a.map, b.map);
        assert_eq!(a.map.bots.len(), 40);
        // Every bot's nearest neighbours share its style.
        for (k, bot) in a.map.bots.iter().enumerate() {
            for &(j, _) in bot.near.iter().take(5) {
                assert_eq!(j as usize % 3, k % 3, "{}", bot.id);
            }
        }
        assert!(a.map.trustworthiness > 0.8, "{}", a.map.trustworthiness);
        // A warm start from the previous state keeps the styles where they were (bots of one
        // style are interchangeable here, so only the groups' centres are fixed).
        let c = compute(&p, Some(&a.state), "1v1", "2026-11-14T00:00:00Z", 1).expect("a map");
        let centre = |map: &StyleMap, style: usize| {
            let group: Vec<&MapBot> = map.bots.iter().skip(style).step_by(3).collect();
            let len = group.len() as f64;
            (
                group.iter().map(|b| b.x).sum::<f64>() / len,
                group.iter().map(|b| b.y).sum::<f64>() / len,
            )
        };
        for style in 0..3 {
            let ((x0, y0), (x1, y1)) = (centre(&a.map, style), centre(&c.map, style));
            let moved = ((x0 - x1).powi(2) + (y0 - y1).powi(2)).sqrt();
            assert!(moved < 0.25, "style {style} moved {moved}");
        }
    }

    #[test]
    fn bots_with_few_pairings_are_not_placed() {
        let mut p = synthetic(30, 1.0);
        let n = 30;
        for j in 1..n {
            p.battles[j] = 0;
            p.battles[j * n] = 0;
        }
        let run = compute(&p, None, "1v1", "t", 1).expect("a map");
        assert_eq!(run.map.unplaced, vec!["t.Bot000".to_string()]);
        assert_eq!(run.map.bots.len(), 29);
        assert!(compute(&synthetic(10, 1.0), None, "1v1", "t", 1).is_none());
    }

    /// The production-sized benchmark: `ROBOCODES_MAP_MATRIX=<file>` (u32 n, n × (u16 len,
    /// id), n² f64 scores, n² u32 battles), `cargo test --release -p robocode-s-rumble-core
    /// map::tests::real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real() {
        let Ok(path) = std::env::var("ROBOCODES_MAP_MATRIX") else {
            return;
        };
        let bytes = std::fs::read(path).expect("matrix file");
        let mut at = 0;
        let mut take = |k: usize| {
            let s = &bytes[at..at + k];
            at += k;
            s
        };
        let n = u32::from_le_bytes(take(4).try_into().unwrap()) as usize;
        let ids: Vec<String> = (0..n)
            .map(|_| {
                let len = u16::from_le_bytes(take(2).try_into().unwrap()) as usize;
                String::from_utf8(take(len).to_vec()).unwrap()
            })
            .collect();
        let score = (0..n * n)
            .map(|_| f64::from_le_bytes(take(8).try_into().unwrap()))
            .collect();
        let battles = (0..n * n)
            .map(|_| u32::from_le_bytes(take(4).try_into().unwrap()))
            .collect();
        let p = Pairings {
            ids,
            score,
            battles,
        };
        let t = std::time::Instant::now();
        let a = compute(&p, None, "1v1", "t", 1).expect("a map");
        println!(
            "cold: {:?}, iterations {:?}, trust {}, placed {}",
            t.elapsed(),
            a.iterations,
            a.map.trustworthiness,
            a.map.bots.len()
        );
        let t = std::time::Instant::now();
        let b = compute(&p, Some(&a.state), "1v1", "t", 1).expect("a map");
        println!(
            "warm: {:?}, iterations {:?}, trust {}",
            t.elapsed(),
            b.iterations,
            b.map.trustworthiness
        );
        let moved = a
            .map
            .bots
            .iter()
            .zip(&b.map.bots)
            .map(|(p, q)| ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt())
            .collect::<Vec<f64>>();
        let mut sorted = moved.clone();
        sorted.sort_by(f64::total_cmp);
        println!(
            "warm moved: mean {}, median {}, 95% {}",
            moved.iter().sum::<f64>() / moved.len() as f64,
            sorted[sorted.len() / 2],
            sorted[sorted.len() * 95 / 100]
        );
        if let Ok(out) = std::env::var("ROBOCODES_MAP_OUT") {
            std::fs::write(out, serde_json::to_vec(&b.map).unwrap()).unwrap();
        }
    }
}
