# difftest

Differential testing against classic Robocode (design §13).

## Oracle

`Oracle` runs a battle in classic Robocode 1.11.1 (the official jars from Maven Central) and records every turn as JSON Lines. These recordings are the reference that the robocode-s engine is compared against.

```sh
./gradlew :tools:difftest:run --args="--robots sample.Fire,sample.Walls --seed 42 --rounds 10 --out /tmp/fire-walls-42.jsonl.gz"
```

| Option | Meaning |
|---|---|
| `--robots` | Comma-separated robot class names, as in classic's battle files |
| `--seed` | Battle seed (classic `RANDOMSEED` and the per-robot random streams) |
| `--rounds` | Number of rounds (default 10) |
| `--width`, `--height` | Battlefield size (default 800×600) |
| `--jars` | Extra robot jars, as a path list (e.g. downloaded RoboRumble bots) |
| `--with-output` | Also record robot console output (not deterministic, see below) |
| `--out` | Output file; gzip-compressed when it ends in `.gz` |

The classic sample robots are always available.

### How the recordings are made reproducible

Same seed and robots give a **byte-identical** recording. This is verified by `OracleTest`, and by hand for top RoboRumble bots (BeepBoop vs DrussGT; a melee with Neuromancer, Shadow, Diamond, Nullstride and others). Four things make this work:

1. **`-Ddebug=true`:** classic waits for every robot each turn (up to 10 minutes), so there are no timing-dependent skipped turns.
2. **`-XX:-InlineMathNatives`:** HotSpot's `Math.sin/cos/tan` intrinsics differ from `StrictMath` for about 3% of inputs on x86-64. Without them, classic computes with `StrictMath` (fdlibm) semantics, which is the definition of "bit-exact" in ADR 0003.
3. **`RANDOMSEED`:** seeds classic's own randomness (start positions, robot order each turn).
4. **The determinism agent (`clients/java-determinism`):** classic cannot seed robots' own `Math.random()` on modern JVMs, and never seeded `new Random()`. The agent gives each robot its own stream, derived from the battle seed and the robot's name (ADR 0009). robocode-s runners use the same derivation, so robots draw the same numbers in both systems.

5. **Classic patches (`ClassicPatches`):** `Utils.getRandom()` returns the robot's own generator on robot threads, as in robocode-s (ADR 0010). And a timing fix: `RobotPeer.startRound` in classic 1.11.1 has a lost-wakeup race. If a robot falls asleep before the battle thread starts waiting for it, the battle waits for the full timeout: 10 minutes with `-Ddebug=true`. The difftest jar is also loaded as an agent that makes this one wait conditional, as classic's `waitSleeping` already does. This changes timing only, never results.

The oracle checks 1, 2, 4 and 5 at startup and refuses to run without them. The Gradle `run` task sets everything up.

Classic's security mode stays on, as in the RoboRumble. On Java 21 that means class-loading and thread checks only: there is no `SecurityManager` sandbox anymore, so only run robots you would also run in a RoboRumble client.

### Format (version 1)

One JSON object per line:

- `header`: format version, robot-randomness version, classic version, JVM, seed, rules, robot names (in index order).
- `roundStart`: the state at turn 0 of each round.
- `turn`: the state after each turn.
- `results`: classic's indexed battle results. Note: classic's `rank` here is the contestant's list position, not its placement.

A state contains, per robot (in index order): `state`, `x`, `y`, `energy`, `velocity`, `bodyHeading`, `gunHeading`, `radarHeading`, `gunHeat`, `score`, `totalScore`; and per bullet: `owner`, `id`, `state`, `x`, `y`, `heading`, `power`, `victim`. Doubles are written in Java's shortest round-trip form, so parsing them gives the exact bits back.

Events are not in classic's snapshots; recording them needs a robot-side hook (future work).

### Known limits

- Robots that read the clock (`System.nanoTime`, `currentTimeMillis`) to budget their own computation can still behave differently between runs. None of the tested top bots did, but four of the six read the clock.
- Robot console output is buffered by the robot threads, so the turn in which it appears varies. That's why it's opt-in.
