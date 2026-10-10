# 0006. Time budgets measured in robot CPU time

- Status: Accepted
- Date: 2026-10-10

## Context

Network latency and container scheduling would otherwise count against robots; Docker CPU quotas are too coarse (100 ms periods) for per-turn fairness.

## Decision

Per-turn budgets are measured in CPU time of the robot's thread or process, by the runner or the trusted client library. A wall-clock deadline acts only as a liveness guard. Measurements from untrusted remote runners are not trusted (design §7.3).

## Consequences

Fair budgets on all launchers. Container CPU limits are a safety ceiling only.
