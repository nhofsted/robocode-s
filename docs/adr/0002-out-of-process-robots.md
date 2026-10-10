# 0002. Robots run out of process

- Status: Accepted
- Date: 2026-10-10

## Context

Classic Robocode runs robots in the engine's JVM and sandboxes them with the `SecurityManager`, which was permanently disabled in JDK 24 (JEP 486). We also want robots in many languages and per-robot CPU and memory limits.

## Decision

Robot code never runs inside the server process. Robots talk to the server over the robocode-s protocol. They're started by a pluggable launcher: local process, Docker container or remote runner agent; WebAssembly may come later (design §6).

## Consequences

Isolation and polyglot support come naturally. In exchange, time budgets must be measured as robot CPU time and not wall-clock time (ADR 0006), and lockstep latency matters for throughput.
