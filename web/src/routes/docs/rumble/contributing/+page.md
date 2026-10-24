---
title: Contributing battles
description: How to lend your computer to the rumble, from signing in with GitHub to what runs on your machine.
---

The [rumble](/docs/rumble) runs on volunteers' computers, and robocode-s is the client. Anyone with a GitHub account can contribute. You need robocode-s [installed](/download); it contains the client.

## Log in and run

```sh
robocode-s rumble login
robocode-s rumble run
```

`login` is GitHub's device flow: it shows a code and opens GitHub in your browser, where you confirm it. The client keeps an upload token in its settings folder. Revoke it on your [account page](/account), or with `robocode-s rumble logout`, which revokes it and removes it.

`run` loops until you stop it with Ctrl+C: it gets a batch of battles, runs them, posts the results, uploads the recordings the rumble asks for and asks for more, printing a line per battle and per post (`--json` prints the events as JSON lines instead). One Ctrl+C finishes the running battles and posts what is done; a second abandons them. Results that could not be posted wait in the data folder and are posted at the next start.

`robocode-s rumble status` shows who you are, your tier and counters, and whether this version fits the rumble. Every option is in the [`rumble` reference](/docs/cli/rumble).

## In the GUI

The GUI has the same on its **Rumble** page: sign in, then switch Contribute on.

## How many battles at once

`--parallel N` sets how many battles run at once. The default is half your cores, and one per 2 GiB of memory. A melee battle takes five of the slots. `--divisions 1v1,melee` chooses what to offer, and `--batches N` stops after N batches.

## Sandboxes

The rumble runs other people's robots, so the client runs them in a sandbox (no network, no files outside the robot's own folder) and refuses to contribute without one.

- **Linux:** built in (seccomp, and Landlock where the kernel has it). Bubblewrap and Docker work too (`--sandbox bwrap`, `--sandbox docker`).
- **Windows:** built in (AppContainer and a Job Object). Nothing to install.
- **macOS:** none built in. Install Docker (`--sandbox docker`), or pass `--unsandboxed` if you accept running robots with your user's rights.

## What runs

- **Other people's robots, in [deterministic mode](/docs/robot-development/deterministic-mode).** The rumble gives each battle a seed, so the same battle gives the same result on any machine.
- **Fresh data in every battle.** A robot's data folder starts empty in every rumble battle, so results do not depend on which volunteer's computer ran what before. Robots that learn *during* a battle are unaffected.
- **Verification.** Some of your battles are re-run by others, and you re-run theirs. Matching results raise your reputation and lower the share that is checked; a mismatch is settled by a third volunteer. See [who runs the battles](/docs/rumble#who-runs-the-battles).

## Your standing

Your tier, reputation and counters are on your [account page](/account). The [contributors](/contributors) list shows who has contributed, and you can hide yourself there.
