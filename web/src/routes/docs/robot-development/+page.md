---
title: Developing a robot
description: How to set up, test, debug and release a Robocode robot with robocode-s, from the first build to the rumble.
---

This guide covers the whole loop: setting up a robot project, running battles, finding out why your robot does what it does, and releasing a version to the rumble. It is written for Java robots on the classic API, the only kind robocode-s runs today; native APIs for other languages (Python in milestone M5; .NET, Rust, C/C++ and Kotlin in M8) are not started and will get their own sections.

> **Status:** running, testing and debugging battles works today from the command line and the GUI (see [Getting started](/docs/getting-started)). The parts marked *planned* depend on the rumble (M5) or on tools still to come.

## How your robot runs

In classic Robocode every robot is a thread inside the Robocode JVM. In robocode-s each robot is **its own process**:

- The **runner** starts a JVM for your robot, inside a sandbox, with the **robot host**: classic Robocode's own robot-side code (the `robocode.*` API, event handling, blocking calls), connected to the engine over the robocode-s protocol (on the process's stdin and stdout) instead of in-process calls. The JVMs are started while the battle is looked up, so a battle starts quickly.
- Every turn the engine sends your robot its status and events, your code runs until it calls `execute()` (directly, or through a blocking call such as `ahead(100)`), and the robot host sends your commands back.
- Team members are separate processes too; team messages travel through the server.

For you this mostly means things work as before, with a few benefits: a crash or an `OutOfMemoryError` only takes down your robot, your robot's CPU time is measured precisely per turn, and the robot is an ordinary JVM process you can attach a debugger to.

## Set up a project

A robot is a normal Java project that compiles against the classic API. With Gradle:

```kotlin
// build.gradle.kts
plugins { java }

java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }

repositories { mavenCentral() }

dependencies {
    // Provided by Robocode at runtime; never bundle it into your robot jar.
    compileOnly("net.sf.robocode:robocode.api:1.11.1")
}
```

With Maven, use `net.sf.robocode:robocode.api:1.11.1` with `<scope>provided</scope>`.

A good layout keeps one package per author, with your bots and their shared code in it:

```
src/main/java/yourinitials/
├── Kestrel.java          the robot (extends AdvancedRobot)
├── movement/             wave surfing, wall smoothing, ...
├── targeting/            guns
└── util/
src/main/resources/yourinitials/
└── Kestrel.properties    robot metadata (see Packaging)
```

> **Tip:** compile for the oldest Java version you need. The rumble runs robots on a current Temurin JRE, but if you also want your robot to run in classic RoboRumble clients, target what they run.

## Write the robot

