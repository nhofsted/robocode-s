---
title: robocode-s agent
description: Run robots for a battle on another machine with a runner agent.
---

```sh
robocode-s agent --server HOST:PORT --token TOKEN [options]
```

A runner agent connects to a battle run with [`robocode-s battle --agents`](/docs/cli/battle#runner-agents), starts the robots it is given in its sandbox, fetches their jars (cached by hash) and relays their stdin and stdout to the battle. It connects again after each battle and when the connection drops. Ctrl+C stops it.

On the battle machine, then on each other machine (the same robocode-s version):

```sh
robocode-s battle ... --agents 0.0.0.0:7655 --agent-token TOKEN --wait-agents 2
robocode-s agent --server HOST:7655 --token TOKEN --fingerprint sha256:...
```

| Option | Meaning |
|---|---|
| `--server HOST:PORT` | The battle's `--agents` address. |
| `--token TOKEN` | The battle's `--agent-token` (`$ROBOCODES_AGENT_TOKEN`). |
| `--fingerprint F` | The battle's certificate, as it prints it (`sha256:AB:CD:...`). Without one, TLS needs a CA-issued certificate, and a loopback server is reached without TLS. |
| `--slots N` | Robots at once (default: no limit). |
| `--warm N` | Robot JVMs to keep started, waiting for the next battle's robots (default: `--slots`; 0: none, they start when the battle asks). |
| `--sandbox KIND` | `os`, `bwrap`, `docker`, `podman` or `none`; see [sandboxes](/docs/cli#sandboxes). |
| `--data DIR`, `--fresh-data`, `--java PATH`, `--host-lib DIR`, `--agent-jar PATH` | See [shared options](/docs/cli#shared-options). |

Agents are the organizer's machines: the battle's machine is trusted, so the robots' own CPU measurements count as for local robots, and in `--classic` mode the network's delay adds to a robot's turn. Agents run jars only, not class folders, and the GUI's Start Battle doesn't use them yet.
