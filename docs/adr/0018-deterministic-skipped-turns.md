# 0018. Deterministic skipped turns from counted work

- Status: Accepted
- Date: 2026-10-17

## Context

ADR 0005 made deterministic mode wait for every robot and never skip turns, so slow robots behave differently from the rumble, where classic skips the turns they are late for. A robot that loops for a long time (florent.XSeries.Xmen 0.9 in the RoboRumble corpus) also made outcomes depend on the machine. ADR 0014 gave robots a deterministic measure of their work: counted instructions as virtual time, calibrated against classic.

Classic waits for each robot up to its CPU constant per turn (ten times that on turn 1, `Battle.wakeupRobots`). If the robot isn't done, the turn runs without it: its last commands stay in effect. A robot that answers late gets one `SkippedTurnEvent` per missed turn, and more than 30 (240 for I/O robots) disable it (`RobotPeer.checkSkippedTurn`). Classic times only the robot's main thread between waking it and its next execute, including its event handlers.

## Decision

In deterministic mode, robots skip turns by their **counted work** instead of wall time:
- **Budget:** `TURN_BUDGET_NANOS` = 2.5 ms of counted work (virtual time, ADR 0014) per turn. That is classic's CPU constant (2.52 ms) on the machine the virtual clock was calibrated on (OpenJDK 21, all intrinsics, as rumble clients run it), so a robot's work relative to its budget is about the same as in the rumble. It is part of the deterministic rules, versioned with the clock's constants.
- **Late turns:** a robot that woke for turn T and did work W before its next execute answers `k` turns late: 0 if W is below the allowance (the budget, ten times it on turn 1), otherwise `1 + (W − allowance) / budget`. The work before a round's first execute is free.
- **robocode-s:** the robot host reports W with each `Execute` (`cpu_nanos`). The server holds a late `Execute` until turn T + k and then handles it as classic mode handles a late one: the turns in between ran with the robot's last commands, it gets the `SkippedTurn` events, and over 30 it is disabled.
- **When a robot learns it skipped turns:** with its next `Tick`, as classic's `checkSkippedTurn` runs when the battle wakes it. The turn it wakes in has already run, so that turn's non-critical events are cleared too. A robot over the limit is disabled and stopped then (classic stops its thread), so its code doesn't run again. This also changed classic mode, which handled all of this one turn earlier, when the late `Execute` arrived.
- **The classic oracle:** the determinism agent patches classic's `RobotPeer`. Classic still waits for every robot (`-Ddebug=true`), then leaves a late robot asleep for the `k` turns it would have been busy, so classic's own `checkSkippedTurn` sees it answer late. The robot already published its new commands before it went to sleep, so during those turns the battle keeps loading its old ones (`performLoadCommands`), as in classic, where the robot would still be computing them.

## Consequences

- Slow robots skip turns in deterministic mode as in the rumble, the same way on every machine, and the difftest checks it against classic.
- The work of a robot's other threads counts too (the counter is per robot); classic only times the main thread.
- JDK calls cost a flat 10 instructions each, also expensive ones such as `Arrays.sort` or `System.arraycopy`. Garbage collection isn't counted.
- **Not covered yet:** a robot that never finishes its turn (an endless loop) is still only stopped by the hard limit (ADR 0005), and classic's oracle waits up to 10 minutes per turn for it. Disabling it as soon as its count makes that certain is the next step (#15).
- Partly supersedes ADR 0005: deterministic mode skips turns, by work instead of time.
