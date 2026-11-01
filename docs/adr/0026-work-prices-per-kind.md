# 0026. Work prices per kind of instruction and JDK call

- Status: Accepted (the owner's decisions of 2026-11-01)
- Date: 2026-11-01

## Context

Deterministic mode counts robot code's work (ADR 0014) and skips a robot's turns when its counted work exceeds the turn budget (ADR 0018). Rules 1 and 2 count every bytecode instruction alike at 160 ps, plus a flat 10 instructions per JDK call and per-element costs for sized JDK calls. 160 ps was the median of 10 heavy robots of the 2026-10-17 calibration, whose real costs ranged 45-209 ps per counted instruction.

The rumble's rankings (2026-10-31, 252k 1v1 battles) matched LiteRumble's closely (Spearman 0.994), except for robots far outside sampling noise. Two were traced to the price (`robocode-s battle --robot-property`, PR #157):
- **asd.Cthulhu 1.2** (a neural-net gun): 6 APS against LiteRumble's 39; 11% against sample.Walls (LiteRumble 95%). Its work grows to ~28.6M counted instructions a turn, 4.6 ms at 160 ps against the 3.2 ms budget: a turn late every other turn. Its real cost is ~55-60 ps per counted instruction.
- **stelo.MatchupAGF 1.1**: 47 APS against 68. A fixed 25.3M-instruction turn every ~10 turns, charged 4.05 ms, real ~2.9 ms.

Benchmarks of one kind of work each (`robots/testbots` `calib.*`) measured in classic: 42-90 ps per counted instruction for inlined calls, int loops, array work like a neural net's; 90-230 ps for fields, collections, clone, arraycopy; 370-640 ps for division, cache misses, `Vector`, `sin`/`cos`/`atan2` (StrictMath-backed: their intrinsics are disabled, design §4.3); ~1750 ps for `exp`/`log`/`pow` and `new double[64]` (one instruction, but the JVM zeroes 64 elements). One price per instruction overcharges robots whose code the JIT runs fast, which then skip turns they don't skip in classic.

## Decision

- **Prices per kind** (`WorkPrices`, rules 3): a price per group of instructions (`WorkProfile.Group`: constants, local loads and stores, stack, int, int mul/div, float, float div, conversions, compares, branches, switches, array loads and stores, fields, statics, invokes, returns, `new`, new arrays, type checks, ...), a price per call for the JDK methods whose cost is known to differ (StrictMath-backed `Math` functions, `sqrt`, `Vector`, `Hashtable`, `HashMap.get/put/containsKey`) and a shared price for other JDK calls, a price per unit for sized JDK calls (fitted for `clone` of `double[]`, `arraycopy`, `Arrays.sort(int[])`, `Vector.contains`; others keep rules 2's relative weights), and a price per element of new arrays (`newarray`, `anewarray`). Counted in units of 10 ps (`PICOS_PER_UNIT`, was `PICOS_PER_INSTRUCTION` = 160); every instruction costs at least 1 unit, so every loop costs something and the work cutoff still ends endless loops. Prices are fixed when a class is instrumented: no cost at run time.
- **Calibration** (2026-11-01, classic 1.11.1, OpenJDK 21, x86-64 WSL2): `calibrate.py` ran the 22 benchmarks, 61 duels of the rumble's top 120 and the outliers, and 538 battles of the 15 corpus batches, metered (with `WorkProfile`: per robot, instructions by group, JDK calls by method, sized units, array elements) and unmetered (real CPU per robot thread). Neither run skips turns, so heavy robots stay in. Robots that read the clock more than once in 20 turns, or whose turns differ between the runs, are left out: 1491 robots. `fit-costs.py` fits real CPU = per turn + Σ price × count, without negative prices, weighted by relative error, with a mild pull towards priors; `write-prices.py` writes the table.
- **Generous** (owner, 2026-11-01: "err on the side of giving too much budget"): the fitted prices are scaled by 0.588, the 5th percentile of real/charged over held-out robots doing 300 000+ counted instructions a turn: 95% of them are charged no more than their real time. Held out (5 folds by battle, 222 robots), real/charged went from p5 0.27, median 1.13, spread p95/p5 10.3× (flat 160 ps) to p5 0.59, median 1.34, spread 5.2× (before the scale).
- **The virtual clock** reads the same counted units: a robot that times itself sees the time it is charged.
- **Versions:** `DETERMINISTIC_RULES` and `rulesVersion()` 3; `ENGINE_VERSION` stays 1 (owner: keep the rankings): the rumble purges the battles the new prices change and fights them again (`POST admin/purge`, rumble-backend §12.4), settles every verification item queued before the switch (`rep.settleBefore`), and deletes every recording (none can be shown to match the new rules).

## Consequences

- asd.Cthulhu 1.2 wins 97% against sample.Walls, stelo.MatchupAGF 1.1 94% against sample.Corners, with no skipped turns (LiteRumble: 95%, 94%).
- Robots heavy in the expensive kinds (StrictMath functions, synchronized collections, big new arrays) are charged more than in rules 2, but at most their real time for 95% of heavy robots; robots at the cheap end get more work per turn than before.
- One price per kind still can't fit every robot: 11 of 222 held-out heavy robots stay overcharged (worst myl.nano.Kakuru 1.20, about 2.6× its real time after the scale; with the flat 160 ps it was 8×).
- The calibration tooling stays for the next one (and for other languages, #15 step 3): `-Drobocodes.calibrate` with metering writes the profile; `rules-impact.py` finds the rumble bots a rules change affects.
