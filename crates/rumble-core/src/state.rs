// SPDX-License-Identifier: EPL-2.0

//! The pairing state of one (engine, division) and its folding (spec §10.2, §10.5): per
//! pairing the battle count and sums of the pairing score and survival, per version the battle
//! and failure tallies, and the `pairings.bin` file. Pure: bytes in, bytes out.

use crate::config::Config;
use crate::ids::Division;
use crate::record::{Record, division_code, division_from_code, pairing_score, pairing_survival};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};

const MAGIC: &[u8; 4] = b"RSPS";
/// Version 2 added the reporters of each version's failures; version 1 files are still read.
const FILE_VERSION: u16 = 2;
/// Bytes per pairing in the file: `u32 a, u32 b, u32 n, f64 s, f64 q, f64 v, u16 day`.
const PAIRING_BYTES: usize = 38;
/// Bytes per version in the file: version 1 `u32 vid, battles, failed, last20`; version 2 adds
/// `u8 reporter count, 3 × u32 reporter hash`.
const VERSION_BYTES_V1: usize = 16;
const VERSION_BYTES: usize = 29;
/// Reporters kept per version: enough for the rule "at least 3 distinct users" (§12.3).
pub const MAX_REPORTERS: usize = 3;
/// A successful battle that leaves fewer failures than this in the last 20 forgets the
/// reporters: the failure share is then below any sensible `probation.failShare`.
const RESET_FAILURES: u32 = 10;
const LAST_20: u32 = (1 << 20) - 1;

/// The sums of one pairing, from the side of A, the lower vid (§10.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pairing {
    /// Battles.
    pub n: u32,
    /// Sum of A's pairing scores.
    pub s: f64,
    /// Sum of their squares.
    pub q: f64,
    /// Sum of A's survival.
    pub v: f64,
    /// Day (caller's unit, e.g. days since the epoch) of the last battle.
    pub day: u16,
}

/// The statistics derived from a pairing (§10.2), from A's side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairingStats {
    pub n: u32,
    /// `s / n`, 0 for n = 0.
    pub mean: f64,
    /// Sample variance `(q - n m²) / (n - 1)`, for n >= 2.
    pub variance: Option<f64>,
    /// `(k0 σ0² + (n-1) v̂) / (k0 + n - 1)`; `σ0²` for n < 2.
    pub shrunk_variance: f64,
    /// `σ / sqrt(max(n, 1))`.
    pub se: f64,
    /// `1.96 · SE`.
    pub ci95: f64,
    /// Mean survival of A, 0 for n = 0.
    pub survival: f64,
}

impl Pairing {
    /// Derived statistics with the prior `σ0²` and weight `k0` (config `plan.priorVariance`
    /// for the division and `plan.priorWeight`).
    pub fn stats(&self, prior_variance: f64, prior_weight: f64) -> PairingStats {
        let n = f64::from(self.n);
        let mean = if self.n > 0 { self.s / n } else { 0.0 };
        let variance = (self.n >= 2).then(|| ((self.q - n * mean * mean) / (n - 1.0)).max(0.0));
        let shrunk_variance = match variance {
            Some(v) => (prior_weight * prior_variance + (n - 1.0) * v) / (prior_weight + n - 1.0),
            None => prior_variance,
        };
        let se = (shrunk_variance / n.max(1.0)).sqrt();
        PairingStats {
            n: self.n,
            mean,
            variance,
            shrunk_variance,
            se,
            ci95: 1.96 * se,
            survival: if self.n > 0 { self.v / n } else { 0.0 },
        }
    }
}

/// The tallies of one version in this division.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VersionTally {
    /// Counted battles, including the ones it failed (trusted reports only).
    pub battles: u32,
    /// Trusted failure reports.
    pub failed: u32,
    /// The outcome of its last 20 battles, bit 0 the newest, 1 = failed.
    pub last20: u32,
    /// Hashes ([`reporter_hash`]) of up to 3 distinct users who reported recent trusted failures.
    pub reporters: [u32; MAX_REPORTERS],
    pub reporter_count: u8,
}

