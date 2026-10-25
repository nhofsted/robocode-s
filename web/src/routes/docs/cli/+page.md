---
title: Command line
description: The robocode-s commands battle, replay, ide-config, agent, rumble and update, with every option.
---

`robocode-s` is the command line of robocode-s. It has six commands:

| Command | What it does |
|---|---|
| [`battle`](/docs/cli/battle) | Runs a headless battle and prints the results. Can record it, let the GUI watch it, and run robots on other machines. |
| [`replay`](/docs/cli/replay) | Re-runs a recorded battle, verifies every turn and prints the results. |
| [`ide-config`](/docs/cli/ide-config) | Prints the run configuration for debugging a robot from your IDE. |
| [`agent`](/docs/cli/agent) | Runs robots for a battle on another machine. |
| [`rumble`](/docs/cli/rumble) | Contributes battles to the rumble: `login`, `run`, `status`, `logout`. |
| [`update`](#updates) | Installs a newer release of robocode-s, after asking. |

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

## Updates

`robocode-s update` looks for a newer release and, if there is one, asks before installing it. `--check` only says whether there is one; `--yes` installs without asking (without a terminal it doesn't ask, so it needs `--yes`).

How it installs depends on how robocode-s was installed:

| Installed from | Update |
|---|---|
| An archive (`.zip`, `.tar.gz`) or the macOS app | Downloaded, checked against the release's checksums, and put in place of the old files. Robots you added to its `robots` folder stay. |
| The Windows installer (or winget) | The new installer runs once robocode-s has exited, with the components you chose before. |
| apt, dnf or Homebrew | It names the command: `sudo apt update && sudo apt install --only-upgrade robocode-s`, `sudo dnf upgrade --refresh robocode-s` or `brew upgrade --cask robocode-s`. |

The other commands look once a day by themselves, in the background, and say on the terminal when there is a newer release; `ROBOCODES_NO_UPDATE_CHECK=1` turns that off. The GUI does the same with a notice, and its options page has the setting and a *Look now* button. A build from source never looks.