Everything in the [classic API](https://robocode.sourceforge.io/docs/robocode/) works: `Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot` and `JuniorRobot`, custom events, data files, debug painting with `onPaint`, and `out.println` for console output. The [RoboWiki](https://robowiki.net/) remains the place to learn movement, targeting and strategy.

Some things to keep in mind:

- **One turn, one `execute()`.** Do your thinking between calls. If your code runs over the turn budget, the robot **skips the turn** (its previous commands stay in effect, you get a `SkippedTurnEvent`), and after too many skipped turns it is disabled. In the rumble's deterministic mode the budget is counted in work rather than time; see below.
- **Data files** (`getDataFile`, `RobocodeFileOutputStream`) work as in classic, with the same 200 KB quota per robot. They are kept from battle to battle, as in classic (a robot such as Sonda reads the statistics of earlier battles), in the data folder under `robot-data/<robot and version>`. `--fresh-data` (or `ROBOCODES_FRESH_DATA=1`) starts every robot with no files and keeps none: use it to reproduce a battle exactly, since with data files a battle also depends on the ones before it. Two robots of the same class in one battle share one folder.
- **No network, no files outside your data directory.** The sandbox enforces this (classic's SecurityManager denied the same): sockets and connections fail with an exception, other files can't be read or written, and a robot can't start other processes. On Linux the default `os` sandbox limits files only where the kernel has Landlock; the battle's header says `filesystem not limited` if it doesn't. `--sandbox none` is a plain process, for local experiments only: treat the limits as the rule anyway, since the rumble will.
- **Threads are discouraged.** They still work, but anything timing-dependent between threads makes your robot nondeterministic.

## Run battles

Run headless battles with `robocode-s battle`: choose robots, number of rounds, battlefield size and a **seed**, and get per-round results and, with `--record`, a recording. The desktop GUI does the same with a live view, speed control, pausing and single-stepping, and your robot's debug painting. See [Getting started](/docs/getting-started#run-a-battle-from-the-command-line) for the commands.

Your robot can be a jar or a **class folder**: `--jar path/to/classes` (or a folder added in the GUI's options) reads your IDE's output in place, as classic's development folders do. Recompile, and the next battle runs the new code, with no packaging step in the loop. Name it with classic's `*`: `my.Robot*`, or `my.Robot 1.0*` when its `.properties` has a version.

A battle can also be watched from another machine (`--observe`), or run its robots on other machines (`robocode-s agent`); both are described in Getting started.

## Test it

Battles are random: start positions and a lot of robot behavior differ every round. Two habits make testing meaningful.

**Use seeds and deterministic mode to reproduce.** In robocode-s's deterministic mode, the same robots and the same seed give the same battle, on every machine, every time. That covers the engine's randomness, your robot's `Math.random()` and `new Random()` (each robot gets its own stream derived from the seed and its name), and even the clock (see [Deterministic mode](#deterministic-mode)). When you see a bug, note the seed: you can replay that exact battle until it is fixed.

**Measure over many battles.** A single battle says little; compare versions over many battles against a fixed set of opponents (a "test bed") and look at the average score. The rumble's numbers are averages over hundreds of pairings for the same reason. Keep a few opponents of each style in your test bed: a strong surfer, a pattern matcher, a rammer, a wall bot, and the samples for sanity.

> **Status:** batch runs over a test bed with a summary table are planned; for now, run `robocode-s battle` in a loop over seeds.

## Debug it

**Print and paint first.** `out.println` output goes to the robot's console, and `onPaint(Graphics2D)` draws on the battlefield: waves, predicted positions, danger maps. In the GUI you switch painting on per robot (the pencil in its output tab), and `--record ... --paint` keeps it in a recording. Painting is only requested while a viewer shows it, so it costs nothing in normal battles.

**Attach a debugger.** Because your robot is its own JVM, you can debug it like any Java program: start the robot's JVM with a debug agent and attach your IDE.

```text
-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=localhost:5005
```

> **Status:** planned ([#17](https://github.com/nhofsted/robocode-s/issues/17)): an option for the runner and GUI to start one robot with that flag and wait for your IDE.

Use deterministic mode while debugging. In classic mode, a robot paused at a breakpoint misses its turns and gets disabled. In deterministic mode budgets count the work your code does, not time, and a paused robot does no work, so you can step through a turn and continue the same battle; the debug option only switches off the server's wall-clock safety check for that robot.

One caveat: debugger features that run your code (watch expressions, `toString()` in the variables view) count as work and can change your robot's state, so the battle may then go differently than without the debugger. Combine it with a seed to stop at the exact turn where things go wrong.

**Replay what happened.** `robocode-s replay FILE` (or the GUI's Watch Battle) re-runs a recording turn for turn, with the robots' output and debug painting.

**Reproduce rumble battles.** Every battle the rumble runs has a seed and runs in deterministic mode, so any rumble battle can be re-run locally, turn for turn. Replays in the browser (M6) show what happened; a local re-run under your debugger shows why.

## Deterministic mode

The rumble runs every battle in deterministic mode, so results can be checked by running a battle twice. It differs from classic mode in a few ways that matter to robot authors:

- **The clock is virtual.** In robot code, `System.nanoTime()` and `System.currentTimeMillis()` return a virtual time that advances with the turns and with the work your code does (counted in bytecode instructions). A loop that runs "until 2 ms have passed" does a fixed amount of work, the same on every machine. Seeding a generator from the clock gives the same seed every run.
- **Randomness is seeded per robot**, from the battle seed and the robot's name: `Math.random()`, `new Random()` and `Utils.getRandom()`.
- **Identity hash codes are deterministic** for objects your robot creates, so iterating over a `HashSet` of your own objects has the same order every run.
- **Turns are skipped by counted work, not by the clock.** The engine waits for every robot. A robot whose work in a turn is over the budget (3.2 ms of virtual time, how long classic really waits for a robot) answers as many turns late as classic would have, with classic's skipped-turn events, and is disabled after too many. A robot that never returns, such as an endless loop, is cut off at a work limit.
- **Not covered:** time the JDK reads for you (`Thread.sleep`, timeouts in concurrent code) is still real, so robots whose threads race against each other stay nondeterministic.

If your robot budgets its own computation by the clock, check that it still does enough work per turn under the virtual clock; virtual time was calibrated against real CPU time for this ([#15](https://github.com/nhofsted/robocode-s/issues/15)). `--classic` shows how your robot behaves under classic's real timing.

## Package it

A robot jar is the same as in classic: your classes plus a `.properties` file next to the robot class, for example `yourinitials/Kestrel.properties`:

```properties
robot.classname=yourinitials.Kestrel
robot.version=1.2
robot.author.name=Your Name
robot.description=A surfer with a GuessFactor gun
robot.webpage=https://robowiki.net/wiki/Kestrel
robocode.version=1.11.1
```

The rumble identifies a robot as `classname version` (here `yourinitials.Kestrel 1.2`), so **every release needs a new version string**. Classic's packager in the Robocode GUI writes this file for you; with Gradle, add it as a resource and build the jar as usual.

**Codesize** decides your robot's weight class: under 250 bytes is a nano bot, under 750 a micro, under 1500 a mini, anything larger competes only in the general rumbles. Codesize counts the bytecode of your classes, not the jar size.

## Release it to the rumble

> **Status:** submissions open with the rumble (M4). See [The rumble](/docs/rumble) for how it works.

1. **Sign in with GitHub** on this site and [submit](/submit) your jar, or a URL the rumble mirrors it from. Choose the divisions to enter (limits: a 5 MiB jar, 5 bots per account, 5 new versions of a bot per day). Your versions and their states are on [My bots](/account/bots).
2. **Static checks** run on upload: jar structure, the properties file, the robot class, codesize.
3. **Probation:** your new version fights a few battles on trusted clients. If it fails to load, crashes or gets disabled every round, it is deactivated and you get a message.
4. **Ranked:** after probation it enters normal scheduling and fights every other bot in its rumbles. Pairings start fresh for every new version; your previous version is retired.

Tips for a smooth release: test the exact jar you upload, keep data files small and expect them to start empty, and don't release several versions a day: each version needs a few hundred battles before its scores settle.
