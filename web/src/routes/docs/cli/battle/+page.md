---
title: robocode-s battle
description: Run a headless battle from the command line, with every option for robots, rules, seeds, recordings, observers and runner agents.
---

```sh
robocode-s battle [options] <robot>...
```

Runs a battle and prints the results as a table (or JSON). Robots are class names as in classic Robocode, optionally with a version (`sample.Fire`, `jk.mega.DrussGT 3.1.16`), separated by commas or spaces; a team expands into its members.

```sh
robocode-s battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
robocode-s battle jd.MyRobot*,sample.Walls --jar build/classes/java/main --seed 42
```

`@ide` is a robot you debug from your IDE: see [`ide-config`](/docs/cli/ide-config). One `@ide` per robot, after the others.

## Battle

| Option | Meaning |
|---|---|
| `--battle FILE` | A classic `.battle` file: its robots and rules. Options after it override its rules. |
| `--rounds N` | Rounds (default 10). |
| `--seed N` | The battle seed, a Java long (default: random, and printed). |
| `--width N`, `--height N` | Battlefield size (default 800x600). |
| `--gun-cooling-rate X` | Default 0.1. |
| `--inactivity-time N` | Turns (default 450). |
| `--sentry-border N` | Sentry border size (default 100). |
| `--hide-enemy-names` | Hide enemy names from robots. |
| `--classic` | Classic timing: robots that are too slow skip turns by the clock, instead of [deterministic mode](/docs/robot-development/deterministic-mode). |
| `--turn-timeout MS` | With `--classic`: time per turn (default 50). |

## Robots and runtime

| Option | Meaning |
|---|---|
| `--jar PATH` | A robot jar, or a class folder (an IDE's output, read in place; name its robots with classic's `*`, such as `jd.MyRobot*`). Repeatable. Default: `$ROBOCODES_ROBOT_JARS`, else the installation's robots, or the sample robots of a source tree. |
| `--memory MB` | Heap per robot JVM (default 512). |
| `--warm N` | Start N robot JVMs while the robots are looked up, instead of afterwards (default: one per robot; 0 starts them when the battle does). |
| `--ide-port N` | The port robots from your IDE connect to (default 7878). |
| `--sandbox KIND` | `os`, `bwrap`, `docker` or `none`; see [sandboxes](/docs/cli#sandboxes). |
| `--data DIR`, `--fresh-data`, `--java PATH`, `--host-lib DIR`, `--agent-jar PATH` | See [shared options](/docs/cli#shared-options). |

A robot's data files stay from battle to battle, as in classic; `--fresh-data` starts every robot with none.

## Output

| Option | Meaning |
|---|---|
| `--json` | Results as JSON instead of a table. |
| `--record FILE` | Record the battle. Replay it with [`robocode-s replay`](/docs/cli/replay) or the GUI. |
| `--paint` | Ask every robot for its debug painting, which `--record` stores. |

## Watching from the GUI

A battle can let the GUI watch it, from this machine or another (the GUI's Watch Battle, Remote Server, or `robocode-s-gui --watch HOST:PORT`). Observers control the battle as in a live battle: pause, speed, stop, kill, input.

| Option | Meaning |
|---|---|
| `--observe ADDR` | Let observers watch on ADDR (`PORT`, or `HOST:PORT`; HOST defaults to 127.0.0.1). The battle starts once the first one has connected. |
| `--observe-token TOKEN` | Observers must present TOKEN (`$ROBOCODES_OBSERVE_TOKEN`). Required unless ADDR is a loopback address. |

```sh
robocode-s battle ... --observe 0.0.0.0:7654 --observe-token TOKEN
robocode-s-gui --watch HOST:7654 --token TOKEN --fingerprint sha256:AB:CD:...
```

## Runner agents

For a big battle, robots can run on other machines through [`robocode-s agent`](/docs/cli/agent). The battle machine still describes the robots, and hands them out round robin.

| Option | Meaning |
|---|---|
| `--agents ADDR` | Listen for runner agents on ADDR (`PORT`, or `HOST:PORT`) and run the robots on them. |
| `--agent-token TOKEN` | Agents must present TOKEN (`$ROBOCODES_AGENT_TOKEN`). Required with `--agents`. |
| `--wait-agents N` | Wait for at least N agents (default 1)... |
| `--agent-wait S` | ...for up to S seconds (default 60). |

## TLS

A listener (`--observe` or `--agents`) that is not on loopback uses TLS, with a self-signed certificate made once in the data folder. The battle prints its fingerprint, for clients to pin with `--fingerprint`, as SSH pins host keys.

| Option | Meaning |
|---|---|
| `--tls` | TLS on loopback too. |
| `--tls-cert FILE`, `--tls-key FILE` | A certificate chain (PEM) and its key instead, for example from a CA. Clients without a fingerprint check it against the Mozilla roots. |
