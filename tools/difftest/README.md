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

### Format (version 2)

One JSON object per line:

- `header`: format version, robot-randomness version, classic version, JVM, seed, rules, robot names (in index order).
- `roundStart`: the state at turn 0 of each round.
- `turn`: the state after each turn.
- `results`: classic's indexed battle results. Note: classic's `rank` here is the contestant's list position, not its placement.

A state contains, per robot (in index order): `state`, `x`, `y`, `energy`, `velocity`, `bodyHeading`, `gunHeading`, `radarHeading`, `gunHeat`, `score`, `totalScore`; and per bullet: `owner`, `id`, `state`, `x`, `y`, `heading`, `power`, `victim`. Doubles are written in Java's shortest round-trip form, so parsing them gives the exact bits back.

Each `turn` line also has, per robot (in index order), what classic's engine did in that turn. These come from `ClassicPatches` hooks in `RobotPeer` (`OracleHooks`), which only observe:
- `commands`: the command state loaded at the start of the turn (`performLoadCommands`, engine-spec §11.1). This includes the remaining values, adjust flags, max velocity and turn rate, `scan`, `io`, bullet commands (`id`, `power`, fire assist, `consumed`) and team messages (`sender`, `recipient`, `bytes`). A replay of these drives the robocode-s engine without real robots. `null` for robots that are dead: their thread may still be finishing its last `execute` and replaces the command state at an unpredictable moment (this made CI fail). Classic ignores a dead robot's commands.
- `remaining`: the remaining body/gun/radar turn and distance reported back at the end of the turn (`publishStatus`); `null` for dead robots, for the same reason.
- `events`: every event the engine generates for the robot, in order, with all fields (radians; bullets with id, owner, victim, position, power and active flag). Classic only queues an event while the robot's thread runs, which is timing-dependent shortly after a death; delivery is the server's job in robocode-s.

Recording costs about 1% of the battle time.

### Known limits

- Robots that read the clock (`System.nanoTime`, `currentTimeMillis`) to budget their own computation can still behave differently between runs. None of the tested top bots did, but four of the six read the clock.
- Robot console output is buffered by the robot threads, so the turn in which it appears varies. That's why it's opt-in.

## Engine replay

`crates/difftest` replays recordings in the robocode-s engine: it feeds each turn's recorded commands into the engine and compares robot states, bullets, remaining values and events bit for bit, reporting the first difference.

```sh
tools/difftest/record-samples.sh /tmp/recordings          # 9 sample battles, ~12 s
cargo run --release -p robocode-s-difftest --bin replay -- /tmp/recordings/*.jsonl.gz
ROBOCODES_RECORDINGS=/tmp/recordings cargo test -p robocode-s-difftest
```

CI records the sample corpus and replays it on every push (job "Engine replay against classic"). Top RoboRumble bots are not in CI: their jars belong to other authors. Pass them as the second argument of `record-samples.sh`, or record by hand with `--jars`.

## Live battles

`live` fights the battle of each recording with real robot processes: the server, the runner and one robot-host JVM per robot. It then compares every turn with the recording. The result must be identical.

```sh
./gradlew :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
cargo run --release -p robocode-s-difftest --bin live -- /tmp/recordings/*.jsonl.gz
```

`ROBOCODES_JAVA` selects the Java runtime; without it the pinned Temurin JRE is downloaded. `ROBOCODES_ROBOT_JARS` (a path list) adds robot jars, e.g. RoboRumble bots.

## RoboRumble corpus

Sample robots are too simple to cover classic's corners, so #11 also tests a fixed, seeded selection of **real RoboRumble robots** (duels, melee, teams). `corpus/rumble-corpus.json` lists them with source URL and SHA-256. The jars themselves aren't committed, because they belong to their authors.

```sh
# Download exactly the robots in the manifest (verified by SHA-256)
python3 tools/difftest/corpus/fetch-corpus.py download --manifest tools/difftest/corpus/rumble-corpus.json --jars /tmp/corpus-jars
# Fight the seeded battle plan in classic and live, and compare every turn
python3 tools/difftest/corpus/run-corpus.py --manifest tools/difftest/corpus/rumble-corpus.json \
  --jars /tmp/corpus-jars --out /tmp/corpus-run --check-oracle
```

`fetch-corpus.py select --avoid <earlier manifests>` draws a new batch of robots that weren't tested before (e.g. `batch-2.json`).

The runner works in two phases:
- **Classic recordings** run with `--oracle-jobs` parallelism (default 4). Classic is only reproducible at moderate load (#16), and a recording is stopped after 15 minutes (a robot stuck in a loop).
- **Live battles** run with `--jobs` parallelism. Robot JVMs need memory, so `--big-jobs` limits the melee and team battles running at once. Each result is one of:
- **`OK`:** identical.
- **`FAIL`:** a robocode-s difference.
- **`RACE`:** a known classic race (engine-spec §15.3). Only the rounds before it count.
- **`ORACLE-NONDETERMINISTIC`:** classic itself isn't reproducible for this battle; reported with `--check-oracle`, which records every battle twice.

Robots that make classic itself nondeterministic are kept in the manifest with an `exclude` reason and left out of the plan. So are robots that can loop forever without an action: classic only ends that through wall-clock skipped turns, which become reproducible with CPU budgets (#15).

To fight some battles of an earlier run again, e.g. after a fix: `--plan <out>/plan.json --only 25-duel,31-melee`.
