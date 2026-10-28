# 0014. Work metering and a virtual clock for robots

- Status: Accepted (recalibrated 2026-10-17, #15)
- Date: 2026-10-11

## Context

Robots read the clock: to seed their own generators (`new Random(System.currentTimeMillis())`), and to limit their work to a time budget (`while (System.nanoTime() - start < budget)`). Wall-clock time makes a robot's behavior depend on the machine and its load. In the RoboRumble corpus, such robots made classic itself non-reproducible (#11). ADR 0009 called this a known limit. Deterministic mode also needs a deterministic measure of a robot's work for CPU budgets later.

## Decision

In deterministic mode the determinism agent **counts the work** of robot code and gives robot code a **virtual clock**:
- **Counting:** at the start of every straight-line block of bytecode in robot classes (defined by classic's robot class loader), `fuel[0] += n`. Here `n` is the block's instructions plus `JDK_CALL_COST` (10) per call into the JDK. JDK calls whose work grows with their arguments also count per element (ADR 0018). `fuel` is one counter per robot class loader (in classic: one per robot). At the start of loops and handlers the count is also checked against a limit, past which robot code throws (ADR 0018).
- **Clock:** in robot code, `System.nanoTime()` and `System.currentTimeMillis()` read
  `elapsed = turns × TURN_NANOS + fuel × PICOS_PER_INSTRUCTION / 1000`.
  It starts at `NANO_ORIGIN` and `EPOCH_MILLIS` (2026-01-01T00:00Z). Turns are counted in classic's robot-side `executeImpl` and `waitForBattleEndImpl`.
  JDK calls that read the wall clock themselves are rewritten too: `new Date()` becomes `new Date(virtual millis)`, `Calendar.getInstance()` gets `setTimeInMillis(virtual millis)` and `Instant.now()` becomes `Instant.ofEpochMilli(virtual millis)`. `pa.Improved` seeds its `Random` from `new Date().getTime()`. Other JDK time sources (`LocalDateTime.now()`, `new GregorianCalendar()`, `Clock`) still read real time; the corpus's double classic recordings catch robots that use them.
- **The constants are part of the deterministic rules:** `TURN_NANOS` = 1 ms, `PICOS_PER_INSTRUCTION` = 160. Every language gets its own counting and calibration.
- **First calibration (2026-10-13, #15), superseded:** `tools/difftest/corpus/calibrate.py` ran 36 RoboRumble battles in classic 1.11.1 on OpenJDK 21 (x86-64, WSL2). Each battle ran twice: once with counting (instructions per robot), once without (each robot thread's real CPU time). Robots that read the clock, or whose turns differed between the runs, were left out (111 robots kept).
  - **The fit:** CPU time ≈ 104 µs per turn + 858 ps per counted instruction. For robots doing over 100 000 instructions per turn, where their own code dominates, the median is 943 ps per instruction (46 robots); over 1 000 000, 901 ps (821–1009, 8 robots). `PICOS_PER_INSTRUCTION` = 900.
  - **The 104 µs per turn** is classic's robot-side work per turn (events, `executeImpl`), not the robot's code. `TURN_NANOS` stands for waiting on the other robots instead and stays at 1 ms.
  - **The counting itself** costs about 1% CPU.
  - It ran classic with `-XX:-InlineMathNatives`, which also turned off the exact math intrinsics (`sqrt` took 313 ns instead of 6). So it measured a JVM several times slower than the rumble's for math-heavy robots, and 900 ps was 3–5× too high.
- **Calibration (2026-10-17, #15):** the same 72 battles (batches 8 and 9) and method, with only the inexact math intrinsics disabled (design §4.3), as deterministic mode now runs robots: nearly the speed of classic with all intrinsics.
  - **The fit:** CPU time ≈ 64 µs per turn + 104 ps per counted instruction (228 robots, 12 left out).
  - **Per robot**, the cost of an instruction now varies widely: tight loops that the JIT compiles well are cheap, calls and allocation are expensive. Over 1 000 000 instructions per turn: 45–209 ps, median 159 (10 robots). Over 100 000: 45–772 ps, median 330 (43 robots).
  - `PICOS_PER_INSTRUCTION` = 160, the median of the heaviest robots, as in the first calibration. One constant can't be close for every robot; it's a fair price for robots that do a lot of work, which are the ones a budget limits.

The values depend only on what the robot executes, the same in classic's oracle and in robocode-s, on every machine.

## Consequences

- **Deterministic time:** robots that seed from or budget by the clock are deterministic. Budget loops always terminate and do a fixed amount of work.
- **Overhead:** robot code runs slower in deterministic mode (a counter update per block; about 1% CPU in the calibration).
- **Not covered:** time read by the JDK on the robot's behalf (`Thread.sleep`, timeouts) stays real, so robots with timing-dependent threads stay nondeterministic; such robots are reported (ADR 0024); ADR 0025 proposes scheduling their threads deterministically, with `sleep` and timed joins on the virtual clock. Robot classes that can't be instrumented (a method growing beyond 64 KiB) run uncounted, with a warning.
- The same counter decides deterministic skipped turns (ADR 0018).
