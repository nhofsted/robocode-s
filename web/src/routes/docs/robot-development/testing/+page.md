---
title: Running and testing
description: Run battles of your robot from the command line or the GUI, reproduce them with seeds and measure its strength over many battles.
---

## Run battles

Run headless battles with [`robocode-s battle`](/docs/cli/battle): choose robots, number of rounds, battlefield size and a **seed**, and get per-round results and, with `--record`, a recording. The desktop GUI does the same with a live view, speed control, pausing and single-stepping, and your robot's debug painting.

Your robot can be a jar or a **class folder**: `--jar path/to/classes` (or a folder added in the GUI's options) reads your IDE's output in place, as classic's development folders do. Recompile, and the next battle runs the new code, with no packaging step in the loop. Name it with classic's `*`: `jd.MyRobot*`, or `jd.MyRobot 1.0*` when its `.properties` has a version.

A battle can also be watched from another machine (`--observe`), or run its robots on other machines ([`robocode-s agent`](/docs/cli/agent)).

## Test it

Battles are random: start positions and a lot of robot behavior differ every round. Two habits make testing meaningful.

**Use seeds and deterministic mode to reproduce.** In robocode-s's deterministic mode, the same robots and the same seed give the same battle, on every machine, every time. That covers the engine's randomness, your robot's `Math.random()` and `new Random()` (each robot gets its own stream derived from the seed and its name), and even the clock (see [deterministic mode](/docs/robot-development/deterministic-mode)). When you see a bug, note the seed: you can replay that exact battle until it is fixed.

**Measure over many battles.** A single battle says little; compare versions over many battles against a fixed set of opponents (a "test bed") and look at the average score. The rumble's numbers are averages over hundreds of pairings for the same reason. Keep a few opponents of each style in your test bed: a strong surfer, a pattern matcher, a rammer, a wall bot, and the samples for sanity.

> **Status:** batch runs over a test bed with a summary table are planned; for now, run `robocode-s battle` in a loop over seeds.

Next: [debug it](/docs/robot-development/debugging).
