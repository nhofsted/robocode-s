---
title: robocode-s ide-config
description: Print the run configuration for debugging a robot from IntelliJ IDEA, Eclipse or VS Code.
---

```sh
robocode-s ide-config [options]
```

Prints the run configuration for debugging a robot from your IDE: the IDE starts the robot host with its debugger, and `robocode-s battle sample.Walls,@ide` (or the GUI's *From your IDE...*) waits for it. It has ready-to-paste lines for IntelliJ IDEA, Eclipse and VS Code.

| Option | Meaning |
|---|---|
| `--ide-port N` | The port the robot connects to (default 7878). |
| `--json` | The parts as JSON. |
| `--data DIR` | The data folder, where the token is kept (`$ROBOCODES_DATA`). |
| `--host-lib DIR` | The robot host's lib folder (`$ROBOCODES_HOST_LIB`). |
| `--agent-jar PATH` | The determinism agent (`$ROBOCODES_AGENT_JAR`). |

[Using an IDE](/docs/ide#debug-it-with-your-ides-debugger) walks through the setup.
