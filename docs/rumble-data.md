# Rumble data files (v1)

The rankings Lambda (design §12.6) publishes the rumble's state as static JSON files in the site bucket. The website (`web/`) reads them in the browser; nothing else is needed to show rankings, bot pages and pairings. Field-level semantics are defined in [`web/src/lib/rumble/types.ts`](../web/src/lib/rumble/types.ts), which is the source of truth; the Rust side mirrors it.

## Files

| Path | Type | Written |
|---|---|---|
| `/data/v1/rumbles.json` | `RumbleIndex`: all rumbles with bot and battle counts | every rankings run |
| `/data/v1/rankings/<rumble>.json` | `Rankings`: one row per bot, sorted by APS | every rankings run |
| `/data/v1/bots/<bot id>.json` | `BotDetails`: metadata, versions, and per rumble the metrics, all pairings and daily history | every rankings run, for bots whose numbers changed |

- **Rumble ids** are the division (`1v1`, `melee`, `teams`, `twinduel`), plus `-mini`, `-micro` or `-nano` for the weight-class rumbles (`1v1-micro`).
- **Bot ids** are the robot's fully qualified class name without version (`sample.Walls`). A bot's ranked version is in the file; history and pairings belong to the current version.
- **Cheap metrics** (APS, PWIN, survival) are updated by every run; **full metrics** (Glicko-2, ANPP, Vote) by the daily full pass, stamped `fullMetricsAt`, and `null` until a bot's first one.

## Rules

- Percentages are 0–100, timestamps ISO 8601 UTC, dates `YYYY-MM-DD`.
- Within v1, fields are only added, never removed or changed in meaning. Anything else is v2, under `/data/v2/`, published next to v1 until the site has moved.
- Files are served through CloudFront with a short cache time (minutes), compressed. Uncompressed, a 1v1 ranking with 1,100 bots is about 250 KB and a bot file with all its pairings about 150 KB (measured on the fixtures at 600 bots: 131 KB and 76 KB).

## Sizes and cost

A rankings run rewrites `rumbles.json`, one file per rumble and the bot files that changed. With batched uploads most bots change every run, so a run writes roughly one PUT per ranked bot per day it runs: at about 1,500 bots and a few runs a day, that is about 150k PUTs a month, about $0.75 at S3's PUT price. If that matters, the bot files can be written by the daily full pass only, with the rankings files carrying the fresh numbers in between.

## Development data

`web/src/lib/rumble/fixtures.ts` generates a seeded, realistic data set in this format (600 bots in 1v1, all divisions and weight classes). The website's dev and preview servers serve it at `/data/v1`, so the site can be built before the backend exists.
