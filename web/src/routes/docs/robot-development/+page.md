---
title: Developing a robot
description: How to set up, test, debug and release a Robocode robot with robocode-s, from the first build to the rumble.
---

This guide covers the whole loop: setting up a robot project, running battles, finding out why your robot does what it does, and releasing a version to the rumble. It is written for Java robots on the classic API, the only kind robocode-s runs today; native APIs for other languages (Python in milestone M5; .NET, Rust, C/C++ and Kotlin in M8) are not started and will get their own sections.

> **Status:** running, testing and debugging battles works today from the command line and the GUI. The parts marked *planned* depend on the rumble (M5) or on tools still to come.

## How your robot runs

In classic Robocode every robot is a thread inside the Robocode JVM. In robocode-s each robot is **its own process**:

- The **runner** starts a JVM for your robot, inside a sandbox, with the **robot host**: classic Robocode's own robot-side code (the `robocode.*` API, event handling, blocking calls), connected to the engine over the robocode-s protocol (on the process's stdin and stdout) instead of in-process calls. The JVMs are started while the battle is looked up, so a battle starts quickly.
- Every turn the engine sends your robot its status and events, your code runs until it calls `execute()` (directly, or through a blocking call such as `ahead(100)`), and the robot host sends your commands back.
- Team members are separate processes too; team messages travel through the server.

For you this mostly means things work as before, with a few benefits: a crash or an `OutOfMemoryError` only takes down your robot, your robot's CPU time is measured precisely per turn, and the robot is an ordinary JVM process you can attach a debugger to.

## The steps

1. **[Project and code](/docs/robot-development/project):** a Gradle or Maven project against the classic API, and what to keep in mind when writing the robot.
2. **[Running and testing](/docs/robot-development/testing):** run battles from the command line or the GUI, reproduce them with seeds, measure over many battles.
3. **[Debugging](/docs/robot-development/debugging):** output and painting, your IDE's debugger, replays and re-running rumble battles.
4. **[Deterministic mode](/docs/robot-development/deterministic-mode):** how the virtual clock, seeded randomness and counted work make a battle repeatable.
5. **[Packaging and release](/docs/robot-development/release):** the `.properties` file, codesize and weight classes, and entering the rumble.

Working in an IDE? [Using an IDE](/docs/ide) sets up IntelliJ IDEA, Eclipse and VS Code. Every command is in the [command line reference](/docs/cli).
