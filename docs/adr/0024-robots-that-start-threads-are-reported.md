# 0024. Robots that start threads of their own are reported

- Status: Proposed
- Date: 2026-10-27

## Context

Deterministic mode (ADR 0005, 0014, 0018) makes a battle a function of its seed by metering the work of each robot's thread and giving it a virtual clock. Threads that robot code starts itself are outside that: they run in real time, `Thread.sleep` is real, and the scheduler decides how their work interleaves with the robot's turns. Toad (max-priority tree threads), Fermat (a `DataWriter extends Thread` whose files later rounds read), TNT, Rapture and FlamingForest (a `PatternAnalyzer` thread with a `Thread.sleep(500)` loop) are such robots. Their battles are not reproducible, so a rumble verification of one can mismatch through no fault of the volunteer who ran it. The server needs to know which robots those are to treat their battles as no-fault.

## Decision

- **Detection** is in the determinism agent. It patches `Thread.start()` (and JDK 21's `Thread.start(ThreadContainer)`, which executors use) to call a hook, `RobotRandomness.threadStart`. The hook flags a robot when
  1. the starting thread is in the robot's thread group (the group names the agent already recognizes robots by), and
  2. the robot's own code asked for the thread: the innermost stack frame that is not `Thread`, `java.util.Timer`, `java.util.concurrent.*` (executors, fork/join, `CompletableFuture`) or the glue between them, is a class of classic's robot class loader.

  The second rule matters: the JDK, classic and our robot host start threads on a robot's thread as a side effect (Java2D's `Disposer` when the host first creates a paint graphics, found by running `sample.Fire`), and those are not the robot's. Threads that classic starts for the robot (its run thread) are started from outside the robot's group. A thread the robot's thread starts through a JDK facility not in the list (for example a parallel stream's helper) is not counted.
- **Protocol:** `Execute.threads` (new `bool`, field 10). The robot host sets it on every `Execute` once a thread was started, and sends one `Execute` with only `threads` set, `turn` -1 (it answers no `Tick`, so the server's existing turn check ignores the rest), at the moment of the first start: robots start threads when the server expects nothing, such as in `onRoundEnded`. The server reads the field from every `Execute`, also those it holds or ignores.
- **Server and clients:** `BattleOutcome.threads` (per robot index) carries it out of the battle; `RobotResult.threads` in the rumble API (`skip_serializing_if` false, so absent for ordinary robots and for old clients) reports it in the results post; `robocode-s battle` prints a note under the table and a `threads` list of robot names in `--json`.
- Flagging is sticky for the battle and says nothing about how much a thread matters: a robot that starts a thread and never depends on its timing is flagged too. It is the robot, not the battle's outcome, that is known not to be reproducible.

## Consequences

- Old robot hosts never set the field: their robots are not flagged, as before.
- A thread started after the server stopped reading (in the last round's `onRoundEnded`, after `BattleOver`) is lost. Robots that matter start theirs in `run` or in the first rounds; none of the corpus robots checked depend on that.
- The classic oracle (difftest) JVM runs several robots in one process; `RobotRandomness.robotsWithThreads()` lists them by name, but nothing reports them there.
- Reproducibility could be restored for some of these robots (a deterministic scheduler for their threads); this ADR only reports it. ADR 0025 does that, for robot code's own threads and those of its `Timer`s and executors, including their monitors, `wait`/`notify` and `java.util.concurrent`; the report is now only for what its scheduler doesn't cover (virtual threads, `Unsafe`'s parking, threads started outside the scheduler).
