# 0028. A round's start allows classic's wait, in counted work

- Status: Proposed
- Date: 2026-11-09

## Context

ADR 0018 made robots skip turns by their counted work, but left the work before a round's first `execute` free: robocode-s's server never counted an answer to the round's start (turn 0) as late, the robot host reported no work for it, and the classic oracle's patch started counting at the robot's first turn. The only limit was the work cutoff (`WORK_CUTOFF_NANOS`, 0.82 s), after which the robot's code throws and it crashes with its energy drained. Classic doesn't make the round start free, so the two differed for robots that work a lot before their first `execute` (building tables in a constructor, say), and the difftest couldn't see it: the oracle had the same patch.

### What classic does (1.11.1 and 1.11.2 alike; checked against both jars' bytecode)

- **The wait.** `Battle.initializeRound` computes `waitTime = min(300 × cpuConstant, 10 s)` and calls `RobotPeer.startRound(waitTime / 10⁶, waitTime % 10⁶)` for each robot in random order. `startRound` starts the robot's thread (which creates the robot, so its static initializers run here in the first round, then runs its code up to the first `execute`) and waits **once**, `isSleeping.wait(waitMillis, waitNanos)`. If the robot isn't asleep by then it logs "still has not started after N ms... giving up." and goes on.
- **Rounding.** JDK 9 and later (`Object.wait(long, int)`, checked in the JDK 21 the robot host runs) add a millisecond when `nanos > 0`. The calibration machine's CPU constant (ADR 0018, the robot host's `CPU_CONSTANT_NANOS`) is 2.52 ms: 300 × 2.52 ms = 756 ms and 0 ns, so `wait(756)`, which takes 756.2 ms on Linux (median of 15; one wait, so the per-wait overshoot of ADR 0018's 1 ms waits doesn't add up). Classic's own work in that time (starting the thread, creating the robot) isn't counted, and is more than the 0.2 ms: the allowance is 756 ms.
- **Turn 1.** The first turn runs with the robot's initial commands. `wakeupRobots` wakes only a robot that is asleep (`waitWakeup` does nothing otherwise), but its wait for turn 1, `waitSleeping(10 × millisWait, 1)` (21 waits of 1 ms on the calibration machine, ADR 0018's seven budgets), waits for every living robot, so also for one still finishing its start. A robot that falls asleep during the wait of turn j wakes in turn j + 1, and its `checkSkippedTurn` finds the turns in between skipped.
- **Start and turn 1 don't add up.** A robot late from its start is never woken for turn 1: the turn-1 wait is spent on its start. When it wakes (turn ≥ 2), its next turn has one budget, not turn 1's seven.
- **`checkSkippedTurn` across rounds.** It compares the current turn with `currentExecutionTime`, the turn the robot last woke in. Classic sets that field only in `checkSkippedTurn` (`initializeRound` resets `lastExecutionTime`, which the check overwrites first), so it carries over from the round before: the round's last turn for a robot still running then (the battle interrupts it at the round's end, and the check runs once more), or the turn it died or was disabled in. It starts at 0 for the battle's first round. A robot that answers a later round's start late therefore usually finds nothing skipped: no `SkippedTurnEvent`s, its events aren't cleared, and it isn't disabled, however late it is. Only the first round counts a late start in full (or a round after one in which the robot died early).

### In budgets (B = `TURN_BUDGET_NANOS` = 3.2 ms)

With W the robot's counted work from the round's start to its first `execute`:

| W | late turns k | classic |
|---|---|---|
| below 756 ms (236.25 B) | 0 | woken in turn 1, which allows 7 B |
| 756 ms to 778.4 ms (+7 B) | 1 | misses turn 1, falls asleep in its wait, wakes in turn 2 |
| from 778.4 ms | 2 + ⌊(W − 756 ms − 7 B) / B⌋ | wakes in turn k + 1 |

So `k(W) = 0` below 756 ms, else `1 + k₁(W − 756 ms)`, where `k₁` is ADR 0018's rule for turn 1. In the first round it skips turns 1 to k, and more than 30 (W ≥ 871.2 ms, 756 ms + 36 B) disable it; in later rounds it skips the turns after the one it last woke in, before turn k + 1, if any.

## Decision

Deterministic mode follows classic's round start (rules version 4: `DETERMINISTIC_RULES` and `rulesVersion()`):
- **Allowance.** `ROUND_START_NANOS` = 756 ms (300 × `CPU_CONSTANT_NANOS`) of counted work before the first `execute`; past it, the robot runs into turn 1's seven budgets, then one per turn (`late_turns` / `lateTurns` for turn 0, the table above).
- **The work counted.** The robot host reports the work from the round's start (`RemotePeer.newRound`, before classic's robot side creates the robot) to the first `execute`. In the classic oracle a hook at the start of `HostingRobotProxy.run()` (classic's robot side, on the robot's thread, the same code in both) notes the robot's count, and the first `execute`'s work is measured from there, as an answer to turn 0 (classic's `currentExecutionTime` there still holds the round before's turn).
- **`checkSkippedTurn` across rounds.** The server keeps, per robot, the turn it last woke in (the `Tick`s it sent; the round's last turn for a robot whose `Execute` is still awaited or held when the round ends) into the next round, and a late answer to a round's start compares with that (`skipped::carried_last`). Nothing skipped: no events cleared, no `SkippedTurnEvent`s, no disable. The same in classic mode, which shares the check. The oracle needs nothing: classic's own check runs there.
- **The work cutoff at a round's start** starts again at the round's start (it used to run on from the robot's last turn of the round before, so handlers at a round's end ate into it) and is `ROUND_START_CUTOFF_NANOS` = 756 ms + `WORK_CUTOFF_NANOS` = 1.58 s. A robot that classic lets finish its start in time is never stopped: the old cutoff, 0.82 s, already lay above 756 ms, but it stopped robots between 0.82 s and 0.87 s that classic only makes late. Reaching the new cutoff means 251 late turns, more than even an I/O robot may skip in the first round.

## Consequences

- Robots that work up to 756 ms before their first `execute` play as before. Heavier starts skip turns as in classic, in the first round with its events and disabling; the difftest checks this against classic (test robots `testbots.SlowStart500`, `SlowStart800` and `SlowStart1000` in `record-samples.sh`: on time; 8 turns late; 71 late and disabled in the first round, then late without skipped turns in the next ones). Their battles are identical in the engine replay, the server replay and live.
- **Still not like classic:**
  - Classic waits for robots one after another, and a robot still busy keeps running while the battle waits for the next one (at the start and in every turn). Counted per robot, as ADR 0018 does for turns.
  - Classic times wall clock: class loading and code that isn't compiled yet (the first round's start above all) take time that counted work doesn't price. A robot close to 756 ms of counted work can already be late in classic.
  - A robot that keeps computing at a later round's start is never disabled in classic (the check above finds nothing), it only idles until it executes; robocode-s's cutoff ends it after 1.58 s, as a crash with its energy drained. Classic's endless loop never ends either way; a robot this slow loses those rounds in both.
  - Other machines: the wait follows the CPU constant (and Windows rounds each wait to its timer tick), as ADR 0018 says for turns; the allowance follows the calibration machine.
- **Robot hosts and rules.** A robot host of rules 3 counted no round start, so the runner refuses it for deterministic battles (ADR 0018). `ENGINE_VERSION` stays 1 for now, as for rules 2 and 3: only battles of robots that work for more than 756 ms before a round's first `execute` change. That is for the owner to confirm.
- Amends ADR 0018 (the round start was free) and ADR 0025's note on the first turn.
