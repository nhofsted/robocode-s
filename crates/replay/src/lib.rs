// SPDX-License-Identifier: EPL-2.0

//! Compact lossy replays for the browser viewer (ADR 0017).
//!
//! A [`Track`] is one round as the viewer draws it, quantized to what a screen can show. The
//! codec is lossless over a `Track`: all loss happens in quantization. Values are predicted
//! from what the decoder has already reconstructed (closed loop, so errors never accumulate),
//! and the residuals go through an adaptive binary range coder. The predictor for each field is
//! chosen backward-adaptively from the recent errors of several candidates, so the choice costs
//! no bits. Only bullet births and ends are stored; their flight follows from the rules.

mod range;
#[cfg(feature = "json")]
pub mod recording;

use range::{Bit, CONTEXTS, Decoder, Encoder, IntModel, Io, SymbolModel};
use robocode_s_fdlibm as fdlibm;
use std::f64::consts::PI;

/// Positions: units per pixel (motion at ½ px is smooth on screen).
pub const POS_SCALE: i64 = 2;
/// Robot body, gun and radar headings: units per full turn. 0.35°: a gun barrel's tip moves
/// about 0.1 px per step.
pub const ANGLE_STEPS: i64 = 1024;
/// Bullet headings: units per full turn (bullets fly far, so they need more precision).
pub const BULLET_ANGLE_STEPS: i64 = 65536;
/// Energy and bullet power: units per point.
pub const ENERGY_SCALE: i64 = 100;