/// The 32-bit hash of a reporter's uid kept in the failure tallies.
pub fn reporter_hash(uid: &str) -> u32 {
    let digest = Sha256::digest(uid.as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

impl VersionTally {
    /// Distinct reporters of recent failures, at most 3.
    pub fn distinct_reporters(&self) -> u32 {
        u32::from(self.reporter_count)
    }

    fn add_reporter(&mut self, hash: u32) {
        let count = self.reporter_count as usize;
        if count < MAX_REPORTERS && !self.reporters[..count].contains(&hash) {
            self.reporters[count] = hash;
            self.reporter_count += 1;
        }
    }

    /// Failures among the last 20 battles.
    pub fn recent_failures(&self) -> u32 {
        (self.last20 & LAST_20).count_ones()
    }

    /// How many of the last 20 outcomes exist.
    pub fn recent_battles(&self) -> u32 {
        self.battles.min(20)
    }

    fn push(&mut self, failed: bool) {
        self.last20 = ((self.last20 << 1) | u32::from(failed)) & LAST_20;
        if !failed && self.recent_failures() < RESET_FAILURES {
            self.reporters = [0; MAX_REPORTERS];
            self.reporter_count = 0;
        }
    }
}

/// The pairing state of one engine and division.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub engine: u32,
    pub division: Division,
    /// The last hour folded, `YYYYMMDDHH`; the state file is its own cursor (§10.5).
    pub done: u32,
    /// Counted battles of the division.
    pub battles: u64,
    pub versions: BTreeMap<u32, VersionTally>,
    pairings: HashMap<(u32, u32), Pairing>,
}

impl State {
    pub fn new(engine: u32, division: Division, done: u32) -> State {
        State {
            engine,
            division,
            done,
            battles: 0,
            versions: BTreeMap::new(),
            pairings: HashMap::new(),
        }
    }

    /// The pairing of two versions, in either order, with A the lower vid.
    pub fn pairing(&self, a: u32, b: u32) -> Option<&Pairing> {
        self.pairings.get(&(a.min(b), a.max(b)))
    }

    /// The derived statistics of a pairing (`None` if no battle), from the lower vid's side.
    pub fn pairing_stats(&self, a: u32, b: u32, config: &Config) -> Option<PairingStats> {
        let prior = *config.plan.prior_variance.get(self.division);
        self.pairing(a, b)
            .map(|p| p.stats(prior, config.plan.prior_weight))
    }

    pub fn pairing_count(&self) -> usize {
        self.pairings.len()
    }

    /// All pairings in unspecified order.
    pub fn pairings(&self) -> impl Iterator<Item = ((u32, u32), &Pairing)> {
        self.pairings.iter().map(|(&key, pairing)| (key, pairing))
    }

    /// All pairings sorted by (a, b).
    pub fn sorted_pairings(&self) -> Vec<((u32, u32), Pairing)> {
        let mut all: Vec<_> = self.pairings.iter().map(|(&k, &p)| (k, p)).collect();
        all.sort_unstable_by_key(|&(key, _)| key);
        all
    }

    /// Folds one record dated `day` (§10.5). Records of another division are ignored. A counted
    /// (not failed) record updates every pair of contestants with its weight, the division's
    /// battle count and each contestant's battle count and failure window. A failed record
    /// updates the failed version's tallies only if its reporter was trusted (§12.3). A
    /// negative weight undoes the counts but cannot undo the failure window's bits.
    pub fn fold(&mut self, record: &Record, day: u16) {
        self.fold_from(record, day, &record.uid);
    }

    /// [`State::fold`] with an explicit reporter uid (the log item's uid): a trusted failure
    /// adds the reporter to the version's distinct reporters (§12.3).
    pub fn fold_from(&mut self, record: &Record, day: u16, reporter: &str) {
        if record.division != self.division {
            return;
        }
        let w = i64::from(record.weight);
        match record.failed {
            Some((robot, _)) => {
                if !record.trusted {
                    return;
                }
                let Some(entry) = record.contestants.get(robot as usize) else {
                    return;
                };
                let tally = self.versions.entry(entry.vid).or_default();
                tally.battles = add(tally.battles, w);
                tally.failed = add(tally.failed, w);
                if w > 0 {
                    tally.push(true);
                    tally.add_reporter(reporter_hash(reporter));
                }
            }
            None => {
                self.battles = self.battles.saturating_add_signed(w);
                for entry in &record.contestants {
                    let tally = self.versions.entry(entry.vid).or_default();
                    tally.battles = add(tally.battles, w);
                    if w > 0 {
                        tally.push(false);
                    }
                }
                let contestants = &record.contestants;
                for (i, x) in contestants.iter().enumerate() {
                    for y in &contestants[i + 1..] {
                        if x.vid == y.vid {
                            continue;
                        }
                        // A is the lower vid; B's values are 100 - A's, so only A's are kept.
                        let (a, b) = if x.vid < y.vid { (x, y) } else { (y, x) };
                        self.fold_pair(
                            (a.vid, b.vid),
                            pairing_score(a.score, b.score),
                            pairing_survival(a.firsts, b.firsts),
                            w,
                            day,
                        );
                    }
                }
            }
        }
    }

    fn fold_pair(&mut self, key: (u32, u32), score: f64, survival: f64, w: i64, day: u16) {
        let wf = w as f64;
        match self.pairings.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let p = slot.get_mut();
                p.n = add(p.n, w);
                p.s += wf * score;
                p.q += wf * score * score;
                p.v += wf * survival;
                p.day = day;
                if p.n == 0 {
                    slot.remove();
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                // Removing a battle that was never folded: nothing to cancel.
                if w > 0 {
                    slot.insert(Pairing {
                        n: 1,
                        s: score,
                        q: score * score,
                        v: survival,
                        day,
                    });
                }
            }
        }
    }

    /// Drops the versions in `set` and every pairing that involves one (§10.5 step 2).
    pub fn drop_versions(&mut self, set: &HashSet<u32>) {
        self.versions.retain(|vid, _| !set.contains(vid));
        self.pairings
            .retain(|(a, b), _| !set.contains(a) && !set.contains(b));
    }

    /// `pairings.bin`: gzip of the layout in §10.5 (the lengths of the file's header, versions
    /// and pairings are exact; the pairings are sorted by (a, b)).
    pub fn to_bytes(&self) -> Vec<u8> {
        let pairings = self.sorted_pairings();
        let mut raw = Vec::with_capacity(
            31 + VERSION_BYTES * self.versions.len() + 4 + PAIRING_BYTES * pairings.len(),
        );
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&FILE_VERSION.to_le_bytes());
        raw.extend_from_slice(&self.engine.to_le_bytes());
        raw.push(division_code(self.division));
        raw.extend_from_slice(&self.done.to_le_bytes());
        raw.extend_from_slice(&self.battles.to_le_bytes());
        raw.extend_from_slice(&(self.versions.len() as u32).to_le_bytes());
        for (vid, tally) in &self.versions {
            raw.extend_from_slice(&vid.to_le_bytes());
            raw.extend_from_slice(&tally.battles.to_le_bytes());
            raw.extend_from_slice(&tally.failed.to_le_bytes());
            raw.extend_from_slice(&tally.last20.to_le_bytes());
            raw.push(tally.reporter_count);
            for r in tally.reporters {
                raw.extend_from_slice(&r.to_le_bytes());
            }
        }
        raw.extend_from_slice(&(pairings.len() as u32).to_le_bytes());
        for ((a, b), p) in &pairings {
            raw.extend_from_slice(&a.to_le_bytes());
            raw.extend_from_slice(&b.to_le_bytes());
            raw.extend_from_slice(&p.n.to_le_bytes());
            raw.extend_from_slice(&p.s.to_le_bytes());
            raw.extend_from_slice(&p.q.to_le_bytes());
            raw.extend_from_slice(&p.v.to_le_bytes());
            raw.extend_from_slice(&p.day.to_le_bytes());
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&raw).expect("writing to a Vec");
        encoder.finish().expect("writing to a Vec")
    }

    /// Parses `pairings.bin`.
    pub fn from_bytes(bytes: &[u8]) -> Result<State, String> {
        let mut raw = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut raw)
            .map_err(|e| format!("pairings.bin: bad gzip: {e}"))?;
        let mut reader = Reader(&raw);
        if reader.take(4)? != MAGIC {
            return Err("pairings.bin: bad magic".to_string());
        }
        let version = reader.u16()?;
        if !(1..=FILE_VERSION).contains(&version) {
            return Err(format!("pairings.bin: unsupported version {version}"));
        }
        let engine = reader.u32()?;
        let division = division_from_code(reader.u8()?)?;
        let done = reader.u32()?;
        let battles = reader.u64()?;
        let version_count = reader.u32()? as usize;
        let version_bytes = if version == 1 {
            VERSION_BYTES_V1
        } else {
            VERSION_BYTES
        };
        if version_count > reader.0.len() / version_bytes {
            return Err("pairings.bin: truncated versions".to_string());
        }
        let mut state = State::new(engine, division, done);
        state.battles = battles;
        for _ in 0..version_count {
            let vid = reader.u32()?;
            let mut tally = VersionTally {
                battles: reader.u32()?,
                failed: reader.u32()?,
                last20: reader.u32()?,
                ..VersionTally::default()
            };
            if version >= 2 {
                tally.reporter_count = reader.u8()?;
                if tally.reporter_count as usize > MAX_REPORTERS {
                    return Err(format!(
                        "pairings.bin: version {vid} has too many reporters"
                    ));
                }
                for r in &mut tally.reporters {
                    *r = reader.u32()?;
                }
            }
            if state.versions.insert(vid, tally).is_some() {
                return Err(format!("pairings.bin: version {vid} twice"));
            }
        }
        let count = reader.u32()? as usize;
        if count != reader.0.len() / PAIRING_BYTES || reader.0.len() % PAIRING_BYTES != 0 {
            return Err("pairings.bin: pairing count does not match the size".to_string());
        }
        state.pairings.reserve(count);
        let mut last = None;
        for _ in 0..count {
            let key = (reader.u32()?, reader.u32()?);
            if key.0 >= key.1 || last.is_some_and(|l| l >= key) {
                return Err("pairings.bin: pairings not sorted".to_string());
            }
            last = Some(key);
            let pairing = Pairing {
                n: reader.u32()?,
                s: reader.f64()?,
                q: reader.f64()?,
                v: reader.f64()?,
                day: reader.u16()?,
            };
            state.pairings.insert(key, pairing);
        }
        Ok(state)
    }
}

