# Rumble data files (v1)

Every 6 hours, the rankings Lambda (design §12.6) publishes the rumble's state as static JSON files in the site bucket. The website (`web/`) reads them in the browser; nothing else is needed to show rankings, bot pages and pairings. Field-level semantics are defined in [`web/src/lib/rumble/types.ts`](../web/src/lib/rumble/types.ts), which is the source of truth; the Rust side mirrors it.

## Files

| Path | Type | Written |
|---|---|---|
| `/data/v1/rumbles.json` | `RumbleIndex`: all rumbles with bot and battle counts and the engine version they rank | every 6 hours |
| `/data/v1/rankings/<rumble>.json` | `Rankings`: one row per bot, sorted by APS, and the engine version they rank | every 6 hours |
| `/data/v1/bots/<bot id>.json` | `BotDetails`: metadata, versions, and per rumble the metrics, all pairings and its history since the first version (daily for a year, weekly before) | every 6 hours, for bots whose numbers changed |
| `/data/v1/archive.json` | `Archive` (new in v1, additive): every downloadable version of every bot that has not been taken down, `{schema, generatedAt, bots: [{id, author, versions: [{version, sha256, size, state, added}]}]}` sorted by bot id, versions newest first; `state` is `active`, `probation`, `retired` or `failed`; the jar is `/jars/<sha256>.jar` on the same origin. Removed bots and versions are omitted | every 6 hours |
| `/data/v1/rec/<division>/<bot id>.json` | Recordings index (M6, additive): the bot's current version's filled recording slots, `division` is `1v1` or `melee` (the mini, micro and nano rumbles share their division's recordings). Only bots with recordings have a file; `BotDetails.recordings` says which (below) | every 6 hours, for bots whose list changed |
| `/data/v1/contributors.json` | `Contributors`: volunteers who contributed battles (login, country, battles, battles in the last week, tier), sorted by battles; volunteers can hide themselves | every 6 hours |

- **Jars:** `BotDetails.versions[].size` (additive; 0 in files written before it existed) and `sha256` give a bot page the link `/jars/<sha256>.jar` for each version it shows (active and retired); downloads are saved as `<id>_<version>.jar` through the `download` attribute (same origin). The archive page uses `archive.json`: about 100 bytes a version, so 1,500 bots with a few versions each (4,000 versions) are about 400 KB uncompressed and far less compressed, one file.
- **Rumble ids** are the division (`1v1`, `melee`, `teams`, `twinduel`), plus `-mini`, `-micro` or `-nano` for the weight-class rumbles (`1v1-micro`).
- **Bot ids** are the robot's fully qualified class name without version (`sample.Walls`). A bot's ranked version is in the file; metrics and pairings belong to the current version, the history spans all versions (each day records the version ranked then).
- **Engine version:** rumble and ranking files carry `engineVersion` (design §4.4). Results of different engine versions are never mixed: the files rank only the current one. When it changes, the rankings start from scratch as the new version's pairings fill; older versions' aggregates are kept in the backend but not published.
- **All metrics** (APS, PWIN, survival, Glicko-2, ANPP, Vote) come from the same run, so they are always consistent with each other.

## Recordings

`BotDetails.recordings` (additive, absent when empty): `{"1v1": 212, "melee": 14}`, the recorded rounds of the current version per division; a file exists where it is above 0. The slots fill over time, so a bot's list grows. Fetch `/data/v1/rec/<division>/<bot id>.json` (bot ids are path-safe, as for `bots/`). All strings are as in the bot files; a round is an array, to keep the files small (a top bot has up to 11,000 rounds in 1v1, about 40 bytes each).

1v1 (`/data/v1/rec/1v1/<id>.json`):

```json
{"schema": 1, "engine": 7, "bot": "sample.Walls", "version": "1.0", "sha16": "0123456789abcdef",
 "pairs": [{"opp": "jk.mega.DrussGT", "oppVersion": "3.1.7", "oppSha16": "fedcba9876543210",
            "rounds": [["a-0", 3, "-8113940187746132118", 3512, 0], ["b-1", 17, "42", 3498, 1]]}]}
```

A round is `[file, round, seed, size, me]`: `file` is the track's name without `.track` (the letter is `a` if the bot whose id sorts first won the round, `b` if the other did; the index is the slot), `round` the 0-based round in the battle, `seed` the battle's seed as a decimal string (64 bits), `size` the track's bytes, `me` this bot's robot index in the track (0 or 1; the opponent has the other). `pairs` are sorted by `opp` (bot id), rounds by `file`. The same tracks appear in the opponent's file, with its own `me`. The version fields name the pair's current versions: a bot that releases a new version starts with an empty list.

Track URL: `/rec/e<engine>/1v1/<A>/<B>/<file>.track` with `<X>` = `<bot id>_<sha16>`, A the bot whose id sorts first (plain byte order, `<`), B the other, each with its own `sha16` (this bot's from the file's top level, the opponent's from `oppSha16`). Tracks are immutable (`Cache-Control: immutable`) and are served from the same origin as the site.

Melee (`/data/v1/rec/melee/<id>.json`):

```json
{"schema": 1, "engine": 7, "bot": "sample.Walls", "version": "1.0", "sha16": "0123456789abcdef",
 "bots": [{"bot": "sample.Walls", "version": "1.0", "sha16": "0123456789abcdef"}, {"bot": "sample.Crazy", "version": "1.2", "sha16": "..."}],
 "rounds": [["w-0", 2, "6", 3600, [1, 0, 2]], ["l-3", 9, "17", 3599, [0, 2, 1]]]}
```

A round is `[file, round, seed, size, order]`: `file` starts with `w` where this bot won the round, `l` where it didn't; `order[k]` is the index into `bots` of the track's robot `k`. Track URL: `/rec/e<engine>/melee/<id>_<sha16>/<file>.track` (this bot's id and `sha16`). `bots` lists the robots of those rounds, in first-use order (a field member may be a retired version; `sha16` is the first 16 hex digits of its jar's SHA-256, as in `BotVersion.sha256`).

The index contains only filled slots recorded with their robot order (slots filled before M6 are reopened and refill) and only versions that are current for both bots (1v1) or for the bot (melee).

## Rules

- Percentages are 0–100, timestamps ISO 8601 UTC, dates `YYYY-MM-DD`.
- Within v1, fields are only added, never removed or changed in meaning. Anything else is v2, under `/data/v2/`, published next to v1 until the site has moved.
- Files are served through CloudFront, compressed, cached until shortly after the next daily run. Uncompressed, a 1v1 ranking with 1,100 bots is about 250 KB and a bot file with all its pairings and its full history about 150–200 KB (measured on the fixtures at 600 bots: 131 KB, and 104 KB for a bot with 2.5 years of history).

## Sizes and cost

The daily run rewrites `rumbles.json`, one file per rumble and the bot files that changed; most bots change every day. At about 1,500 bots that is about 45k PUTs a month, roughly $0.25 at S3's PUT price, next to the storage costs of design §12.2.

## Development data

`web/src/lib/rumble/fixtures.ts` generates a seeded, realistic data set in this format (600 bots in 1v1, all divisions and weight classes). The website's dev and preview servers serve it at `/data/v1`, so the site can be built before the backend exists.
