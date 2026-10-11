# Rumble data files (v1)

Once a day, the rankings Lambda (design §12.6) publishes the rumble's state as static JSON files in the site bucket. The website (`web/`) reads them in the browser; nothing else is needed to show rankings, bot pages and pairings. Field-level semantics are defined in [`web/src/lib/rumble/types.ts`](../web/src/lib/rumble/types.ts), which is the source of truth; the Rust side mirrors it.

## Files

| Path | Type | Written |
|---|---|---|
| `/data/v1/rumbles.json` | `RumbleIndex`: all rumbles with bot and battle counts | daily |
| `/data/v1/rankings/<rumble>.json` | `Rankings`: one row per bot, sorted by APS | daily |
| `/data/v1/bots/<bot id>.json` | `BotDetails`: metadata, versions, and per rumble the metrics, all pairings and a year of daily history across versions | daily, for bots whose numbers changed |

- **Rumble ids** are the division (`1v1`, `melee`, `teams`, `twinduel`), plus `-mini`, `-micro` or `-nano` for the weight-class rumbles (`1v1-micro`).
- **Bot ids** are the robot's fully qualified class name without version (`sample.Walls`). A bot's ranked version is in the file; metrics and pairings belong to the current version, the history spans all versions (each day records the version ranked then).
- **All metrics** (APS, PWIN, survival, Glicko-2, ANPP, Vote) come from the same daily run, so they are always consistent with each other.

## Rules

- Percentages are 0–100, timestamps ISO 8601 UTC, dates `YYYY-MM-DD`.
- Within v1, fields are only added, never removed or changed in meaning. Anything else is v2, under `/data/v2/`, published next to v1 until the site has moved.
- Files are served through CloudFront, compressed, cached until shortly after the next daily run. Uncompressed, a 1v1 ranking with 1,100 bots is about 250 KB and a bot file with all its pairings about 150 KB (measured on the fixtures at 600 bots: 131 KB and 76 KB).

## Sizes and cost

The daily run rewrites `rumbles.json`, one file per rumble and the bot files that changed; most bots change every day. At about 1,500 bots that is about 45k PUTs a month, roughly $0.25 at S3's PUT price, next to the storage costs of design §12.2.

## Development data

`web/src/lib/rumble/fixtures.ts` generates a seeded, realistic data set in this format (600 bots in 1v1, all divisions and weight classes). The website's dev and preview servers serve it at `/data/v1`, so the site can be built before the backend exists.
