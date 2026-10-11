---
title: The rumble
description: How the robocode-s rumble works - divisions, weight classes, ranking metrics, volunteer battles, verification, submissions and replays.
---

The robocode-s rumble is a continuously running league. Every bot fights every other bot in its rumbles, again and again, and the rankings on this site are averages over all those battles. It follows the tradition of classic [RoboRumble](https://robowiki.net/wiki/RoboRumble), with a new backend, a new website, battle replays in the browser, and results that can be checked.

> **Status:** the rumble starts with milestone M5. The [rankings](/rankings) shown now are generated sample data, so the website can be built ahead of the backend. Everything below describes the design; numbers marked *initial* will be tuned once real battles flow.

## Divisions and weight classes

| Division | Battle | Rounds | Battlefield |
|---|---|---|---|
| 1v1 | two bots | 35 | 800 × 600 |
| Melee | ten bots at once | 35 | 1000 × 1000 |
| Teams | two teams of up to five | 10 | 1200 × 1200 |
| Twin Duel | two teams of two | 75 | 800 × 800 |

These are classic RoboRumble's settings, *initial* for now.

1v1 and melee also have **weight-class rumbles** for small bots, by codesize (the bytecode size of the robot's classes): **mini** under 1500 bytes, **micro** under 750, **nano** under 250. A nano bot appears in the nano, micro, mini and general rankings; in each, it is ranked only against the bots of that class.

## How scores are computed

Each pair of bots in a rumble is a **pairing**. A battle's score is the classic Robocode score (survival, bullet damage, ram damage and their bonuses); a bot's **pairing score** is its share of the total score in that pairing's battles, averaged over all of them. From the pairings:

| Metric | Meaning |
|---|---|
| **APS** | Average percent score: the mean pairing score over all opponents. The main ranking. |
| **PWIN** | Percentage of pairings won (pairing score above 50%; exactly 50% counts half). |
| **Survival** | Average percentage of rounds survived. |
| **Glicko-2** | A rating computed from all pairings with the Glicko-2 system, shown with its deviation. |
| **ANPP** | Average normalised percentage pairs: per opponent, your pairing score scaled between the lowest (0) and highest (100) score any bot achieved against that opponent, averaged. It rewards beating opponents better than others do. |
| **Vote** | Every bot "votes" for the opponent that scores best against it; Vote is a bot's share of all votes. |

The rankings are recomputed once a day, all metrics together. A bot that hasn't met every opponent yet is marked in the rankings; its numbers are still settling.

## Who runs the battles

The rumble's servers never run a robot. **Volunteers** do: anyone with a GitHub account can run the rumble client (built into the robocode-s command line and GUI) and contribute battles.

1. The client asks for work and gets a batch of battles: which bots, which rumble, how many rounds, and a **seed**. The rumble picks pairings with the fewest battles first.
2. It downloads the robot jars (mirrored by the rumble and checked by hash) and runs the battles in [deterministic mode](/docs/robot-development#deterministic-mode).
3. It uploads the results of the whole batch.

Because battles are deterministic, a battle with the same robots and seed gives the same result on any machine. That makes results **checkable**:

- Some battles are handed out twice. A new volunteer starts with about 20% of their battles re-run by an established volunteer (*initial*); the share drops towards 1% as their results keep matching, and never reaches zero.
- Results count immediately; checks happen in the background.
- On a mismatch a third volunteer breaks the tie. The one who was wrong loses their standing and their recent results are re-checked; repeated mismatches lead to a ban and removal of that volunteer's results. If all three disagree, it's treated as a bug in robocode-s, and nobody is penalised.

In deterministic mode no turn is ever skipped for being slow, so a slow bot may behave slightly differently here than in classic RoboRumble.

## Entering a bot

1. **Sign in with GitHub** and submit a robot jar, or a URL to mirror it from. Choose divisions.
2. **Static checks** run immediately: jar structure, robot properties, the robot class, codesize.
3. **Probation:** a few battles on trusted clients. Bots that fail to load, crash or get disabled every round are deactivated, with a message to the author.
4. **Ranked:** the bot enters normal scheduling. A new version starts with fresh pairings; the previous version is retired.

See [Developing a robot](/docs/robot-development#package-it) for how to package a bot.

**Existing bots.** At launch the rumble imports the bots in the current RoboRumble participant lists, attributed to their authors. Authors can claim their bots by signing in with GitHub.

**Mirroring and takedown.** The rumble keeps a copy of every jar so the rumble keeps working when original download links disappear. Authors can ask for a bot to be removed at any time, through the site or a [GitHub issue](https://github.com/nhofsted/robocode-s/issues), and removals are handled promptly.

## Replays

> **Status:** milestone M5b.

Every pairing page will have a **watch** button that plays a battle in the browser, with the same engine that ran it, compiled to WebAssembly. The rumble keeps recordings of five rounds won by each side for every matchup with at least one top-50 bot. Any other round can be requested: a volunteer re-runs that battle (same seed, same result) and uploads the round.
