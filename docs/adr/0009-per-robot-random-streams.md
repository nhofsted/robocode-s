# 0009. Per-robot random streams via a Java agent

- Status: Accepted
- Date: 2026-10-10

## Context

Top RoboRumble bots use `Math.random()` (all six we checked). Classic Robocode seeds `Math.random()` by replacing a field inside `java.lang.Math` with reflection, which fails on modern JVMs. It never seeded `new Random()` at all. Even when it worked, it was **one shared stream** for all robots in the JVM, so the numbers a robot got depended on the other robots.

robocode-s runs each robot in its own process, so it could never reproduce a shared stream. Without seeded robot randomness, neither deterministic mode (ADR 0005) nor differential testing against classic is possible: two runs of BeepBoop vs DrussGT diverged at turn 9.

## Decision

Each robot gets its **own** random streams, derived only from the battle seed and the robot's unique name:

```
base        = mix64(battleSeed) ^ mix64(fnv1a64(utf8(robotName)))
Math.random = new java.util.Random(mix64(base ^ 0x4D41544852414E44)).nextDouble()
k-th new Random() (k = 1, 2, ...): seed = mix64(base + k * 0x9E3779B97F4A7C15)
```

`mix64` is the SplitMix64 finalizer; `robotName` is classic's unique robot name (e.g. `jk.mega.DrussGT 3.1.16`, `sample.Fire (2)` for duplicates).

For JVM robots this is done by a Java agent (`clients/java-determinism`) that rewrites `Math.random()`, `StrictMath.random()` and `Random()` at JVM start. Robots are recognized by their thread group, which classic names after the robot; threads a robot starts inherit it. The difftest oracle uses the agent, and so will the robocode-s Java runner. Golden values in `RobotRandomnessTest` pin the derivation for other implementations.

## Consequences

- Oracle recordings with top bots are byte-identical across runs, including 10-robot melees.
- The numbers robots draw differ from what classic would draw on an old JVM. Classic never had a reproducible answer there, so nothing authentic is lost; physics and rules are untouched.
- Deterministic mode must run robot JVMs with the agent and with the inexact math intrinsics disabled (design §4.3), so robots' own trigonometry is identical on every platform and volunteers' results can be cross-checked.
- Robots that read the clock to budget their work can still be nondeterministic. That's a known limit.
