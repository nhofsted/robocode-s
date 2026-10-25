---
title: Debugging
description: Find out why your robot does what it does, with output, debug painting, your IDE's debugger, replays and re-runs of rumble battles.
---

**Print and paint first.** `out.println` output goes to the robot's console, and `onPaint(Graphics2D)` draws on the battlefield: waves, predicted positions, danger maps. In the GUI you switch painting on per robot (the pencil in its output tab), and `--record ... --paint` keeps it in a recording. Painting is only requested while a viewer shows it, so it costs nothing in normal battles.

**Use your IDE's debugger.** Because your robot is its own JVM, you can debug it like any Java program. The IDE starts the robot, and robocode-s waits for it: breakpoints, stepping and watches work as usual. [Using an IDE](/docs/ide#debug-it-with-your-ides-debugger) has the setup; [`robocode-s ide-config`](/docs/cli/ide-config) prints the run configuration.

Use deterministic mode while debugging. In classic mode, a robot paused at a breakpoint misses its turns and gets disabled. In deterministic mode budgets count the work your code does, not time, and a paused robot does no work, so you can step through a turn and continue the same battle; the debug option only switches off the server's wall-clock safety check for that robot.

One caveat: debugger features that run your code (watch expressions, `toString()` in the variables view) count as work and can change your robot's state, so the battle may then go differently than without the debugger. Combine it with a seed to stop at the exact turn where things go wrong.

**Replay what happened.** [`robocode-s replay FILE`](/docs/cli/replay) (or the GUI's Watch Battle) re-runs a recording turn for turn, with the robots' output and debug painting.

**Reproduce rumble battles.** Every battle the rumble runs has a seed and runs in deterministic mode, so any rumble battle can be re-run locally, turn for turn. The [replays on the website](/docs/rumble#replays) show what happened (each shows the seed and the engine version); a local re-run under your debugger shows why. The quickest way: download the round's `.track`, open it in the GUI's Watch Battle and press **Run this battle**. The GUI fetches the robots it doesn't have from the rumble, runs the battle as the rumble did, and pauses at the start of that round, with names, scores, console output and debug properties.

Next: [deterministic mode](/docs/robot-development/deterministic-mode), in detail.
