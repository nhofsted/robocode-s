---
title: Deterministic mode
description: How robocode-s makes a battle repeatable with a virtual clock, seeded randomness, priced work, scheduled threads and a fixed environment, and what it means for robot authors.
---

The rumble runs every battle in deterministic mode, so results can be checked by running a battle twice. It is also the default for every battle you run: the same robots and the same `--seed N` give the same battle on every machine, turn for turn. `--classic` runs a battle with classic's real-time timing instead.

Most robots need nothing special for it. This page is what changes compared with classic, and what to check if your robot measures its own time or uses threads.

## Time and work

- **The clock is virtual.** In robot code, `System.nanoTime()` and `System.currentTimeMillis()` return a virtual time. It advances with the turns and with the work your code does, so a loop that runs "until 2 ms have passed" does a fixed amount of work, the same on every machine, and a generator seeded from the clock gets the same seed every run.
- **Work has a price per kind.** Your code's bytecode instructions are counted, and each kind (local variables, integer and floating-point arithmetic, comparisons, fields, array accesses, calls, `new`, ...) has a price of its own. JDK calls have one price, except where their cost is known to differ (`Math.sin`, `cos`, `atan2`, `exp`, `log`, `pow`, `sqrt`, `Vector`, `Hashtable`, `HashMap`). Calls whose cost grows with their arguments (`System.arraycopy`, `clone()` of an array, `Arrays.sort`, a list's `contains`, and the like) and new arrays are charged per element. The prices are fixed: the same code costs the same on every machine, and garbage collection costs nothing.
- **The prices are generous.** They were fitted to the real CPU time of about 1 500 RoboRumble robots in classic Robocode (OpenJDK 21) and then lowered, so that 95% of robots that do a lot of work a turn are charged no more than their real time in classic. Your clock reads the time you are charged.
- **Turns are skipped by charged work, not by the clock.** The engine waits for every robot. The budget is 3.2 ms of charged work a turn, how long classic really waits for a robot, and seven times that on a round's first turn. A robot over the budget answers as many turns late as classic would have made it, with classic's `SkippedTurnEvent`s, and more than 30 skipped turns disable it, as in classic.
- **Endless loops end.** A robot that keeps computing is stopped at a work cutoff (0.82 s of charged work since its turn began): its code throws, at the same instruction on every machine, and classic's rules treat it as crashed. As a last resort, a robot whose turn takes 60 s of real time is disabled (the *hard limit*); that depends on the machine, so the battle says so and the rumble doesn't use it to check anyone's results.

If your robot budgets its own computation by the clock, check that it still does enough work a turn: run it and watch for skipped turns. `--classic` shows how it behaves under classic's real timing.

## Randomness

- **Random numbers are seeded per robot**, from the battle seed and the robot's name: `Math.random()`, `new Random()` and `Utils.getRandom()`.
- **Identity hash codes are deterministic** for objects your robot creates, so iterating over a `HashSet` of your own objects has the same order every run.

## Threads

A robot can start threads of its own, and they are deterministic too: each robot gets **one virtual CPU**. Its threads run one at a time and take turns at points that depend only on what the robot executes: when a thread blocks (`sleep`, `join`, `wait`, a lock another of its threads holds) or ends, when it calls `Thread.yield()`, and after every 100 µs of charged work. So a thread that busy-waits on another one still lets it run.

- **Covered:** `Thread` and its subclasses, `java.util.Timer`, executors and thread pools, `synchronized`, `wait` and `notify`, and the locks, conditions, latches and queues of `java.util.concurrent`.
- **Their work counts toward your turn.** In classic only the robot's main thread is timed, and on a multi-core machine its helpers run in parallel. Here every thread of the robot shares its budget, so a helper's work can make the robot skip turns. When the main thread has finished its turn, the other threads get what is left of the budget, at most 1 ms.
- **Priorities count.** The highest-priority waiting thread runs next. As in classic, no thread of the robot can have a higher priority than its main thread.
- **Sleeping uses the virtual clock.** Every thread of the robot reads the same clock. Once the robot has started a thread, a turn moves it 8 ms (classic's typical time per turn in such battles), so `sleep(500)` lasts about 62 turns. A main thread that sleeps is late for its turns, as in classic: about one skipped turn per 8 ms slept.
- **At the end of a round**, threads that are still running are interrupted, then stopped (a `ThreadDeath` is thrown in them, as in classic up to Java 19). The next round starts without them.
- **Not covered:** virtual threads (`Thread.ofVirtual`, `Executors.newVirtualThreadPerTaskExecutor`) and `Unsafe`'s parking. A robot that uses them is reported as nondeterministic: its battles still count in the rumble, but they are never checked. A thread that blocks in native I/O hangs the robot until the hard limit.

## The environment

A robot sees the same system wherever it runs:

- `System.getenv()` is empty, and its process has no environment of the user who runs it.
- System properties are neutral: `os.name` is `Linux`, `user.name` is `robocode-s`, `user.home` and `user.dir` are `/home/robot`, the class path is empty, and `line.separator` is `\n`. `file.separator` and `path.separator` stay the platform's, for building paths.
- The locale is `en_US`, the time zone UTC and the encoding UTF-8, so `String.format("%.3f", x)` writes `1234.568` on every machine.
- `Runtime.availableProcessors()` is 1, and the heap has a fixed size.

Classic mode clears the environment and fixes the locale, time zone and encodings too, but keeps the real system properties and the machine's processors.

## Versions

These rules have a version, and robocode-s and the robot host that runs your robot must follow the same one. A robot host from another robocode-s version is refused for deterministic battles: in [your IDE](/docs/ide#debug-it-with-your-ides-debugger), use the `lib/robot-host` jars of the robocode-s you run.

Next: [package and release it](/docs/robot-development/release).
