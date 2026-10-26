---
title: robocode-s rumble
description: The rumble client's commands for logging in with GitHub, running the rumble's battles and checking your status, and the owner's admin commands.
---

```sh
robocode-s rumble <command> [options]
```

The client for [the rumble](/docs/rumble): volunteers run its battles. You need a GitHub account. [Contributing battles](/docs/rumble/contributing) is the guide; this page is the reference.

| Command | What it does |
|---|---|
| `login` | Sign in with GitHub (a code to confirm in your browser) and keep an upload token in your settings folder. |
| `logout` | Revoke the token and remove it. |
| `status` | Who you are, your tier and counters, the server, and whether this robocode-s fits it. |
| `run` | Fetch batches of battles, run them, post the results and upload the recordings the rumble asks for. Ctrl+C stops after the running battles; twice abandons them. |
| `admin ...` | The owner's commands, [below](#admin). |

## Options of every command

| Option | Meaning |
|---|---|
| `--server URL` | The rumble (`$ROBOCODES_RUMBLE_SERVER`; default `https://rumble.planethofstede.be`). `login` stores it with the token; the other commands use that one unless it is given. |

## Options of `run`

| Option | Meaning |
|---|---|
| `--parallel N` | Battles at once (default: half the cores, and one per 2 GiB of memory). A melee battle takes 5 of them. |
| `--divisions LIST` | `1v1`, `melee` or `1v1,melee` (default both). Melee needs 5 parallel battles. |
| `--sandbox KIND` | `os`, `bwrap`, `docker` or `none`; see [sandboxes](/docs/cli#sandboxes). |
| `--unsandboxed` | Run robots as plain processes (`--sandbox none`). The rumble runs robots from anyone, so without this the client refuses to run without a sandbox. On macOS: Docker, or this. |
| `--memory MB` | Heap per robot JVM (default 512). |
| `--batches N` | Stop after N batches have been posted (default: go on). |
| `--json` | Print the events as JSON lines instead of one line per battle and per post. |
| `--data DIR` | The data folder: Java runtime, jar cache, pending batches, log (`$ROBOCODES_DATA`). |
| `--java PATH`, `--host-lib DIR`, `--agent-jar PATH` | See [shared options](/docs/cli#shared-options). |

The upload token is in the settings folder's `rumble.json`, or in `$ROBOCODES_RUMBLE_TOKEN`. Results that could not be posted wait in `<data>/rumble/pending` and are posted at the next start. `<data>/rumble/log.txt` keeps the last 1 MiB of log.

## Admin

For an admin account; the commands print the server's answer.

| Command | What it does |
|---|---|
| `admin import --division 1v1\|melee [--list URL\|FILE] [--local DIR] [--mirror TEMPLATE\|none] [--dry-run] [--concurrency N] [--report FILE]` | Seed a division from the RoboWiki participants list (default: the page itself). Jars come from `--local` (files `<id>_<version>.jar`), the list's URL, then the mirror. `--dry-run` only downloads and validates; `--concurrency` is the downloads at once (default 4); the report is JSON (default `import-<division>-<date>.json`). |
| `admin import --history FILE [--dry-run] [--concurrency N] [--report FILE]` | Register the archives' older versions (`versions.json`: per version its `id`, `version`, `sha256`, the jar in `files`, its `releaseDate` and the participants pages it was on) as retired versions of the bots the rumble has, with their release dates. Versions on today's participants lists only get their release date; versions of bots the rumble doesn't have are reported as `noBot`. `--dry-run` only checks the jars. Takes no `--division`, `--list`, `--local` or `--mirror`; the report defaults to `history-<date>.json`. |
| `admin user UID [--rep N] [--state new\|quar\|ban] [--admin true\|false] [--trusted true\|false]` | Change a user (UID as `g<GitHub id>`). |
| `admin bot claim <id> <github login>` | Give a bot to the GitHub user who claimed it. |
| `admin bot remove <id> --reason TEXT` | Take a bot down. |
| `admin alerts` | The open alerts (verification mismatches and the like). |
| `admin config get` | Print the configuration overrides. |
| `admin config set FILE` | Replace them with the JSON in `FILE`. |
