---
title: Command line
description: The robocode-s commands battle, replay, ide-config, agent and rumble, with every option.
---

`robocode-s` is the command line of robocode-s. It has five commands:

| Command | What it does |
|---|---|
| [`battle`](/docs/cli/battle) | Runs a headless battle and prints the results. Can record it, let the GUI watch it, and run robots on other machines. |
| [`replay`](/docs/cli/replay) | Re-runs a recorded battle, verifies every turn and prints the results. |
| [`ide-config`](/docs/cli/ide-config) | Prints the run configuration for debugging a robot from your IDE. |
| [`agent`](/docs/cli/agent) | Runs robots for a battle on another machine. |
| [`rumble`](/docs/cli/rumble) | Contributes battles to the rumble: `login`, `run`, `status`, `logout`. |

`robocode-s --version` prints the version, the engine version and the protocol version. Every command has `--help` with its options, which these pages follow.

The desktop GUI is a separate program, `robocode-s-gui`; it does most of this with buttons. See [Getting started](/docs/getting-started).

## Shared options

These work the same in `battle`, `agent` and `rumble run`:

| Option | Meaning |
|---|---|
| `--sandbox KIND` | What robots run in: `os`, `bwrap`, `docker` or `none`. See [sandboxes](#sandboxes). |
| `--data DIR` | The data folder: Java runtime, robots' data files, caches. Also `$ROBOCODES_DATA`. Default: the user data folder of an installation, `target/robocodes-data` in a source tree. |
| `--java PATH` | Java executable (`$ROBOCODES_JAVA`). Default: the pinned runtime, downloaded on first use. |
| `--host-lib DIR` | The robot host's lib folder (`$ROBOCODES_HOST_LIB`). |
| `--agent-jar PATH` | The determinism agent (`$ROBOCODES_AGENT_JAR`). |
| `--fresh-data` | Robots start with no data files and keep none (`$ROBOCODES_FRESH_DATA`). |

An installation finds the host library and the agent by itself. The options and variables matter for a [build from source](/download/source) run elsewhere than in the repository.

## Sandboxes

Robots are code from anyone, so they run in a sandbox. The battle's header says which one is active.

| Kind | What it is |
|---|---|
| `os` | The default where there is one. Linux: seccomp, and Landlock where the kernel has it. Windows: an AppContainer and a Job Object. No network, no files but the robot's own. |
| `bwrap` | Bubblewrap, on Linux, if installed. |
| `docker` | Each robot in a container of the Temurin JRE image, without network. Needs Docker; pulls the image the first time. Any platform. |
| `none` | A plain process. macOS has only this and `docker`. |
