# Rumble backend and client (M4): implementation spec

Status: **Draft for review** · 2026-10-24 · ADR [0021](adr/0021-rumble-backend.md)

This spec turns design §12 into something agents can build: crates, HTTP API, DynamoDB items, S3 layout, algorithms, configuration values, CDK resources, the client loop and the seeding import. Where it departs from design §12, it says so (§19 lists every decision taken here). Field semantics of the published data stay in [`web/src/lib/rumble/types.ts`](../web/src/lib/rumble/types.ts) and [`rumble-data.md`](rumble-data.md).

Scope of M4 (owner, 2026-10-24): 1v1 and melee, each with its mini, micro and nano rumble; recording upload slots (the browser viewer is M6); work allocation without a hard cap; the RoboWiki seeding import; a deployed AWS setup that hands out work and takes results; a CLI and GUI client that can be downloaded from the website. Teams and twin duel are not built, but nothing below assumes two bots or one bot per side where it would block them (§6.4).

## 1. Overview

```
 volunteer (robocode-s rumble run, GUI Rumble page)          AWS, eu-west-1 (+ CloudFront)
   GET  /api/v1/work      ─────────────────────────►  api Lambda ── reads plan (S3, cached)
   GET  /jars/<sha>.jar   ─────────────────────────►  CloudFront → jars bucket
   run battles (deterministic, sandboxed)
   POST /api/v1/results   ─────────────────────────►  api Lambda ── 1 transaction per post (DynamoDB)
   PUT  <presigned S3 URL> (only rounds that fill a slot) ►  jars bucket rec/...
   POST /api/v1/recordings/done ───────────────────►  api Lambda ── HeadObject, slot filled

 EventBridge Scheduler ─ hourly ─► batch Lambda "plan":    result log (DynamoDB) → pairing state (S3) → plan (S3)
                       ─ daily  ─► batch Lambda "publish": pairing state → metrics → /data/v1 (site bucket)
```

The main change against design §12.6: **pairing statistics are not kept in DynamoDB.** Every results post writes one compact log item; an hourly job folds the new log items into a pairing state file in S3, from which it computes the work plan, and the daily job computes the rankings. A 10-bot melee battle then costs DynamoDB one log record instead of 45 pairing updates, the daily job needs no table scan, and per-user removal (§12.4) reads the same log. Rationale and numbers in §9.

## 2. Crates and build

| Path | Kind | Workspace | Contents |
|---|---|---|---|
| `crates/rumble-core` | library | root | Pure, no I/O, no async: API types (serde), batch tokens (HMAC), configuration and its defaults, rumble/division definitions, the result record format, pairing state and its folding, the planner (priorities, quotas, plan file), the metrics of the data contract and the data-file types (`types.ts` mirrored), reputation and verification state machine, result validation, jar validation, `.properties` parsing, the codesize class-file parser, the `Store` trait and `MemStore` (tests). |
| `crates/rumble-client` | library | root | Blocking HTTP client (`ureq`, already in the tree) for every endpoint, GitHub device flow, token file, jar cache, battle execution through `runner`/`server` (§13), round and battle hashes, track building (`replay`), the `Contributor` loop with events for CLI and GUI, the seeding importer (§16). |
| `crates/cli` | binary | root | `robocode-s rumble ...` subcommands (§13.6). |
| `crates/gui` | binary | root | The Rumble page (§13.7). |
| `rumble/lambda` | 2 binaries | **own workspace** | `api` (all `/api` routes) and `batch` (`plan`, `publish` and `cleanup` modes); DynamoDB, S3, SSM and GitHub implementations of `rumble-core`'s traits; tokio, `lambda_http`, `lambda_runtime`, `aws-sdk-dynamodb`, `aws-sdk-s3`, `aws-sdk-ssm`, `aws-config`, `reqwest` (rustls). |

