# 0021. Rumble backend: a result log folded into S3, signed batches, no work cap

- Status: Proposed
- Date: 2026-10-24

## Context

M4 builds the rumble of design §12 (ADR 0007) in the free tier of one AWS Organization (ADR 0016): 25 RCU and 25 WCU of provisioned DynamoDB for every table and index together, 1M Lambda requests and 400k GB-s a month. The design kept per-pairing statistics and per-bot sums in DynamoDB, updated by delta for every battle, and a daily scan for the other metrics. At about 1,100 bots in 1v1 that is about 600k pairing items: a 10-bot melee battle touches 45 of them, a nano 1v1 battle also updates its bots in four rumbles, and the daily scan (about 50 MB) takes longer than one Lambda run at the free read rate.

The owner decided (2026-10-24) that the rumble has no hard cap on work: every battle volunteers offer is used and directed to the most useful pairings; verification re-runs are always served. Lambda function URLs behind CloudFront with origin access control take the `Authorization` header for CloudFront's signature and accept no unsigned request bodies.

## Decision

The full specification is [`docs/rumble-backend.md`](../rumble-backend.md).

- **The result log is the only per-battle store.** Each results post writes one DynamoDB transaction: an idempotency marker for the batch, a log item with the batch's compact records (24 bytes a 1v1 battle, 104 a melee battle) and the user's counters. An hourly job folds the new log items into a pairing state file in S3 (n, Σ score, Σ score², Σ survival per pairing) and writes the work plan; the daily job computes all metrics from that file and publishes `/data/v1`. There are no pairing items and no table scans. Corrections (a wrong result found by verification, a banned user) are records with weight −1.
- **Assignments are signed batch tokens** (HMAC-SHA256, key in an SSM SecureString), not rows: handing out work reads and writes nothing in DynamoDB.
- **One table, no GSIs**, 25/25 provisioned in prod, on-demand in a dev stage. At the design's 3 battles/s the backend uses about 4 WCU/s and 1 RCU/s.
- **Work allocation without a cap:** hourly quotas per pairing, water-filled over all pairings by the expected variance reduction of one more battle (LiteRumble's shrunk variance, prior 16 with 3 pseudo-battles), ×10 for pairings within their CI of 50, more weight for bots close in rank and bots with incomplete pairings. The API samples battles from the plan in memory. Verification, probation and watch work come first for clients allowed to run them.
- **Melee pairings** use classic RoboRumble's split: each of the 45 pairs scored by the two bots' total scores in the battle.
- **Crates:** `crates/rumble-core` (pure: types, tokens, records, planner, metrics, reputation, jar validation, codesize) and `crates/rumble-client` (HTTP, device flow, battles, hashes, tracks, import) in the root workspace; the Lambdas in `rumble/lambda`, a workspace of its own so the AWS SDK stays out of `cargo clippy --workspace`, built with cargo-zigbuild for arm64.
- **HTTP:** `/api/v1` behind CloudFront with OAC; our token in `X-Rumble-Token`, every POST with `x-amz-content-sha256`; GitHub OAuth code flow with a `__Host-` session cookie for the site, device flow for the CLI and GUI with the GitHub token checked against our app and then deleted.
- **Rumble battles** run in deterministic mode, sandboxed, with no robot data files kept between battles; volunteers report a battle hash over per-turn snapshot hashes and per-round track hashes, which verification compares and recording uploads must match (signed into the presigned PUT, so S3 checks it).

## Consequences

- A melee battle costs about as much as a 1v1 battle; the free capacity carries about 15 battles a second, so throughput is limited by volunteers, not by AWS.
- Pairing data for planning is up to an hour old; the website stays daily as designed.
- The S3 state file has one writer (the batch Lambda, reserved concurrency 1). Losing it would need a rebuild from the log, which holds 400 days.
- Assignment tokens can't be revoked individually; they expire with the batch deadline (3 hours).
- Robots that learn across battles (data files) start fresh in every rumble battle, unlike classic's rumble.
- Design §12.3 and §12.6 are updated when M4 lands; #23's threshold is dropped.
