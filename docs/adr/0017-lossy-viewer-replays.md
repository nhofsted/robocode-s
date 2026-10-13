# 0017. Rumble replays are lossy viewer tracks, not re-simulated intents

- Status: Accepted
- Date: 2026-10-12

## Context

The design planned browser replays as per-round robot intents, re-run by the engine compiled to WebAssembly, at an estimated 10–20 KB per 1v1 round (design §11, §12.7). We measured it on oracle recordings of top RoboRumble bots (Diamond vs Shadow, BeepBoop vs DrussGT, Nullstride vs BeepBoop):

- Top bots send **new full-precision doubles** for body, gun and radar turn on 87–99% of turns. Only maximum turn rate, often maximum velocity, distance and bullet power are predictable.
- Below the top 10–16 bits, these mantissas are noise. Predicting them from what the engine reports back, from a value cache, or from recognised formulas (radar lock, head-on aim) saves 1–2 bytes per value at best; formulas match the robots' exact bits less than 1% of the time.
- Lossless intents therefore cost **37–39 bytes per turn** after xz, and top-bot rounds are long: 2,600–7,300 turns for BeepBoop vs DrussGT, so **100–300 KB per round**.

Nobody watching a replay can see bit-exactness. Exactness matters for verification and debugging, and battles are deterministic (ADR 0005, 0014): seed, bot versions and round index reproduce a round exactly.

## Decision

- **Ground truth is the battle itself.** A rumble round is identified by seed, bot versions and round index. Anyone can reproduce it exactly with the robots' jars, which the rumble mirrors.
- **What the browser plays is a lossy viewer track** (`crates/replay`): one round as the viewer draws it, quantized to what a screen can show:
  - positions in ½ px;
  - body, gun and radar headings in 1/1024 of a turn (0.35°; a gun barrel's tip moves about 0.1 px per step);
  - energy and bullet power in 0.01;
  - bullets as birth (origin, heading in 1/65536 of a turn, power) and end (turn, wall, bullet or victim robot, explosion point).
- **The codec is lossless over that track.** It codes each value against a prediction from what the decoder has already reconstructed (closed loop, so errors never accumulate). The residuals go through an adaptive binary range coder.
- **The tricks:**
  - Backward-adaptive choice among predictors, so the choice costs no bits: last value, linear, the bearing to the nearest enemy (radar and gun follow it, bodies orbit it), radar lock (twice the offset mirrors the radar around the bearing), and width lock (overshoot by the arc of the last turn).
  - Classic's movement: turn, then move along the new heading. Only the speed change is coded.
  - Classic's energy rules: fire costs, hit damage and bonuses are predicted from the bullets.
  - Gun heat is tracked as the context for the "fired" bit.
  - The bullet flight is derived from the rules; drawn paths stay within 1 px of classic's.
  - Robot death explosions are implied by the death.
  - No strings, names or container: the S3 key identifies the matchup. The always-zero first byte of the range coder and its trailing zeros are trimmed.
  - Deterministic float math (fdlibm), so every volunteer encodes the same round to the same bytes, and the upload hash can be checked against the reported one.
- **Not in rumble replays:** debug paint, console output, scores and gun heat. Rumble battles run with painting disabled.
- **Desktop recordings (design §11) are unchanged:** they keep intents and keyframes, for exact replay with debug paint and scrubbing by re-simulation.

## Consequences

- Measured on the oracle corpus (`replay-size`): **3.46 bytes per turn** overall; top 1v1 bots 3.3–3.8. That is about 3.5 KB for a 900-turn round and 20 KB for a 6,000-turn BeepBoop vs DrussGT round, **10–11× smaller** than lossless intents. Top 10-bot melee: 8 bytes per turn.
- Most of what remains is real decisions: where the gun aims (GF guns re-aim every turn), speed changes and turns. Coarser angles save less than a bit per halving.
- The browser viewer doesn't need the engine. M5b becomes a canvas player for tracks. Scrubbing decodes the round once and then jumps between its frames.
- A rumble replay can't be re-simulated or shown with debug paint. To debug, authors reproduce the battle locally from its seed (exact) and record it with the desktop GUI.
- The format is version 1 of a prototype. The quantization and predictors may still change before the rumble stores replays; the version byte allows that.
