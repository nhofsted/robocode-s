# 0005. Optional deterministic mode

- Status: Accepted
- Date: 2026-10-10

## Context

Reproducible battles are needed for replays, testing and verifiable rumble results, but classic skips turns based on timing.

## Decision

Classic mode (default) uses per-turn CPU budgets with skipped turns. Deterministic mode never skips turns for timing reasons; robots that exceed a hard limit are disabled. All randomness is seeded, including robot-side `java.util.Random` (design §7).

## Consequences

The rumble runs in deterministic mode. Slow robots behave slightly differently there than in classic.
