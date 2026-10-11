# 0013. Deterministic identity hash codes for robots

- Status: Accepted
- Date: 2026-10-11

## Context

Many RoboRumble bots keep their own objects (waves, enemies, bullets) in `HashMap`s, `HashSet`s or `Hashtable`s without overriding `hashCode()`. The iteration order then follows the objects' **identity hash codes**.

HotSpot's default (`-XX:hashCode=5`) generates them per thread, from a state seeded according to how many threads the JVM created before. That includes JIT compiler threads, which HotSpot starts depending on load. The same robot therefore iterates in a different order from one run to the next, and summing doubles in a different order changes the last bits. Through the RoboRumble corpus (#11) we saw:
- classic recordings of the same seeded melee diverging, especially under load;
- live battles differing from classic in the last bits of a velocity or position.

Classic runs all robots in one JVM, while robocode-s runs each robot in its own JVM, so even a stable per-JVM sequence would differ between the two.

## Decision

The determinism agent gives robot objects identity hash codes from a **per-robot stream**, like the random streams (ADR 0009):
- **Robot classes:** when classic's robot class loader defines one, and it is the top robot class of a hierarchy that would inherit `Object.hashCode()`, the agent adds a `hashCode()`. On first use it takes the robot's next value (`RobotRandomness.identityHashCode`: SplitMix64 over the robot's base seed; 31 bits, never 0, like HotSpot's) and keeps it in an added transient field. Classes with their own `hashCode()` are left alone.
- **Serialization:** for serializable classes the agent first fixes the `serialVersionUID` to the original class's default, so serialized data stays readable.
- **Enums:** `Enum.hashCode()` is final, so its call to `System.identityHashCode` asks the same hook. It uses the robot's stream only for the robot's **own** enums. JDK and other enums keep the JVM's hash: code that isn't the robot's also hashes them on the robot thread (in robocode-s, the robot host's protocol code), which would shift the robot's sequence. A first version did that and diverged from classic.
- **Everything else** (other threads, JDK classes) keeps the JVM's own identity hash codes. Robot code that calls `System.identityHashCode` directly gets a warning on stderr.

Values only depend on the battle seed, the robot's name and the order in which the robot hashes its objects: the same in classic (one JVM) and robocode-s (one JVM per robot), on every machine.

**First approach, replaced the same day:** `-XX:hashCode=2` (every identity hash code 1) also made the corpus deterministic. But it degrades every identity-keyed hash structure in the JVM to a linear search.

## Consequences

- Hash collections perform normally; each robot object costs one extra `int`.
- Robots see different identity hash codes and `Object.toString()` values than on a normal JVM. Classic never had a reproducible answer there either.
- **Not covered:** JDK objects without their own `hashCode()` used as keys (`new Object()`, `Class`, `Thread`, JDK enums); direct `System.identityHashCode` calls. Rare in rumble bots; the corpus's double classic recordings detect what remains.
- **Remaining sources:** wall-clock time (#15) and robots' own timing-dependent threads.
