# 0007. Own distributed rumble on the AWS free tier

- Status: Accepted
- Date: 2026-10-10

## Context

We want a rumble with a better web interface, and we do not contribute to classic LiteRumble. The backend should cost (almost) nothing.

## Decision

robocode-s runs its own rumble. Volunteers run all battles. A serverless backend in eu-west-1 coordinates work, verifies results through reputation-based cross-checks by other volunteers, and serves the website at `rumble.planethofstede.be`: Rust Lambdas behind CloudFront, provisioned DynamoDB at the free limits, S3, CDK. AWS never runs battles. Per-round replays (5 rounds won by each side) are kept for every matchup (design §12).

## Consequences

No server-side verification or validation battles. S3 storage for jars and replays costs a few dollars per month at most. DynamoDB holds aggregates only.
