# Rumble data files (v1)

Every 6 hours, the rankings Lambda (design §12.6) publishes the rumble's state as static JSON files in the site bucket. The website (`web/`) reads them in the browser; nothing else is needed to show rankings, bot pages and pairings. Field-level semantics are defined in [`web/src/lib/rumble/types.ts`](../web/src/lib/rumble/types.ts), which is the source of truth; the Rust side mirrors it.

## Files

| Path | Type | Written |
|---|---|---|
| `/data/v1/rumbles.json` | `RumbleIndex`: all rumbles with bot and battle counts and the engine version they rank | every 6 hours |
| `/data/v1/rankings/<rumble>.json` | `Rankings`: one row per bot, sorted by APS, and the engine version they rank | every 6 hours |
| `/data/v1/bots/<bot id>.json` | `BotDetails`: metadata, versions, and per rumble the metrics, all pairings and its history since the first version (daily for a year, weekly before) | every 6 hours, for bots whose numbers changed |
| `/data/v1/archive.json` | `Archive` (new in v1, additive): every downloadable version of every bot that has not been taken down, `{schema, generatedAt, bots: [{id, author, versions: [{version, sha256, size, state, added}]}]}` sorted by bot id, versions newest first; `state` is `active`, `probation`, `retired` or `failed`; the jar is `/jars/<sha256>.jar` on the same origin. Removed bots and versions are omitted | every 6 hours |
| `/data/v1/contributors.json` | `Contributors`: volunteers who contributed battles (login, country, battles, battles in the last week, tier), sorted by battles; volunteers can hide themselves | every 6 hours |

- **Jars:** `BotDetails.versions[].size` (additive; 0 in files written before it existed) and `sha256` give a bot page the link `/jars/<sha256>.jar` for each version it shows (active and retired); downloads are saved as `<id>_<version>.jar` through the `download` attribute (same origin). The archive page uses `archive.json`: about 100 bytes a version, so 1,500 bots with a few versions each (4,000 versions) are about 400 KB uncompressed and far less compressed, one file.
- **Rumble ids** are the division (`1v1`, `melee`, `teams`, `twinduel`), plus `-mini`, `-micro` or `-nano` for the weight-class rumbles (`1v1-micro`).
- **Bot ids** are the robot's fully qualified class name without version (`sample.Walls`). A bot's ranked version is in the file; metrics and pairings belong to the current version, the history spans all versions (each day records the version ranked then).
- **Engine version:** rumble and ranking files carry `engineVersion` (design §4.4). Results of different engine versions are never mixed: the files rank only the current one. When it changes, the rankings start from scratch as the new version's pairings fill; older versions' aggregates are kept in the backend but not published.
- **All metrics** (APS, PWIN, survival, Glicko-2, ANPP, Vote) come from the same run, so they are always consistent with each other.

## Rules

- Percentages are 0–100, timestamps ISO 8601 UTC, dates `YYYY-MM-DD`.
- Within v1, fields are only added, never removed or changed in meaning. Anything else is v2, under `/data/v2/`, published next to v1 until the site has moved.
- Files are served through CloudFront, compressed, cached until shortly after the next daily run. Uncompressed, a 1v1 ranking with 1,100 bots is about 250 KB and a bot file with all its pairings and its full history about 150–200 KB (measured on the fixtures at 600 bots: 131 KB, and 104 KB for a bot with 2.5 years of history).

## Sizes and cost

The daily run rewrites `rumbles.json`, one file per rumble and the bot files that changed; most bots change every day. At about 1,500 bots that is about 45k PUTs a month, roughly $0.25 at S3's PUT price, next to the storage costs of design §12.2.

## Development data

`web/src/lib/rumble/fixtures.ts` generates a seeded, realistic data set in this format (600 bots in 1v1, all divisions and weight classes). The website's dev and preview servers serve it at `/data/v1`, so the site can be built before the backend exists.
