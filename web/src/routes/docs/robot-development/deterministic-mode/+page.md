---
title: Deterministic mode
description: How robocode-s makes a battle repeatable with a virtual clock, seeded randomness and counted work, and what it means for robot authors.
---

The rumble runs every battle in deterministic mode, so results can be checked by running a battle twice. It is also the default for every battle you run. It differs from classic mode in a few ways that matter to robot authors:

- **The clock is virtual.** In robot code, `System.nanoTime()` and `System.currentTimeMillis()` return a virtual time that advances with the turns and with the work your code does (counted in bytecode instructions). A loop that runs "until 2 ms have passed" does a fixed amount of work, the same on every machine. Seeding a generator from the clock gives the same seed every run.
- **Randomness is seeded per robot**, from the battle seed and the robot's name: `Math.random()`, `new Random()` and `Utils.getRandom()`.
- **Identity hash codes are deterministic** for objects your robot creates, so iterating over a `HashSet` of your own objects has the same order every run.
- **Turns are skipped by counted work, not by the clock.** The engine waits for every robot. A robot whose work in a turn is over the budget (3.2 ms of virtual time, how long classic really waits for a robot) answers as many turns late as classic would have, with classic's skipped-turn events, and is disabled after too many. A robot that never returns, such as an endless loop, is cut off at a work limit.
- **Not covered:** time the JDK reads for you (`Thread.sleep`, timeouts in concurrent code) is still real, so robots whose threads race against each other stay nondeterministic.

If your robot budgets its own computation by the clock, check that it still does enough work per turn under the virtual clock; virtual time was calibrated against real CPU time for this ([#15](https://github.com/nhofsted/robocode-s/issues/15)). `--classic` shows how your robot behaves under classic's real timing.

Next: [package and release it](/docs/robot-development/release).
