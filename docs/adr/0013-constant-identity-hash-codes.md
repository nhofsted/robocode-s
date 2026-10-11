# 0013. Constant identity hash codes in deterministic mode

- Status: Accepted
- Date: 2026-10-11

## Context

Many RoboRumble bots keep their own objects (waves, enemies, bullets) in `HashMap`s, `HashSet`s or `Hashtable`s without overriding `hashCode()`. The iteration order then follows the objects' **identity hash codes**.

HotSpot's default (`-XX:hashCode=5`) generates them per thread, from a state seeded according to how many threads the JVM created before. That includes JIT compiler threads, which HotSpot starts depending on load. The same robot therefore iterates in a different order from one run to the next, and summing doubles in a different order changes the last bits. Through the RoboRumble corpus (#11) we saw:
- classic recordings of the same seeded melee diverging, especially under load;
- live battles differing from classic in the last bits of a velocity or position.

Classic runs all robots in one JVM, while robocode-s runs each robot in its own JVM, so even a stable per-JVM sequence would differ between the two.

## Decision

In deterministic mode, robot JVMs, and the difftest oracle, run with `-XX:+UnlockExperimentalVMOptions -XX:hashCode=2`: every identity hash code is 1. Iteration order then depends only on what the robot itself does (insertion order within the one bucket), the same in one JVM or many. The oracle refuses to run without it.

## Consequences

- **Verified:** the corpus battles that differed in the last bits are now identical. Two classic recordings of the same battle are byte-identical, also under load.
- **Cost:** hash structures keyed by identity degrade to linear search, as do `IdentityHashMap` and serialization handle tables. Only in deterministic mode; a robot with very large identity-keyed maps runs slower there.
- **Changed behavior:** robots see different identity hash codes and `Object.toString()` values (`…@1`) than on a normal JVM. Classic never had a reproducible answer there either.
- **Remaining sources:** wall-clock time and the robot's own threads (ADR 0009) aren't covered. Robots that depend on them stay nondeterministic, in classic too, and are excluded from the difftest corpus with the reason.
