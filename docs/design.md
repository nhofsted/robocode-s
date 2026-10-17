# robocode-s — Design Document

Status: **Draft** · Last updated: 2026-10-11

## 1. Summary

robocode-s is a reimplementation of [Robocode](https://robocode.sourceforge.io/) with:

- a battle engine written in **Rust**,
- a **GPU-accelerated** battlefield viewer,
- a **client/server** architecture where robots run out of process (optionally in containers) and talk to the server over a well-defined protocol,
- **bit-exact backward compatibility** with existing classic Java robots (`robocode.*` API), and
- client libraries for robots in other languages (Java/Kotlin, Python, C#/.NET, Rust, C/C++),
- its own **distributed RoboRumble**: volunteers run battles, a serverless AWS backend ranks the robots, and a web interface shows rankings and replays battles in the browser.

It runs on Windows, Linux and macOS.

## 2. Goals and non-goals

### Goals

1. **Classic compatibility.** Existing robot jars run unmodified. For a given seed, a battle produces the same turn-by-turn state as classic Robocode (reference: **1.11.1**, see §13). This is verified by an automated differential test harness.
2. **Isolation and resource limits.** Robot code never runs inside the server process. Untrusted robots can be confined (CPU, memory, file system, network).
3. **Polyglot robots.** One language-neutral protocol, with thin client libraries per language.
4. **Speed.** Headless battles should run at least as fast as classic Robocode with the GUI disabled. That matters for RoboRumble and for training loops.
5. **Optional determinism.** A deterministic mode in which the same seed and robots always give the same result, independent of machine load.
6. **Cross-platform.** First-class support on Windows, Linux and macOS (x86-64 and arm64).
7. **GUI.** Battle setup, a live viewer with robot debug painting, recording and replay.
8. **Our own RoboRumble**: distributed battle execution, verifiable results, a web interface with in-browser replays (§12). Contributing to classic LiteRumble is a non-goal.

### Non-goals (for now)

- A built-in robot editor or compiler (use an IDE).
- Changing the classic game rules. Rule variants may come later behind explicit flags; the default is always classic.
- Contributing results to classic LiteRumble.
- Wire compatibility with Robocode Tank Royale. An adapter may be added later (§14).

### Relationship to Robocode Tank Royale

[Tank Royale](https://github.com/robocode-dev/tank-royale) is also client/server and polyglot, but it uses a different game API and does not run classic `robocode.*` robots. robocode-s puts exact classic compatibility first and uses a binary, lockstep-optimized protocol. A Tank Royale bridge is on the roadmap but is not a design constraint.

## 3. Architecture overview

```
                    ┌──────────────────────────────────────────────┐
                    │                robocode-s server             │
 ┌──────────┐       │  ┌───────────┐   ┌──────────┐  ┌──────────┐  │
 │   GUI    │◄─────►│  │  Battle   │──►│  Engine  │  │ Recorder │  │
 │ (wgpu +  │observer  │  runner   │   │ (pure,   │  └──────────┘  │
 │  egui)   │       │  │ (lockstep)│◄──│ determ.) │                │
 └──────────┘       │  └─────┬─────┘   └──────────┘                │
                    └────────┼─────────────────────────────────────┘
                             │ robocode-s protocol (protobuf, framed)
             ┌───────────────┼─────────────────────┐
             ▼               ▼                     ▼
      ┌────────────┐  ┌────────────────┐   ┌────────────────┐
      │ Local      │  │ Container      │   │ Remote host    │
      │ process    │  │ (Docker)       │   │ (runner agent) │
      │ launcher   │  │ launcher       │   │                │
      └─────┬──────┘  └──────┬─────────┘   └──────┬─────────┘
            ▼                ▼                    ▼
      robot process    robot process        robot process
      (JVM + compat    (in container)       (any language)
       lib + robot.jar)
```

Components:

| Component | Language | Responsibility |
|---|---|---|
| `engine` | Rust | Pure, side-effect-free simulation: rules, physics, collisions, events. No I/O, no threads, no clocks. |
| `protocol` | protobuf + generated code | Message schema, framing, version negotiation. |
| `server` | Rust | Accepts connections, runs the lockstep battle loop, enforces time budgets, broadcasts to observers, records battles. |
| `runner` | Rust | Launches robot processes: local process, Docker container, or as a remote agent. Measures the robot's CPU time. |
| `gui` | Rust (wgpu + egui) | Battle setup, viewer, debug painting, replay. Connects as an observer and can embed a server. |
| `cli` | Rust | Headless battles, batch runs, recordings, rumble client. |
| `rumble/*` | Rust (Lambdas) | Rumble backend: work assignment, result intake, auth, submission, rankings. AWS never runs battles. |
| `web` | SvelteKit (static) + WASM | Rumble website, user documentation and in-browser replay viewer. |
| `infra` | TypeScript (CDK) | AWS infrastructure for the rumble. |
| `clients/java-compat` | Java | Drop-in replacement for `robocode.jar`, implementing the classic API on top of the protocol. |
| `clients/*` | various | Native robot APIs for Java/Kotlin, Python, .NET, Rust and C/C++. |
| `tools/difftest` | Java + Rust | Differential testing against classic Robocode. |

## 4. Engine

### 4.1 Principles

- **Pure function per turn:** `step(state, intents[]) -> (state', events[])`. All randomness comes from a seeded PRNG that is part of `state`.
- **Classic semantics:** the code mirrors classic Robocode's `Battle`, `RobotPeer`, `BulletPeer` and `Rules`, including update order, event generation and event priorities. Where classic behavior is odd but observable, we copy it and document it.
- **Java-compatible numerics:** all game math uses IEEE-754 `f64` with Java's semantics. Rust and Java both use strict IEEE `double` arithmetic for `+ - * /` and `sqrt`. Transcendental functions are the risk (§4.3).
- **Java-compatible PRNG:** `java.util.Random` (48-bit LCG) is reimplemented exactly, so initial robot placement and other engine randomness match classic for the same seed.

### 4.2 Turn structure (classic)

The turn structure follows classic `Battle.runTurn()`, roughly:

1. Deliver events and state to robots, then wait for each robot's actions or skip it.
2. Process bullets (movement, collisions with robots, other bullets and walls).
3. Update robots: gun heat, turning (body, gun, radar), movement, wall and robot collisions.
4. Scan (radar sweep arcs), then generate `ScannedRobotEvent`s.
5. Generate the remaining events (hit, death, win, custom).
6. Check for round end.

The exact order, all formulas and every random draw are specified in **[engine-spec.md](engine-spec.md)**, written from the 1.11.1 sources and pinned down by differential tests.

### 4.3 Math functions — the main bit-exactness risk

Classic Robocode calls `java.lang.Math.sin/cos/atan2/...`. On HotSpot these may run as intrinsics, which may differ by up to 1 ulp from `StrictMath` (fdlibm) and can vary between JVMs and CPUs. That means classic itself is only bit-exact per platform and JVM.

**Decision: "bit-exact" means bit-exact against classic Robocode running with `StrictMath` semantics (fdlibm).**
1. The engine uses a Rust port of fdlibm (`crates/fdlibm`) for every transcendental function. It's exact on every platform.
2. The difftest oracle runs classic with `-XX:+UnlockDiagnosticVMOptions -XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow`, which makes `Math` identical to `StrictMath`. Measured on JDK 21/x86-64: with intrinsics, `Math.sin/cos/tan` differ from `StrictMath` for about 3.3% of inputs, and `exp/log/log10/pow` for 7–15%; `atan2`, `hypot` and the other functions never do (they call `StrictMath`, or are exact like `sqrt`). With these intrinsics disabled, 0 differences in 2M samples per function, in the interpreter, C1 and C2.
   Only the inexact intrinsics are disabled: the exact ones (`sqrt`, `abs`, `floor`, ...) stay fast. The JVM-wide `-XX:-InlineMathNatives`, used before (#15), disables them all and made math-heavy robots several times slower (`sqrt` 313 ns instead of 6, `pow` 762 instead of 20); with only the inexact ones disabled, `sin` takes 42 ns (18 with intrinsics) and `pow` 81.
3. Classic running on a JVM with intrinsics may differ from us in rare cases. That is classic's own platform nondeterminism, not a compatibility bug.
4. Scanning and collisions use `java.awt.geom` (`Arc2D`, `Line2D`, `Rectangle2D`), whose algorithms must be ported as well.

Robot-side math runs in the robot's own runtime and is not affected.

### 4.4 Engine version

A battle's outcome depends on more than the protocol: rules and physics, the PRNG, fuel counting and its calibration (§7.2), the pinned JRE and java-compat. A change to any of them can change who wins, so a result only means something together with the engine that produced it.

- The engine gets an explicit **engine version**, separate from the protocol version (§5.1) and from release versions. It changes with every change that can alter a battle's outcome.
- It is stored with every result: in replay headers (§11), in rumble result submissions (§12.3), in the rumble data model (§12.6), in CLI battle results and in difftest reports. Results are only compared or verified within one engine version (§12.4).
- `robocode-s --version` shows it.

## 5. Protocol

### 5.1 Encoding and transport

- **Schema:** Protocol Buffers (proto3). It has the broadest code generation coverage: Java, Kotlin, Python, C#, Rust (`prost`), C++ and C (`nanopb`/`upb`).
- **Framing:** a varint length prefix followed by the message, over any byte stream.
- **Transports:** TCP (default), with TLS required when the peer is not on loopback. Unix domain sockets and Windows named pipes are later optimizations for local mode.
- **Versioning:** `Hello` / `HelloAck` handshake with protocol major/minor versions and feature flags.

### 5.2 Roles

Every connection starts with `Hello` / `HelloAck` (protocol version, role, token, features) and then has one role:

- **Runner ↔ server:** the runner registers its capabilities (`RunnerInfo`: runtimes, mode, trusted CPU time). It receives `LaunchRobot` (package by SHA-256 and URL or inline bytes, entry point, server address, session token, and the robot name and battle seed for its random streams per ADR 0009), and reports `LaunchResult` and `RobotExited`.
- **Robot ↔ server:** a robot connects with the one-time **session token** from `LaunchRobot` and then takes part in the lockstep loop (§5.3).
- **Observer ↔ server:** the GUI or other tools receive `BattleInfo`, then a `TurnSnapshot` per turn and `RoundResult`s, and send `ObserverControl` (pause, step, speed, debug paint per robot, abort, kill a robot, mouse and keyboard input). **Abort** is classic's stop: before its next step the engine aborts (every robot loses 5 energy a turn and its thread stops, robots get `BattleEnded` with `aborted`), the round runs out without pause or pacing, no further round starts, and robots get `BattleOver { aborted: true }`. Classic reports no results for a stopped battle; robocode-s reports the results so far, with the stopped round counted up to the abort (the engine freezes them there, matching the ranks it gives robots). An abort before the robots connect ends the battle without a round. **Kill** (#36) is classic's "Kill robot" in a robot's console (`KillRobotCommand`): before the next step the robot dies as if destroyed (`RobotPeer.kill`: energy 0, an explosion, a team leader's death hurting its team), gets its `Death` event and scores as a death in that turn; a robot already dead is left alone, and a kill that arrives after a round's last step doesn't reach into the next round. **Input** (#36) is classic's interactive events: an `InputEvent` (a mouse move, drag, press, release, click, enter, exit or wheel turn, or a key press, release or typed key, with the fields of the java.awt event classic wraps, in battlefield coordinates with y up) that, as classic's `SendInteractiveEventCommand`, goes to every robot that is interactive (`RobotType.interactive`: classic's `isInteractiveRobot`, a robot implementing `IInteractiveRobot` or overriding one of `Robot`'s `onMouse*`/`onKey*` handlers) and still running (classic's `addEvent` adds only to running robots). The server queues it and, before the next step, records it and gives it to those robots with that step's events (`Event.input`, before the engine's; the robot side orders events by priority anyway). A mouse move or drag replaces one still queued, so robots get at most one of each a turn. Like other non-critical events, it is dropped for a robot that skipped turns. The engine never sees input; the robot host turns it into classic's `robocode.MouseMovedEvent`, `KeyPressedEvent`, ... around java.awt events (as classic's deserializers do), and classic's event manager dispatches them.

### 5.3 Lockstep loop (robot role)

Defined in `proto/robocode_s/v1/robocode_s.proto`. The exchange mirrors classic's `execute` (engine-spec §11), so the classic API can be rebuilt exactly:

```
server -> robot : HelloAck { RobotStatics (names, type, team, battle rules) }
server -> robot : RoundStart { round, status, commands }      the robot starts its code
robot  -> server: Execute { turn, commands, output, paint, cpu_nanos, debug_properties }
server -> robot : Tick { turn, status, commands, events[], team_messages[],
                         bullet_updates[], halt, should_wait, paint_enabled, budget_nanos }
robot  -> server: Execute ...                                  (applies in the next turn)
server -> robot : RoundOver / BattleOver
```

- `commands` is the robot's **complete command state** (classic's `ExecCommands`): remaining turn angles and distance, adjust flags, max velocity and turn rate, colors, plus the one-shot parts (scan, bullet commands, team messages).
- `Tick.commands` returns the server's updated copy (remaining values, energy-related zeroing), and the robot continues from it, as classic's robot side does.
- Blocking calls (`ahead(100)`, `turnGunRight(...)`) and `waitFor(Condition)` are implemented **in the client library** as a sequence of turns, so the wire protocol stays purely per turn.
- Custom events (`Condition.test()`) run client-side. Because they run during the robot's turn, firing them only affects that robot's own event queue, as in classic.
- The server sends ticks to all robots in parallel and waits up to the turn budget (§7). Robots that miss the deadline **skip the turn**, with the classic semantics: the previous command state stays in effect, a `SkippedTurn` event is sent, and the robot is disabled after too many skips.
- Classic runs robots one after another by default. Since robots cannot see each other's commands before the engine step, parallel execution produces the same results.
- A `Tick` with `halt` ends a robot's participation. A robot that is still alive (halted after winning, or disabled) sends **one final `Execute`**, whose commands still take effect: classic's `executeImpl` stores the commands before it throws. Dead robots just stop.
- Debug painting is a list of vector operations (`PaintOp`), sent only while `Tick.paint_enabled` is set.
- Framing: a varint length prefix, messages of at most 16 MiB (`crates/protocol`).

### 5.4 Observer stream

- Full snapshot on subscribe, then per-turn deltas: robot states, bullets, explosions, scores and debug paint.
- Each `RobotSnapshot` carries the CPU time the robot reported for that turn (`cpu_nanos`, from its `Execute`s since the last snapshot, so a robot that skipped turns shows them all at once) and the turn budget (`budget_nanos`, 0 in deterministic mode, which has no wall-clock budget). The GUI graphs them per robot, with each robot's energy (#33). In deterministic mode a robot's CPU time is its counted work (#15) as virtual time (ADR 0014): the same on every machine, and not inflated by the robot JVM running without math intrinsics.
- Debug paint is only requested from a robot while an observer has it enabled. This saves bandwidth and robot CPU time, as in classic.
- `BattleInfo` carries the battle's seed, and observers get `BattleOver` too, with the rounds fought and the final results (robots get it without them), so an observer elsewhere can show the results.
- **Observers from elsewhere** (#36, `server::Observers`, `robocode-s battle --observe ADDR`): a listener of their own beside the robots' one, which stays on loopback, so robot tokens are never reachable from outside. It takes only observers (`Role.OBSERVER`), with the token in their `Hello` when one is set (compared in constant time; a wrong one is answered after a delay), at most 16 at once, each handshake on its own thread with a 10 s timeout. The CLI binds a port alone to 127.0.0.1 and refuses an address that isn't loopback without `--observe-token` (or `$ROBOCODES_OBSERVE_TOKEN`). The connection is plain TCP: the token and the battle travel unencrypted, so a remote observer belongs on a trusted network or an SSH tunnel (`ssh -L 7654:localhost:7654 host`). With `--observe` the battle starts once the first observer connected, so it sees it from the first turn; later observers join where the battle is. Every observer controls the battle (pause, step, speed, paint, stop, kill, input), as Tank Royale's controllers do; an observer that leaves while the battle is paused resumes it, so the command line's own observer isn't held up, and once the last observer from elsewhere left, the battle runs at full speed again.
- **Local security** (PR #48 review): the robots' listener is on loopback, but any process on the machine reaches it, another user's too. So in a local battle (`runner::local`) every robot gets a fresh unpredictable token per battle (`server::random_token`: 128 bits from the operating system's randomness), and so does the embedded observer (`ServerConfig.observer_token`), which the robots' listener now requires of observers as well; both are compared in constant time. Nothing else connects there: the GUI's and the CLI's own observer is the embedded one, and observers from elsewhere use their own listener. A robot's token reaches its host in the environment (`ROBOCODES_ROBOT_TOKEN`), not on the command line, which other users can read (`ps`); the observer's never leaves the process. Tokens are neither recorded nor part of the battle, so recordings and outcomes don't change. Robots themselves run unsandboxed as the same user (`NOSECURITY`, §8), so a malicious robot could still read another robot's environment before that one connected (each token works once) or attack the user's processes in other ways: isolating untrusted robots is the container runner's job (§6). An `--observe` port on loopback without `--observe-token` is the user's choice and is open to every local process; set a token on a shared machine.
- **Debug properties** (#36, classic's `setDebugProperty(key, value)`): an `Execute` brings the properties the robot set since its last one, in order (classic's `ExecCommands.getDebugProperties()`), and the next `RobotSnapshot` carries them as `debug_properties`: changes, as classic's snapshot does, not the current set. An observer keeps the current values; an empty value (classic's `null` or `""`) removes a property, as classic's robot console does. The GUI shows them as a key/value table beside the robot's output.

## 6. Robot hosting (runners)

### 6.1 Launcher modes (pluggable)

| Mode | Use case | Isolation | Notes |
|---|---|---|---|
| **Local process** | Development, fast rumble runs with trusted robots | Low: OS process only. Memory limited through runtime flags (e.g. `-Xmx`); optionally Job Objects on Windows or cgroups on Linux. | The JRE is **downloaded on demand**: on first use, a pinned Eclipse Temurin JRE for the platform is fetched into the robocode-s data dir and its checksum verified. Other runtimes follow the same pattern where possible. |
| **Container** | Untrusted robots, tournaments | High: CPU, memory, pids, read-only rootfs, no general network | Requires Docker (Docker Desktop on Windows/macOS). |
| **Remote agent** | Distributed tournaments, spreading load | Whatever the remote host's launcher provides | `runner` in agent mode, authenticated to the server. |
| *WASM (future)* | Native and new-language robots | High, plus deterministic fuel metering | wasmtime embedded in the runner (§14). |

### 6.2 Container images

- One image **per runtime**, not one polyglot image: `robocode-s-runner-jvm`, `-python`, `-dotnet`, `-native`.
- Each image contains a small launcher shim: it receives the robot package and starts it with the compat or client library on the classpath or path.
- Multi-arch builds (amd64 and arm64), published to GHCR.
- **Warm pools:** the runner keeps N containers per image started ahead of time, to hide container and JVM startup.

### 6.3 Container network isolation

A robot must reach the server and nothing else. **Decision:** robot containers sit on an `--internal` Docker network that also contains a tiny relay container. The relay forwards only to the server's robot port. This works the same on all three platforms and needs no host firewall rules.

### 6.4 One process per robot

Each robot, including each team member, gets its own process (and JVM). This keeps CPU accounting, memory limits and crash isolation simple. Team messages travel through the server.

### 6.5 Robot data files

Classic robots get a per-robot data directory with a quota (`getDataFile`, `RobocodeFileOutputStream`, default 200 KB). The runner keeps these directories on the host under the robocode-s data dir, mounts them into containers, and enforces the quota. The compat library implements the classic API on top of them.

## 7. Time budgets and determinism

### 7.1 Classic mode (default)

- Each robot gets a per-turn CPU budget derived from a "CPU constant", calibrated on the host like classic Robocode does.
- The budget is measured in **CPU time of the robot's thread or process**, measured by the runner or the trusted client library (e.g. `ThreadMXBean` for the JVM). Wall-clock time is not used, so network latency and container scheduling don't count against the robot.
- The server also applies a generous **wall-clock deadline** per turn, as a liveness guard.
- Docker CPU quotas (`--cpus`) are coarse (100 ms CFS periods). They're used only as a safety ceiling, not for fairness. Use `--cpuset-cpus` where available.

### 7.2 Deterministic mode (optional)

- There are no skipped turns based on time: the server always waits for every robot's intent, up to a hard limit. A robot that exceeds the hard limit is **disabled**. Only that pathological case can make results depend on timing.
- Robots skip turns by their **counted work** instead (ADR 0018): a robot whose work in a turn exceeds the budget (2.5 ms of virtual time, ADR 0014) answers as many turns late as classic would have run without it, with classic's skipped-turn events and disabling.
- All engine randomness is seeded.
- Robots' own randomness is seeded **per robot**: `Math.random()` and `new Random()` draw from streams derived from the battle seed and the robot's name (ADR 0009). For JVM robots, a Java agent (`clients/java-determinism`) does this. Classic itself can't: its `RANDOMSEED` only seeds `Math.random()` on old JVMs, as a single stream shared by all robots.
- Robot JVMs run with the inexact math intrinsics disabled (`-XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow`), so robots' own trigonometry is identical on every platform (§4.3). Otherwise volunteers on different hardware could get different results for the same seed.
- The determinism agent also gives robot objects deterministic identity hash codes, so robots iterating over identity-keyed hash collections behave the same in every run (ADR 0013).
- It counts the work of robot code and gives it a virtual clock, so robots that read the time behave the same everywhere (ADR 0014).
- Known limit: robots that read the clock to budget their own computation can still behave differently between runs.
- Under WASM, budgets become instruction fuel, which makes even the time limits deterministic.

### 7.3 Trust

Robot-side CPU measurement is only trusted when the runner is trusted: a local runner, or a remote runner operated by the tournament organizer. Untrusted remote runners only get wall-clock budgets.

## 8. Java compatibility library (`clients/java-compat`)

The **robot host** runs classic Java robots unchanged (ADR 0012). It does not reimplement the `robocode.*` API. Instead it runs **classic Robocode's own robot side** (`robocode.api`, `core`, `host`, `repository`, `battle` 1.11.1 from Maven Central, EPL): the robot proxies, event manager, blocking calls, custom events, team messages, data files and painting. Only the **peer** is replaced:

- `RemotePeer` implements classic's `IRobotPeer`. Each `execute` becomes an `Execute` message; the next `Tick` becomes classic's `ExecResults`. Dead or halted robots get classic's `DeathException`, `WinException` or `AbortedException` after sending their final commands, exactly like classic's `RobotPeer.executeImpl`.
- Crashed robots (`drainEnergy`) and misbehaving ones (`punishBadBehavior`) are reported in `Execute`.
- The peer measures the robot thread's CPU time per turn (`ThreadMXBean`).
- **Same name, same object:** in classic every event carries the very same `String` instance for a robot's name, the robot's own name included, and rumble bots compare names with `==`. The host keeps one instance per name (not `intern()`: classic's names aren't interned).
- **A robot whose code ends** (crash, robot-side disable, `run()` returning) sends one final `Execute` with its current commands, `drain_energy` and `stopped`. That matches classic's `waitForBattleEndImpl`, which stores its commands once more and then only re-sends the server's copy.
- **Kotlin:** like classic 1.11.1, the host has `kotlin-stdlib` 2.4.0 on its class path, for robots written in Kotlin.
- `RobotHost` (the main class) connects, receives its identity in `HelloAck`, configures the robot's random streams (ADR 0009, 0010), looks up the robot in its jar through classic's repository, creates classic's proxy and then drives rounds from the server's messages.
- **Class folders** (#36): a `--jar` may name a folder of class files (an IDE's output). The host makes it one of classic's development paths (`robocode.options.development.path` in the home's `robocode.properties`), so classic reads it in place and names its robots as development versions (`my.Robot*`, `my.Robot 1.0*`; not `sample.*` and `tested.*`). Every battle starts fresh JVMs in a new home, so a recompiled robot is new in the next battle; the home's `robots` folder is emptied first, so a reused home keeps no old jars or robot database. Each listing of the robots (the GUI's) has a home of its own, `data/list/<pid>-<n>`, removed afterwards (one a crash left behind goes once it is an hour old): two listings sharing one, as two GUIs at once did, broke each other. `ALWAYSUSECACHEFORDATA=true` sends such robots' data files to the home, as for jars, instead of into the folder.
- It runs with `NOSECURITY=true` (one class loader for classic's engine classes and the peer); isolation is the process or container (§6).
- **Distribution:** `./gradlew :clients:java-compat:installDist` gives `robot-host/lib/` with the classic jars as separate files, because classic finds its modules by jar name.

**Verification:** the live difftest (`crates/difftest`, `live`) fights recorded classic battles with real robot processes through the runner, robot host, server and engine, and compares every turn. All recorded battles are identical, top bots included.

## 9. Other client libraries

Native APIs for new robots, all generated from the same `.proto`:

- **Java/Kotlin** (modern API, non-legacy), **Python**, **C#/.NET**, **Rust**, **C/C++**.
- Each language provides two layers: a low-level **turn-loop API** (receive tick, send intent) and an ergonomic **robot API** (event handlers, blocking helpers) modeled on classic concepts.
- Classic .NET robots (`robocode.dotnet`) may get a compat layer later, like the Java one.

## 10. GUI (`gui`)

- **Stack:** `wgpu` (Vulkan / Metal / DX12 / GL fallback) for the battlefield, `egui` for panels and dialogs. Bevy is not used, to avoid ECS overhead for a simple 2D scene and to keep the engine independent.
- **Features (scope v1):** battle setup (robot selection, rules, number of rounds, seed, launcher mode), live viewer with speed control, pause and single-step, robot debug painting per robot, robot console output, results table, recording and replay with scrubbing.
- **Layout (Steam-like, `crates/gui/src/shell.rs`):** a top bar with the tabs Start Battle, Watch Battle and Run Rumble (M5) on the left and a gear for the options on the right, and a footer with the version. Pages with inputs sit in a centred column of at most 960 points; the margins show classic's floor, darkened, with a few tanks wandering as on the website's home page (`backdrop.rs`, at most 30 frames a second and still when the margins are too narrow to see). Start Battle shows the available robots (with a filter) beside the ones in the battle, then the battle's settings; while the robots are described (`runner::local::prepare`, on a background thread) and their JVMs start, a loading screen shows the contestants, or the error with a way back. Watch Battle has two sections in the same style: From File (a recording to open) and Remote Server (a battle to watch). A live battle, a watched one or a replay fills its tab with the battle screen (`screen.rs`): the battlefield, the controls below it as drawn icons (stop, pause/play, step, speed; round, turn and seed on the right; a replay adds the timeline and stepping back), the robots' output below those in a tab per robot with its debug-paint marker (a pencil, green when on) in the tab, and the robots on the right, the full height, with square energy bars (empty at 0) and scores; the energy and CPU graphs (#33) go there too. The loading screen can be cancelled: a battle that is closed stops (`ObserverControl.abort`). Icons are drawn with the painter (`icons.rs`), as egui's default fonts lack many symbols. The app is dark whatever the system's theme (egui would follow a light Windows or macOS theme, which the app's colours aren't made for), and so is the system's title bar where it draws one. Under WSLg (whose compositor crashes on winit's frame, #28) and on Windows (for a window like Steam's) the app draws its own window frame, and the top bar is its title bar: its empty parts move the window, a double click maximizes it, and thin edges resize it; `ROBOCODES_OWN_FRAME=0` (or `1`) asks for the system's frame (or ours) instead. On Windows winit's undecorated window keeps the system's caption and sizing styles and only hides the non-client area, so it keeps the drop shadow, snapping (Win+arrows, dragging to a screen edge) and the system's own move and resize loops (which hold the minimum size and end with the button's release); maximized, it covers the work area, not the taskbar. Under WSLg the compositor keeps the pointer's release after a move or resize, so the app makes one up (`window::release_after_grab`). The window is at least 720 × 480 points: the minimum goes to the compositor (`xdg_toplevel.set_min_size`), and since sizes the app asks for are not held to it (nor, maybe, every compositor's resizes), a window that has stayed smaller for 0.4 s asks for the minimum (`window::keep_min_size`; `ROBOCODES_FRAME_LOG=1` logs its sizes).
- **Options (`settings.rs`, #30):** robot folders (scanned for `.jar` files; a folder holding `.class` files a few levels down is also passed on as a class folder, #36; `ROBOCODES_ROBOT_JARS` still adds jars and class folders), scan arcs, coloured bullets, and the default mode, rounds and battlefield size of new battles. Saved as JSON in the platform's config directory (`$XDG_CONFIG_HOME/robocode-s/gui.json`, `~/Library/Application Support/…` on macOS, `%APPDATA%\…` on Windows; `ROBOCODES_GUI_CONFIG` names another file) on every change, and loaded at start. `--tab start|replay|options` opens a tab at start, for screenshots.
- **Scan arcs** are derived in the viewer, not sent: the engine scans from the radar heading at the start of the turn to the one after moving, the shorter way, around the robot's new position with radius 1200 (`RobotPeer.scan`), and the previous turn's snapshot holds that start heading. So the protocol, recordings and the website's replays stay as they are. Unlike classic, which keeps showing the last scan while a robot stands still, a robot whose radar didn't turn shows a line.
- **Headless:** the same battles run from `cli` without a GUI, or with a remote GUI attached as an observer: `robocode-s battle --observe ADDR` (§5.4) waits for the first observer, and the GUI's Watch Battle tab has **Remote Server** (an address and token; `--watch HOST:PORT [--token T]` on the command line). A watched battle shows on the same battle screen as a live one (`Live::watch`, `server::watch::Watcher`): its robots, rules and seed come with `BattleInfo`, its results with `BattleOver`, and the controls go to that server. Closing it leaves the battle running; its results offer no restart. It shows in the Watch Battle tab, so a battle started here (Start Battle) and a watched one run side by side; a watched battle and a replay take turns in the tab, the one opened last replacing the other (`app::Battle`, one for each tab).
- The battlefield is drawn with egui's painter (meshes on wgpu): classic's floor tiles, textured quads for the tank parts (classic's images, recoloured with classic's `ColorFilter`), bullets, explosions and vector debug paint. That is cheap even with dozens of robots; a dedicated instanced wgpu pass can replace it if profiling asks for it. Turns that can't be displayed are dropped, so rendering never slows down the simulation.
- **Embedded server:** a live battle runs the server and the robot processes on a background thread (`runner::local`); the GUI is its observer and sends pause, step, speed, debug-paint toggles, stop, kill and input (`ObserverControl`).
- **Interactive robots** (`crates/gui/src/input.rs`, classic's `InteractiveHandler`): the live battlefield turns egui's pointer and keyboard events into `InputEvent`s. Pointer events count over the battlefield only (not under a window over it); a drag or release that began there is followed outside, as java.awt does. Screen points map back to battlefield points as classic rounds them (`(int) (v + 0.5)`, y mirrored). Clicks follow java.awt: a release without movement since the press, counted up within 500 ms. Wheel deltas become whole notches (negative: away from the user). Keys get java.awt key codes, characters and locations, and egui's text becomes typed keys; they go to robots while the battlefield has focus (the pointer is over it or it was clicked last) and no text field wants them. The GUI only reads events, never consumes them, so the app's shortcuts still work. It sends at most one move a frame. `--input-at TURN:EVENT` sends scripted events, for screenshots. Ctrl+wheel and Ctrl+drag are the view's (zoom and pan) and are not passed on.
- **Zoom and pan** (`view.rs`, #36): a `Camera` per battle and replay (zoom 1–16 relative to fitting the whole field, and the battlefield point at the centre of the view) gives the view's `Transform`, which drawing (tanks, bullets, names, scan arcs, debug paint) and the interactive robots' coordinates share. Zooming keeps the battlefield point under the pointer in place; the centre is clamped so the field fills the view along each axis where it is larger than the view and is centred where it is smaller. Live battles take only Ctrl+wheel (egui's zoom delta, which also covers a touchpad's pinch) and Ctrl+drag, since the plain wheel and drags belong to interactive robots and the GUI cannot tell whether a robot is interactive; replays also take the plain wheel, any drag and a double-click to fit. A fit button with the zoom shows over the field's top right corner while zoomed in. `--zoom Z[,X,Y]` starts battles and replays zoomed in, for screenshots.
- **Sound** (`sound.rs`, #36): classic's `SoundManager` and its sound files (`crates/gui/sounds/`, EPL, attributed in its README). Each turn's snapshot maps to effects as classic's `onTurnEnded` does: a bullet in frame 0 of `FIRED` (a gunshot at volume power / 3, classic's gain of -20 dB × (1 - volume)), `HIT_VICTIM`, `HIT_BULLET` or `EXPLODED` (a robot's death; a wall hit is silent), a robot in `HIT_WALL`, and one `HIT_ROBOT` a turn; panned by x. A small mixer plays them with classic's voices per effect (5 gunshots, 3 hits and deaths, 2 of the rest; another restarts the oldest). The output opens on a thread of its own when sound is first on: on Linux PulseAudio's native protocol through the pure-Rust `pulseaudio` crate (PulseAudio, PipeWire and WSLg serve it; building needs no ALSA headers, which cpal and rodio would), about 60 ms buffered; cpal (WASAPI, Core Audio) on Windows and macOS. Without a server the GUI stays silent and says why in the options and on stderr. Live battles play the turns that arrive, replays the turns they play one at a time; only for the battle on screen and at most `sound::MAX_TPS` (100) turns a second, so fast battles stay quiet. On by the options (off by default, as classic), with a volume.
- **Debug paint:** the robot host replays classic's serialized `onPaint` calls onto a recording `Graphics2D` that emits `PaintOp`s. The server asks a robot for paint only while an observer has it on, as classic does.
- **Replay:** opening a recording runs it once and keeps an engine keyframe every 200 turns, so scrubbing replays at most 199 turns.

## 11. Recording and replay

- **Format** (`proto`, `RecordingEntry`; gzip): a header (format version, engine version, seed, rules, robots) followed by **every input the server gave the engine**, in order: round start and end, each turn's step, the commands each `Execute` brought, the drained, disabled and stopped robots, and an observer's abort, killed robots and mouse and keyboard input (which the engine ignores: robots' reactions are recorded as their commands, so the input is there for completeness and viewers). The server feeds the engine only through one driver (`crates/server/src/driver.rs`) that records each input before applying it, so a replay takes exactly the server's path.
- A **snapshot marker** is recorded wherever the server published a `TurnSnapshot`. It holds what the engine doesn't know (console output, debug paint, each robot's CPU time and the turn budget, and the debug properties robots set) and a 64-bit hash of the snapshot, so every replayed turn is verified. CPU times and debug properties are not battle state: the hash leaves them out (as output and paint), and a replay shows them. Recordings from before they were recorded replay with CPU times of 0 and no debug properties.
- Replay re-runs the deterministic engine. Keyframes aren't stored: the engine state is cheap to clone, so a viewer takes them in memory while it loads a recording, and scrubbing replays from the nearest one. The GUI loads on a thread of its own (`replay::Opening`), showing the progress with Cancel, so a long recording doesn't hold the window up; a website track takes the same way. A 5-round, 10-robot sample battle (4,777 turns) records to 190 KB and replays in 70 ms.
- `robocode-s battle --record FILE` records; `robocode-s replay FILE` replays, verifies and prints the results.
- The file is created when the battle starts, so that a path that can't be written fails at once, and removed again when the battle is stopped before its first round (a cancelled start in the GUI): such a battle leaves no recording. A battle stopped later keeps one, up to the stop.
- Rumble replays in the browser use a different, lossy format (§12.7, ADR 0017).
- Classic `.br` recordings can be imported if feasible (low priority).

## 12. robocode-s Rumble

We run our own distributed rumble. Volunteers run battles with the robocode-s rumble client, and a serverless AWS backend collects results, computes rankings and serves a web interface for browsing results and replaying battles. We do not contribute to classic LiteRumble.

**Hard constraint: the backend stays within the AWS free tier.** AWS never runs a battle or a robot. All simulation is done by volunteer clients; AWS only coordinates, stores and serves.

### 12.1 Architecture

```
 rumble clients (cli / gui)                     AWS (eu-west-1 + CloudFront)
 ┌──────────────────────┐   HTTPS    ┌──────────────────────────────────────────────┐
 │ GET  /api/work       │──────────► │ CloudFront (rumble.planethofstede.be)        │
 │ run battles locally  │            │   ├─ /*      → S3: static site, ranking JSON │
 │ POST /api/results    │──────────► │   ├─ /jars/* → S3: mirrored robot jars       │
 │ PUT recording        │──────────► │   ├─ /rec/*  → S3: recordings                │
 │   (presigned S3 URL) │            │   └─ /api/*  → Lambda function URL (OAC)     │
 └──────────────────────┘            │                  work · results · auth ·     │
 browser ──────────────────────────► │                  submit · admin              │
                                     │ DynamoDB (provisioned, within free tier)     │
                                     │ EventBridge Scheduler → rankings Lambda      │
                                     └──────────────────────────────────────────────┘
```

- **Lambdas in Rust** (`cargo-lambda`, arm64), so they share the protocol and recording code with the rest of the project. Rust Lambdas start quickly and use little memory, so GB-seconds stay low.
- **No API Gateway.** The HTTP API is only free for the first 12 months. Lambda **function URLs** sit behind CloudFront (origin access control), and everything is served from one domain, `rumble.planethofstede.be`. That means one certificate (ACM, us-east-1), no CORS and no separate API domain.
- **Infrastructure as code:** AWS CDK (TypeScript) in `infra/`.
- **Website:** SvelteKit built with `adapter-static` on S3 + CloudFront (ADR 0015). It also hosts the user documentation. The home page and docs are prerendered; ranking, bot and pairing pages render in the browser from pre-computed JSON (`docs/rumble-data.md`). Interactive features (login, bot submission, "watch", reputation and account pages) call the `/api` Lambdas from the browser, and those Lambdas read and write DynamoDB. Unknown paths fall back to `200.html` (CloudFront custom error response), so client-side routes like `/bot/<id>` work; a CloudFront Function maps extensionless URLs to the prerendered `.html` files (`web/README.md`).
- **Deployment:** a dedicated member account `robocode-s` in the owner's AWS Organization (ADR 0016), region **eu-west-1** (Ireland). An SCP limits it to eu-west-1 and us-east-1 and to services within the free-tier plan. The CloudFront certificate needs a small us-east-1 stack next to the main eu-west-1 stack. The DNS zone `planethofstede.be` stays in the management account: the `rumble` CNAME and the certificate's validation record are added there by hand, once.

### 12.2 Free-tier budget

| Service | Always-free allowance (approx.) | How we stay within it |
|---|---|---|
| Lambda | 1M requests + 400k GB-s / month | Rust on arm64 with small memory; clients fetch work and upload results in **batches**, not one request per battle |
| DynamoDB | 25 GB storage, 25 RCU + 25 WCU **provisioned** | Provisioned mode in prod (on-demand is not covered by the free tier) and no auto-scaling beyond 25; aggregates instead of per-battle rows (§12.6). A dev stage uses on-demand, which costs cents at test traffic and leaves the free capacity to prod |
| CloudFront | 1 TB transfer + 10M requests / month | Site, ranking JSON, jars and recordings are all served through CloudFront and cached |
| EventBridge Scheduler | 14M invocations / month | One scheduled ranking run per day |
| CloudWatch Logs | 5 GB ingestion | Low log level and short retention (7 days) |
| SSM Parameter Store | Standard parameters free | GitHub OAuth secret stored there (not Secrets Manager, which costs $0.40/month per secret) |
| ACM | Public certificates free | — |

Costs outside the free tier, kept deliberately small (the replay costs are detailed in §12.7):
- **Route 53:** none. The records live in the existing `planethofstede.be` zone, so the rumble adds no hosted zone.
- **S3 storage:** S3's free allowance only lasts 12 months. Afterwards it costs about $0.023/GB-month, so storage is capped (§12.7) at a few GB, which means cents per month.

Guardrails:
- AWS Budgets alert at $1/month.
- **Reserved concurrency** on every Lambda, so abuse can't scale costs.
- Per-token rate limits in the API.
- DynamoDB capacity fixed at the free limits. Overload causes throttling, not a bill.

### 12.3 Server-assigned work

Clients do not choose pairings themselves:

1. A client calls `GET /api/work`. It gets a **batch** of battle assignments: division, robot IDs and jar hashes, number of rounds, **seed** and assignment ID. Pairings with the fewest battles come first, as in LiteRumble.
2. The client runs the battles in **deterministic mode** (§7.2). It downloads jars through CloudFront and verifies them by hash.
3. The client calls `POST /api/results` with all results of the batch: per-round scores, a recording hash for each battle, and the engine version.
4. The results Lambda checks that each assignment exists, is unexpired and belongs to this user, then updates the aggregates (§12.6).

Note: in deterministic mode slow robots skip turns by their counted work, not their time (ADR 0018). Their behavior can still differ slightly from classic on a given machine. This is accepted and documented for the rumble.

### 12.4 Trust and verification (volunteers only)

- **Authentication:** GitHub OAuth. The auth Lambda exchanges the code and issues a long-lived upload token for the client and a session cookie for the web UI.
- **Assumption:** most of the community is honest. Verification should be cheap, while still catching cheating and broken clients (bugs, modified engines, nondeterminism).
- **All verification is done by other volunteers.** The backend only decides which battles to hand out twice and compares the results.
- **Reputation-based verification rate:** each user has a reputation that controls what fraction of their battles is re-run by someone else:
  - **New users start slightly suspicious:** about 20% of their battles are verified (initial value; configurable).
  - Each verified, matching battle raises reputation, and the rate falls toward a **floor of about 1%**. The floor never reaches zero, so a trusted account that gets compromised or runs a broken client is still noticed.
  - A user reaches the floor after roughly 100–200 matching verified battles (curve to be tuned).
  - All values are configuration, not code.
- **How a battle is verified:** the same assignment (same seed and robot versions) goes to a **high-reputation user**. Results are only compared when both runs used the same engine version.
- **Optimistic acceptance:** results count in the rankings immediately, even before verification. With a mostly honest community, this keeps rankings fresh.
- **Mismatch handling:**
  1. A third, high-reputation user runs the battle as a tie-breaker. A mismatch can also be our own bug (e.g. nondeterminism), so it's not automatically treated as cheating.
  2. The user who was wrong loses their reputation (back to "new", or below) and is **quarantined**: new results are held back, and a sample of their recent results is re-verified by others.
  3. If all three runs disagree, it's an engine bug. An alert is raised and nobody is penalized.
  4. Repeated confirmed mismatches lead to a ban, and that user's results are removed from the aggregates.

Removing a user's results requires their per-battle contributions. These are kept in a compact per-user log in S3 (append-only batches); per-battle rows in DynamoDB would exceed the free tier.

### 12.5 Participants and robot jars

- **Submission:** a web form with GitHub login. Authors upload a jar (via presigned S3 URL) or provide a URL to mirror. They choose divisions (1v1, melee, teams, twin duel) and can later replace the version or retire the bot.
- **Validation without running robots:**
  1. **Static checks in the submit Lambda:** jar structure, robot `.properties`, the main class exists, and codesize for Java weight classes (nano/micro/mini/mega). Codesize is computed by parsing class files in Rust, without a JVM.
  2. **Probation:** a new bot first gets a few battles assigned to high-reputation clients. If it fails to load, crashes or is disabled every round, clients report this and the bot is deactivated with a message to the author. Only after probation does it enter normal scheduling.
- **Seeding:** a one-time import of all robots in the current RoboWiki participant lists. It runs from the owner's machine (a `cli` admin command), not in Lambda, and the jars are mirrored to S3. Imported bots are attributed to their original author and are claimable by that author after logging in with GitHub.
- **Mirror and takedown policy:** jars are mirrored to keep the rumble working when original URLs die. Authors can request removal through the site or a GitHub issue, and removals are handled promptly. The import is announced on RoboWiki.
- Other-language robots (M4) use the same flow with their own package format. Weight classes only apply where codesize is defined.

### 12.6 Data model and rankings

DynamoDB holds only aggregates; there are no per-battle rows.
- **Pairing statistics:** battles and the running sums needed for each metric, per (division, bot A, bot B).
- **Per-bot aggregates:** sums of pairing averages, updated by delta in the same transaction as the pairing statistic. APS, PWIN and survival can then be read from the bots table alone, without a scan.
- **Users:** reputation, quarantine flag, counters.
- **Assignments:** outstanding work, with a DynamoDB TTL so expired ones disappear for free.

Write cost: a battle costs roughly 4–10 WCU (transactional), so 25 WCU sustains about 3 battles/s, more than 200k battles/day. Batching uploads smooths out spikes.

Rankings:
- **Once a day** a scheduled Lambda computes all metrics together: APS, PWIN and survival from the per-bot aggregates; Glicko-2, ANPP, Vote and pairing matrices from a slow, rate-limited scan of the pairing statistics, kept inside the free read capacity. It writes the data files of `docs/rumble-data.md`: per-rumble rankings and per-bot files with pairings and history. The website shows the rumble as of that run.
- **History:** a daily snapshot of each bot's rank and score, appended to small per-bot JSON files, for trend charts.

### 12.7 Recordings and replays

Recordings are stored per **round**, not per battle. A rumble recording is a **lossy viewer track** (ADR 0017, `crates/replay`): the round as the viewer draws it, quantized to what a screen shows, predicted and range-coded. The exact round is reproducible from its seed, bot versions and round index. Measured on top bots: 3.3–3.8 bytes per turn in 1v1, so about 3.5 KB for a 900-turn round and 20 KB for a 6,000-turn one. Lossless intents would take 37–39 bytes per turn.

**Stored round slots:**
- **Which matchups:** every matchup where **at least one bot is in the top 50** of its division.
- **Per matchup:** the **5 rounds won by each side**, so authors can study why they win and why they lose. One-sided matchups may have fewer.
- **Slot key:** (division, bot A version, bot B version, winner, index). A slot is filled by the first qualifying rounds played after that version pair appears.
- **No expiry:** a recording is never replaced while both bot versions are current. Only a **new bot** or a **new version** creates new empty slots, and only new slots cause uploads.
- **Clean-up:** recordings of retired bot versions are deleted, which is free in S3. A bot that drops out of the top 50 keeps its existing recordings; it just doesn't get new slots.

**Upload flow:**
1. A client keeps the round recordings of its current batch until the results are acknowledged.
2. The results Lambda replies with the rounds that fill an empty slot, each with a presigned S3 PUT URL. Most batches upload nothing.
3. Only users above a reputation threshold are asked for recordings. Each recording's hash must match the round hash reported in the results.

**Everything else on request:** for any other matchup or round, "watch" turns into a high-priority assignment. A volunteer re-runs the battle deterministically and uploads the requested round, which is kept in a small LRU cache with a size cap of about 1 GB.

**Estimated cost** (1v1 with about 1,100 bots, plus the other divisions):

| Item | Estimate |
|---|---|
| Stored rounds | about 520k (1v1) + other divisions → **about 3–5 GB** (5–10 KB per round, depending on round length) → about $0.07–0.12/month |
| Initial fill | about 550k PUTs, **one-time about $2.75** |
| Ongoing uploads | new bot outside the top 50: about 500 rounds; new bot entering the top 50: about 11k rounds. At a few dozen new versions/month: **about $0.10–0.50/month** |
| Downloads | Served through CloudFront and cached; within CloudFront's free allowance |

### 12.8 Web interface

- Rankings tables per division and weight class, sortable by every metric, with filters and search.
- Bot pages: metadata, author, versions, rating history chart, best and worst pairings, and a full pairing matrix with heatmap.
- Pairing pages: per-pairing statistics and a "watch a battle" button (§12.7).
- **Battle replay in the browser:** the replay decoder (`crates/replay`, compiled to WebAssembly) and a canvas viewer play the recordings. No engine runs in the browser: tracks are what the viewer draws (ADR 0017). Scrubbing jumps between decoded frames.
- Contributor stats (battles run, reputation tier) and a page for submitting or managing bots.
- **User documentation** for robot authors: about the project, getting started, developing, testing, debugging and releasing a robot, and how the rumble works. Markdown in `web/src/routes/docs`, prerendered.
- **Data contract:** the rankings Lambda writes versioned JSON files under `/data/v1` (`docs/rumble-data.md`). Until it exists, the site's dev server serves seeded synthetic data in the same format.
- **Look:** classic Robocode, with its own images: the blue-metal floor tiles as background, its tank sprites (recoloured like classic) and explosions, and a roaming battlefield behind the home page. Bot pages show rating history across versions, with a marker at every release.

## 13. Compatibility reference and differential testing

- **Reference version:** classic Robocode **1.11.1** (released 2026-07-13, the latest at the time of writing). Before 1.10, the 1.9.x line received bug fixes and Java-version support, not rule changes. LiteRumble accepts uploads from 1.10.3, 1.11.0 and 1.11.1 (O2). The only rule-relevant difference among them is classic's fix #89 in 1.11.0: in 1.10.3, a pending fire command of a robot that stopped sending commands (returned from `run()`, crashed, skipped turns) fired again each time its gun cooled down, with the same bullet id. robocode-s follows 1.11.x (each command fires once, engine-spec §5); it only affects robots that stop executing, so the harness doesn't run against 1.10.3.
- **Harness (`tools/difftest`):**
  1. Runs a battle in classic Robocode via the `robocode.control` API (`RobocodeEngine`, `BattleListener`, `ITurnSnapshot`) with `RANDOMSEED`, `-Ddebug=true` (no timing-based skipped turns), the inexact math intrinsics disabled (§4.3) and the determinism agent. Recordings are byte-identical across runs (`tools/difftest/README.md`).
  2. Runs the same battle in robocode-s, deterministic mode, with the same seed.
  3. Compares per-turn snapshots field by field (positions, headings, energy, gun heat, bullets, events) and reports the first divergence with a diff of both states.
- **Corpus:** the classic sample robots first (bundled, see §13.1), then a growing set of RoboRumble robots (1v1, melee, teams).
- **CI:** the sample corpus runs on every PR, and the large corpus runs nightly.

### 13.1 Bundled classic sample robots

The 27 classic sample robots (`sample`, `sampleex`, `samplesentry`, `sampleteam`; Copyright 2001–2025 Mathew A. Nelson and Robocode contributors) are all EPL-1.0. They are **bundled in the repo and installer under EPL-2.0**, using EPL-1.0 §7, which allows distribution under a later version of the Agreement. The original copyright lines are kept, and each header gets a note that the file was relicensed from EPL-1.0. They serve as test corpus, examples and opponents for new users.

## 14. Future work

- **WASM runner** with fuel metering, for native-language robots (deterministic, no Docker needed).
- **Tank Royale adapter** that translates between the two protocols.
- Shared-memory or local-socket transports for maximum local throughput.
- Classic .NET robot compat layer.

## 15. Repository layout

```
robocode-s/
├── Cargo.toml                # workspace
├── crates/
│   ├── engine/               # pure simulation
│   ├── fdlibm/               # StrictMath port (fdlibm 5.3)
│   ├── java-random/          # java.util.Random port
│   ├── geom/                 # java.awt.geom behavior (boxes, lines, radar arcs)
│   ├── protocol/             # generated + framing
│   ├── server/
│   ├── runner/
│   ├── gui/
│   ├── viewer-wasm/          # engine + renderer for the browser
│   └── cli/
├── rumble/                   # Lambda functions (Rust, cargo-lambda)
│   ├── api/                  # work, results, auth, submit, admin
│   ├── codesize/             # class-file parser for weight classes
│   └── rankings/
├── web/                      # rumble website (SvelteKit, adapter-static)
├── infra/                    # AWS CDK (TypeScript)
├── proto/                    # .proto schema (source of truth)
├── clients/
│   ├── java-compat/          # robocode.jar drop-in (Gradle)
│   ├── java/                 # modern JVM API
│   ├── python/
│   ├── dotnet/
│   ├── rust/                 # (workspace crate)
│   └── c/
├── docker/                   # runner images, relay
├── tools/difftest/
└── docs/
    ├── design.md
    └── adr/                  # architecture decision records
```

## 16. Milestones

| # | Milestone | Exit criteria |
|---|---|---|
| M0 | Bootstrap | Repo, CI (Linux/Windows/macOS), design doc, proto skeleton |
| **M1** | **Headless engine + Java compat (MVP)** | Engine, server, local-process runner, java-compat library, `cli` battles; difftest is bit-exact for all classic sample robots over ≥1000 seeded rounds |
| M2 | GUI | Viewer, battle setup, debug paint, recording and replay |
| M3 | Isolation and distribution | Docker runner images, network relay, warm pools, remote runner agents with TLS and tokens |
| M4 | Polyglot | Python, .NET, Rust, C/C++ and modern Java/Kotlin client libraries plus sample robots |
| M5 | Rumble | Free-tier AWS backend (work/results/auth/submit/rankings), rumble client, seeding import from RoboWiki, website with rankings, reputation-based cross-checks |
| M5b | Web replay | Replay decoder (WASM) and canvas viewer; on-demand replays in the browser |
| M6+ | Extras | WASM runner, Tank Royale adapter, .NET compat |

## 17. Open questions

- **O1** ~~LiteRumble contribution~~ Decided: no; it's a non-goal.
- **O2** ~~Classic version of current rumble clients~~ Answered (2026-10-12): LiteRumble's statistics page allows 1.10.3, 1.11.0 and 1.11.1. Of the changes since 1.10.3, only 1.11.0's fix #89 (a fire command fires at most once) touches the rules; robocode-s follows 1.11.x (§13).
- **O3** ~~Container network isolation~~ Decided: relay container on an internal Docker network (§6.3).
- **O4** ~~Meaning of bit-exact~~ Decided: exact against classic with StrictMath (fdlibm) semantics (§4.3).
- **O5** ~~JRE distribution~~ Decided: a pinned Temurin JRE, downloaded on demand (§6.1).
- **O6** ~~Sample robot licensing~~ Decided: bundled under EPL-2.0. All 27 files were checked and carry the EPL-1.0 header (§13.1).
- **O7** ~~AWS account, region and domain~~ Decided: owner's account, eu-west-1, `rumble.planethofstede.be` (§12.1). The API is served under `/api` on the same domain.
- **O8** ~~Cross-check fraction~~ Decided: reputation-based, verified by volunteers only (§12.4). The exact numbers (start rate, floor, curve) get tuned once real data exists.
- **O9** ~~Recording retention~~ Decided: per-round recordings, 5 rounds won by each side, for matchups involving a top-50 bot; no expiry; the rest on demand (§12.7).
- **O10** ~~Frontend framework~~ Decided: SvelteKit (static) calling the `/api` Lambdas from the browser (§12.1, ADR 0015).
- **O11** Is the daily full scan for Glicko-2/ANPP enough, or should those metrics be updated incrementally? Revisit once the bot count is known.
- **O12** ~~JDK geometry and licensing~~ Decided: independent implementation in `crates/geom`, verified against the JDK with ~24,000 edge cases and 5M random cases, all matching (ADR 0011).
- **O13** ~~`Utils.getRandom()`~~ Decided: a per-robot generator, and the oracle patches classic to match (ADR 0010, engine-spec §15.1).
- **O14** Engine version (§4.4, #22): one number or components (rules, metering calibration, runtime)? And what happens to rumble results when it changes: rerun, keep but rank separately, or mark as stale?

## 18. Decisions so far

| Decision | Choice |
|---|---|
| Compatibility level | Bit-exact with classic (verified by difftest) |
| Sandbox | Pluggable: local process, Docker, remote; WASM later |
| Tank Royale | Independent protocol; adapter later |
| Determinism | Optional deterministic mode |
| Name | robocode-s |
| License | EPL-2.0 |
| Client languages | Java compat + Java/Kotlin, Python, .NET, Rust, C/C++ |
| First milestone | Headless engine + Java compat + difftest |
| Remote robots | Supported (authenticated, TLS) |
| GUI scope | Setup and viewer, recording and replay, rumble client |
| Design doc location | In repo (`docs/design.md`) |
| Classic LiteRumble | Not contributing (non-goal) |
| Bit-exact definition | Against classic with StrictMath/fdlibm semantics |
| Container network | Relay container on an internal Docker network |
| Java runtime (local mode) | Pinned Temurin JRE, downloaded on demand |
| Classic sample robots | Bundled under EPL-2.0 |
| Rumble frontend | SvelteKit, static, with browser-side API calls; also hosts the user documentation |
| Rumble | Own distributed rumble on AWS serverless (Rust Lambdas, DynamoDB, S3, CloudFront) |
| Rumble participants | Web form with GitHub login; seeded from RoboWiki participant lists |
| Robot jars | Mirrored to S3, with a takedown policy |
| Rumble clients | Anyone with GitHub login; server-assigned seeded work; reputation-based verification by other volunteers (new users ~20%, trusted users ~1%) with optimistic acceptance |
| IaC | AWS CDK (TypeScript) |
| AWS deployment | Own member account `robocode-s` (ADR 0016), eu-west-1; `rumble.planethofstede.be` (API under `/api`) in the existing Route 53 zone of the management account |
| Replays | Per-round lossy viewer tracks (ADR 0017): 5 rounds won by each side, for matchups involving at least one top-50 bot; never expire, replaced only by new bots or versions; the rest on demand. About $0.40/month plus about $2.75 one-time |
| AWS cost | Stay within the free tier (S3 aside, which costs cents): AWS never runs battles; Lambda function URLs behind CloudFront (no API Gateway); provisioned DynamoDB at 25/25; capped S3 |
