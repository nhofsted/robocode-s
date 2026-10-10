# 0010. `Utils.getRandom()` returns a per-robot generator

- Status: Accepted
- Date: 2026-10-10

## Context

In classic Robocode, `robocode.util.Utils.getRandom()` returns the engine's own `java.util.Random`, the one that also shuffles robots and bullets every turn (engine-spec §3). A robot drawing from it shifts every later engine decision. robocode-s runs robots in their own processes, so they can't share the engine's generator.

## Decision

`Utils.getRandom()` returns one `java.util.Random` per robot, seeded with `mix64(base ^ 0x5554494C53524E44)` ("UTILSRND"), where `base` is the per-robot base seed of ADR 0009.

- `java-compat` implements this directly.
- The difftest oracle patches classic's `Utils.getRandom()` the same way for robot threads (`ClassicPatches`, hook `RobotRandomness.UTILS_RANDOM_KEY`), so recordings stay comparable. Engine threads keep classic's behavior.

## Consequences

- A deliberate deviation from classic for robots that use `Utils.getRandom()`. None of the checked top bots or samples do.
- The golden value in `RobotRandomnessTest` pins the derivation for other implementations.
