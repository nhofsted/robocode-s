---
title: Getting started
description: What you can do with robocode-s today, building it from source, running battles, and what installing will look like at the first release.
---

> **Status:** there is no release to download yet; the first one is coming, and the owner decides when. Until then you build robocode-s from source, which works today on Linux, Windows and macOS. Installers and archives are built and tested; this page says how installing will work.

## Write a robot

Robots for robocode-s are classic Robocode robots. Everything you write against the classic `robocode.*` API (`Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot`) runs on robocode-s unmodified:

1. Install [classic Robocode 1.11.1](https://robocode.sourceforge.io/) and do its [first-robot tutorial](https://robowiki.net/wiki/Robocode/My_First_Robot), or set up a plain Java project against the `robocode.api` library (see [Developing a robot](/docs/robot-development#set-up-a-project)).
2. Compile it, or package it as a robot jar with classic's packager or your build tool.
3. Point robocode-s at the jar, or at the folder your IDE compiles into (below).

## Build robocode-s from source

You need Rust (stable, via [rustup](https://rustup.rs)), a C toolchain and JDK 21:

```sh
git clone https://github.com/nhofsted/robocode-s.git
cd robocode-s
cargo test --workspace     # engine, protocol, server, runner, cli
./gradlew build            # Java robot host, sample robots, test oracle
```

## Run a battle from the command line

```sh
./gradlew :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
cargo run --release -p robocode-s-cli -- battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
```

Every robot runs in its own JVM. A pinned Java runtime (about 50 MB) is downloaded on first use. The options that matter:

- **Your robots:** `--jar path/to/robot.jar`, or `--jar path/to/classes` for a folder of class files such as your IDE's output (`bin`, `build/classes/java/main`). A folder is read in place, as classic's development folders are, so a recompiled robot is new in the next battle. Its robots are named with classic's `*` (`my.Robot*`). `ROBOCODES_ROBOT_JARS` takes a list of jars and folders too.
- **Rules:** `--rounds` and the other rules, or a classic `.battle` file with `--battle FILE`.
- **Deterministic by default:** the same robots and seed give the same battle, on every machine. `--classic` uses classic's timing instead, where robots that are too slow skip turns by the clock. See [deterministic mode](/docs/robot-development#deterministic-mode).
- **Sandbox:** robots are code from anyone, so they run in one. `--sandbox os` is the default on Linux (seccomp, and Landlock where the kernel has it) and on Windows (AppContainer and Job Object): no network, no files but the robot's own. `--sandbox bwrap` uses bubblewrap on Linux, `--sandbox docker` runs each robot in a container (any platform with Docker installed and running; macOS has only this and `none`), and `--sandbox none` is a plain process. The battle's header says which sandbox is active.
- **Recording:** `--record FILE` records a battle (`--paint` includes the robots' debug painting). `robocode-s replay FILE` re-runs it, checks every turn and prints the results. Recordings keep the engine version they ran with; `robocode-s --version` shows yours, and a replay by another version warns.

`robocode-s battle --help` lists everything.

## The desktop GUI

```sh
cargo run --release -p robocode-s-gui                 # battle setup, then the live battle
cargo run --release -p robocode-s-gui -- battle.rsr   # replay a recording
```

Start Battle sets up the robots, game type and rules and runs the battle live, with the robots' output and debug painting, speed control, pausing and single steps, and zoom for large fields. Watch Battle opens a recording or a website replay from a file, or a battle running elsewhere. The options cover robot folders, the sandbox, what the battlefield shows and sound, and are saved in your config folder.

## Watch a battle on another machine

A battle can let the GUI watch it, from the same machine or another:

```sh
robocode-s battle ... --observe 0.0.0.0:7654 --observe-token TOKEN
robocode-s-gui --watch HOST:7654 --token TOKEN --fingerprint sha256:AB:CD:...
```

Off loopback the connection uses TLS. The battle makes a self-signed certificate once and prints its fingerprint, which the watcher pins (the GUI's Fingerprint field), as SSH pins host keys. Watchers can pause, change the speed, stop and send input, as in a live battle.

## Run robots on other machines

For a big battle, robots can run on other machines through runner agents. On the battle machine:

```sh
robocode-s battle ... --agents 0.0.0.0:7655 --agent-token TOKEN --wait-agents 2
```

On each other machine, with the same robocode-s version:

```sh
robocode-s agent --server HOST:7655 --token TOKEN --fingerprint sha256:...
```

The agent starts the robots it is given, with its own `--sandbox`, at most `--slots N` at a time, and connects again for the next battle. Agents are the organizer's machines: they are trusted with robot timing, and in classic mode the network's delay adds to a robot's turn. Agents run jars only, not class folders, and the GUI's Start Battle doesn't use them yet. `robocode-s agent --help` has the options.

## What installing will look like

The first release will have, per platform, an installer or package and an archive on the [releases page](https://github.com/nhofsted/robocode-s/releases). Package-manager channels (apt and dnf repositories, winget, Homebrew) are planned ([#79](https://github.com/nhofsted/robocode-s/issues/79)) but not live. The files are not code-signed for now, so Windows and macOS warn once.

- **Windows:** a setup program that installs for your user, without administrator rights, with a Start menu entry and an optional `PATH` entry. SmartScreen says "Windows protected your PC": choose "More info", then "Run anyway". There is also a `.zip` to unpack on an NTFS drive.
- **Debian, Ubuntu, Fedora, RHEL:** a `.deb` (`sudo apt install ./robocode-s-<version>-linux-x86_64.deb`) or `.rpm` (`sudo dnf install ./robocode-s-<version>-linux-x86_64.rpm`), for x86-64 and arm64. They put `robocode-s` and `robocode-s-gui` on your `PATH` and add an application menu entry.
- **macOS:** a zipped `robocode-s.app`. It is not signed or notarized, so Gatekeeper refuses it after a download: right-click it and choose Open, or "Open Anyway" in System Settings, Privacy & Security.
- **Archives:** `.tar.gz` for Linux and macOS, `.zip` for Windows. Unpack anywhere and run `robocode-s-gui` or `robocode-s` from it.

All of them contain the same things: the window, the command line, the robot host and the sample robots. The Java runtime is fetched on first use into your user data folder, not into the installation. Uninstalling leaves that folder and the GUI's settings; delete them to remove everything.

Not built yet: the rumble client, for volunteering battles to [the rumble](/docs/rumble), and robots in other languages.
