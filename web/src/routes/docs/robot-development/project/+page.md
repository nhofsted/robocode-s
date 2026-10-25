---
title: Project and code
description: Set up a Gradle or Maven project for a Robocode robot and write it against the classic API, with what to keep in mind under robocode-s.
---

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

With Maven, use `net.sf.robocode:robocode.api:1.11.1` with `<scope>provided</scope>`. [Jane Doe's first robot](/docs/robot-development/example) has complete build files for both, including one that bundles a library.

A good layout keeps one package per author, with your bots and their shared code in it:

```
src/main/java/yourinitials/
├── Kestrel.java          the robot (extends AdvancedRobot)
├── movement/             wave surfing, wall smoothing, ...
├── targeting/            guns
└── util/
src/main/resources/yourinitials/
└── Kestrel.properties    robot metadata (see Packaging and release)
```

> **Tip:** compile for the oldest Java version you need. The rumble runs robots on a current Temurin JRE, but if you also want your robot to run in classic RoboRumble clients, target what they run.

[Using an IDE](/docs/ide) goes through IntelliJ IDEA, Eclipse and VS Code.

## Write the robot

Everything in the [classic API](https://robocode.sourceforge.io/docs/robocode/) works: `Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot` and `JuniorRobot`, custom events, data files, debug painting with `onPaint`, and `out.println` for console output. The [RoboWiki](https://robowiki.net/) remains the place to learn movement, targeting and strategy.

Some things to keep in mind:

- **One turn, one `execute()`.** Do your thinking between calls. If your code runs over the turn budget, the robot **skips the turn** (its previous commands stay in effect, you get a `SkippedTurnEvent`), and after too many skipped turns it is disabled. In the rumble's deterministic mode the budget is counted in work rather than time; see [deterministic mode](/docs/robot-development/deterministic-mode).
- **Data files** (`getDataFile`, `RobocodeFileOutputStream`) work as in classic, with the same 200 KB quota per robot. They are kept from battle to battle, as in classic (a robot such as Sonda reads the statistics of earlier battles), in the data folder under `robot-data/<robot and version>`. `--fresh-data` (or `ROBOCODES_FRESH_DATA=1`) starts every robot with no files and keeps none: use it to reproduce a battle exactly, since with data files a battle also depends on the ones before it. Two robots of the same class in one battle share one folder.
- **No network, no files outside your data directory.** The sandbox enforces this (classic's SecurityManager denied the same): sockets and connections fail with an exception, other files can't be read or written, and a robot can't start other processes. On Linux the default `os` sandbox limits files only where the kernel has Landlock; the battle's header says `filesystem not limited` if it doesn't. `--sandbox none` is a plain process, for local experiments only: treat the limits as the rule anyway, since the rumble will.
- **Threads are discouraged.** They still work, but anything timing-dependent between threads makes your robot nondeterministic.

Next: [run and test it](/docs/robot-development/testing).