- `rumble/lambda` has its own `[workspace]` and `Cargo.lock` and is listed in the root `Cargo.toml` as `exclude = ["rumble"]`. The AWS SDK crates take minutes to compile; keeping them out means `cargo clippy --workspace` and `cargo test --workspace` at the root stay as they are. It depends on `rumble-core` by path (`../../crates/rumble-core`). The root workspace's `[workspace.dependencies]` gets `robocode-s-rumble-core` and `robocode-s-rumble-client`; `members = ["crates/*"]` picks them up.
- `rumble-core` depends only on: `serde`, `serde_json` (`=1.0.151`), `sha2`, `hmac` (new, RustCrypto, matches `sha2 0.11`), `base64` (in the tree), `zip` (`=8.6.0`, deflate only, as `runner`), `thiserror` is not used (plain `String` errors, as elsewhere). No `rand`: callers pass random bytes in (`getrandom` in the client and Lambdas), so the core stays deterministic under test.
- **Lambda build** (cargo-lambda isn't installed; cargo-zigbuild and zig are, as for the Linux release builds):
  ```sh
  cd rumble/lambda
  cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.34 --bins
  # tools/rumble/build-lambdas.sh copies each binary to target/lambda/<name>/bootstrap
  ```
  Runtime `provided.al2023`, architecture `arm64`. `tools/rumble/build-lambdas.sh` (new) does both steps; `infra` `pnpm deploy` runs it first (`build:lambdas`), and CDK takes `Code.fromAsset('../rumble/lambda/target/lambda/<name>')`. Profile: `opt-level = "s"`, `lto = "thin"`, `strip = true`, `codegen-units = 1`.
- **Release gate** (`tools/release/release.sh` check, there is no CI): add `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` with `--manifest-path rumble/lambda/Cargo.toml`, and `pnpm -C infra test typecheck`.

## 3. Configuration

All tunables are one struct, `rumble_core::Config`, with the defaults below in code. Overrides are a JSON object in the state bucket, `state/config.json` (missing: defaults), written by `robocode-s rumble admin config set` (§12); Lambdas re-read it at most every 5 minutes. Secrets are not configuration (§11.5).

| Key | Default | Meaning |
|---|---|---|
| `engineVersion` | `ENGINE_VERSION` of the Lambda build | The engine version being ranked and handed out. Changing it starts the new version's pairings (§10.6). |
| `minClient` | `"0.2.0"` | Oldest client release accepted (semver; `X-Rumble-Client`). |
| `paused` | `false` | Work requests get 204 (results are still accepted). |
| `divisions.1v1` | rounds 35, field 800×600, robots 2, share 0.7 | Battle rules per division; `gunCoolingRate` 0.1, `inactivityTime` 450, `sentryBorder` 100, `hideEnemyNames` false for all. |
| `divisions.melee` | rounds 35, field 1000×1000, robots 10, share 0.3 | |
| `weightLimits` | nano 250, micro 750, mini 1500 | Codesize must be **below** the limit. |
| `batch.maxBattles` | 1v1 50, melee 10 | Per work request. |
| `batch.deadlineMinutes` | 180 | Results after the deadline are refused. |
| `rate.workSeconds` | 10 | Minimum interval between work requests per token. |
| `rate.resultsSeconds` | 5 | Minimum interval between results posts per token. |
| `rep.startRate` | 0.20 | Verification rate of a new user. |
| `rep.floorRate` | 0.01 | |
| `rep.tau` | 40 | `rate = max(floor, start · e^(−rep/τ))`: the floor at about 120 matches. |
| `rep.verifierMin` | 50 | Reputation needed to get verification, probation and watch work. |
| `rep.recorderMin` | 20 | Reputation needed to be asked for recordings. |
| `rep.quarantineMatches` | 20 | Matching verified battles that end a quarantine. |
| `rep.recheckOnPenalty` | 20 | Recent battles re-verified when a user is penalized. |
| `rep.banMismatches` | 3 | Confirmed mismatches within `rep.banWindowDays` (90) that ban. |
| `probation.battles` | 10 | Battles before a new version is ranked. |
| `probation.failShare` | 0.8 | Share of failed battles that fails probation (and deactivates an active bot over its last 20). |
| `slots.top` | 50 | Matchups with a bot ranked this high or better get recording slots (1v1 also each bot's best, worst and problem pairings, §12.5). |
| `slots.perSide` | 5 | |
| `slots.maxBytes` | 262144 | Largest track accepted (a 6,000-turn melee round is about 50 KB). |
| `plan.priorVariance` | 1v1 16, melee 25 | Prior variance of a battle's pairing score (APS points²). |
| `plan.priorWeight` | 3 | Pseudo-battles of the prior. |
| `plan.closeFactor` | 10 | Weight of a pairing whose score is within its 95% CI of 50. |
| `plan.rankFactor` | 2 | Extra weight for bots whose rank is contested (§10.3). |
| `plan.minHourly` | 1v1 200, melee 20 | Lower bound of the battles planned per hour. |
| `plan.headroom` | 1.5 | Planned battles = last hour's battles × headroom. |
| `jar.maxBytes` | 5 MiB | Jar size limit (compressed). |
| `jar.maxUnpacked` | 20 MiB, 5,000 entries | Zip-bomb guard. |
| `submit.maxBots` | 5 | Bots one user may own (§12.2). |
| `submit.versionsPerDay` | 5 | New versions per bot per day. |
| `submit.minAccountDays` | 30 | Younger GitHub accounts need the owner's approval to submit. |
| `admins` | `[4036752]` (nhofsted) | GitHub numeric ids with the admin role. |

## 4. Identifiers

- **Bot id:** fully qualified class name without version (`jk.mega.DrussGT`), as in `rumble-data.md`. Allowed characters `[A-Za-z0-9_$.]`, at most 100.
- **Version:** the robot's `robot.version` property, 1–40 printable ASCII characters, no `,` (classic's participant format).
- **vid:** a `u32` per bot version, allocated from a counter item (§9.2). All internal keys, records and state use vids; files and API responses show bot id and version as well. Base 36 in DynamoDB keys (`vid36`).
- **Division:** `1v1`, `melee` (later `teams`, `twinduel`). Pairings, battles and recordings belong to a division. **Rumble id:** division plus `-mini`/`-micro`/`-nano` (`types.ts`). A battle belongs to a division; it counts in each rumble both bots are members of.
- **User id (`uid`):** `g<GitHub numeric id>` (`g583231`): stable when the login changes.
- **Engine version `E`:** prefix of every key that holds results, so versions are never mixed (O14).

## 5. HTTP API

Base `https://<domain>/api/v1/`. JSON bodies, UTF-8, `camelCase`. 64-bit seeds are strings (JavaScript can't hold them).

### 5.1 Transport through CloudFront

- `/api/*` is a CloudFront behavior to the `api` Lambda's **function URL** (auth type `AWS_IAM`) with **origin access control**: CloudFront signs every origin request with SigV4.
- **Request bodies:** Lambda function URLs don't accept unsigned payloads, and CloudFront doesn't hash bodies. Every `POST` must therefore carry **`x-amz-content-sha256: <hex SHA-256 of the exact body bytes>`** (for an empty body, of the empty string). `rumble-client` sets it on every POST; the website's single fetch wrapper (`web/src/lib/api.ts`) computes it with `crypto.subtle.digest('SHA-256', body)`. Without it, the request fails at the function URL with 403 before reaching our code. Only `GET` and `POST` are used (no `PUT`/`DELETE` to the API).
- **Our token is not in `Authorization`:** OAC overwrites that header with its signature. Clients send **`X-Rumble-Token: <token>`**; the website uses the session cookie.
- Cache policy `CACHING_DISABLED`; origin request policy `ALL_VIEWER_EXCEPT_HOST_HEADER` (passes cookies, query strings and `X-Rumble-*`; `x-amz-content-sha256` cannot be listed, OAC uses the viewer's header itself when signing); allowed methods all; response headers policy `SECURITY_HEADERS`. Request bodies over 1 MiB get 413 (the function URL's own limit is 6 MB).
- The Lambda sees the viewer IP in `CloudFront-Viewer-Address` (added to the origin request policy as a CloudFront header).

### 5.2 Versioning and common headers

- The path carries the API major version (`/api/v1`). Within v1 fields are only added. A breaking change is `/api/v2`, served next to v1 until the last client release using v1 is below `minClient`.
- Clients send `X-Rumble-Client: robocode-s/<release> (engine <E>; <os>-<arch>)`. A release below `minClient` gets **426** `client_outdated` with the download page in `message`.
- Every response is `application/json` except 204 and redirects. Errors: `{"error": "<code>", "message": "<text for humans>"}`.

| Status | `error` | When |
|---|---|---|
| 400 | `bad_request` | Malformed JSON, missing fields, invalid values |
| 401 | `unauthorized` | No or unknown token or session |
| 403 | `forbidden` | Banned user, not an admin, not the bot's owner, bad CSRF origin |
| 404 | `not_found` | |
| 409 | `engine_mismatch` | The client's engine isn't `engineVersion` (`message` says which) |
| 409 | `expired` | Batch past its deadline |
| 409 | `conflict` | Bot version exists with another jar, slot already filled, ... |
| 413 | `too_large` | Body over 1 MiB, jar over `jar.maxBytes` |
| 426 | `client_outdated` | |
| 429 | `rate_limited` | With `Retry-After` (seconds) |
| 503 | `busy` | DynamoDB throttled after the SDK's retries; with `Retry-After: 30` |

### 5.3 Rate limits

Per token, in each Lambda instance's memory (approximate: at most reserved-concurrency instances): work requests `rate.workSeconds`, results posts `rate.resultsSeconds`, other calls 2/s burst 10. **Admin tokens are exempt** from these per-token limits (the importer paces itself at 10 calls/s and still retries 429s, which a failed deploy or a proxy could produce). Per viewer IP without a token: 1/s burst 20 for `auth/*`. Global limits come from reserved concurrency (§14.3) and DynamoDB's fixed capacity (§9.5). No DynamoDB rate counters.

### 5.4 Endpoints

Auth column: `-` none, `T` upload token or session, `S` session cookie only (browser), `A` admin (token or session).

| Method, path | Auth | Purpose |
|---|---|---|
| `GET info` | - | Server facts for clients and the site |
| `GET auth/login?return=<path>` | - | Web sign-in: redirect to GitHub |
| `GET auth/callback?code&state` | - | GitHub's redirect back; sets the session |
| `POST auth/logout` | S | Ends the session |
| `POST auth/token` | - | Exchanges a GitHub device-flow token for an upload token |
| `GET me` | T | Profile, reputation tier, counters |
| `POST me` | T | `{"country"?, "hidden"?}`: change the country (ISO 3166-1 alpha-2 or `null`) and whether the user is hidden from the contributors list (§15) |
| `GET me/tokens` | T | Upload tokens and sessions (id, label, created, last used) |
| `POST me/tokens/revoke` | T | `{"id": "..."}`; `"current"` revokes the caller's own |
| `GET me/bots` | T | The user's bots with versions and states |
| `GET work?...` | T | A batch of battles |
| `POST results` | T | Results of a batch |
| `POST recordings/done` | T | Confirms uploaded recordings |
| `POST bots/upload-url` | T | Presigned PUT for a jar |
| `POST bots/submit` | T | Registers a bot version from an upload or a URL |
| `POST bots/retire` | T | `{"bot": "id"}`: the owner retires the bot |
| `POST admin/jars/upload-url` | A | As `bots/upload-url`, for the importer |
| `POST admin/bots/import` | A | Registers an imported bot version (§16) |
| `POST admin/bots/claim` | A | `{"bot", "github"}`: gives a bot to a user |
| `POST admin/bots/remove` | A | `{"bot", "reason"}`: takedown (§12.4) |
| `POST admin/users/update` | A | `{"uid", "rep"?, "state"?, "admin"?}` |
| `GET admin/alerts` | A | Open alerts (§11.4) |
| `GET admin/config`, `POST admin/config` | A | Read and replace `state/config.json` |

#### `GET info`

```json
{"api": 1, "engineVersion": 1, "minClient": "0.2.0", "paused": false,
 "githubClientId": "Ov23li...", "deterministicRules": 1,
 "divisions": [{"id": "1v1", "rounds": 35, "battlefield": {"width": 800, "height": 600}, "robots": 2},
               {"id": "melee", "rounds": 35, "battlefield": {"width": 1000, "height": 1000}, "robots": 10}],
 "downloads": "https://rumble.planethofstede.be/download"}
```

#### `GET work?engine=<E>&max=<n>&divisions=1v1,melee&slots=<n>`

`max`: battles wanted (1..`batch.maxBattles` of the division; the client asks for about 20 minutes of work, §13.3). `slots`: the client's parallel battle slots (melee battles need `slots ≥ 1` of 10 JVMs; informational). Response 200:

```json
{"batch": "<batch token>", "batchId": "b-01JB2...", "engineVersion": 1,
 "deadline": "2026-10-24T15:00:00Z",
 "battles": [
  {"index": 0, "kind": "normal", "division": "1v1", "seed": "-4512334012341234",
   "rules": {"rounds": 35, "width": 800, "height": 600, "gunCoolingRate": 0.1,
             "inactivityTime": 450, "sentryBorder": 100, "hideEnemyNames": false},
   "bots": [{"id": "jk.mega.DrussGT", "version": "3.1.16", "vid": 812,
             "sha256": "9f1c...", "size": 210345, "url": "/jars/9f1c....jar", "data": null},
            {"id": "voidious.Diamond", "version": "1.8.28", "vid": 77, "sha256": "...", "size": 98123, "url": "/jars/....jar"}]}
 ]}
```

- `data` (reserved, always `null` in M4): a later server-held data snapshot per bot (`{"sha256", "size", "url"}`, a zip of the robot's data folder) that the client copies in before the battle. Clients of M4 refuse a battle whose `data` isn't `null` (they report it `notRun`), so the feature can be added without misrunning battles.
- `kind`: `normal`, `verify` (a re-run of someone's battle, §11), `probation` (a new version's battles, §12.3), `watch` (M6). Robot order in `bots` is the battle's robot order (the server randomizes it).
- 204 with `Retry-After` when `paused`, or when no division the client offers has two active bots. Otherwise there is always work (no cap, §10).
- 409 `engine_mismatch` if `engine` isn't `engineVersion`.

**Batch token** (`rumble_core::token`): `base64url(payload JSON) "." base64url(HMAC-SHA256(key, payload bytes))`, no padding. Payload: `{"v":1, "id": batchId, "uid", "e": engine, "iat": unix s, "exp": unix s, "b": [[division, kind, seed, rounds, [vid...], ref?]...]}`. `ref` is the verification item's sort key for `verify` and `probation` battles. The rules come from the division's configuration at `iat` and are part of the payload hash only through `division`; a configuration change of rules must therefore also change `engineVersion` or the division id. The key is the SSM SecureString `signing-key` (§14.5). Keys rotate by appending: the parameter holds `kid:hexkey` lines; the newest signs, all verify; the token's first segment carries `"k": kid`.

Assignments are therefore **not stored**: the token is the assignment. What makes a result count once is the batch marker (§9.2).

#### `POST results`

```json
{"batch": "<batch token>", "engineVersion": 1,
 "client": {"release": "0.2.0", "os": "linux", "arch": "x86_64", "sandbox": "os"},
 "battles": [
  {"index": 0, "status": "ok", "durationMs": 41230,
   "battleHash": "<hex sha256>",
   "robots": [{"score": 4123.5, "survival": 1100, "lastSurvivorBonus": 220, "bulletDamage": 2201.3,
               "bulletDamageBonus": 312.4, "ramDamage": 12.0, "ramDamageBonus": 0,
               "firsts": 22, "seconds": 13, "thirds": 0, "rank": 1}, {"...": "..."}],
   "rounds": [{"turns": 912, "winner": 0, "scores": [151.2, 60.3],
               "hash": "<hex sha256>", "track": {"sha256": "<hex>", "size": 3401}}]},
  {"index": 1, "status": "robotFailed", "failure": {"robot": 1, "reason": "load", "detail": "class not found"}},
  {"index": 2, "status": "notRun"}
 ]}
```

- One post per batch, with every battle of it. Battles the client didn't run (stopped, deadline near) are `notRun`. `robotFailed` reasons: `load` (the robot couldn't be described or never connected), `crash` (its process ended during the battle); the battle's other fields are optional then. `error` is a client-side failure (no JRE, sandbox broken): not counted, and the client stops after 3 in a row.
- `robots` in the battle's robot order, as classic's results (the CLI's JSON results without `name`). `rounds[i].scores` is each robot's score in round `i` (the per-round difference of the cumulative `RoundResult`), `firsts` the robots whose `firsts` rose in that round (classic gives robots that die together a first each, so a round can have several; added 2026-10-24, absent from older clients), and `winner` the one robot whose `firsts` rose, or `-1` if none or several did (a tie fills no "rounds won by" slot). The server counts wins from `firsts`, or from `winner` when `firsts` is absent.
- Hashes are defined in §13.4.

Response 200:

```json
{"accepted": [0, 1], "rejected": [{"index": 3, "error": "invalid", "message": "rounds: 34 of 35"}],
 "duplicate": false,
 "uploads": [{"slot": "1v1.812.77.a.2", "index": 0, "round": 7,
              "url": "https://<bucket>.s3.eu-west-1.amazonaws.com/rec/...?X-Amz-...",
              "headers": {"Content-Length": "3401", "x-amz-checksum-sha256": "<base64>",
                          "Content-Type": "application/octet-stream"},
              "expires": "2026-10-24T13:10:00Z"}],
 "user": {"battles": 1834, "reputation": 61, "tier": "verifier"}}
```

- `accepted` includes `robotFailed` battles (they count for probation and failure tracking, not for pairings) and `notRun` ones (nothing to do).
- Validation per battle (`rumble_core::results::validate`): index in the batch, at most once; `ok` battles have `robots.len()` = the battle's bots, `rounds.len()` = its rounds, finite non-negative scores, the sum of `rounds[i].scores[r]` within 1e-6 (relative) of `robots[r].score`, `firsts` equal to the rounds the robot won, hashes of 64 lowercase hex digits, `track.size` 1..`slots.maxBytes`. An invalid battle is rejected and the rest still counts; a client sending more than 5 invalid battles in a day gets a log line, not a penalty (bugs happen; verification catches cheats).
- **Retries are idempotent:** a second post of the same batch returns 200 with `duplicate: true`, the same `accepted`/`rejected`, and fresh URLs for the slots the batch claimed and hasn't filled. A post after `deadline` gets 409 `expired`; one whose `engineVersion` or token engine isn't the current one, 409 `engine_mismatch` (results of an old engine are never counted).

#### `POST recordings/done`

`{"batch": "<batch token>", "slots": ["1v1.812.77.a.2"]}` → `{"filled": ["1v1.812.77.a.2"], "failed": [{"slot": "...", "error": "missing" | "checksum" | "expired"}]}`. The server reads the object's `ChecksumSHA256` with `HeadObject` (`ChecksumMode=ENABLED`) and marks the slot filled if it matches the claim.

#### `POST auth/token`

`{"githubToken": "gho_...", "label": "nick-desktop"}` → `{"token": "rsu_<43 chars base64url>", "user": {...me}}`. The token is shown once. Errors: 401 if GitHub rejects the token or it wasn't issued to our OAuth app (§7.2).

#### `POST bots/upload-url`, `POST bots/submit`

`upload-url`: `{"sha256": "<hex>", "size": 123456}` → `{"url", "headers", "expires"}`: a presigned PUT to `uploads/<uid>/<sha256>.jar` in the state bucket, valid 15 minutes, with `Content-Length` and `x-amz-checksum-sha256` signed, so S3 refuses any other body.

`submit`: `{"source": "upload", "sha256": "<hex>"}` or `{"source": "url", "url": "https://..."}`, plus `"divisions": ["1v1", "melee"]`, `"homepage": "https://..."` (optional). → 200 `{"bot": "id", "version": "v", "vid": 913, "state": "probation", "codesize": 1234, "weightClass": "mini"}` or 400 `invalid` with the validation messages (§12.1). The bot id and version come from the jar. 403 if the bot exists and isn't the caller's; 409 if the version exists with another jar.

#### `GET me`

`{"uid": "g583231", "login": "nhofsted", "name": "Nick Hofstede", "country": "be", "reputation": 61, "tier": "new" | "verifier" | "quarantined" | "banned", "verifyRate": 0.044, "battles": 1834, "verifiedMatches": 61, "mismatches": 0, "hidden": false, "admin": true, "since": "2026-10-24T..."}`.

#### `GET me/tokens`, `GET me/bots`

`me/tokens`: `{"tokens": [{"id", "kind": "upload" | "session", "label", "created", "lastUsed", "current"}]}` (`id` is the opaque handle `me/tokens/revoke` takes, not the token). `me/bots`: `{"bots": [{"id", "state", "versions": [{"version", "vid", "state", "codesize", "weightClass", "created", "probation": {"battles", "failed"}, "failures": ["<detail>"]}]}]}`, versions newest first; `state` is `probation`, `active`, `failed`, `retired` or `removed`; `failures` holds the clients' `detail` strings of a failed version. The website (`web/src/lib/api.ts`) relies on these shapes.

## 6. Data model of results

### 6.1 Per-battle record

The unit the server stores and the planner folds (`rumble_core::record`). A record is the result of one battle as far as the rankings need it:

```rust
pub struct Record {
    pub division: Division,
    pub weight: i8,              // +1 counts; −1 cancels an earlier record (§11.3)
    pub uid: Uid,                // who ran it (for removal), "system" for corrections
    pub contestants: Vec<Entry>, // battle robot order
    pub failed: Option<(u8, FailReason)>,
}
pub struct Entry { pub vid: u32, pub score: f32, pub firsts: u16 }
```

Binary encoding (little-endian), so log items stay small: `u8 division | i8 weight | u8 flags (bit0 failed, bit1 trusted reporter) | u8 n | n × (u32 vid, f32 score, u16 firsts) | [u8 failedRobot, u8 reason]`. 1v1: 24 bytes, melee: 104 bytes. `uid` is on the log item, not per record. `f32` scores lose nothing that matters (percentages to 1e-5).

**Teams later:** an entry becomes a team (`vid` of the team's bot version; the team's summed score), so the format holds; twin duel likewise.

### 6.2 Pairing score and survival (classic definitions)

For a battle and two of its contestants A and B (in melee, each of the 45 pairs):
- **pairing score** of A: `100 · score_A / (score_A + score_B)`, 50 if both are 0. This is classic RoboRumble's melee definition: a melee battle is split into pairs, each scored by the two bots' total scores in that battle. For 1v1 it is the usual percent score.
- **survival** of A: `100 · firsts_A / (firsts_A + firsts_B)`, 50 if both are 0: in 1v1 the share of rounds won (about the rounds survived), in melee the share of the two bots' wins.
- B's values are `100 −` A's, so a pairing stores A's side only.

## 7. Authentication

### 7.1 Web: authorization code flow

1. `GET /api/v1/auth/login?return=/account` (a relative path; anything else becomes `/`): creates a 128-bit nonce, sets `__Host-rs_oauth=<nonce>; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=600`, and redirects (302) to `https://github.com/login/oauth/authorize?client_id=<id>&redirect_uri=https://<domain>/api/v1/auth/callback&state=<state>&allow_signup=true` with no scopes (the public profile is enough). `state` = `base64url({"n": nonce, "r": return})` + HMAC as the batch token.
2. `GET /api/v1/auth/callback?code&state`: checks the HMAC and that `state.n` equals the cookie; `POST https://github.com/login/oauth/access_token` with client id, secret and code; `GET https://api.github.com/user` with that token; upserts the user (§9.2 `USER`); deletes the GitHub token (`DELETE /applications/{client_id}/token`, basic auth with id and secret), as we never need it again.
3. Creates a session: 32 random bytes, `rss_` + base64url. Sets `__Host-rs_session=<session>; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000` (30 days), clears the oauth cookie, 302 to `return`.
- **Sessions are renewed** when used with less than 7 days left (one write).
- **CSRF:** `POST`s authenticated by the cookie must have `Origin: https://<domain>` (the stage's site origin); otherwise 403. `SameSite=Lax` already keeps the cookie off cross-site POSTs; the check covers old browsers.
- `POST auth/logout` deletes the session item and clears the cookie.

### 7.2 CLI and GUI: device flow

1. The client `POST https://github.com/login/device/code` (`client_id`, `scope` empty) and shows `user_code` and `verification_uri` (and opens the browser when it can).
2. It polls `POST https://github.com/login/oauth/access_token` (`client_id`, `device_code`, `grant_type=urn:ietf:params:oauth:grant-type:device_code`) at GitHub's `interval`, handling `authorization_pending`, `slow_down` (+5 s) and `expired_token`. The device flow needs no client secret, so the client never has one. The OAuth app must have "Enable Device Flow" on.
3. It sends the GitHub token to `POST /api/v1/auth/token`. The server checks it with `POST https://api.github.com/applications/{client_id}/token` (basic auth, our id and secret), which only succeeds for tokens issued to **our** app (a token of another app would otherwise pass `/user`), reads the user from that response, upserts the user, **deletes the GitHub token** (`DELETE /applications/{client_id}/token`) and issues an upload token: `rsu_` + base64url(32 random bytes).
4. The client stores `{ "server": "https://<domain>", "token": "rsu_...", "login": "...", "savedAt": "..." }` in `rumble.json` next to `gui.json` (`$XDG_CONFIG_HOME/robocode-s/`, `~/Library/Application Support/robocode-s/`, `%APPDATA%\robocode-s\`; `ROBOCODES_RUMBLE_CONFIG` overrides), created with mode 0600 on Unix. `ROBOCODES_RUMBLE_TOKEN` overrides the file (for headless machines).

### 7.3 Token storage and revocation

- Tokens and sessions are stored only as **SHA-256 hashes** (§9.2 `TOK`). Upload tokens don't expire; they are revoked by the user (`me/tokens/revoke`), by `robocode-s rumble logout`, or by a ban. Sessions expire after 30 days (TTL).
- Each Lambda instance caches a hash → user lookup for 5 minutes: a revocation takes effect within 5 minutes everywhere (immediately in the instance that revoked it).
- A banned user's tokens all fail (the user's state is checked with the cached lookup).

## 8. S3

| Bucket | Served | Prefixes |
|---|---|---|
| Site bucket (exists) | `/*` | `data/v1/...` is written by `publish` (the sample-data deployment is turned off in prod once it runs, §14.6) |
| **Jars bucket** (new; `RemovalPolicy.RETAIN`) | `/jars/*`, `/rec/*` | `jars/<sha256>.jar` (immutable, `Cache-Control: public, max-age=31536000, immutable`, `Content-Type: application/java-archive`); `rec/e<E>/<division>/...` recordings (§12.5) |
| **State bucket** (new; private, `RETAIN`, versioned, noncurrent versions expire after 7 days) | no | `state/config.json`; `state/e<E>/<division>/pairings.bin`; `state/e<E>/<division>/plan.json.gz`; `state/e<E>/snapshots/<rumble>/<date>.json`, `.../snapshots/users/<date>.json`; `state/e<E>/probation.json` (probation items issued), `state/e<E>/cleanup.json` (cleaned versions, ban-removal cursors), `state/users.json` (every uid seen in the log: the users list of `publish`); `uploads/<uid>/<sha256>.jar` (lifecycle: expire after 1 day) |

Two buckets so that CloudFront's OAC grant (`s3:GetObject` on the whole bucket) can never expose state or uploads. Removing a jar (takedown) deletes `jars/<sha>.jar` and invalidates `/jars/<sha>.jar`.

## 9. DynamoDB

### 9.1 One table

Table `rumble` (prod) / `rumble-dev`: partition key `pk` (S), sort key `sk` (S), TTL attribute `ttl` (N, unix seconds). **No GSIs.** Prod: provisioned **25 RCU / 25 WCU**, no auto-scaling, point-in-time recovery off (it costs), deletion protection on. Dev: on-demand. Attribute names are short because they count towards item size, and so towards capacity.

### 9.2 Items

| Item | `pk` | `sk` | Attributes | TTL |
|---|---|---|---|---|
| User | `U#<uid>` | `P` | `login`, `name`, `country`, `ghid` (N), `rep` (N), `st` (`new`/`quar`/`ban`), `adm` (BOOL), `bat` (N, battles), `vm` (N, verified matches), `mm` (N, mismatches), `mmt` (L of unix s, mismatches in the ban window), `qm` (N, matches in quarantine), `cr` (created), `lp` (last post) | - |
| Token | `T#<sha256 hex of token>` | `T` | `uid`, `k` (`upload`/`session`), `lbl`, `cr`, `lu` (last used, written at most daily) | sessions: expiry |
| Token index | `U#<uid>` | `T#<first 12 hex of hash>` | `h` (full hash), `k`, `lbl`, `cr` | sessions: expiry |
| Counter | `CTR` | `vid` | `n` (N) | - |
| Bot | `BOT#<bot id>` | `M` | `au` (author name), `uid` (owner, if claimed), `hp`, `ds` (description), `dv` (SS divisions), `vs` (L of vids, newest first), `cur` (current ranked vid), `st` (`active`/`retired`/`removed`), `cr`, `src` (`import`/`submit`) | - |
| Version | `REG` | `V#<vid36>` | `bot`, `ver`, `sha`, `sz`, `cs` (codesize or absent), `wc` (`mega`/`mini`/`micro`/`nano`), `dv` (SS), `st` (`probation`/`active`/`failed`/`retired`/`removed`), `pb` (probation battles), `pf` (probation failures), `cls` (main class), `cr`, `ret` (retired at), `url` (source URL) | - |
| Batch marker | `B#<batchId>` | `B` | `uid`, `acc` (accepted indices, binary), `rej` (JSON of rejections), `slots` (SS of claimed slot ids) | `exp` + 7 days |
| Result log | `R#<E>#<YYYYMMDDHH>` (post time, UTC) | `<post ms>#<batchId>` | `uid`, `recs` (B: concatenated records), `nb` (N battles), `held` (BOOL, quarantined user) | 400 days |
| Verification | `V#<E>` | `<created ms>#<id>` | `div`, `seed`, `bots` (L of vids), `rounds`, `runs` (L of `{uid, h, rec, held}`), `uids` (SS), `stage` (1, 2), `cl` (claimant uid), `cu` (claim until), `kind` (`check`/`probation`/`recheck`) | 30 days |
| Slot | `S#<E>#<division>#<key>` | `S` | `m` (M: slot id → `{st: c/f, b: batchId, r: round, h: sha256 b64, n: size, x: claim expiry, sd: seed, v: L of N}`; `v` (M6) is the battle's robot order, the vids as passed to `claim_battle`, absent in entries written before M6), `full` (BOOL) | - |
| Alert | `ALERT` | `<ms>#<id>` | `kind`, `text`, `ref` | 90 days |
| Watch (M6, model only) | `W#<E>` | `<ms>#<id>` | `div`, `bots`, `seed`, `round`, `uid`, `st` (`open`/`claimed`/`done`), `cl`, `cu` | 1 day |

Slot `key` (§12.5): 1v1 `<vidLo36>#<vidHi36>`, melee `<vid36>`. Slot ids in the API: `<division>.<vid>.<vid>.<side>.<i>` (1v1, `side` `a` for the lower vid winning, `b` for the higher) and `melee.<vid>.<w|l>.<i>`.

### 9.3 Writes per operation

| Operation | DynamoDB | WCU | RCU |
|---|---|---|---|
| Auth lookup (cache miss, ≤ once per token per instance per 5 min) | GetItem `T#` + GetItem user | - | 1 |
| `GET work`, normal | none (plan in memory, from S3) | 0 | 0 |
| `GET work`, verifier | Query `V#<E>` (Limit 25, cached empty for 30 s), claim each served item (UpdateItem, condition) | 1 per item served | 0.5 |
| `POST results` (1v1, 20 battles) | TransactWriteItems: Put `B#` (~0.3 KB), Put `R#` (20 × 24 B + keys ≈ 0.6 KB), Update user (~0.3 KB) | 2 × (1 + 1 + 1) = **6** | 0 |
| ... same, melee, 5 battles (≈ 0.6 KB of records) | | 6 | 0 |
| ... each battle picked for verification (rate r) | Put `V#` (~0.5 KB) | 1 | - |
| ... each eligible battle (a top-50 bot or a `slotPairs` matchup, user ≥ `rep.recorderMin`) | GetItem slot (cached when `full`); UpdateItem claim if a side has an open slot | 2 on claim (item ≤ 2 KB) | 0.5 |
| ... claims recorded on the marker | UpdateItem `B#` `ADD slots` | 1 | - |
| `recordings/done` | UpdateItem slot per slot (condition: claimed by this batch) | 2 each | - |
| Verification result | GetItem `V#`, UpdateItem/Delete `V#`, Update 2–3 users | 3–5 | 0.5 |
| `plan` (hourly) | Query `REG` (~1,500 × 0.12 KB = 180 KB), Query the hour's `R#` partition, Update changed `REG` items | ~1 | ~25 |
| `publish` (daily) | Query `REG`, GetItem users of claimed bots | - | ~40 |

**Budget at the design's target load** (3 battles/s 1v1 sustained, batches of 20, so 0.15 posts/s; verification rate averaging 5%; 9% of battles eligible for slots):

| | WCU/s | RCU/s |
|---|---|---|
| Results transactions (0.15 × 6) | 0.90 | - |
| Verification items created (3 × 0.05 × 1) | 0.15 | - |
| Verification served and resolved (0.15 × 6) | 0.90 | 0.08 |
| Slot reads (3 × 0.09 × 0.5) | - | 0.14 |
| Slot claims and fills, initial fill (≈ 4 per eligible 1v1 battle while slots are open: 3 × 0.09 × 4 × 2) | 2.2 | - |
| Auth lookups, work requests, site API | 0.1 | 0.5 |
| Planner and publisher, averaged | ~0 | 0.02 (bursts of ~25 RCU once an hour) |
| **Total** | **≈ 4.3** | **≈ 0.8** |

So 25 WCU carry about 15 battles/s even during the initial slot fill, five times the design's target, and reads are far below 25. Bursts (a big melee post, the planner's queries) are absorbed by DynamoDB's burst capacity (up to 300 s of unused capacity). If writes are throttled anyway, the SDK retries; a post that still fails gets 503 `busy` and the client retries it later (idempotent).

**Storage:** log items about 1.3 KB per 20 battles: at 100k battles a day, 6.5 MB a day, about 2.4 GB with the 400-day TTL. Everything else is small. Within the 25 GB free.

### 9.4 Why no pairing items

The design's per-pairing items with per-bot sums updated by delta (§12.6) cost 1 WCU per pairing per battle plus the bot items (a nano 1v1 battle: 1 pairing + 2 bots × 4 rumbles = 9 WCU; a melee battle 45+) and a full table scan daily (~50 MB, about 6k RCU, beyond one Lambda run at the free rate). Folding a log hourly costs one transaction per post and makes the daily run a pure S3 job. The price: pairing counts used for planning are up to an hour old (the plan is hourly anyway), and the live state is a file only one writer (`plan`, reserved concurrency 1) may touch.

### 9.5 Dev vs prod

Same table layout and code. Dev's on-demand table costs cents at test traffic and keeps the 25/25 free provisioned capacity for prod (ADR 0016).

## 10. Work allocation

### 10.1 Principles

- **No hard cap** (owner, 2026-10-24; this replaces #23's threshold). The rumble uses all the work volunteers offer and directs it to where one more battle is most useful. The only limits are physical: DynamoDB writes (§9.3, about 15 battles/s) and the Lambda request allowance (§14.3), which the batch sizes keep far away.
- The planner runs **hourly** and writes a **plan** per division: how many battles each pairing should get in the next hour. The `api` Lambda samples battles from the plan held in memory, with no DynamoDB reads. Duplicate issue across Lambda instances only adds battles to useful pairings; nothing breaks.
- **Verification re-runs and probation battles are always served first** to clients that may run them (reputation ≥ `rep.verifierMin`), up to half of a batch; then normal battles.

### 10.2 Pairing statistics

Pairing state per (engine, division, A, B) with A < B by vid, in `pairings.bin` (§10.5): `n` battles, `s = Σ score_A`, `q = Σ score_A²`, `v = Σ survival_A`, `d` = day of the last battle. From it:
- mean `m = s/n`;
- sample variance `v̂ = (q − n·m²)/(n − 1)` for n ≥ 2;
- shrunk variance `σ² = (k₀·σ₀² + (n−1)·v̂) / (k₀ + n − 1)` with `σ₀² = plan.priorVariance` (1v1 16, i.e. 4 APS points per battle, LiteRumble's prior) and `k₀ = plan.priorWeight` (3); `σ² = σ₀²` for n < 2;
- standard error `SE = σ/√max(n,1)`; pairing 95% CI `1.96·SE`. Unlike LiteRumble, `n` is not capped at 5,000 (no cap; at 5,000 battles the SE is 0.06 points anyway).

Per bot X in a rumble (members only): `APS_X` = mean of m over its pairings with n > 0, `SE_APS_X = √(Σ σ²_XO/n_XO) / P_X` (P_X pairings with n > 0), `missing_X` = members it has no battle against.

### 10.3 Priority of one more battle

For pairing (A, B) in division D, with `R(A,B)` the rumbles of D that both are members of (the general one always; `-mini` if both are mini or smaller, and so on):

```
gain(n)   = σ² / ((n + ½)(n + 1½))                 expected variance reduction of the pairing mean
                                                     (n = 0: 1.33 σ², so unfought pairings come first)
close     = plan.closeFactor (10) if |m − 50| ≤ 1.96·SE, else 1        (n = 0: 10)
contested_X,r = 1 + plan.rankFactor · exp(−z²/2),   z = gap_X,r / √(SE_APS_X² + SE_APS_nb²)
                gap = APS distance to the nearer neighbour in rumble r's APS order (nb that neighbour)
                (a bot with no SE yet, i.e. P_X < 2: contested = 1 + rankFactor)
coverage_X,r  = 1 + missing_X,r / members_r          bots with incomplete pairings get more
V(A,B)    = gain(n) · close · Σ_{r ∈ R(A,B)} ½ (contested_A,r · coverage_A,r + contested_B,r · coverage_B,r)
```

- Fewest battles first falls out of `gain` (it falls with n²), uncertainty out of σ², closeness out of `close` (pairing score near 50, LiteRumble's ×10) and `contested` (bots close in rank). Weight-class pairings count once per rumble they are in, so the small rumbles fill in proportion.
- Probation versions, failed, retired and removed versions are not in the normal plan.
- **Hourly target** `H_D = max(plan.minHourly_D, plan.headroom · battles_D in the last full hour)`.
- **Quotas (1v1):** water-filling: a max-heap of `V` over all pairings; pop the top, give it one battle, recompute its `V` with `n + 1` (σ², m unchanged), push it back; `H_D` times. A pairing popped k times gets quota k. Cost O(P + H log P): about 0.3 s for 605k pairings and H = 20,000.
- **Melee:** quotas per *bot* instead: water-filling over bots with `V_X = Σ_O V(X,O)` (recomputed for X after each pop by scaling its pairings' n by 1/9 of a battle each, i.e. `n_XO += 1/9` for its top 9 partners), `H_D` battles = `H_D` anchors. For each bot the plan also keeps its 40 partners with the highest `V(X,O)`.

### 10.4 Serving a batch (`api`, `GET work`)

1. Choose divisions: among the client's `divisions`, each with probability ∝ its `share` (1v1 0.7, melee 0.3) among those it offers; a batch is one division.
2. If the user may verify (rep ≥ `rep.verifierMin`, state `new`): up to `⌈max/2⌉` battles from `V#<E>` (oldest first, `NOT contains(uids, me)`, unclaimed or claim expired), each claimed with a conditional UpdateItem `cl = me, cu = now + deadline`. Probation battles (§12.3) come from the same queue. Watch requests (M6) likewise.
3. Fill the rest from the plan: **1v1** samples pairings with probability ∝ remaining quota (an alias table rebuilt when the plan changes; the instance decrements its copy). When the instance has used its quotas, it samples ∝ the original quotas, so extra volunteers still get the most useful pairings. **Melee** samples an anchor ∝ remaining anchor quota, then adds 9 partners one at a time, each sampled ∝ `Σ V(chosen, O)` over the anchor's and the chosen bots' partner lists (bots not yet in the battle).
4. Seeds: 64 random bits each (`getrandom`). Robot order: shuffled.
5. Sign the token; no write.

### 10.5 The planner (`batch plan`, hourly at :12, so the hour that just ended is past its 10 minutes of grace)

1. Read `pairings.bin` (none yet: empty state, `done` = the hour before the engine version's first batch). For each hour `H` from its `done + 1` up to the last hour that ended at least 10 minutes ago: Query `R#<E>#<H>` (all pages) and fold each non-held record into the pairing state: for each pair of contestants (A, B) of a counted record, `n += w`, `s += w·score`, `q += w·score²`, `v += w·surv`, `d = today` (w = the record's weight, ±1). `robotFailed` records update the versions' failure tallies instead (§12.3). Every counted record adds 1 to the division's battle count and to each contestant's per-version battle count. The 10-minute grace covers a post whose time was taken just before an hour ended and whose write landed later.
2. Read `REG` (all versions). Drop pairings with a version that is retired, removed or failed (their state is not needed: pairings belong to current versions, `rumble-data.md`).
3. Compute the plan (§10.3) for each division and write `plan.json.gz`, then `pairings.bin` with the new `done`. The state file is its own cursor: a crash before it is written folds the same hours again into the old state, so nothing is counted twice.
4. Probation and failure transitions (§12.3) as UpdateItems on `REG`.

`pairings.bin` (`rumble_core::state`): gzip of `"RSPS" | u16 version 1 | u32 engine | u8 division | u32 done hour (YYYYMMDDHH) | u64 battles | u32 versions | versions × (u32 vid, u32 battles, u32 failed, u32 last-20 failure bits) | u32 count | count × (u32 a, u32 b, u32 n, f64 s, f64 q, f64 v, u16 day)` sorted by vid and (a, b); 38 bytes a pairing, about 23 MB for 1v1 at 1,100 bots before compression. Weights of −1 can make `n` reach 0, in which case the entry is dropped.

`plan.json.gz`:

```json
{"engine": 1, "division": "1v1", "generatedAt": "...", "hourly": 6000,
 "bots": [[vid, "id", "version", "sha256", size], ...],
 "pairs": [[vidA, vidB, quota], ...],
 "anchors": [[vid, quota], ...], "partners": {"vid": [[vid, weight], ...]},
 "top": [vid, ...],
 "slotPairs": [[vidLo, vidHi], ...],
 "probation": [vid, ...]}
```

`top` holds the versions ranked in the top `slots.top` of any rumble of the division in the latest `publish` (from its snapshot), for slot eligibility (§12.5). `slotPairs` (additive; absent or empty in older plans and in melee) holds the 1v1 matchups that get slots besides those with a `top` bot, lower vid first, sorted: `plan` reads them from `state/e<E>/1v1/slot-pairs.json` (`{"pairs": [[lo, hi], ...]}`), which `publish` writes from the general 1v1 rumble's metrics (`rumble_core::slots::slot_pairs`, §12.5), so they are as old as the last publish. The `api` Lambda reloads the plan when its S3 ETag changes (checked at most every 60 s with a conditional GET).

### 10.6 A new engine version

Bumping `engineVersion` in the config (after releasing the client that has it): new batches carry the new engine; old batches' results are refused (409); the planner starts empty state files for the new `E` and every pairing has n = 0, so coverage comes first by itself; `publish` ranks only `E`, and the old state is kept. Old clients get `engine_mismatch` and tell the user to update.

### 10.7 To be refined from the LiteRumble research

> **Refinement point.** The algorithm above is complete and is what M4 builds. The research on classic RoboRumble and LiteRumble (2026-10-24) is folded in as follows; further results may adjust the marked constants without changing the structure.
> - *Adopted from LiteRumble:* the pairing variance shrunk toward a prior of 16 (4 APS points per battle) with weight 3 pseudo-battles; the pairing CI `1.96·√(σ²/n)`; the variance reduction of one more battle as the base value; ×10 for a pairing whose score is within its CI of 50; bots without a CI first, then bots weighted by their APS uncertainty (here `contested` and `coverage`); incomplete pairings first (here `gain(0)` and `coverage`).
> - *Different from LiteRumble:* no cap of 5,000 battles in the CI; quotas for an hour computed over all pairings at once (water-filling) instead of one priority pair per upload, because our clients don't choose pairings and the server must hand out whole batches; melee priority computed every hour like 1v1 instead of on 2% of uploads.
> - *Open:* LiteRumble's candidate pool (opponents scoring at least 0.5 of the best, 0.25 when small) is replaced by sampling ∝ quota, which spreads work in the same way. How LiteRumble splits melee battles into pairings was not confirmed; we use classic's per-pair split (§6.2). Tunables: `closeFactor`, `rankFactor`, the `coverage` term and the melee prior variance (25, a guess: melee scores vary more).

## 11. Verification and reputation (§12.4)

### 11.1 State

User `rep` (matches since the last reset), `st`: `new` (normal), `quar` (quarantined), `ban`. Verification rate `r = max(rep.floorRate, rep.startRate · e^(−rep/rep.tau))`: 20% at 0, 8% at 37, 1% from 120 on. Tier shown: `new` below `rep.verifierMin`, `verifier` at or above it, `quarantined`, `banned`.

The owner's accounts are made verifiers once with `admin/users/update` (`rep: 1000`), so verification works from day one.

### 11.2 Choosing battles to verify

At `POST results`, for each accepted `ok` battle of a user in state `new`: with probability `r` (from the server's random bytes), Put a `V#` item `kind: check`, `stage: 1`, `runs: [{uid, h: battleHash, rec, held: false}]`. A quarantined user's battles all get one, with `held: true`, and their records are written with `held` on the log item (the planner skips them).

### 11.3 Resolving

A verifier's `verify` battle comes back in its results post (the token's `ref` names the `V#` item). The server reads the item and compares `battleHash`:

- **Stage 1, match:** both users `rep += 1`, `vm += 1` (a quarantined original: `qm += 1`; at `rep.quarantineMatches` it returns to `new` with `rep` 0). If the original was held, its record is released: appended to the verifier's log item as a record with the original's `uid`. Delete the item. The verifier's own run is not counted again (the battle counted once already).
- **Stage 1, mismatch:** add the verifier's run, `stage: 2`, clear the claim. The next verifier (not in `uids`) breaks the tie.
- **Stage 2:** three runs. If two hashes agree, the third user is **penalized**: `rep = 0`, `st = quar`, `mm += 1`, mismatch time appended to `mmt`. If the penalized run is the one that was counted, the server appends a correction to the log: the original record with weight −1 and the majority's record with weight +1, both `uid: system`. The two that agreed get `rep += 1`. `rep.recheckOnPenalty` of the penalized user's most recent counted battles (found by querying the last 48 hourly `R#` partitions, filtered by `uid`) get `V#` items of `kind: recheck`, each a battle that is corrected the same way if it turns out wrong. If all three disagree: an **alert** (`ALERT` item and the log line `ALERT engine-mismatch <id>`, which a CloudWatch metric filter turns into an alarm e-mail), nobody is penalized, the counted result stays.
- **Ban:** `rep.banMismatches` confirmed mismatches within `rep.banWindowDays` → `st = ban`: every token fails, and the `cleanup` job writes correction records (weight −1) for every counted record of the user still in the log (400 days), rate-limited to 5 WCU/s.
- A claimed item whose claim expired goes back to the queue. One still open after 30 days disappears by TTL: that battle simply stays unverified.

### 11.4 Alerts

`ALERT` items are listed by `admin/alerts` and `robocode-s rumble admin alerts`. The `api` and `batch` Lambdas log `ALERT <kind> ...` for: three-way mismatch, a bot failing after it was active (§12.3), planner errors, a takedown. A metric filter on both log groups counts them, and an alarm (≥ 1 in 5 minutes) mails the owner through an SNS topic (free tier: 1,000 e-mails a month).

## 12. Bots: submission, validation, probation, recordings

### 12.1 Static validation (`rumble_core::jar`)

On the bytes of a jar, in this order; each failure is a message for the author:
1. Size ≤ `jar.maxBytes`; a zip with ≤ 5,000 entries, unpacked ≤ 20 MiB, no entry name with `..`, an absolute path or a backslash.
2. Exactly one robot `.properties` with `robot.classname` (classic's packager writes `<package path>/<ShortName>.properties`); `robot.version` present and valid (§4); a jar with no robot properties but a team file (`.team`) is refused for now ("teams are not open yet"); a robot's jar that also ships a team of it (`davidalves.Phoenix`) is a robot.
3. `robot.classname` is a valid bot id and `<classname with / >.class` is in the jar and parses as a class file (magic `CAFEBABE`, major version ≤ 65, Java 21).
4. **Codesize:** the sum of `code_length` of every `Code` attribute of every method of every `.class` entry in the jar, as classic RoboRumble's codesize tool (`codesize.Codesize`) counts it. Weight class: nano `< 250`, micro `< 750`, mini `< 1500`, else mega. The parser reads the constant pool only to find attribute names (`Code`); it doesn't resolve anything else.
5. Metadata: `robot.author.name` (author, else the first package segment), `robot.description`, `robot.webpage`, `robot.java.source.included` ignored.

The client runs the same validation before it uploads (import, and `rumble submit` later), so errors show before any upload.

### 12.2 Submission

`bots/upload-url` then a PUT, or a URL (`http`/`https`, fetched by the Lambda with a 10 s timeout and the size limit; redirects followed up to 3), then `bots/submit`: the Lambda validates, copies the jar to `jars/<sha256>.jar` (if not there), allocates a vid, and writes the version `REG V#<vid>` with `st: probation`, and the bot `M` item (created, or the new vid prepended to `vs`) in one transaction. A new bot belongs to the submitter (`uid`). **Limits** (config `submit.*`): at most 5 bots per user (`submit.maxBots`), 5 new versions per bot per day (`submit.versionsPerDay`), and a GitHub account younger than 30 days (`submit.minAccountDays`, from `created_at` at sign-in, stored on the user as `ghc`) gets 403 `forbidden` with a message to ask the owner, who can lift it (`admin/users/update` `{"trusted": true}`, attribute `tr`). Admins and imported bots are exempt. An existing bot can only get a new version from its owner (403 otherwise; unclaimed imported bots need a claim first, §16.4). The same version with the same jar: 200, nothing changes; with another jar: 409.

### 12.3 Probation, ranking, retiring

- A version in `probation` gets `probation.battles` battles, `kind: probation`, through the verification queue (`V#` items of `kind: probation`, created by the planner, each against an active bot sampled near the middle of the ranking), so only verifiers run them. They count normally when they succeed.
- The planner tallies `robotFailed` for the version in the state file and copies the probation counts to `REG` (`pb`, `pf`). After `probation.battles`: if `pf/pb ≥ probation.failShare` → `failed` (shown on the owner's bots page with the clients' `detail` strings; the author can submit a fixed version); else → `active`, and the bot's previous active version → `retired` (`ret` = now), `cur` = the new vid. The previous version stays ranked until then, so a bot never drops out of the rankings while its new version is on probation.
- An active version whose last 20 battles failed at least `failShare` → `failed` with an alert (a jar that stopped working, a JVM change).
- **Only trusted reports count towards failure:** `robotFailed` battles from users below `rep.verifierMin` are logged but don't enter the failure tallies (probation battles only go to verifiers anyway), and deactivating an active version also needs failures reported by at least 3 distinct users. Otherwise one client could knock a strong bot out by reporting it failed. The log record of a `robotFailed` battle carries a `trusted` flag (flags bit1) set by the results Lambda.
- **Retire** (`bots/retire`): the bot's current version and the bot → `retired`. **Takedown** (`admin/bots/remove`): `removed`, the jar and its recordings are deleted from S3 and CloudFront is invalidated; the bot disappears from the next published files.
- Imported bots skip probation (§16.3).

### 12.4 Old versions' data

A retired version's pairings leave the pairing state at the next plan; its history stays in its bot file. Its recordings are deleted by `cleanup` (S3 deletes are free), its slot items are deleted (1 WCU each).

### 12.5 Recording slots (§12.7)

- **Eligibility** of a counted `ok` battle: the user's `rep ≥ rep.recorderMin` and not quarantined; and in 1v1 one of the two versions is in the plan's `top`, or the pair is in the plan's `slotPairs` (owner, 2026-10-24): for every bot X of the general 1v1 rumble, its 5 best pairings (highest score), 5 worst pairings (lowest score) and 5 problem bots (most negative KNN-PBI, only negative ones), pairs counted once whichever side picked them (`publish` computes them, §15). Once a matchup has slots it keeps them while both versions are current: existing slot items persist even when the pair drops out of the set. In melee, slots are per version: each contestant in `top`.
- **Slots:** 1v1: per pairing (lower vid, higher vid), `slots.perSide` (5) rounds won by the lower vid's bot (`a.0`..`a.4`) and 5 won by the other (`b.0`..`b.4`); melee: per version, 5 rounds it won (`w.*`) and 5 it didn't (`l.*`). A slot is filled by the first qualifying rounds played after the version pair (version) appears, never replaced while the versions are current.
- **Claiming (results post):** for each eligible battle, GetItem the slot item (cached per instance once `full`). For each open slot (absent, or `st: c` with `x < now`), take the next round of that side from this battle, in round order; claim all of this battle's picks in one UpdateItem with a condition on the slots' previous states. Each claim stores the round's track `sha256`, `size`, the battle's seed and round index, the batch id, and `x = now + 1 h`. The batch marker gets the slot ids (`ADD slots`).
- **Upload:** the response lists a presigned S3 PUT per claimed slot (1 hour), to `rec/e<E>/<division>/<key path>`, with `Content-Length` and `x-amz-checksum-sha256` (the reported track hash) **signed**: S3 refuses any other size or content. `recordings/done` checks the stored checksum with `HeadObject` and sets `st: f`; `full` when all 10 are filled.
- **Key paths** (readable by the M6 viewer from the published files: bot ids and the jar hashes in `BotVersion.sha256`): 1v1 `rec/e<E>/1v1/<idA>_<sha16A>/<idB>_<sha16B>/<a|b>-<i>.track` with A the bot whose id sorts first (and `a` = A won), melee `rec/e<E>/melee/<id>_<sha16>/<w|l>-<i>.track`; `sha16` is the first 16 hex digits of the version's jar hash. Objects get `Cache-Control: public, max-age=31536000, immutable` (signed into the PUT as well). The track header holds the engine version (ADR 0017); the seed, round and robot order are in the slot item and published by the recordings index (below).
- **Robot order (M6):** a claim stores the battle's robot order `v` (vids, server-randomized), so a viewer can name the track's tanks. **Migration:** entries without `v` (claimed before M6) are reopened by the first `cleanup` runs: each filled or claimed entry of a group that has recordings, and without `v`, is removed from its slot item (`full` cleared) and its track deleted, so the slot fills again with an order. Found through the listing of `rec/e<E>/` (a claim in a group without any track expires and is claimed again). It reads each listed group once (0.5 RCU) and is `done` (`state/e<E>/cleanup.json`, `rec_order`) after a complete pass that began 2 h after the first pass (claims made before the release can't be confirmed later); then it reads nothing. It runs after the ban removals and stops at the time budget. No CDN invalidation: no viewer fetched those tracks before M6.
- **Recordings index (M6):** `publish` writes `data/v1/rec/<division>/<bot id>.json` for every bot with filled slots with an order, for its current version (`rumble-data.md`). There is no slot partition to query, so it lists `rec/e<E>/` once (1,000 keys a request), maps each object to its slot item and GetItems only the groups whose objects differ from `state/e<E>/rec-cache/<division>.json` (the filled slots read so far: filled slots don't change while their versions are current), at most what 5 min of the 10 RCU/s allowance reads per run. Files are put only if their MD5 differs from the ETag in one listing of `data/v1/rec/`; files of bots without recordings are deleted. `BotDetails.recordings` (`{"1v1": n, "melee": m}`, rounds of the current version) tells the site whether to fetch.
- **Size cap:** `slots.maxBytes` per track; the total stays bounded by construction: top-50 matchups as before, plus at most 15 extra matchups per bot (5 best, 5 worst, 5 problem), about 16.5k matchups at 1,100 bots, 10 tracks each, so about 0.6 GB more (about 3.6 KB a track), within design §12.7's budget; the slot items add 16.5k items of at most 2 KB.
- **Watch requests (M6):** a `W#` item; served like verification work to recorders; the round is uploaded to `rec/watch/<id>.track`, an LRU cache capped at 1 GB by `cleanup`. M4 defines the item only.

## 13. The client (`rumble-client`)

### 13.1 Start

1. Read the token (`rumble.json` or `ROBOCODES_RUMBLE_TOKEN`); none: "run `robocode-s rumble login`".
2. `GET info`. If `api` major isn't 1, or the release is below `minClient`, stop with the download link. If `engineVersion ≠ ENGINE_VERSION`, **refuse to run** and say which version the rumble ranks and which this build has. Check the robot host's deterministic rules (`runner::local::host_info`), as `robocode-s battle` does.
3. **Sandbox:** the rumble runs untrusted jars, so the client refuses `none` unless `--unsandboxed` is given (and the GUI asks); on macOS, which has no built-in sandbox (ADR 0019), that means Docker or `--unsandboxed`.
4. Post the batches left in `<data>/rumble/pending/` (§13.5) before asking for new work.

### 13.2 Running battles

- One `runner::pool::Pool` for the process, warm hosts `2 × slots`, mode deterministic, the chosen sandbox, `memory_mb` 512 (`--memory`).
- **Slots:** `--parallel N`, default `max(1, min(cores/2, RAM GiB/2))`; a 1v1 battle takes 1 slot, a melee battle 5. The client asks for `divisions` it has slots for.
- Each battle: an `Environment` like the CLI's with `jars` = the battle's cached jars only, **`fresh_data = true`** (robots' data files from earlier battles would make results differ between volunteers), `deterministic`, the batch's rules and seed, no recording file, no paint. `prepare_with` and `run_local_battle_with` as `crates/cli/src/battle.rs` does, on a thread per running battle. Robot names in the description are `"<id> <version>"`.
- The observer callback receives every `TurnSnapshot` (the embedded observer runs unpaced) and `RoundResult`; from them the client computes hashes (§13.4) and builds one `replay::Track` per round (§13.4), keeping the encoded tracks in memory until the batch's results are acknowledged.
- A robot that can't be described (`prepare` fails naming it) is `robotFailed` `load`; a robot that disconnects (`RobotDisconnected`, or its process exited) is `robotFailed` `crash`; any other error is `error`.

### 13.3 The loop (`Contributor`)

```
loop:
  if no batch in hand or the current batch has < slots battles left to start:
      GET work (max = clamp(battles per 20 min measured, 4, maxBattles), divisions, slots)
      204 / 429 / 503 → wait Retry-After (+ up to 20% jitter); network error → backoff 5 s .. 5 min
  download missing jars (4 at a time) to the cache, check size and sha256; a mismatch is
      retried once, then the battles with that jar are reported robotFailed/load with "jar hash"
  start battles while slots are free
  when every battle of a batch is done (or 10 min before its deadline, the rest notRun):
      write pending/<batchId>.json and tracks, POST results (retry: 2 s, 4 s ... 5 min; up to the deadline)
      PUT the requested uploads (with exactly the given headers), POST recordings/done
      delete pending/<batchId>*
```

- At most two batches in flight (one running, one prefetched).
- Stop (Ctrl+C, GUI Stop): no new battles; running battles finish (a second Ctrl+C aborts them: their battles are `notRun`); post what is done.
- 401 → stop and ask for a login; 403 → stop with the message; 409 `engine_mismatch` → stop, update needed; 409 `expired` → drop the batch.
- Events for the UI (`Event`): `Batch{division, battles}`, `BattleStarted{bots}`, `Round{n, of}`, `BattleDone{result summary}`, `Posted{accepted, rejected}`, `Uploaded{n}`, `Waiting{until, why}`, `Error{text}`.

### 13.4 Hashes and tracks

- **Round hash** = SHA-256 of `"rsr1"` ‖ `u32 LE round` ‖ for every `TurnSnapshot` of the round in order, `u64 LE server::recording::check(snapshot)` ‖ the prost encoding of the round's `RoundResult`. `check` already leaves out console output, paint, CPU times and debug properties.
- **Track:** the round as a `replay::Track` (ADR 0017) built from its snapshots: `rumble_client::track::from_snapshots(rules, snapshots) -> Track` (positions ×2, headings to 1/1024 turn, energy ×100, bullets from their first and last snapshot state, `engine_version` = `ENGINE_VERSION`), encoded with `replay::encode`; `track.sha256` = SHA-256 of the bytes. Deterministic (fdlibm), so every volunteer gets the same bytes.
- **Battle hash** = SHA-256 of `"rsb1"` ‖ `u32 LE engine` ‖ `i64 LE seed` ‖ per round (round hash ‖ track sha256) ‖ the prost encoding of each robot's final `Results` in robot order.
- Verification compares battle hashes; the per-round hashes let a mismatch report name the first round that differs (logged with the alert).

### 13.5 Local files

Under the data folder (`Environment::data`): `rumble/jars/<sha256>/<id>_<version>.jar` (the cache, like the agent's; least recently used removed above 2 GiB at start), `rumble/pending/<batchId>.json` and `<batchId>/<round>.track`, `rumble/log.txt` (last 1 MiB).

### 13.6 CLI

```
robocode-s rumble login [--server URL]           device flow; stores the token
robocode-s rumble logout                         revokes and removes the token
robocode-s rumble status                         who, tier, battles, server, engine compatibility
robocode-s rumble run [--parallel N] [--divisions 1v1,melee] [--sandbox KIND] [--unsandboxed]
                      [--memory MB] [--batches N] [--json]
robocode-s rumble admin import (--division 1v1|melee) [--list URL|FILE] [--local DIR]
                      [--mirror TEMPLATE] [--dry-run] [--concurrency 4] [--report FILE]
robocode-s rumble admin user UID [--rep N] [--state new|quar|ban] [--admin true|false]
robocode-s rumble admin bot claim|remove ...      admin alerts      admin config get|set FILE
```

`--server` defaults to `https://rumble.planethofstede.be` (`ROBOCODES_RUMBLE_SERVER`; the dev stage's `https://dev.rumble.planethofstede.be`). `run` prints one line per battle (`1v1 jk.mega.DrussGT 3.1.16 vs voidious.Diamond 1.8.28: 54.2% (41 s)`) and a line per post; `--json` prints the events as JSON lines.

### 13.7 GUI

A **Rumble** page in the shell's navigation: sign in (shows the device code with a Copy button and opens the browser; "Signed in as …" with Sign out), Contribute toggle, parallel battles slider, division checkboxes, sandbox (from the existing setting; `none` asks once), a list of running battles with round progress, totals (this session, all time, tier), and a log. It runs a `Contributor` on a thread and reads its events; stopping it is the loop's stop. Settings in `gui.json` (`rumble.parallel`, `rumble.divisions`). Contributing goes on while the user watches other pages; it stops when the window closes (after posting).

## 14. AWS infrastructure (CDK)

### 14.1 Stacks

| Stack | Region | Contents |
|---|---|---|
| `RobocodeS-Cert` | us-east-1 | unchanged |
| `RobocodeS-Site` | eu-west-1 | existing site, plus a `Rumble` construct (`infra/lib/rumble.ts`): tables, buckets, Lambdas, function URL, behaviors, schedules, alarms |
| `RobocodeS-Dev-Site` | eu-west-1 | the same `SiteStack` with `stage: 'dev'`: at `dev.rumble.planethofstede.be` with its own certificate stack `RobocodeS-Dev-Cert` (us-east-1), on-demand table, its own buckets, its own GitHub OAuth app, sample data kept |

The rumble lives in the site's stack because CloudFront's behaviors, the OAC grants (Lambda permission and bucket policy with the distribution's ARN) and the publisher's write access to the site bucket would otherwise make cross-stack cycles. `bin/robocode-s.ts` takes `-c stage=prod|dev` (default prod): prod deploys Cert and Site, dev deploys `RobocodeS-Dev-Site` only. `SiteStack` gets `stage`, and `domain`/`certificate` become optional.

### 14.2 Resources of the `Rumble` construct

- **Table** `rumble` / `rumble-dev` (§9.1): prod `billingMode PROVISIONED`, `readCapacity 25`, `writeCapacity 25`; dev `PAY_PER_REQUEST`; `timeToLiveAttribute: 'ttl'`; `RemovalPolicy.RETAIN` in prod.
- **Buckets** jars and state (§8): block public access, S3-managed encryption, `enforceSSL`; state bucket versioned, lifecycle rules (`uploads/` 1 day, noncurrent 7 days); the jars bucket with CORS off (uploads are signed PUTs from clients, not browsers; the browser upload of a jar for submission uses the state bucket's `uploads/` with CORS `PUT` from the site origin).
- **Lambda `api`**: `provided.al2023`, arm64, 256 MB, timeout 25 s, reserved concurrency 10, environment `STAGE`, `TABLE`, `JARS_BUCKET`, `STATE_BUCKET`, `SITE_ORIGIN`, `GITHUB_CLIENT_ID`, `SSM_PREFIX=/robocode-s/<stage>`; log group with 7 days' retention; `RUST_LOG=info`. Function URL `authType: AWS_IAM`, `invokeMode: BUFFERED`.
- **Lambda `batch`**: same runtime, 1,024 MB, timeout 900 s, reserved concurrency 1, 7 days' logs.
- **EventBridge Scheduler** schedules (`aws-cdk-lib/aws-scheduler`, with a role allowed `lambda:InvokeFunction`): `plan` `cron(12 * * * ? *)` with input `{"mode":"plan"}`; `publish` `cron(30 3,9,15,21 * * ? *)` UTC with `{"mode":"publish"}` (every 6 hours; owner, 2026-10-24); `cleanup` `cron(45 4 * * ? *)` with `{"mode":"cleanup"}`. 24 × 31 + 62 ≈ 810 invocations a month.
- **CloudFront behaviors** added to the existing distribution: `api/*` → `FunctionUrlOrigin.withOriginAccessControl(url)` (§5.1; the origin request policy is a custom one with an allow-list of the headers the API reads plus `CloudFront-Viewer-Address`, because the managed `ALL_VIEWER_EXCEPT_HOST_HEADER` cannot be extended and forwarding `Host` breaks the function URL); `jars/*` and `rec/*` → `S3BucketOrigin.withOriginAccessControl(jarsBucket)`, `CACHING_OPTIMIZED`, no compression for `jars/*` (already zipped), compression for `rec/*` off too (range-coded). The origin access control for Lambda adds the resource policy for `cloudfront.amazonaws.com` with the distribution's `SourceArn`; it must allow both `lambda:InvokeFunctionUrl` and `lambda:InvokeFunction` (AWS requires both for function URLs since October 2025; check the generated policy).
- **IAM** (least privilege, generated by `grant*` where possible): `api`: table read/write (`GetItem`, `Query`, `PutItem`, `UpdateItem`, `DeleteItem`, `TransactWriteItems`, `BatchGetItem`); state bucket get/put on `state/config.json` (get), `state/*/plan.json.gz` (get), `uploads/*` (get, put for presigning, delete); jars bucket put on `jars/*` and `rec/*`, get/head on `rec/*`; `ssm:GetParameter` on `/robocode-s/<stage>/*` with `kms:Decrypt` through the AWS-managed `aws/ssm` key (condition `kms:ViaService ssm.eu-west-1.amazonaws.com`). `batch`: table Query/Get/Update/Put/Delete/BatchWrite; state bucket read/write; site bucket put/get on `data/*`; jars bucket delete on `rec/*` and `jars/*`; `cloudfront:CreateInvalidation` on the distribution.
- **Alarms:** metric filter `ALERT` on both log groups → alarm → SNS topic with the owner's e-mail (context `alertEmail`); alarm on `api` `Throttles` > 0 for 15 minutes; on the table's `WriteThrottleEvents` > 100 in 15 minutes.
- **Dev origin:** dev has its own domain (`devDomain` in `infra/cdk.json`), so `SITE_ORIGIN` and the state bucket's CORS origin are known before the deploy, as in prod. Its two DNS records are added by hand like prod's (`infra/README.md`).
- **Outputs:** function URL (for debugging only; it refuses unsigned calls), bucket names, table name.

### 14.3 Free-tier figures

- Lambda requests: about 2 per batch (work, results) plus 1 per batch with uploads. At 3 battles/s and 20-battle batches: 0.3/s, **780k a month**, under the 1M allowance; the client's 20-minute batches make real batches larger (1v1 at 4 slots: about 50), so typically a third of that. A budget action is not needed: the AWS free-tier usage alert (85%) is turned on by `account/setup.sh`.
- Lambda compute: `api` 256 MB × ~50 ms × 0.8M ≈ 10k GB-s; `plan` 1 GB × ~60 s × 744 ≈ 45k GB-s; `publish` 1 GB × ~300 s × 31 ≈ 9k GB-s. About 65k of the 400k GB-s.
- **Reserved concurrency needs the account's Lambda concurrency quota above 10.** New accounts often have 10, and reserved concurrency must leave 10 unreserved. Request 1,000 through Service Quotas (free) before the first deploy; `infra/README.md` says so.
- CloudFront: jars about 100–300 KB, cached; recordings cached; within 1 TB / 10M requests.
- S3 PUTs: daily data files (≈45k a month, `rumble-data.md`), plan and state hourly (≈2k), jars and recordings (§12.7's estimate). S3 GETs from the `api` Lambda's plan checks: conditional GETs once a minute per instance.

### 14.4 SCP

No change is needed: Lambda, DynamoDB, S3, CloudFront, EventBridge Scheduler, SSM, SNS, CloudWatch and KMS (the AWS-managed `aws/ssm` key) are all allowed in eu-west-1, and `PutProvisionedConcurrencyConfig` stays denied (reserved concurrency is `PutFunctionConcurrency`, which is free). `secretsmanager:CreateSecret` stays denied: the secrets are SSM SecureStrings.

### 14.5 SSM parameters (created by hand, once per stage)

`tools/rumble/secrets.sh <stage>` runs:
- `/robocode-s/<stage>/github-client-secret`: SecureString, the OAuth app's secret.
- `/robocode-s/<stage>/signing-key`: SecureString, lines `kid:64 hex` (32 random bytes), for batch tokens and OAuth state.

Standard tier, the AWS-managed key: free. CDK doesn't create them (CloudFormation can't create SecureStrings, and their values must stay out of templates); the Lambdas read them at cold start. The GitHub **client id** is CDK context (`githubClientId`, `githubClientIdDev`): not a secret. GitHub OAuth apps: one per stage, callback `https://rumble.planethofstede.be/api/v1/auth/callback` (dev: its CloudFront domain, known after the first deploy), device flow enabled.

### 14.6 Site changes

- `DeploySampleData` only in dev (context `sampleData`, default true for dev, false for prod), because its `prune` would delete the published files under `data/v1/`.
- `publish` writes `data/v1/...` with `Cache-Control: public, max-age=3600, s-maxage=90000` and then invalidates `/data/v1/*` (one path; 1,000 free a month).

## 15. Rankings (`batch publish`, every 6 hours)

1. Run `plan` first (same invocation), so the state is current to the hour.
2. Read `pairings.bin` of each division, `REG`, the bots' `M` items (Query per bot: about 1,500 × 0.5 RCU spread over the run at 10 RCU/s, ~75 s) and the claimed bots' users (country).
3. Per rumble (division × weight class; members: active versions with the division whose class is at or below the rumble's; probation versions are not members): restrict pairings to members, then compute (`rumble_core::metrics`):
   - **APS** mean of pairing scores over the bot's pairings with n > 0; **PWIN** share of those pairings with score > 50 (= 50 counts half), ×100; **survival** mean pairing survival.
   - **ANPP:** for each opponent O, `normalized(X,O) = 100 · (m_XO − min_Y m_YO) / (max_Y m_YO − min_Y m_YO)` over the members Y with a pairing against O (50 if max = min); ANPP is its mean.
   - **Vote:** each bot O gives one vote to the opponent X with the highest `m_XO` (ties share it); `vote_X = 100 · votes_X / members`.
   - **Glicko-2** (Glickman 2012, τ = 0.5, ε = 1e-6): every bot starts at r 1500, RD 350, σ 0.06; 20 rating periods over the same games, each period one game per pairing with outcome `m/100`, each using the previous period's ratings; the 20th period's r and RD are published. Deterministic and converging to the paired-comparison fit. (*Refinable with the research, §10.7.*)
   - **KNN-PBI** per pairing, as in `types.ts`: `m_XO` minus the mean of `m_YO` over the k = ⌈√members / 2⌉ bots on either side of X in APS order that have a pairing with O.
   - `rank` by APS (ties: more battles first, then bot id); `pairings` = pairings with n > 0; `battles` = Σ n of its pairings (1v1) or the battles it took part in (melee, counted by the planner per version).
   - `rankWeekAgo` from `state/e<E>/snapshots/<rumble>/<date − 7>.json` (null if absent).
4. Write the files of `rumble-data.md` exactly as `types.ts` (the Rust types in `rumble_core::data` mirror it; a test round-trips `web/src/lib/rumble/fixtures` output through them): `rumbles.json`, `rankings/<rumble>.json`, `bots/<id>.json`. `RumbleSummary.battles` from the state's battle count, `rounds` and `battlefield` from the configuration, `engineVersion` = E. Then the slot pairs of §12.5 from the general 1v1 rumble's metrics, written to `state/e<E>/1v1/slot-pairs.json` for `plan`.
5. **Bot files and history:** for each bot, read its current `bots/<id>.json` (if any) for `history`, append today's point per rumble (`version`, `rank`, `aps`, `glicko`; a later run on the same day replaces that day's point, so history stays one point a day), thin it (keep every point of the last 365 days; before that only days whose day number since 1970-01-01 is divisible by 7), and write it **only if the bytes changed** (compare the MD5 with the object's ETag). Unchanged files are not written. Bots not ranked today keep their file (with versions updated) and get no point.
6. **Archive** (owner, 2026-10-24): `data/v1/archive.json` (new in v1, additive; `Archive` in `types.ts`, built by `rumble_core::archive` from the `REG` items already read, written by the batch Lambda's `archive` module): `{schema, generatedAt, bots: [{id, author, versions: [{version, sha256, size, state, added}]}]}`, bots sorted by id, versions newest first, `state` one of `active`, `probation`, `retired`, `failed`. Removed versions (§12.3 takedown) and removed bots are omitted, so a takedown disappears from the archive at the next run. About 100 bytes a version: 1,500 bots × a few versions is a few hundred KB, one file. `BotDetails.versions` also gets `size`, so bot pages link every version they show.
7. **Contributors** (owner, 2026-10-24: public, opt-out): `data/v1/contributors.json` (new in v1, additive; `Contributors` in `types.ts`): `{generatedAt, engineVersion, contributors: [{login, country, battles, battlesWeek, tier, since}]}`, sorted by battles, users with `battles > 0` that haven't hidden themselves (user attribute `hid`, set with `POST me` `{"hidden": true}`) and aren't banned. `battlesWeek` from the user's counter snapshot 7 days ago (`state/e<E>/snapshots/users/<date>.json`). **Listing users without a GSI:** `plan` adds every uid of the log items it folds to `state/users.json`; `publish` GetItems those users (0.5 RCU each, paced at 10 RCU/s), so the cost is one read per contributor per run instead of a table scan. Banned users are found by the `ban` alert the ban writes (`ALERT` partition), which `cleanup` reads.
8. Write today's snapshot `state/e<E>/snapshots/<rumble>/<date>.json` (`{id: [rank, aps, glicko, version]}`; one per day, the day's last run wins), delete snapshots older than 14 days, invalidate `/data/v1/*`.

9. **Recordings index** (M6, §12.5): after the rankings and before the bot files: `data/v1/rec/<division>/<bot id>.json`, and the `recordings` counts of the bot files. A failure is logged and leaves the bot files' counts as they were.

At 1,500 bots the run takes a few minutes: about 1,500 GETs and up to 1,500 PUTs, 32 at a time. The index adds, at full fill (about 70k slot items, 700k tracks), 700 LIST requests and about 1,500 index files (first run: 1,500 PUTs; later runs only the files of bots with new tracks).

`cleanup` (daily 04:45): delete slot items and recordings of versions retired, failed or removed; run ban removals (§11.3); reopen slots without a robot order (§12.5, once); trim the watch cache (M6).

## 16. Seeding import

### 16.1 Sources

`robocode-s rumble admin import --division 1v1|melee`, run by the owner, as admin:
- **Lists:** `https://robowiki.net/w/index.php?title=RoboRumble/Participants&action=raw` (1v1) and `...title=RoboRumble/Participants/Melee&action=raw` (melee), or `--list` (URL or file). Lines between `<pre>` and `</pre>`; each `<id> <version>,<url>`; blank lines and lines starting with `#` skipped; duplicates reported. The weight-class rumbles have no lists of their own: classes come from codesize.
- **Jars,** for each entry, the first that yields a valid jar whose properties match the entry's id and version: (1) `--local DIR`, a file `<id>_<version>.jar` (the owner's RoboRumble corpus of difftest); (2) the list's URL; (3) `--mirror`, default `https://robocode-archive.strangeautomata.com/robots/{id}_{version}.jar` (the archive the classic rumble clients fall back on). Downloads: `--concurrency` (4) at a time, 30 s timeout, 2 retries, redirects followed, `http` allowed (many RoboWiki links are), 5 MiB limit.

### 16.2 Per entry

1. Validate locally (§12.1, the same code). 2. Ask `POST admin/jars/upload-url` (`{"sha256", "size"}`; the answer is `{"exists": true}` when `jars/<sha>.jar` is already there) and PUT the jar. 3. `POST admin/bots/import`:

```json
{"bot": "jk.mega.DrussGT", "version": "3.1.16", "sha256": "...", "division": "1v1",
 "author": "Julian Kent", "sourceUrl": "http://...", "source": "list" | "mirror" | "local"}
```

→ `{"vid": 812, "result": "created" | "exists" | "divisionAdded"}` or 409 `conflict` (that version is registered with another jar). The server validates the jar again from S3 (it never trusts the client), registers the version **active** (no probation) with the bot's `src: import`, and adds the division to an existing bot's set (a bot in both lists is one bot with both divisions).

### 16.3 Idempotency and report

- Rerunning the import is safe: existing versions with the same jar are `exists`, nothing is uploaded twice (`exists` from the upload-url call).
- A newer version of a bot already imported (the lists change) is registered as the new active version and the old one retired.
- `--dry-run` downloads and validates only. The report (`--report`, default `import-<division>-<date>.json`) lists per entry `{id, version, result, source, sha256, codesize, weightClass, error}`, `error` one of `download (<status or reason>)`, `invalid (<validation message>)`, `mismatch (properties say <id> <version>)`, `conflict`; the command prints counts per result and the failures.
- Imported bots are attributed to the properties' author and claimable (§16.4); the import is announced on RoboWiki (§12.5).

### 16.4 Claiming

An author asks through a GitHub issue (template "Claim my bot") or the site; the owner checks and runs `robocode-s rumble admin bot claim <id> <github login>`, which sets the bot's `uid`. Automatic proof (a token in the jar, RoboWiki edits) is not built.

## 17. Website changes (M4)

- `web/src/lib/api.ts`: the one fetch wrapper (adds `x-amz-content-sha256`, JSON, error mapping).
- `/download`: a chooser for Windows, Linux, macOS and Build from source, each its own page (`/download/<os>`, `/download/source`) with that system's installers (GitHub release links, the apt/dnf instructions of §6.9). How to start contributing (`robocode-s rumble login`, `run`, the GUI's Rumble page) is the docs page `/docs/rumble/contributing`.
- `/account`: sign in/out, profile (country, "hide me from the contributors list"), tier and counters, tokens (revoke).
- Bot pages: every version (current and retired) has a download link `<a href="/jars/<sha256>.jar" download="<id>_<version>.jar">`, same origin (§8), with its size and dates. There is no `/archive` page (owner, 2026-10-24); `archive.json` stays in the data for a later listing. Taken-down bots and versions are in neither. No classic-style URLs and no bulk zip.
- `/contributors`: the public leaderboard from `contributors.json`. `/submit` and `/account/bots`: submit by upload or URL, see versions and their states, retire.
- Docs: `docs/rumble` gets the contributor guide (status note removed, the replays note kept for M6); `docs/robot-development` links to submission.

## 18. Implementation plan

Packages are sized for one agent each. "Small" marks those mechanical enough for a smaller model. Every package runs `cargo fmt`, `cargo clippy --workspace -- -D warnings` and the tests it touches (no CI; `rumble/lambda` packages also its own manifest).

| # | Package | Depends on | Acceptance |
|---|---|---|---|
| P1 | **core: skeleton, config, API types, batch tokens** (`crates/rumble-core`: `config`, `api`, `token`, `ids`) | - | Serde round-trips of every §5 example; tokens: sign/verify, tamper, expiry, key rotation; config defaults match §3; overrides merge. *Small.* |
| P2 | **core: jar validation, properties, codesize** (`jar`, `codesize`) | P1 | Codesize equals classic's for the 27 sample robots and 20 corpus robots (reference numbers from classic's `codesize` tool, run once and checked in); validation messages for 10 broken jars (fixtures built in the test). *Small.* |
| P3 | **core: records, pairing state, folding** (`record`, `state`) | P1 | Encode/decode; folding 1v1 and melee records (45 pairs), negative weights, `pairings.bin` round trip; §6.2 definitions on hand-computed cases. |
| P4 | **core: metrics and data files** (`metrics`, `data`) | P3 | APS, PWIN, survival, ANPP, Vote, PBI and Glicko-2 on small hand-checked matrices; weight-class membership; JSON matches `types.ts` (fixtures from `web/src/lib/rumble/fixtures.ts` deserialize and re-serialize identically); history thinning. |
| P5 | **core: planner** (`plan`) | P3, P4 | §10.3 priorities; quotas sum to H; unfought pairings first; close pairings preferred; melee anchors and partners; the alias sampler's frequencies within 2% of quotas over 1e6 draws; 605k pairings planned in < 2 s (release build). |
| P6 | **core: store, results, reputation** (`store` trait + `MemStore`, `results`, `verify`, `slots`, `auth` logic) | P1, P3 | The §11 state machine (match, mismatch, tie-break, all differ, quarantine, release, ban, recheck) and §12.5 claiming on `MemStore`; result validation cases of §5.4; idempotent repost. |
| P7 | **lambda: api** (`rumble/lambda`, DynamoDB/S3/SSM/GitHub impls, router, all §5 routes) | P6 | Unit tests on the router with `MemStore`; an integration test against DynamoDB Local (`docker run amazon/dynamodb-local`) and a local S3 stand-in (minio) for work → results → slots → done; GitHub calls behind a trait with a fake. Build script produces `bootstrap` for arm64. |
| P8 | **lambda: batch** (`plan`, `publish`, `cleanup`) | P5, P7 | Against DynamoDB Local + minio: fold → plan → publish of a synthetic 50-bot rumble; files validate against `types.ts` (a `node` check script in `web/`); rerun after a simulated crash folds nothing twice. |
| P9 | **infra: Rumble construct and dev stage** | P7, P8 (assets) | `pnpm test` with CDK assertions: table capacity per stage, no GSIs, behaviors, OAC, Lambda permissions (both actions), reserved concurrency, schedules, log retention, IAM scopes; `pnpm typecheck`; `cdk synth` for both stages. `tools/rumble/secrets.sh`, README. |
| P10 | **client: HTTP, auth, jar cache** (`rumble-client`: `http`, `login`, `jars`, token file) | P1 | Against a local fake server (`tiny_http`-style, in tests): body hash header present on every POST, device flow polling incl. `slow_down`, 429/503 Retry-After, cache verification and LRU. *Small.* |
| P11 | **client: battle execution, hashes, tracks** (`battle`, `track`) | P10 | Two runs of the same seeded sample battle (`sample.Walls` vs `sample.Crazy`, 3 rounds) give identical battle hashes; different seeds differ; every turn's snapshot is seen (count = turns); `track::from_snapshots` output decodes and its frames match the snapshots within the quantization. |
| P12 | **client: contributor loop and pending batches** | P10, P11 | Against the fake server: a batch of 3 battles runs, posts, uploads the requested tracks with the exact headers, confirms; kill and restart re-posts the pending batch; stop finishes running battles and reports `notRun`; engine mismatch refuses. |
| P13 | **CLI: `rumble` commands** (login, logout, status, run, admin user/bot/alerts/config) | P12 | Option parsing tests like `battle.rs`'s; `--help` texts; `run --batches 1` against the fake server. *Small.* |
| P14 | **Importer** (`rumble-client::import`, `rumble admin import`) | P2, P10, P7 | Parses a saved copy of both participant pages; source order local → URL → mirror; report format; `--dry-run` on the difftest corpus folder validates all of it. |
| P15 | **GUI: Rumble page** | P12 | Sign-in flow with a fake server; start/stop; events shown; settings persisted. Owner hand-tests. |
| P16 | **Web: api wrapper, account, submit, download, docs** | P7 | `pnpm check`, unit tests for the wrapper (hash header), pages render against a mocked API; docs updated (§17). |
| P17 | **Dev deployment and end-to-end** | P9, P13, P14 | Deploy dev; GitHub app for dev; import 20 bots; two machines run `rumble run` for an hour; plan and publish run; the site shows the rankings; a forced mismatch (a client patched to flip a hash) is quarantined after the tie-break; uploads land in `rec/`. Notes go into `infra/README.md`. |
| P18 | **Prod: deploy, full import, docs** | P17, owner | Lambda quota raised, secrets set, prod deploy, `admin import` for 1v1 and melee with the report reviewed by the owner, design §12 updated (§12.3 work allocation, §12.6 data model, §15 layout) and #23 closed. |

Parallel tracks: P1 first; then P2, P3, P10 in parallel; P4/P5/P6 after P3; P7 after P6; P11 after P10; P8 after P5+P7; P9 after P7/P8; P12–P15 after P11; P16 any time after P7.

## 19. Decisions taken in this spec (for review)

1. **Pairing statistics live in an S3 state file, folded hourly from a DynamoDB result log**, instead of DynamoDB pairing items and per-bot sums updated by delta (design §12.6). A melee battle costs one record instead of 45 pairing writes, the daily run needs no table scan, and the log is the per-user log of §12.4. Rankings data stays daily; planning data is up to an hour old.
2. **Assignments are signed batch tokens, not rows.** One post per batch, made idempotent by a batch marker; the token carries seeds, bots and deadline.
3. **One table, no GSIs**, 25/25 provisioned in prod. Checked 2026-10-24: the Organization's other project uses on-demand tables only, so the provisioned 25/25 is all the rumble's.
4. **Crates:** `crates/rumble-core` and `crates/rumble-client` in the root workspace; `rumble/lambda` a separate workspace (AWS SDK compile times stay out of `cargo clippy --workspace`), built with cargo-zigbuild for `aarch64-unknown-linux-gnu.2.34`.
5. **Our token travels in `X-Rumble-Token`** (OAC owns `Authorization`); every POST carries `x-amz-content-sha256`, computed by the client and by the site's fetch wrapper.
6. **Device-flow tokens are checked with GitHub's application token API** (proves they were issued to our app) and then deleted; the server never keeps a GitHub token.
7. **Work allocation without a cap** (§10): hourly water-filled quotas over all pairings from LiteRumble-style variance reduction, a ×10 closeness factor and a rank-contest factor; verification, probation and watch work first. Supersedes #23's threshold.
8. **Melee pairings by classic's split:** each of the 45 pairs scored by the two bots' total scores in the battle; survival as the share of the two bots' wins (both divisions).
9. **Robots start with no data files** in rumble battles (`fresh_data`; owner, 2026-10-24): needed for verification to compare equal runs. Classic rumble robots keep data between battles; ours don't. Data shipped inside the jar still counts. Server-held per-bot data snapshots (promoted from verified battles) are a possible follow-up; the work item reserves `data` for them.
10. **The rumble client refuses to run unsandboxed** without `--unsandboxed` (owner, 2026-10-24); on macOS this means Docker, or that explicit opt-in (the GUI asks once, with a warning).
11. **A new version replaces the old one only after probation**, so bots don't vanish from the rankings meanwhile. **Imported bots skip probation.**
12. **Codesize** is classic's tool's: the sum of all methods' `Code` lengths in every class of the jar.
13. **Recording keys use bot ids and the first 16 hex digits of the jar hash**, so the M6 viewer finds them from published data; melee slots are per version (5 won, 5 not won).
14. **Glicko-2** is computed from scratch daily: 20 rating periods over the pairing matrix (one game per pairing, outcome = score/100).
15. **Configuration** is code defaults plus `state/config.json` overrides (admin command); secrets are hand-made SSM SecureStrings; the rumble's resources are a construct in the site stack (avoids cross-stack cycles); dev is a second copy of that stack at `dev.rumble.planethofstede.be`.
16. **Bot claims are approved by the owner** (admin command after a GitHub issue).
17. **Session cookie** `__Host-rs_session`, 30 days, renewed in the last 7; upload tokens don't expire; revocations take up to 5 minutes (per-instance cache).
18. **Verified runs don't count twice**; a wrong counted result is corrected by a −1/+1 pair of records.

19. **Rankings every 6 hours** (owner, 2026-10-24) instead of daily; history and snapshots keep one point a day.
20. **Contributors are public, opt-out** (owner): `contributors.json`, a `/contributors` page, a hide switch on the account page.
21. **Launch:** a dev-stage trial first (the owner, maybe a few RoboWiki regulars), then prod opens to everyone after the full import and a stable day. The M4 release (0.2.0) goes to every channel of design §6.9: GitHub release and download page, apt/dnf, winget, Homebrew.

Open questions for the owner:
- Q2: the GitHub OAuth app owner (personal account or an organization) and the admin GitHub id(s).
- Q5: the RoboWiki announcement text. (Q3, Q4 and the archive as a mirror were answered 2026-10-24: fresh data, Docker or `--unsandboxed` on macOS, the archive as the last fallback.)
