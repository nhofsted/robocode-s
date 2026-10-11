---
title: Getting started
description: What you can do with robocode-s today, and what installing and running a first battle will look like.
---

> **Status:** there is no release to download yet. The engine and the host for classic Java robots work and are tested against classic Robocode. A command-line battle runner ([#12](https://github.com/nhofsted/robocode-s/issues/12)) and the desktop GUI (milestone M2) are still to come. This page describes what you can do now and will turn into a real quick start with the first release.

## Write a robot today

Robots for robocode-s are classic Robocode robots. Everything you write against the classic `robocode.*` API (`Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot`) runs on robocode-s unmodified, so the best way to start right now is the classic toolchain:

1. Install [classic Robocode 1.11.1](https://robocode.sourceforge.io/) and do its [first-robot tutorial](https://robowiki.net/wiki/Robocode/My_First_Robot).
2. Set up your robot as a normal Java project in your IDE, against the `robocode.api` library (see [Developing a robot](/docs/robot-development#set-up-a-project)).
3. Package it as a robot jar with classic's packager or your build tool.

The jar you produce is exactly what robocode-s runs and what the rumble will accept.

## Build robocode-s from source

If you'd like to try the engine itself, or follow along with development, build it from the repository. You need Rust (stable), a C toolchain and JDK 21:

```sh
git clone https://github.com/nhofsted/robocode-s.git
cd robocode-s
cargo test --workspace     # engine, protocol, server, runner
./gradlew build            # Java robot host, sample robots, test oracle
```

The [repository README](https://github.com/nhofsted/robocode-s#readme) has the details. At this stage battles are run by the test tools rather than a user-facing command.

## What the first release will look like

- **One download per platform** (Windows, Linux, macOS; x86-64 and arm64). Java robots need a JRE; robocode-s fetches a pinned Eclipse Temurin JRE on first use, so you don't have to install one.
- **A command-line runner** for headless battles: pick robots, rounds and a seed, get results and a recording.
- **The desktop GUI** for setting up battles, watching them live with your robot's debug painting, and replaying recordings.
- **The rumble client**, built into both, for volunteering battles to [the rumble](/docs/rumble).

Watch the [GitHub repository](https://github.com/nhofsted/robocode-s) for releases.
