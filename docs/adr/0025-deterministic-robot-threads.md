# 0025. Deterministic threads for robots: one virtual CPU per robot

- Status: Accepted (the owner's decisions of 2026-10-31, below)
- Date: 2026-10-27, decided 2026-10-31

## Context

Deterministic mode counts each robot's work (ADR 0014) and decides skipped turns by it (ADR 0018), but threads that robot code starts run on the OS scheduler, and `Thread.sleep` and timeouts use real time (ADR 0014, "Not covered"). Robots with threads are therefore not reproducible: the same seeded battle ends differently from run to run and from machine to machine, and rumble verification penalized volunteers for it. ADR 0023 flags such robots and makes their verification no-fault; ADR 0024 has the agent detect robot-started threads and report them (`Execute.threads`). This ADR makes them reproducible. The robot's work counter is shared by its threads and updated without synchronization, so concurrent threads even lose counts.

Verified on 2026-10-27:
- **ak.Fermat 2.0:** `DataWriter extends Thread`, started in `onDeath`, writes `<opponent>.txt`; later rounds read it back. A seeded melee (seed 5001203142733927833, ten robots) gave two different outcomes in 6 runs, first diverging in round 27.
- **TNT.TNT 0.99.1, rapture.Rapture 2.13, techdude.forest.FlamingForest 1.1:** a `PatternAnalyzer` thread loops on `Thread.sleep(500)` and switches the gun's pattern.
- **florent.test.Toad 0.14t:** a tree thread that polls a buffer forever, lowering its priority to 1 when the buffer is empty; rebuild and restore threads; the main thread busy-waits on their flags (`while (addingInProcess) {}`).

A scan of the 1375 RoboRumble jars (ASM, robot classes only, bundled Kotlin left out) finds 8 that start threads from robot code: the five above, dam.MogBot 2.9 and jasolo.Sonda 0.55 (threads that save and load data files) and inonit.robocode.javascript.MyFirstRobot 1.1 (Rhino; its threads are in Rhino's debugger and shell). None uses `java.util.Timer`, executors or `java.util.concurrent` locks; only the Rhino robot has `synchronized`, `wait` and `notify` next to threads. The corpus difftest excludes TNT, Rapture and Toad as not reproducible in classic.

Classic: a robot's threads run in its thread group, concurrently with its main thread (on a multi-core machine, in parallel); classic times only the main thread. At a round's end `RobotThreadManager.waitForStop` interrupts each of them every 100 ms for up to 3 s of wall time; with security on (as in the rumble) it then stops them with `Thread.stop`, which threw `ThreadDeath` in them up to Java 19 and does nothing on Java 20 and later, where such threads run on into the next rounds.

## Decision

In deterministic mode, the determinism agent gives each robot **one virtual CPU**: the robot's threads run one at a time, and the CPU changes hands only at points that depend on what the robot executes (`RobotThreads`, `ThreadCalls`).

- **Which threads:** a thread that robot code starts on one of the robot's threads (the first caller of `Thread.start()` outside `java.lang.Thread` is a robot class; Kotlin's `thread {}` counts as robot code). The thread that starts the first one, the robot's main thread (classic's `runThread`), joins too. Threads the JDK starts for a robot (a `Timer`'s, an executor's) are left alone: their JDK code blocks in ways the scheduler doesn't see. Robots without threads of their own never get a scheduler.
- **How:** the agent patches `java.lang.Thread` (one patch, shared with ADR 0024's report): `start()` reports the thread (ADR 0024) and registers it with the scheduler before it exists (`start(ThreadContainer)`, how executors start theirs, only reports); `run()` (and the `run()` of robot classes that extend `Thread`) waits for the CPU before robot code runs; `exit()` hands the CPU on; `sleep(long)`, `sleep(long, int)`, `join(long)` (which `join()` and `join(long, int)` call) and `interrupt()` go to the scheduler when robot code calls them on one of the robot's scheduled threads. On any other thread these prologues return at once, so the JDK's behaviour and even its stack traces stay the same (a first version rewrote robot code's calls instead, and the helper frame showed in the stack trace that mn.nano.SkippedTurns prints when `sleep` throws, which changed its recording). Only the native `Thread.yield` is rewritten in robot code, to a synthetic helper.
- **Switch points:** a thread blocks (`sleep`, `join`) or ends; `yield`; the end of a **quantum**; the end of the main thread's turn. A new thread waits in line; the starting thread goes on.
- **Quantum:** 100 µs of virtual time, 625 000 counted instructions. The count is already checked where robot code can repeat (loop starts and handlers, ADR 0018); that check now first does `if (fuel[1] >= 0) robocodes$limit(fuel)`, which asks the scheduler only once the count reached the limit (inline, reusing the block's stack-map frame; a block that starts with `new` calls the helper unconditionally). The limit is the lowest of the robot's work cutoff (ADR 0018), the running thread's quantum end and the end of a slice (below). Busy-waiting on another thread therefore ends, at the same instruction everywhere. Below the limit the call does nothing; at the cutoff the check after it throws, as before.
- **Next thread:** the first waiting thread of the highest Java priority, round-robin among equals. Classic caps a robot's thread group at its main thread's priority (`NORM_PRIORITY - 1`), so helpers can never outrank the main thread, only yield to it: Toad's idle tree thread (priority 1) then only runs when the main thread waits.
- **The main thread's turn:** while it runs, the robot's other threads of its priority get their quanta in turn. Their work counts toward the turn, as decided in ADR 0018.
- **The turn's end:** where classic's robot side executes (`BasicRobotProxy.executeImpl`, before the robot's commands go out; the same code in classic and in robocode-s), the other threads that wait for the CPU run until none can, or for at most what is left of the turn's budget less a quantum, and at most 1 ms (`TURN_NANOS`, the virtual clock's time between turns for waiting on the other robots, ADR 0014). This work counts toward the turn too, but it can't by itself make the main thread late (but for running past the slice's end to the next check). A round's first turn gives none: the work before the first execute is free and isn't measured.
- **Time:** `Thread.sleep` and timed joins wait on the robot's virtual clock (turns and counted work, ADR 0014), for as many turns as they lasted in classic: a sleep of d lasts d × `TURN_NANOS` / `CLASSIC_TURN_NANOS` of virtual time, about d / 8 ms turns. `CLASSIC_TURN_NANOS` (8 ms) is classic's wall time per turn in the threaded robots' battles on the calibration machine of ADR 0014, measured on 2026-10-31 with the classic oracle (tools/difftest; main's agent, so their threads ran freely and slept in real time; quiet machine): TNT, Rapture and FlamingForest against lancel.Lynx 1.09 took 7.2–9.5 ms per turn (median 8.3); the Fermat melee 1.3 ms, but Fermat doesn't sleep. `PatternAnalyzer`'s `sleep(500)` thus wakes about every 62 turns. A sleeper wakes at the first switch point after its deadline, earliest deadline first. When nothing else can run, a sleeper wakes early: the main thread if it sleeps, otherwise the earliest deadline. Virtual time never jumps: the clock stays turns plus work, so a robot that times its own sleep with `nanoTime` sees it last an eighth of what it asked for.
- **Reported threads (ADR 0024):** the robot host reports (`Execute.threads`) only threads the scheduler doesn't run: those the JDK starts for the robot (`Timer`, executors) and those robot code starts outside a scheduled thread. A robot is also reported when it has scheduled threads and one of its classes (checked as it loads) uses what the scheduler doesn't cover: `synchronized`, `wait`/`notify`, or `java.util.concurrent` classes that can block (all but its atomics and non-blocking collections). Robots whose threads are all scheduled are no longer reported, so the rumble can verify their battles again.
- **Interrupts:** robot code's `interrupt()` sets the flag as usual; a sleeping or joining target gets in line and throws `InterruptedException` when it next runs.
- **The round's end:** classic's `RobotThreadManager.waitForStop` first awaits the main thread (as before), then the scheduler, driven from the battle side: the other threads are interrupted 30 times (as classic does every 100 ms for 3 s), each time running until they all wait, for at most 1 ms of counted work; then they are stopped as classic did up to Java 19: a `ThreadDeath` where each next runs, with its limit at its count, so its loops throw too. A thread that still doesn't end never runs again. Classic's own wait (3 s of wall time per thread, with re-interrupts) is skipped for such robots. The next round starts without threads; the work cutoff applies to all of this.
- **Not covered yet:** `synchronized` and `wait`/`notify` among a robot's threads, `java.util.concurrent` locks and `LockSupport`, `Timer` and executors, and JDK code that blocks while it holds the CPU (a robot thread preempted while it holds a monitor or initializes a class, which another of its threads then needs). Such a thread blocks for real while it holds the CPU and the robot hangs until the server's hard limit. No threaded corpus robot does this. The design for monitors: `DUP; hook; MONITORENTER` with ownership kept by the scheduler (the real monitor is then never contended among the robot's threads), `synchronized` methods wrapped likewise, and `wait`/`notify` as scheduler queues; `LockSupport.park`/`unpark` patched in `java.base` for the robot's threads covers `java.util.concurrent`.

## Alternatives

- **Run the other threads only at the turn's end, to quiescence, without preemption.** No quantum and no check helper, but a busy-waiting main thread (Toad's `while (addingInProcess) {}`) waits until its work cutoff and crashes, and a thread that never waits (Toad's tree thread) runs until the cutoff each turn. Busy-waiting needs preemption at counted points, so the quantum stays; a quiescence-only gap would also give helpers unlimited work outside the budget.
- **Free helper work: the other threads use the time the robot waits for the others, uncounted** (after its `Execute` is sent, in `RemotePeer` and in classic's `RobotPeer`). Closest to classic on a multi-core machine, where helpers don't slow the main thread. It needs a hook in each peer and the gap's work subtracted from the counted work on both sides, and ADR 0018 decided that helper work counts. Not chosen (decision 1).
- **Plain round-robin, ignoring priorities** (Linux JVMs ignore them by default). Toad's idle tree thread would then take every other quantum of the main thread's turn, which would likely make it skip turns (not measured); with priorities it uses only the gap (`-Drobocodes.threadPriorities=false` switches them off, to compare).
- **No threads in deterministic mode** (`start()` throws, or runs the thread's code synchronously). Simple, but TNT's analyzer loops forever and would hang its robot, and the eight robots lose their behaviour.
- **A per-thread work counter** (count the main thread only): rejected in ADR 0018 for its cost.
- **Let unfinished threads run on into the next round**, as classic on Java 20 and later. Accumulates threads (TNT starts one per round) and makes the next round depend on leftovers; stopping them matches the rumble's classic clients up to Java 19.

## Classic compatibility

Each classic run interleaves threads differently, so no single deterministic order is "right"; the choices above aim at classic's typical behaviour. What deterministic mode loses against classic on a multi-core machine:
- Helper work counts toward the robot's turn (ADR 0018); in classic it runs in parallel and only the main thread is timed. Helpers get at most 1 ms per turn outside the main thread's quanta.
- `sleep` lasts a fixed number of turns (d / 8 ms); in classic it depends on how fast the battle runs: against cheaper opponents, in melee, classic turns are faster and a sleep spans more of them (the Fermat melee ran 1.3 ms per turn).
- Unfinished threads are stopped at the round's end, as by classic up to Java 19, not left running as on Java 20 and later.
- Robot code's `Thread.stop()` throws `UnsupportedOperationException` on Java 21, in both.

## Consequences

- **Reproducible (2026-10-27 to 31, robocode-s, 35 rounds, 1000×1000; recordings compared with the order of robots' `Execute`s normalized):**

  | Battle | Before (agent of main) | With the scheduler (final build) |
  |---|---|---|
  | Fermat melee (seed 5001203142733927833, 10 robots) | 12 runs: 12 different recordings and battle states (first divergence rounds 1–6) | 6 runs: 1 recording (18 more with earlier builds: the same) |
  | TNT vs lancel.Lynx 1.09 (seed −6337020126646286513) | 7 runs: 7 different battles | 5 runs: 1 recording |
  | Rapture vs Lynx (same seed) | 5 runs: 5 recordings, 1 battle state (only counted work and output differed) | 5 runs: 1 recording |
  | Toad vs Lynx (same seed) | 7 runs: 7 different battles (from turn 1) | 5 runs: 1 recording |
  | FlamingForest vs Lynx (same seed) | 3 runs: 3 recordings, 2 battles | 5 runs: 1 recording |
  | TNT, Rapture, Toad vs Lynx, seed 2 | 3 runs each: 3 recordings each (2, 1 and 3 battles) | 3 runs each: 1 recording each |
  | dam.MogBot 2.9, jasolo.Sonda 0.55 vs Lynx | 2 runs each: 2 battles each | 3 runs each: 1 recording each |

  Earlier builds of the scheduler (before the sleep scale) gave single recordings too, over 12–18 runs per battle. The final runs ran three battles at once, besides the test suites, on a loaded machine. None of these robots is reported as starting threads any more.
- **Scores against classic** (classic: the oracle with main's agent, threads free; "classic, scheduled": the oracle with this agent):

  | Robot (35 rounds) | Classic | robocode-s before | Scheduler, sleeps 1 turn per ms | Final (d / 8 ms) | Classic, scheduled |
  |---|---|---|---|---|---|
  | TNT vs Lynx | 2391, 2391 | 1348–2103 | 1815.4 | 2343.1 | 2451 |
  | Rapture vs Lynx | 2082, 2338 | 2081.9 | 1266.6 | 2142.1 | 2142 |
  | FlamingForest vs Lynx | 2034 (a second run broke: 120) | 1913–2057 | 2186.1 | 2178.5 | 2179 |
  | Fermat in the melee | 17998, 17998 | 16302–17998 | 16301.7 | 16301.7 | 16302 |

  With the scale, the sleeping robots score about as in classic again; classic with the scheduler gives robocode-s's results but for TNT, whose oracle battle also ends with "Unable to stop thread: lancel.Lynx 1.09" (as with main's agent: a separate issue).
- **Robots without threads are unchanged:** no scheduler, the same counts, the same results. Their recordings are byte-identical to main's: BeepBoop vs SandboxDT, DrussGT vs Lynx, MatchupAGF vs Shadow, SkippedTurns vs Fire, EvBotNG vs Lynx and the melee without Fermat (with the scratch data directory in SandboxDT's error message masked). Their code runs one extra load and compare per loop pass, their `yield` calls go through a helper to the JDK's, and `Thread`'s patched methods check a hook first.
- **Cost:** on a quiet machine (4 alternating runs each, median), robots without threads ran +0.4 % (BeepBoop vs SandboxDT, 23.0 s vs 22.9 s), +0.5 % (MatchupAGF vs Shadow, the tightest loops, 122.8 s vs 122.2 s) and +0.5 % (9-robot melee): within noise. A later check (3 alternating runs) gave 28.5 s against main's 29.3 s for BeepBoop; this machine has outliers of 40 s and more on either build. The Fermat melee took 24.4 s instead of 24.6 s, TNT vs Lynx 159–172 s instead of 145–164 s (10 % or less), Toad vs Lynx 241–264 s instead of 268–337 s: its tree thread no longer competes for the machine's cores, it only uses the gap. A first version spent the whole spare budget on Toad's polling thread each turn (3 rounds in 52 s instead of 19 s): hence the 1 ms cap.
- **Versions:** `DETERMINISTIC_RULES` (server, and the rumble backend's copy in `rumble-core`) and `rulesVersion()` are 2, so a robot host without the scheduler is refused and a robot host with it isn't used with an older server. `ENGINE_VERSION` stays 1, and with it the rankings: battles of robots without threads of their own are byte-identical (above), and those of the robots with threads were never reproducible and are no-fault in the rumble (ADR 0023), so no verification compares a result of the old rules with one of the new. Design §4.4 notes the exception; the test that pins the two versions together now pins (1, 2).
- **Rollout:** the rumble backend reports its rules (`GET info`) and clients refuse other rules, so the release with rules 2 and the backend with rules 2 go out together. The five robots flagged nondeterministic on prod (`nd`: Toad, Fermat, TNT, Rapture, FlamingForest) may be unflagged only after the rumble's `minClient` is the release carrying this change: an older client still runs them nondeterministically (and reports their threads).
- **The classic oracle** runs the same agent, so its threaded robots are scheduled the same way (the hooks are in classic's robot side, `RobotThreadManager` and `java.lang.Thread`, which both share): see the scores above. TNT, Rapture and Toad, excluded from the corpus difftest as not reproducible in classic, can return to it once a corpus run confirms classic and robocode-s agree on them (not done here; TNT's oracle battle needs the Lynx stop issue looked at).

## Decisions (the owner, 2026-10-31)

1. **Helper work counts** toward the turn, in the main thread's turn and at the turn's end (ADR 0018), as proposed.
2. **At a turn's end** helpers run for at most the turn's spare budget less a quantum, and at most 1 ms.
3. **Priorities** are honoured.
4. **Quantum:** 100 µs of virtual time.
5. **The round's end:** unfinished threads are stopped (as classic up to Java 19).
6. **Versions:** only `DETERMINISTIC_RULES` / `rulesVersion()` are bumped (to 2); `ENGINE_VERSION` and the rankings stay (Consequences).
7. **Monitors, `wait`/`notify`, `java.util.concurrent`:** not implemented; robots that use them with threads are reported (ADR 0024), so they stay no-fault.
8. **The report:** only threads the scheduler doesn't cover are reported, so scheduled robots become verifiable; the five flagged robots are unflagged once `minClient` carries this (Rollout).
9. **The time scale of `sleep`:** calibrated against classic's wall time per turn (`CLASSIC_TURN_NANOS`, 8 ms), part of rules 2.
