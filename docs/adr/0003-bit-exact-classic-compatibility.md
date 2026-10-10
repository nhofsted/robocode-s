# 0003. Bit-exact compatibility with classic Robocode, against StrictMath

- Status: Accepted
- Date: 2026-10-10

## Context

Competitive robots depend on exact physics and event ordering. Classic Robocode uses `java.lang.Math`, whose HotSpot intrinsics may differ by 1 ulp from `StrictMath` between JVMs and CPUs.

## Decision

For a given seed, robocode-s produces the same turn-by-turn state as classic Robocode 1.11.1 running with `StrictMath` (fdlibm) semantics. The engine uses a Rust port of fdlibm and of `java.util.Random`. A differential test harness against classic is the acceptance criterion (design §4.3, §13).

## Consequences

The engine is exact on every platform. Classic running on a JVM with intrinsics may differ in rare cases; that is classic's own nondeterminism, not a bug of ours.
