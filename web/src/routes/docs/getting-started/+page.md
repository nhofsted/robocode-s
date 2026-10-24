---
title: Getting started
description: Install robocode-s, run a battle, write a robot and see where to go next.
---

> **Status:** version 0.2.1 is out for Windows, Linux and macOS.

## 1. Install robocode-s

Pick your system on the [download page](/download). The installers put the window (`robocode-s-gui`), the command line (`robocode-s`), the robot host and the sample robots in one place. The Java runtime for robots (about 50 MB) is fetched on first use.

## 2. Run a battle

In the window: open `robocode-s-gui`, choose robots on the Start Battle tab and press Start. You see the battle live, with the robots' output and debug painting, speed control, pausing and single steps.

From a terminal:

```sh
robocode-s battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
```

Every robot runs in its own JVM, in a sandbox. Battles are deterministic by default: the same robots and seed give the same battle, on every machine. A recording (`--record FILE`) can be replayed in the window (Watch Battle) or with `robocode-s replay FILE`. All options are in the [command line reference](/docs/cli/battle).

## 3. Write a robot

Robots for robocode-s are classic Robocode robots: everything you write against the `robocode.*` API (`Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot`) runs unmodified. Set up a project in your IDE ([Using an IDE](/docs/ide)), compile it, and point robocode-s at the folder your IDE compiles into:

```sh
robocode-s battle my.Robot*,sample.Walls --jar build/classes/java/main
```

Recompile and the next battle runs the new code. [Developing a robot](/docs/robot-development) covers the project, testing, debugging and packaging.

## 4. Next

- [Developing a robot](/docs/robot-development): set up, test, debug and release.
- [Using an IDE](/docs/ide): IntelliJ IDEA, Eclipse and VS Code, including debugging from the IDE.
- [Command line](/docs/cli): every command and option.
- [The rumble](/docs/rumble): enter your robot in the league, or [contribute battles](/docs/rumble/contributing).