/// `value + w`, not below 0.
fn add(value: u32, w: i64) -> u32 {
    (i64::from(value) + w).clamp(0, i64::from(u32::MAX)) as u32
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("pairings.bin: truncated".to_string());
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Entry, FailReason};

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

    fn duel(a: Entry, b: Entry) -> Record {
        record(Division::OneVsOne, vec![a, b])
    }

    fn state() -> State {
        State::new(1, Division::OneVsOne, 2026102400)
    }

    #[test]
    fn one_v_one_from_the_lower_vids_side() {
        let mut s = state();
        // The second contestant has the lower vid: A is vid 3 with 25 of 100 points, 14 of 35.
        s.fold(&duel(entry(5, 75.0, 21), entry(3, 25.0, 14)), 100);
        let p = s.pairing(3, 5).unwrap();
        assert_eq!((p.n, p.s, p.q, p.v, p.day), (1, 25.0, 625.0, 40.0, 100));
        assert_eq!(s.pairing(5, 3), s.pairing(3, 5));
        assert_eq!(s.battles, 1);
        assert_eq!(s.versions[&3].battles, 1);
        assert_eq!(s.versions[&5].battles, 1);
        // The same battle with the order swapped gives the same pairing.
        s.fold(&duel(entry(3, 25.0, 14), entry(5, 75.0, 21)), 101);
        let p = s.pairing(3, 5).unwrap();
        assert_eq!((p.n, p.s, p.q, p.v, p.day), (2, 50.0, 1250.0, 80.0, 101));
        assert_eq!(s.pairing_count(), 1);
    }

    #[test]
    fn both_zero_is_even() {
        let mut s = state();
        s.fold(&duel(entry(1, 0.0, 0), entry(2, 0.0, 0)), 1);
        let p = s.pairing(1, 2).unwrap();
        assert_eq!((p.s, p.v), (50.0, 50.0));
        s.fold(&duel(entry(1, 0.0, 0), entry(2, 10.0, 3)), 1);
        let p = s.pairing(1, 2).unwrap();
        assert_eq!((p.s, p.v), (50.0, 50.0));
    }

    #[test]
    fn melee_has_45_pairs() {
        let mut s = State::new(1, Division::Melee, 0);
        // Vids 10..19 in descending robot order; robot with vid 10+i has score 10(i+1), i firsts.
        let entries: Vec<Entry> = (0..10u32)
            .rev()
            .map(|i| entry(10 + i, 10.0 * (i + 1) as f32, i as u16))
            .collect();
        s.fold(&record(Division::Melee, entries), 7);
        assert_eq!(s.pairing_count(), 45);
        assert_eq!(s.battles, 1);
        assert_eq!(s.versions.len(), 10);
        assert!(s.versions.values().all(|t| t.battles == 1));
        // (10, 11): A has 10 and B 20 points: 33.33..; firsts 0 and 1: 0.
        let p = s.pairing(10, 11).unwrap();
        assert!((p.s - 100.0 / 3.0).abs() < 1e-9);
        assert_eq!(p.v, 0.0);
        // (18, 19): 90 and 100 points; firsts 8 and 9.
        let p = s.pairing(19, 18).unwrap();
        assert!((p.s - 100.0 * 90.0 / 190.0).abs() < 1e-9);
        assert!((p.v - 100.0 * 8.0 / 17.0).abs() < 1e-9);
        // (10, 19): 10 and 100 points.
        let p = s.pairing(10, 19).unwrap();
        assert!((p.s - 100.0 * 10.0 / 110.0).abs() < 1e-9);
        // Every pairing is from the lower vid's side: scores of lower vids are always < 50.
        assert!(s.pairings().all(|((a, b), p)| a < b && p.s < 50.0 + 1e-9));
        // A division mismatch is ignored.
        s.fold(&duel(entry(1, 1.0, 1), entry(2, 1.0, 1)), 7);
        assert_eq!(s.pairing_count(), 45);
    }

    #[test]
    fn negative_weights_cancel() {
        let mut s = state();
        let first = duel(entry(1, 50.0, 10), entry(2, 50.0, 5));
        let second = duel(entry(1, 75.0, 3), entry(3, 25.0, 1));
        s.fold(&first, 1);
        s.fold(&second, 1);
        let before = s.clone();
        let extra = duel(entry(1, 60.0, 9), entry(2, 20.0, 1));
        s.fold(&extra, 2);
        s.fold(&extra.negated(), 3);
        // Dyadic scores: every sum is exact, so the pairings return exactly.
        assert_eq!(s.pairing(1, 3), before.pairing(1, 3));
        assert_eq!(s.battles, 2);
        assert_eq!(s.pairing(1, 2).unwrap().n, 1);
        s.fold(&first.negated(), 3);
        s.fold(&second.negated(), 3);
        assert_eq!(s.pairing_count(), 0, "entries at n = 0 are dropped");
        assert_eq!(s.battles, 0);
        assert!(s.versions.values().all(|t| t.battles == 0));
        // Removing what was never folded creates nothing.
        s.fold(&first.negated(), 3);
        assert_eq!(s.pairing_count(), 0);
        assert_eq!(s.battles, 0);
    }

    #[test]
    fn failures_count_only_if_trusted() {
        let mut s = state();
        let mut failed = duel(entry(1, 0.0, 0), entry(2, 0.0, 0));
        failed.failed = Some((1, FailReason::Crash));
        failed.trusted = false;
        s.fold(&failed, 1);
        assert!(s.versions.is_empty());
        assert_eq!((s.battles, s.pairing_count()), (0, 0));

        failed.trusted = true;
        s.fold(&failed, 1);
        s.fold(&failed, 1);
        let t = s.versions[&2];
        assert_eq!((t.battles, t.failed, t.last20), (2, 2, 0b11));
        assert_eq!(t.recent_failures(), 2);
        assert!(!s.versions.contains_key(&1), "the other robot is untouched");
        assert_eq!((s.battles, s.pairing_count()), (0, 0));

        // 20 successes push the failures out of the window.
        for _ in 0..20 {
            s.fold(&duel(entry(2, 10.0, 1), entry(3, 10.0, 1)), 1);
        }
        let t = s.versions[&2];
        assert_eq!((t.battles, t.failed, t.recent_failures()), (22, 2, 0));
        assert_eq!(t.recent_battles(), 20);
        assert_eq!(s.battles, 20);

        // Removing a trusted failure report lowers the counts.
        s.fold(&failed.negated(), 1);
        assert_eq!(s.versions[&2].failed, 1);
    }

    #[test]
    fn derived_stats() {
        let mut s = state();
        for (a, b) in [(40.0, 60.0), (50.0, 50.0), (60.0, 40.0)] {
            s.fold(&duel(entry(1, a, 0), entry(2, b, 0)), 1);
        }
        let p = *s.pairing(1, 2).unwrap();
        assert_eq!((p.n, p.s, p.q), (3, 150.0, 7700.0));
        let st = p.stats(16.0, 3.0);
        assert_eq!(st.mean, 50.0);
        assert!((st.variance.unwrap() - 100.0).abs() < 1e-9);
        // (3 * 16 + 2 * 100) / (3 + 2) = 49.6
        assert!((st.shrunk_variance - 49.6).abs() < 1e-9);
        assert!((st.se - (49.6f64 / 3.0).sqrt()).abs() < 1e-12);
        assert!((st.ci95 - 1.96 * st.se).abs() < 1e-12);
        assert_eq!(st.survival, 50.0);

        let one = Pairing {
            n: 1,
            s: 40.0,
            q: 1600.0,
            v: 0.0,
            day: 0,
        };
        let st = one.stats(16.0, 3.0);
        assert_eq!((st.variance, st.shrunk_variance, st.se), (None, 16.0, 4.0));
        let none = Pairing { n: 0, ..one };
        assert_eq!(none.stats(16.0, 3.0).se, 4.0);

        // Through the config: 1v1 prior 16, weight 3.
        let config = Config::default();
        let via = s.pairing_stats(2, 1, &config).unwrap();
        assert!((via.shrunk_variance - 49.6).abs() < 1e-9);
        assert!(s.pairing_stats(1, 9, &config).is_none());
    }

    #[test]
    fn drop_versions() {
        let mut s = state();
        s.fold(&duel(entry(1, 1.0, 1), entry(2, 1.0, 1)), 1);
        s.fold(&duel(entry(2, 1.0, 1), entry(3, 1.0, 1)), 1);
        s.fold(&duel(entry(1, 1.0, 1), entry(3, 1.0, 1)), 1);
        s.drop_versions(&HashSet::from([2]));
        assert_eq!(s.pairing_count(), 1);
        assert!(s.pairing(1, 3).is_some());
        assert_eq!(s.versions.keys().copied().collect::<Vec<_>>(), [1, 3]);
        assert_eq!(s.battles, 3, "the division count is not recomputed");
    }

    #[test]
    fn file_round_trip() {
        let mut s = State::new(3, Division::Melee, 2026102412);
        let entries: Vec<Entry> = (0..10u32)
            .map(|i| entry(100 + i * 3, 7.3 * (i + 1) as f32, i as u16))
            .collect();
        s.fold(&record(Division::Melee, entries.clone()), 20000);
        s.fold(&record(Division::Melee, entries), 20001);
        let mut failed = duel(entry(100, 0.0, 0), entry(103, 0.0, 0));
        failed.division = Division::Melee;
        failed.failed = Some((0, FailReason::Load));
        s.fold(&failed, 20001);
        let bytes = s.to_bytes();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        let back = State::from_bytes(&bytes).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.to_bytes(), bytes);
        assert_eq!(back.done, 2026102412);
        assert_eq!(back.battles, 2);

        let empty = State::new(1, Division::OneVsOne, 0);
        assert_eq!(State::from_bytes(&empty.to_bytes()).unwrap(), empty);
    }

    fn gzip(raw: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    fn gunzip(bytes: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        GzDecoder::new(bytes).read_to_end(&mut raw).unwrap();
        raw
    }

    #[test]
    fn invalid_files() {
        let mut s = state();
        s.fold(&duel(entry(1, 1.0, 1), entry(2, 3.0, 1)), 1);
        let raw = gunzip(&s.to_bytes());
        // Header: magic, version 2, engine 1, division 0.
        assert_eq!(&raw[..4], b"RSPS");
        assert_eq!(&raw[4..6], &[2, 0]);
        assert_eq!(raw.len(), 4 + 2 + 4 + 1 + 4 + 8 + 4 + 2 * 29 + 4 + 38);

        assert!(State::from_bytes(b"not gzip").is_err());
        assert!(State::from_bytes(&gzip(&raw[..raw.len() - 1])).is_err());
        assert!(State::from_bytes(&gzip(&raw[..10])).is_err());
        let mut bad = raw.clone();
        bad[0] = b'X';
        assert!(State::from_bytes(&gzip(&bad)).is_err());
        let mut bad = raw.clone();
        bad[4] = 3;
        assert!(State::from_bytes(&gzip(&bad)).is_err());
        let mut bad = raw.clone();
        bad[10] = 9;
        assert!(State::from_bytes(&gzip(&bad)).is_err());
        let mut bad = raw.clone();
        bad.push(0);
        assert!(State::from_bytes(&gzip(&bad)).is_err());
        // a >= b
        let mut bad = raw.clone();
        let at = raw.len() - 38;
        bad[at..at + 4].copy_from_slice(&5u32.to_le_bytes());
        assert!(State::from_bytes(&gzip(&bad)).is_err());
    }

    #[test]
    fn version_1_files_are_still_read() {
        let mut s = state();
        s.fold(&duel(entry(1, 10.0, 1), entry(2, 30.0, 2)), 5);
        let mut failed = duel(entry(1, 0.0, 0), entry(3, 0.0, 0));
        failed.failed = Some((1, FailReason::Crash));
        s.fold(&failed, 5);
        // Rewrite as a version 1 file: 16 bytes per version, no reporters.
        let raw = gunzip(&s.to_bytes());
        let head = 4 + 2 + 4 + 1 + 4 + 8 + 4;
        let versions = 3;
        let mut old = raw[..head].to_vec();
        old[4] = 1;
        for i in 0..versions {
            let at = head + i * VERSION_BYTES;
            old.extend_from_slice(&raw[at..at + VERSION_BYTES_V1]);
        }
        old.extend_from_slice(&raw[head + versions * VERSION_BYTES..]);
        let back = State::from_bytes(&gzip(&old)).unwrap();
        assert_eq!(back.pairing(1, 2), s.pairing(1, 2));
        assert_eq!(back.versions[&3].failed, 1);
        assert_eq!(back.versions[&3].reporter_count, 0);
        assert_eq!(back.battles, s.battles);
        // Writing it again gives a version 2 file.
        assert_eq!(&gunzip(&back.to_bytes())[4..6], &[2, 0]);
    }

    #[test]
    fn reporters_are_distinct_and_reset() {
        let mut s = state();
        let mut failed = duel(entry(1, 0.0, 0), entry(2, 0.0, 0));
        failed.failed = Some((1, FailReason::Crash));
        for reporter in ["g1", "g1", "g2", "g3", "g4"] {
            s.fold_from(&failed, 1, reporter);
        }
        let t = s.versions[&2];
        assert_eq!((t.failed, t.distinct_reporters()), (5, 3));
        assert_eq!(t.reporters[0], reporter_hash("g1"));
        // fold uses the record's uid.
        failed.uid = "g9".into();
        s.fold(&failed, 1);
        assert_eq!(s.versions[&2].distinct_reporters(), 3);
        // Successes push the failure share down: the reporters are forgotten.
        for _ in 0..12 {
            s.fold(&duel(entry(2, 10.0, 1), entry(3, 10.0, 1)), 1);
        }
        assert_eq!(s.versions[&2].distinct_reporters(), 0);
    }

    /// 1,100 bots, every pair: 604,450 pairings. Run with
    /// `cargo test -p robocode-s-rumble-core --release -- --ignored big_state`.
    #[test]
    #[ignore]
    fn big_state() {
        let started = std::time::Instant::now();
        let mut s = state();
        let bots = 1100u32;
        for a in 1..=bots {
            for b in a + 1..=bots {
                let score = 20.0 + ((a * 31 + b * 17) % 60) as f32;
                s.fold(&duel(entry(a, score, 10), entry(b, 100.0 - score, 5)), 1);
            }
        }
        assert_eq!(s.pairing_count(), 604_450);
        let folded = started.elapsed();
        let bytes = s.to_bytes();
        let written = started.elapsed();
        let back = State::from_bytes(&bytes).unwrap();
        assert!(back == s);
        eprintln!(
            "fold {folded:?}, write {:?} ({} bytes), read {:?}",
            written - folded,
            bytes.len(),
            started.elapsed() - written
        );
    }
}