/// The first byte of every track. The desktop GUI tells tracks from its gzip recordings by
/// it, so a future version must not be gzip's first byte, `0x1f`.
///
/// Version 2 adds the engine version (design §4.4) after it, as a LEB128 integer; version 1
/// tracks decode with engine version 0 (unknown).
const VERSION: u8 = 2;
const _: () = assert!(VERSION != 0x1f);
/// The first version, without an engine version.
const VERSION_1: u8 = 1;
/// Classic allows one bullet per robot per turn; this only bounds corrupt input.
const MAX_BIRTHS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RobotState {
    #[default]
    Active,
    HitWall,
    HitRobot,
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RobotFrame {
    pub state: RobotState,
    pub x: i64,
    pub y: i64,
    pub body: i64,
    pub gun: i64,
    pub radar: i64,
    pub energy: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulletEnd {
    Wall,
    Bullet,
    Robot(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulletTrack {
    pub owner: u8,
    /// Frame index in which it first appears, at `(x, y)`.
    pub born: u32,
    pub x: i64,
    pub y: i64,
    pub heading: i64,
    pub power: i64,
    /// Frame index of the end, how it ended and where it exploded.
    pub end: Option<(u32, BulletEnd, i64, i64)>,
}

impl BulletTrack {
    pub fn speed(&self) -> f64 {
        20.0 - 3.0 * (self.power as f64 / ENERGY_SCALE as f64)
    }

    /// Where the bullet is in frame `frame`, in quantized units. Deterministic (fdlibm), so the
    /// encoder and decoder agree when they use it for prediction.
    pub fn position(&self, frame: u32) -> (f64, f64) {
        let a = self.heading as f64 * (2.0 * PI / BULLET_ANGLE_STEPS as f64);
        let d = (frame as f64 - self.born as f64) * self.speed() * POS_SCALE as f64;
        (
            self.x as f64 + d * fdlibm::sin(a),
            self.y as f64 + d * fdlibm::cos(a),
        )
    }
}

/// [`decode`] returns the track [`encode`] was given, except that a robot's values after the
/// frame it died in are not stored: it decodes as it died.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    /// Engine version (design §4.4) the battle ran with; 0 = unknown (version 1 tracks).
    pub engine_version: u32,
    pub width: i64,
    pub height: i64,
    /// Gun cooling rate in thousandths (classic default 100).
    pub gun_cooling_milli: i64,
    /// `frames[0]` is the round start, `frames[t]` the state after turn `t`.
    pub frames: Vec<Vec<RobotFrame>>,
    pub bullets: Vec<BulletTrack>,
}

#[derive(Debug)]
pub struct DecodeError(pub &'static str);

/// Cost categories reported by [`encode_with_costs`].
pub const LABELS: [&str; 9] = [
    "header",
    "state",
    "body",
    "gun",
    "radar",
    "position",
    "bullet end",
    "bullet birth",
    "energy",
];
const L_HEADER: usize = 0;
const L_STATE: usize = 1;
const L_BODY: usize = 2;
const L_GUN: usize = 3;
const L_RADAR: usize = 4;
const L_POS: usize = 5;
const L_END: usize = 6;
const L_BIRTH: usize = 7;
const L_ENERGY: usize = 8;

pub fn encode(track: &Track) -> Vec<u8> {
    encode_with_costs(track).0
}

/// Encodes and also returns the information content per [`LABELS`] category, in bits.
pub fn encode_with_costs(track: &Track) -> (Vec<u8>, Vec<f64>) {
    let mut enc = Encoder::new(LABELS.len());
    let mut rebuilt = run(&mut enc, Some(track)).expect("encoding cannot fail");
    // The engine version is in the header, not in the coded stream.
    rebuilt.engine_version = track.engine_version;
    debug_assert_eq!(
        rebuilt,
        stored(track),
        "the encoder's reconstruction must match its input"
    );
    let costs = enc.costs.clone();
    let mut out = vec![VERSION];
    write_leb128(&mut out, track.engine_version);
    out.extend(enc.finish());
    (out, costs)
}

/// `track` as the codec stores it: dead robots keep their death frame.
fn stored(track: &Track) -> Track {
    let mut track = track.clone();
    for t in 1..track.frames.len() {
        let (done, rest) = track.frames.split_at_mut(t);
        for (r, prev) in rest[0].iter_mut().zip(&done[t - 1]) {
            if prev.state == RobotState::Dead {
                *r = *prev;
            }
        }
    }
    track
}

pub fn decode(bytes: &[u8]) -> Result<Track, DecodeError> {
    let (engine_version, rest) = match bytes.split_first() {
        Some((&VERSION, rest)) => read_leb128(rest)?,
        Some((&VERSION_1, rest)) => (0, rest),
        _ => return Err(DecodeError("unknown replay version")),
    };
    let mut dec = Decoder::new(rest);
    let mut track = run(&mut dec, None)?;
    if dec.overrun() {
        return Err(DecodeError("truncated or corrupt replay"));
    }
    track.engine_version = engine_version;
    Ok(track)
}

fn write_leb128(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_leb128(bytes: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
    let mut v = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(5) {
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            let v = u32::try_from(v).map_err(|_| DecodeError("corrupt replay header"))?;
            return Ok((v, &bytes[i + 1..]));
        }
    }
    Err(DecodeError("corrupt replay header"))
}

// ---------------------------------------------------------------- prediction

/// Picks, among candidate predictions, the one with the smallest recent error. Both sides see
/// the same history, so the choice is free.
#[derive(Clone)]
struct Chooser<const N: usize> {
    cost: [u64; N],
    ctx: usize,
}

impl<const N: usize> Default for Chooser<N> {
    fn default() -> Self {
        Self {
            cost: [0; N],
            ctx: 0,
        }
    }
}

impl<const N: usize> Chooser<N> {
    fn pick(&self) -> usize {
        (0..N).min_by_key(|&i| self.cost[i]).unwrap()
    }

    fn learn(&mut self, errors: [u64; N], residual: u64) {
        for (c, e) in self.cost.iter_mut().zip(errors) {
            *c = *c - (*c >> 2) + e.min(1 << 20);
        }
        self.ctx = context(residual);
    }
}

fn context(residual: u64) -> usize {
    match residual {
        0 => 0,
        1..=2 => 1,
        3..=15 => 2,
        _ => 3,
    }
    .min(CONTEXTS - 1)
}

/// `a - b`, wrapped into `[-m/2, m/2)` when `m > 0`.
fn diff(a: i64, b: i64, m: i64) -> i64 {
    if m > 0 {
        (a - b + m / 2).rem_euclid(m) - m / 2
    } else {
        a - b
    }
}

fn wrap(a: i64, m: i64) -> i64 {
    if m > 0 { a.rem_euclid(m) } else { a }
}

/// Codes one value against the best of `cands`.
fn field<I: Io, const N: usize>(
    io: &mut I,
    model: &mut IntModel,
    chooser: &mut Chooser<N>,
    cands: [i64; N],
    actual: i64,
    m: i64,
) -> i64 {
    let p = cands[chooser.pick()];
    let r = model.code(io, chooser.ctx, diff(actual, p, m));
    let v = wrap(p + r, m);
    chooser.learn(
        cands.map(|c| diff(v, c, m).unsigned_abs()),
        r.unsigned_abs(),
    );
    v
}

/// A heading in radians as units of `steps` per turn, in `[0, steps)`: how tracks quantize
/// headings ([`ANGLE_STEPS`], [`BULLET_ANGLE_STEPS`]).
pub fn angle_units(radians: f64, steps: i64) -> i64 {
    ((radians * steps as f64 / (2.0 * PI)).round() as i64).rem_euclid(steps)
}

fn angle_radians(units: i64, steps: i64) -> f64 {
    units as f64 * (2.0 * PI / steps as f64)
}

/// Robocode's absolute bearing from `a` to `b` (0 = north, clockwise).
fn bearing(a: &RobotFrame, b: &RobotFrame) -> i64 {
    angle_units(
        fdlibm::atan2(b.x as f64 - a.x as f64, b.y as f64 - a.y as f64),
        ANGLE_STEPS,
    )
}

/// Per-robot reconstructed history, beyond the frames themselves.
#[derive(Clone, Default)]
struct RobotHistory {
    /// Bearing to the nearest enemy in the last two frames.
    b1: i64,
    b2: i64,
    /// Signed speed along the heading in the last two steps (quantized units).
    v1: i64,
    v2: i64,
    /// Gun heat in thousandths.
    heat: i64,
    last_power: i64,
    /// Energy change not explained by bullets in the last frame.
    drift: i64,
}

#[derive(Default)]
struct Models {
    header: IntModel,
    start: IntModel,
    death: [Bit; 3],
    state: [Bit; 4],
    body: IntModel,
    gun: IntModel,
    radar: IntModel,
    speed: IntModel,
    x: IntModel,
    y: IntModel,
    end: [Bit; 4],
    end_kind: [Bit; 4],
    nearest_victim: Bit,
    victim: Option<SymbolModel>,
    end_x: IntModel,
    end_y: IntModel,
    fired: [Bit; 3],
    more: Bit,
    born_ended: Bit,
    power: IntModel,
    heading: IntModel,
    origin_x: IntModel,
    origin_y: IntModel,
    energy: IntModel,
}

#[derive(Clone, Default)]
struct RobotChoosers {
    body: Chooser<3>,
    gun: Chooser<7>,
    radar: Chooser<7>,
    speed: Chooser<4>,
    pos: Chooser<1>,
    power: Chooser<1>,
    heading: Chooser<2>,
    origin: Chooser<4>,
    energy: Chooser<2>,
}

// ---------------------------------------------------------------- the shared code path

fn run<I: Io>(io: &mut I, src: Option<&Track>) -> Result<Track, DecodeError> {
    let mut m = Models::default();
    let get = |f: &dyn Fn(&Track) -> i64| src.map_or(0, f);

    io.label(L_HEADER);
    let mut header = |io: &mut I, v: i64| m.header.code(io, 0, v);
    let width = header(io, get(&|t| t.width));
    let height = header(io, get(&|t| t.height));
    let gun_cooling_milli = header(io, get(&|t| t.gun_cooling_milli - 100)) + 100;
    let robots = header(io, get(&|t| t.frames[0].len() as i64));
    let turns = header(io, get(&|t| t.frames.len() as i64 - 1));
    if !(1..=255).contains(&robots)
        || !(0..=1_000_000).contains(&turns)
        || width <= 0
        || height <= 0
    {
        return Err(DecodeError("implausible header"));
    }
    let n = robots as usize;
    let turns = turns as usize;
    if n > 2 {
        m.victim = Some(SymbolModel::new(usize::BITS - (n - 1).leading_zeros()));
    }

    // Round start: absolute values, each predicted from the one before.
    let mut start = Vec::with_capacity(n);
    for i in 0..n {
        let s = src.map(|t| t.frames[0][i]).unwrap_or_default();
        let mut c =
            |io: &mut I, v: i64, p: i64, md: i64| wrap(p + m.start.code(io, 1, diff(v, p, md)), md);
        let x = c(io, s.x, width * POS_SCALE / 2, 0);
        let y = c(io, s.y, height * POS_SCALE / 2, 0);
        let body = c(io, s.body, 0, ANGLE_STEPS);
        let gun = c(io, s.gun, body, ANGLE_STEPS);
        let radar = c(io, s.radar, gun, ANGLE_STEPS);
        let energy = c(io, s.energy, 100 * ENERGY_SCALE, 0);
        start.push(RobotFrame {
            state: RobotState::Active,
            x,
            y,
            body,
            gun,
            radar,
            energy,
        });
    }
    let mut frames = vec![start];
    let mut bullets: Vec<BulletTrack> = Vec::new();
    let mut active: Vec<usize> = Vec::new(); // indices into `bullets`
    let mut hist = vec![
        RobotHistory {
            heat: 3000,
            ..Default::default()
        };
        n
    ];
    let mut ch = vec![RobotChoosers::default(); n];
    let mut src_bullet = 0; // next bullet of `src` to be born, in birth order
    for (i, h) in hist.iter_mut().enumerate() {
        h.b1 = nearest_bearing(&frames[0], i).unwrap_or(0);
        h.b2 = h.b1;
    }

    for t in 1..=turns {
        if io.exhausted() {
            return Err(DecodeError("truncated or corrupt replay"));
        }
        let prev = frames[t - 1].clone();
        let prev2 = if t >= 2 {
            frames[t - 2].clone()
        } else {
            prev.clone()
        };
        let actual = |i: usize| src.map(|s| s.frames[t][i]).unwrap_or_default();
        let mut cur = prev.clone();

        // Pass 1: state, body heading and position.
        for i in 0..n {
            if prev[i].state == RobotState::Dead {
                continue;
            }
            let a = actual(i);
            let h = &hist[i];
            let c = &mut ch[i];

            // Death and the wall/robot collision state.
            io.label(L_STATE);
            let dctx = if prev[i].energy <= 0 { 1 } else { 0 };
            let dead = io.bit(&mut m.death[dctx], a.state == RobotState::Dead);
            cur[i].state = if dead {
                RobotState::Dead
            } else {
                let sctx = prev[i].state as usize;
                if io.bit(&mut m.state[sctx], a.state != RobotState::Active) {
                    if io.bit(&mut m.state[3], a.state == RobotState::HitRobot) {
                        RobotState::HitRobot
                    } else {
                        RobotState::HitWall
                    }
                } else {
                    RobotState::Active
                }
            };

            // Body: last, linear, following the bearing (orbiting).
            io.label(L_BODY);
            let (h1, h2) = (prev[i].body, prev2[i].body);
            let cands = [
                h1,
                h1 + diff(h1, h2, ANGLE_STEPS),
                h1 + diff(h.b1, h.b2, ANGLE_STEPS),
            ];
            let body = field(io, &mut m.body, &mut c.body, cands, a.body, ANGLE_STEPS);

            // Position: classic turns, then moves along the new heading. The speed is coded
            // (it changes by +1 or -2 px per turn, or stays), then the small remaining offset.
            io.label(L_POS);
            let hr = angle_radians(body, ANGLE_STEPS);
            let (s, co) = (fdlibm::sin(hr), fdlibm::cos(hr));
            let (x1, y1) = (prev[i].x, prev[i].y);
            let along =
                |x: i64, y: i64| ((x - x1) as f64 * s + (y - y1) as f64 * co).round() as i64;
            let (v1, v2) = (h.v1, h.v2);
            let sign = if v1 < 0 { -1 } else { 1 };
            let max = 8 * POS_SCALE;
            let vc = [
                v1,
                (2 * v1 - v2).clamp(-max, max),
                (v1 + sign * POS_SCALE).clamp(-max, max),
                if v1.abs() >= 2 * POS_SCALE {
                    v1 - sign * 2 * POS_SCALE
                } else {
                    0
                },
            ];
            let v = field(io, &mut m.speed, &mut c.speed, vc, along(a.x, a.y), 0);
            let (px, py) = (
                (x1 as f64 + v as f64 * s).round() as i64,
                (y1 as f64 + v as f64 * co).round() as i64,
            );
            let rx = m.x.code(io, c.pos.ctx, a.x - px);
            let ry = m.y.code(io, c.pos.ctx, a.y - py);
            c.pos.learn([0], rx.unsigned_abs().max(ry.unsigned_abs()));
            cur[i] = RobotFrame {
                state: cur[i].state,
                x: px + rx,
                y: py + ry,
                body,
                ..prev[i]
            };
            hist[i].v2 = v1;
            hist[i].v1 = v;
        }

        // Pass 2: gun and radar, now that this turn's bearings are known.
        let b0: Vec<i64> = (0..n)
            .map(|i| nearest_bearing(&cur, i).unwrap_or(hist[i].b1))
            .collect();
        for i in 0..n {
            if prev[i].state == RobotState::Dead {
                continue;
            }
            let a = actual(i);
            let h = &hist[i];
            let c = &mut ch[i];
            // Last, linear, keeping the offset to the bearing (lagged or current), and the
            // radar lock: turning twice the offset mirrors the heading around the bearing.
            let cands = |h1: i64, h2: i64| {
                [
                    h1,
                    h1 + diff(h1, h2, ANGLE_STEPS),
                    h1 + diff(h.b1, h.b2, ANGLE_STEPS),
                    h1 + diff(b0[i], h.b1, ANGLE_STEPS),
                    h.b1 + diff(h.b1, h1, ANGLE_STEPS),
                    b0[i] + diff(b0[i], h1, ANGLE_STEPS),
                    // Width lock: overshoot the bearing by the same arc as last turn.
                    h.b1 + diff(h.b1, h1, ANGLE_STEPS).signum() * diff(h1, h.b2, ANGLE_STEPS).abs(),
                ]
            };
            io.label(L_GUN);
            cur[i].gun = field(
                io,
                &mut m.gun,
                &mut c.gun,
                cands(prev[i].gun, prev2[i].gun),
                a.gun,
                ANGLE_STEPS,
            );
            io.label(L_RADAR);
            cur[i].radar = field(
                io,
                &mut m.radar,
                &mut c.radar,
                cands(prev[i].radar, prev2[i].radar),
                a.radar,
                ANGLE_STEPS,
            );
        }
        for (h, b) in hist.iter_mut().zip(b0) {
            h.b2 = h.b1;
            h.b1 = b;
        }

        // Bullet ends. Contexts: the predicted position left the field, or is near a robot.
        io.label(L_END);
        let mut energy_delta = vec![0i64; n];
        let mut still = Vec::with_capacity(active.len());
        for &b in &active {
            let (px, py) = bullets[b].position(t as u32);
            let ctx = end_context(px, py, width, height, &cur);
            let a_end = src
                .and_then(|s| s.bullets[b].end)
                .filter(|e| e.0 as usize == t);
            if io.bit(&mut m.end[ctx], a_end.is_some()) {
                let end = code_end(io, &mut m, ctx, t as u32, &bullets[b], a_end, px, py, &cur);
                check_end(&end, n)?;
                apply_end(&bullets[b], &end, &mut energy_delta);
                bullets[b].end = Some(end);
            } else {
                still.push(b);
            }
        }
        active = still;

        // Bullet births. Context: whether the gun can be cool enough to fire.
        io.label(L_BIRTH);
        for i in 0..n {
            hist[i].heat = (hist[i].heat - gun_cooling_milli).max(0);
            if prev[i].state == RobotState::Dead {
                continue;
            }
            let ctx = match hist[i].heat {
                0 => 0,
                h if h <= gun_cooling_milli => 1,
                _ => 2,
            };
            let mut first = true;
            loop {
                let born_now = src.is_some_and(|s| {
                    s.bullets
                        .get(src_bullet)
                        .is_some_and(|b| b.born as usize == t && b.owner as usize == i)
                });
                let model = if first {
                    &mut m.fired[ctx]
                } else {
                    &mut m.more
                };
                if !io.bit(model, born_now) {
                    break;
                }
                if !first
                    && bullets
                        .last()
                        .is_some_and(|b| b.born as usize == t && b.owner as usize == i)
                    && bullets
                        .iter()
                        .rev()
                        .take_while(|b| b.born as usize == t && b.owner as usize == i)
                        .count()
                        >= MAX_BIRTHS
                {
                    return Err(DecodeError("too many bullets in one turn"));
                }
                first = false;
                let a = src.map(|s| s.bullets[src_bullet].clone());
                src_bullet += 1;
                let g = |f: &dyn Fn(&BulletTrack) -> i64| a.as_ref().map_or(0, f);
                let c = &mut ch[i];
                let power = field(
                    io,
                    &mut m.power,
                    &mut c.power,
                    [hist[i].last_power],
                    g(&|b| b.power),
                    0,
                );
                let scale = BULLET_ANGLE_STEPS / ANGLE_STEPS;
                let heading = field(
                    io,
                    &mut m.heading,
                    &mut c.heading,
                    [prev[i].gun * scale, cur[i].gun * scale],
                    g(&|b| b.heading),
                    BULLET_ANGLE_STEPS,
                );
                let speed = (20.0 - 3.0 * (power as f64 / ENERGY_SCALE as f64)) * POS_SCALE as f64;
                let hr = angle_radians(heading, BULLET_ANGLE_STEPS);
                let (s, co) = (fdlibm::sin(hr), fdlibm::cos(hr));
                let os = [
                    (cur[i].x as f64, cur[i].y as f64),
                    (cur[i].x as f64 + speed * s, cur[i].y as f64 + speed * co),
                    (prev[i].x as f64, prev[i].y as f64),
                    (prev[i].x as f64 + speed * s, prev[i].y as f64 + speed * co),
                ]
                .map(|(x, y)| (x.round() as i64, y.round() as i64));
                let k = c.origin.pick();
                let rx = m.origin_x.code(io, c.origin.ctx, g(&|b| b.x) - os[k].0);
                let ry = m.origin_y.code(io, c.origin.ctx, g(&|b| b.y) - os[k].1);
                let (x, y) = (os[k].0 + rx, os[k].1 + ry);
                c.origin.learn(
                    os.map(|(px, py)| (x - px).unsigned_abs() + (y - py).unsigned_abs()),
                    rx.unsigned_abs().max(ry.unsigned_abs()),
                );
                let mut b = BulletTrack {
                    owner: i as u8,
                    born: t as u32,
                    x,
                    y,
                    heading,
                    power,
                    end: None,
                };
                let a_end = a.as_ref().and_then(|b| b.end).filter(|e| e.0 as usize == t);
                if io.bit(&mut m.born_ended, a_end.is_some()) {
                    let (px, py) = (x as f64, y as f64);
                    let ctx = end_context(px, py, width, height, &cur);
                    let end = code_end(io, &mut m, ctx, t as u32, &b, a_end, px, py, &cur);
                    check_end(&end, n)?;
                    apply_end(&b, &end, &mut energy_delta);
                    b.end = Some(end);
                } else {
                    active.push(bullets.len());
                }
                energy_delta[i] -= power;
                hist[i].heat = 1000 + power * 1000 / (5 * ENERGY_SCALE);
                hist[i].last_power = power;
                bullets.push(b);
            }
        }

        // Energy: the last value plus what the bullets explain, optionally plus last turn's
        // unexplained change (inactivity, ramming).
        io.label(L_ENERGY);
        for i in 0..n {
            if prev[i].state == RobotState::Dead {
                continue;
            }
            let p = prev[i].energy + energy_delta[i];
            let e = field(
                io,
                &mut m.energy,
                &mut ch[i].energy,
                [p, p + hist[i].drift],
                actual(i).energy,
                0,
            );
            hist[i].drift = e - p;
            cur[i].energy = e;
        }

        frames.push(cur);
    }
    Ok(Track {
        engine_version: 0,
        width,
        height,
        gun_cooling_milli,
        frames,
        bullets,
    })
}

/// Bearing from robot `i` to the nearest other living robot.
fn nearest_bearing(frame: &[RobotFrame], i: usize) -> Option<i64> {
    let me = &frame[i];
    frame
        .iter()
        .enumerate()
        .filter(|&(j, r)| j != i && r.state != RobotState::Dead)
        .min_by(|(_, r), (_, s)| distance2(me, r).total_cmp(&distance2(me, s)))
        .map(|(_, r)| bearing(me, r))
}

fn distance2(a: &RobotFrame, b: &RobotFrame) -> f64 {
    (b.x as f64 - a.x as f64).powi(2) + (b.y as f64 - a.y as f64).powi(2)
}

fn end_context(px: f64, py: f64, width: i64, height: i64, robots: &[RobotFrame]) -> usize {
    let (w, h) = ((width * POS_SCALE) as f64, (height * POS_SCALE) as f64);
    if px < 0.0 || py < 0.0 || px > w || py > h {
        return 1;
    }
    let near = (40 * POS_SCALE) as f64;
    let close = robots.iter().any(|r| {
        r.state != RobotState::Dead
            && (r.x as f64 - px).abs() < near
            && (r.y as f64 - py).abs() < near
    });
    if close { 2 } else { 0 }
}

#[allow(clippy::too_many_arguments)]
fn code_end<I: Io>(
    io: &mut I,
    m: &mut Models,
    ctx: usize,
    frame: u32,
    b: &BulletTrack,
    actual: Option<(u32, BulletEnd, i64, i64)>,
    px: f64,
    py: f64,
    robots: &[RobotFrame],
) -> (u32, BulletEnd, i64, i64) {
    let (_, kind, ax, ay) = actual.unwrap_or((0, BulletEnd::Wall, 0, 0));
    let kind = if io.bit(&mut m.end_kind[ctx], kind != BulletEnd::Wall) {
        if io.bit(&mut m.end_kind[3], matches!(kind, BulletEnd::Robot(_))) {
            // The victim: usually the robot nearest to the bullet, other than its owner.
            let nearest = robots
                .iter()
                .enumerate()
                .filter(|&(j, r)| j != b.owner as usize && r.state != RobotState::Dead)
                .min_by(|(_, r), (_, s)| {
                    let d = |r: &RobotFrame| (r.x as f64 - px).powi(2) + (r.y as f64 - py).powi(2);
                    d(r).total_cmp(&d(s))
                })
                .map_or(0, |(j, _)| j);
            let victim = match kind {
                BulletEnd::Robot(v) => v as usize,
                _ => 0,
            };
            if io.bit(&mut m.nearest_victim, victim == nearest) {
                BulletEnd::Robot(nearest as u8)
            } else if let Some(model) = &mut m.victim {
                BulletEnd::Robot(model.code(io, victim) as u8)
            } else {
                BulletEnd::Robot(1 - nearest as u8)
            }
        } else {
            BulletEnd::Bullet
        }
    } else {
        BulletEnd::Wall
    };
    let (qx, qy) = (px.round() as i64, py.round() as i64);
    let x = qx + m.end_x.code(io, 0, ax - qx);
    let y = qy + m.end_y.code(io, 0, ay - qy);
    (frame, kind, x, y)
}

fn check_end(end: &(u32, BulletEnd, i64, i64), n: usize) -> Result<(), DecodeError> {
    match end.1 {
        BulletEnd::Robot(v) if v as usize >= n => Err(DecodeError("bullet hit an unknown robot")),
        _ => Ok(()),
    }
}

fn apply_end(b: &BulletTrack, end: &(u32, BulletEnd, i64, i64), energy_delta: &mut [i64]) {
    if let BulletEnd::Robot(v) = end.1 {
        let p = b.power;
        let damage = 4 * p
            + if p > ENERGY_SCALE {
                2 * (p - ENERGY_SCALE)
            } else {
                0
            };
        energy_delta[v as usize] -= damage;
        energy_delta[b.owner as usize] += 3 * p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic pseudo-random walk through a round: three robots driving, turning and
    /// firing, bullets hitting walls, bullets and robots, and one robot dying.
    fn synthetic() -> Track {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as i64
        };
        let n = 3;
        let mut frame: Vec<RobotFrame> = (0..n)
            .map(|i| RobotFrame {
                state: RobotState::Active,
                x: 200 + 300 * i as i64,
                y: 300 + 100 * i as i64,
                body: rnd(ANGLE_STEPS),
                gun: rnd(ANGLE_STEPS),
                radar: rnd(ANGLE_STEPS),
                energy: 100 * ENERGY_SCALE,
            })
            .collect();
        let mut track = Track {
            engine_version: 300,
            width: 800,
            height: 600,
            gun_cooling_milli: 100,
            frames: vec![frame.clone()],
            bullets: vec![],
        };
        for t in 1..=600u32 {
            for (i, r) in frame.iter_mut().enumerate() {
                if r.state == RobotState::Dead {
                    continue;
                }
                r.body = (r.body + rnd(21) - 10).rem_euclid(ANGLE_STEPS);
                r.gun = (r.gun + rnd(41) - 20).rem_euclid(ANGLE_STEPS);
                r.radar = (r.radar + 120).rem_euclid(ANGLE_STEPS);
                r.x = (r.x + rnd(17) - 8).clamp(0, 1600);
                r.y = (r.y + rnd(17) - 8).clamp(0, 1200);
                r.state = if rnd(50) == 0 {
                    RobotState::HitWall
                } else {
                    RobotState::Active
                };
                if i == 2 && t == 400 {
                    r.state = RobotState::Dead;
                    r.energy = 0;
                }
                if rnd(20) == 0 {
                    let end = match rnd(4) {
                        0 => None,
                        1 => Some((
                            t + 1 + rnd(30) as u32,
                            BulletEnd::Wall,
                            rnd(1600),
                            rnd(1200),
                        )),
                        2 => Some((t + rnd(30) as u32, BulletEnd::Bullet, rnd(1600), rnd(1200))),
                        _ => Some((
                            t + 1 + rnd(30) as u32,
                            BulletEnd::Robot(((i + 1) % 2) as u8),
                            rnd(1600),
                            rnd(1200),
                        )),
                    };
                    track.bullets.push(BulletTrack {
                        owner: i as u8,
                        born: t,
                        x: r.x,
                        y: r.y,
                        heading: rnd(BULLET_ANGLE_STEPS),
                        power: 10 + rnd(291),
                        end: end.filter(|e| e.0 <= 600),
                    });
                    r.energy -= rnd(300);
                }
            }
            track.frames.push(frame.clone());
        }
        track
    }

    #[test]
    fn round_trip_is_exact() {
        let track = synthetic();
        let bytes = encode(&track);
        assert_eq!(decode(&bytes).unwrap(), track);
    }

    #[test]
    fn a_dead_robot_decodes_as_it_died() {
        let track = synthetic();
        let mut moving = track.clone();
        for (t, frame) in moving.frames.iter_mut().enumerate().skip(401) {
            let r = &mut frame[2];
            r.x += t as i64;
            r.body = (r.body + t as i64).rem_euclid(ANGLE_STEPS);
            r.energy -= 7;
            r.state = RobotState::Active;
        }
        let bytes = encode(&moving);
        assert_eq!(bytes, encode(&track));
        assert_eq!(decode(&bytes).unwrap(), track);
    }

    /// Random tracks within what the format stores: dead robots keep their death frame, bullets
    /// come from living robots in robot order, residuals stay below 2^40.
    fn random_track(seed: u64) -> Track {
        let mut seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut rnd = |m: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as i64
        };
        let many = rnd(4) == 0;
        let n = 1 + rnd(if many { 40 } else { 8 }) as usize;
        let turns = match rnd(5) {
            0 => rnd(3),
            1 => rnd(2000),
            _ => rnd(300),
        } as u32;
        let huge = rnd(4) == 0;
        let width = 1 + rnd(if huge { 1 << 30 } else { 5000 });
        let height = 1 + rnd(if huge { 1 << 30 } else { 5000 });
        // Wild tracks jump anywhere each turn; the others move like robots.
        let wild = rnd(3) == 0;
        let (w, h) = (width * POS_SCALE, height * POS_SCALE);
        let edge = |rnd: &mut dyn FnMut(i64) -> i64, m: i64| match rnd(4) {
            0 => 0,
            1 => m,
            _ => rnd(m + 1),
        };
        let mut frame: Vec<RobotFrame> = (0..n)
            .map(|_| RobotFrame {
                state: RobotState::Active,
                x: edge(&mut rnd, w),
                y: edge(&mut rnd, h),
                body: rnd(ANGLE_STEPS),
                gun: rnd(ANGLE_STEPS),
                radar: rnd(ANGLE_STEPS),
                energy: rnd(if wild { 1 << 30 } else { 20001 }),
            })
            .collect();
        let mut track = Track {
            engine_version: rnd(1000) as u32,
            width,
            height,
            gun_cooling_milli: rnd(2000),
            frames: vec![frame.clone()],
            bullets: vec![],
        };
        for t in 1..=turns {
            let prev = frame.clone();
            for (i, r) in frame.iter_mut().enumerate() {
                if r.state == RobotState::Dead {
                    continue;
                }
                if wild {
                    r.x = edge(&mut rnd, w);
                    r.y = edge(&mut rnd, h);
                    r.body = rnd(ANGLE_STEPS);
                    r.gun = rnd(ANGLE_STEPS);
                    r.radar = rnd(ANGLE_STEPS);
                    r.energy = rnd(1 << 30) - (1 << 20);
                } else {
                    r.body = (r.body + rnd(21) - 10).rem_euclid(ANGLE_STEPS);
                    r.gun = (r.gun + rnd(81) - 40).rem_euclid(ANGLE_STEPS);
                    r.radar = (r.radar + rnd(181) - 90).rem_euclid(ANGLE_STEPS);
                    r.x = (r.x + rnd(33) - 16).clamp(0, w);
                    r.y = (r.y + rnd(33) - 16).clamp(0, h);
                    r.energy = (r.energy - rnd(400) + rnd(30)).max(0);
                }
                r.state = match rnd(40) {
                    0 => RobotState::HitWall,
                    1 => RobotState::HitRobot,
                    2 if rnd(10) == 0 => RobotState::Dead,
                    _ => RobotState::Active,
                };
                if prev[i].state == RobotState::Dead || rnd(if wild { 3 } else { 15 }) != 0 {
                    continue;
                }
                let count = if rnd(10) == 0 { 1 + rnd(8) } else { 1 };
                for _ in 0..count {
                    let power = if wild { rnd(100_000) } else { 10 + rnd(291) };
                    let end = match rnd(5) {
                        0 => None,
                        k => {
                            let at = t + rnd(i64::from(turns - t) + 1) as u32;
                            let kind = match k {
                                1 => BulletEnd::Wall,
                                2 => BulletEnd::Bullet,
                                _ => BulletEnd::Robot(rnd(n as i64) as u8),
                            };
                            Some((at, kind, edge(&mut rnd, w), edge(&mut rnd, h)))
                        }
                    };
                    track.bullets.push(BulletTrack {
                        owner: i as u8,
                        born: t,
                        x: if wild {
                            edge(&mut rnd, w)
                        } else {
                            r.x + rnd(81) - 40
                        },
                        y: if wild {
                            edge(&mut rnd, h)
                        } else {
                            r.y + rnd(81) - 40
                        },
                        heading: rnd(BULLET_ANGLE_STEPS),
                        power,
                        end,
                    });
                }
            }
            track.frames.push(frame.clone());
        }
        track
    }

    #[test]
    fn random_tracks_round_trip() {
        for seed in 0..400 {
            let track = random_track(seed);
            assert_eq!(decode(&encode(&track)).unwrap(), track, "seed {seed}");
        }
    }

    #[test]
    fn engine_version_is_in_the_header() {
        let mut track = synthetic();
        for v in [0, 1, 127, 128, 300, u32::MAX] {
            track.engine_version = v;
            let bytes = encode(&track);
            assert_eq!(bytes[0], VERSION);
            assert_eq!(decode(&bytes).unwrap().engine_version, v);
        }
    }

    #[test]
    fn version_1_tracks_still_decode_with_engine_version_0() {
        // Version 1 had no engine version: the coded stream follows the version byte.
        let mut track = synthetic();
        track.engine_version = 0;
        let v2 = encode(&track);
        assert_eq!(v2[1], 0, "engine version 0 is one LEB128 byte");
        let mut v1 = vec![VERSION_1];
        v1.extend_from_slice(&v2[2..]);
        assert_eq!(decode(&v1).unwrap(), track);
    }

    #[test]
    fn an_overlong_engine_version_is_rejected() {
        assert!(decode(&[VERSION, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0]).is_err());
        assert!(decode(&[VERSION, 0x80, 0x80, 0x80, 0x80, 0x80, 0, 0]).is_err());
    }

    #[test]
    fn empty_round() {
        let track = Track {
            engine_version: 1,
            width: 800,
            height: 600,
            gun_cooling_milli: 100,
            frames: vec![vec![RobotFrame::default(); 2]],
            bullets: vec![],
        };
        assert_eq!(decode(&encode(&track)).unwrap(), track);
    }

    #[test]
    fn corrupt_input_is_rejected_without_panicking() {
        let good = encode(&synthetic());
        let mut seed = 1u64;
        for len in [0, 1, 2, 5, 17, 100, good.len()] {
            for _ in 0..50 {
                let mut bytes: Vec<u8> = (0..len)
                    .map(|_| {
                        seed = seed
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        (seed >> 56) as u8
                    })
                    .collect();
                if let Some(b) = bytes.first_mut() {
                    *b = VERSION;
                }
                let _ = decode(&bytes);
            }
        }
        // Truncations of a valid replay.
        for cut in [1, 10, good.len() / 2, good.len() - 1] {
            let _ = decode(&good[..cut]);
        }
    }
}
