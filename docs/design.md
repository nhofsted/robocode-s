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
| `runner` | Rust | Launches robot processes: local process, in a sandbox or a Docker container, or on another machine as a runner agent (`robocode-s agent`, ADR 0020). Measures the robot's CPU time. |
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

- The engine has one explicit **engine version**, a single integer: `ENGINE_VERSION` in `crates/engine`. It is separate from the protocol version (§5.1) and from release versions.
- **Bump it with every change that can alter a battle's outcome:** rules and physics, the PRNG, fuel counting and its calibration (including the Java-side constants in `clients/java-determinism`, so every `DETERMINISTIC_RULES` bump), java-compat behaviour, the pinned JRE.
- `DETERMINISTIC_RULES` stays the robot host's compatibility check (the runner refuses a host with other rules). Tests guard the bump rule: one pins `(ENGINE_VERSION, DETERMINISTIC_RULES)` together, another hashes the outcome of a small seeded battle (`crates/engine/tests/fingerprint.rs`) and fails when it changes without a bump.
- It is stored with every result: in replay headers (§11), in rumble result submissions (§12.3), in the rumble data model (§12.6), in CLI battle results and exports, and in difftest reports. Results are only compared or verified within one engine version (§12.4).
- `robocode-s --version` shows it: `robocode-s 0.1.0 (engine 1, protocol 1.1)`.

## 5. Protocol

### 5.1 Encoding and transport

- **Schema:** Protocol Buffers (proto3). It has the broadest code generation coverage: Java, Kotlin, Python, C#, Rust (`prost`), C++ and C (`nanopb`/`upb`).
- **Framing:** a varint length prefix followed by the message, over any byte stream.
- **Transports:** the process's **stdin and stdout** for robots the runner starts (ADR 0019); TCP for observers, remote robots and runner agents, with TLS required when the peer is not on loopback (ADR 0020: a listener off loopback uses TLS, a plain connection is taken only from a loopback peer, `server::net`; TLS 1.3 with rustls and ring; the server's self-signed certificate, made once in the data folder, is pinned by its SHA-256 fingerprint, `--fingerprint sha256:...`, or a CA-issued one is given with `--tls-cert`/`--tls-key` and checked against the Mozilla roots; `--tls` forces TLS on loopback). A robot on pipes still sends `Hello` (role, protocol version) and gets `HelloAck`, but presents no token: the pipe pair is its identity, and its index can't be taken over TCP (`server::run_battle_piped`, `RobotPipe`). The server writes each message and flushes it (lockstep), and reads the robot's stdout on a thread of its own, as for a socket; at the end of the battle it closes the robot's stdin. A pipe has no read timeout, so there is no 10 s `Hello` timeout for it: a robot that never says `Hello` holds a reader thread only, until the runner kills its process, and the battle goes on after its connect timeout without it. A robot's stdout carries only the protocol: the host keeps the pipes to itself (§8).
- **Versioning:** `Hello` / `HelloAck` handshake with protocol major/minor versions and feature flags. Peers check the major version; a minor one adds messages or fields (1.1: the runner agent's, ADR 0020; 1.2: a robot's description in its `Hello`, `RecordingHeader.debug` and `BattleInfo.debug`, #17).

### 5.2 Roles

Every connection starts with `Hello` / `HelloAck` (protocol version, role, token, features) and then has one role:

- **Runner ↔ server** (a runner agent, `robocode-s agent`, ADR 0020; `server::agents`, `runner::agent`, `runner::remote`): the agent says `Hello` with the agent token and `RunnerInfo` (runtimes, mode, engine version, the deterministic rules of its robot host, platform, slots); the server refuses another engine version or other rules. It receives `LaunchRobot` (the robot's index as `robot_id`, its one-time session token, its battle and lookup names, the battle seed, deterministic mode, its heap, and its jars by name, SHA-256 and size), asks for the jars it doesn't have (`JarRequest`; `JarData` in 1 MiB parts, only the battle's, cached by hash), starts the robot with stdio in its sandbox and relays the robot's stdio over a connection of its own to the same listener, as that robot with its token, and reports `LaunchResult`. After the battle `StopRobot` ends the robot and `RobotExited` brings its stderr tail. One battle per connection: the agent connects again for the next.
- **IDE robot ↔ server** (#17, `server::ide`, `runner::ide`): an IDE starts the robot host itself (`RobotHost --ide --server 127.0.0.1:PORT --robot my.Robot --jar <output folder> --home <dir>`, the token in `ROBOCODES_IDE_TOKEN`) so that its debugger works; the robot has no battle yet, so the server can't tell it who it is in `HelloAck`. The host runs classic's repository lookup first and says who it is in its `Hello` (`Hello.robot_description`: classic's unique name with version, class name, type flags, no team, the platform and the deterministic rules of the host: what `--describe` prints for one robot); `HelloAck` then comes when a battle takes the robot, as for any robot. This is its own listener (`IdeListener`: 127.0.0.1 only, port 7878 by default, `--ide-port`), which takes only robots, with a token made once in the data folder (`ide-token`, mode 0600 where there are modes; compared in constant time, a wrong one answered after a delay). It validates the description (a name; one robot, not a team, which needs several JVMs) and keeps the connection (a thread notices when the robot leaves, so a battle isn't offered a robot that is gone). A battle takes a robot from the listener (`take`) and gives the battle loop its connection as a `RobotPipe` whose `Hello` it reads again; the robot is a debug robot (§7.4) and its name is made unique with the others' (`battle_statics`: it comes after the other robots). The host ends with the battle (`BattleOver`): one session is one battle, and the IDE starts it again for the next. `robocode-s battle ...,@ide` waits for the robots (printing `robocode-s ide-config`'s run configuration), and the GUI listens while it is open (§10).
- **Robot ↔ server:** a robot connects with the one-time **session token** from `LaunchRobot` (an agent's relay says the robot's `Hello` with it) and then takes part in the lockstep loop (§5.3).
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
- **Observers from elsewhere** (#36, `server::Observers`, `robocode-s battle --observe ADDR`): a listener of their own beside the robots' one, which stays on loopback, so robot tokens are never reachable from outside. It takes only observers (`Role.OBSERVER`), with the token in their `Hello` when one is set (compared in constant time; a wrong one is answered after a delay), at most 16 at once, each handshake on its own thread with a 10 s timeout. The CLI binds a port alone to 127.0.0.1 and refuses an address that isn't loopback without `--observe-token` (or `$ROBOCODES_OBSERVE_TOKEN`). Off loopback the listener uses TLS (§5.1): the CLI prints the certificate's fingerprint, and the observer pins it (`robocode-s-gui --watch HOST:PORT --fingerprint sha256:...`, the Watch tab's Fingerprint field, saved with the address). A plain client to a TLS listener is told it may want the fingerprint. With `--observe` the battle starts once the first observer connected, so it sees it from the first turn; later observers join where the battle is. Every observer controls the battle (pause, step, speed, paint, stop, kill, input), as Tank Royale's controllers do; an observer that leaves while the battle is paused resumes it, so the command line's own observer isn't held up, and once the last observer from elsewhere left, the battle runs at full speed again.
- **Local security** (PR #48 review; since #67 local robots are on pipes, which no other process reaches, so what follows about robot tokens holds for TCP robots only, and the listener there takes just the embedded observer): the listener is on loopback, but any process on the machine reaches it, another user's too. So in a local battle (`runner::local`) every robot gets a fresh unpredictable token per battle (`server::random_token`: 128 bits from the operating system's randomness), and so does the embedded observer (`ServerConfig.observer_token`), which the robots' listener now requires of observers as well; both are compared in constant time. Nothing else connects there: the GUI's and the CLI's own observer is the embedded one, and observers from elsewhere use their own listener. A robot's token reaches its host in the environment (`ROBOCODES_ROBOT_TOKEN`), not on the command line, which other users can read (`ps`); the observer's never leaves the process. Tokens are neither recorded nor part of the battle, so recordings and outcomes don't change. Without a sandbox (`--sandbox none`) robots run as the same user (`NOSECURITY`, §8), so a malicious robot could still read another robot's environment before that one connected (each token works once) or attack the user's processes in other ways: isolating untrusted robots is the sandbox's job (§6.6). An `--observe` port on loopback without `--observe-token` is the user's choice and is open to every local process; set a token on a shared machine.
- **Debug properties** (#36, classic's `setDebugProperty(key, value)`): an `Execute` brings the properties the robot set since its last one, in order (classic's `ExecCommands.getDebugProperties()`), and the next `RobotSnapshot` carries them as `debug_properties`: changes, as classic's snapshot does, not the current set. An observer keeps the current values; an empty value (classic's `null` or `""`) removes a property, as classic's robot console does. The GUI shows them as a key/value table beside the robot's output.

## 6. Robot hosting (runners)

### 6.1 Launcher modes (pluggable)

| Mode | Use case | Isolation | Notes |
|---|---|---|---|
| **Local process** | Development, fast rumble runs with trusted robots | Low: OS process only. Memory limited through runtime flags (e.g. `-Xmx`); optionally Job Objects on Windows or cgroups on Linux. | The JRE is **downloaded on demand**: on first use, a pinned Eclipse Temurin JRE for the platform is fetched into the robocode-s data dir and its checksum verified. Other runtimes follow the same pattern where possible. |
| **OS sandbox** (ADR 0019) | Default where available; untrusted robots, the rumble | High: no network, filesystem limited to its grants, memory and process limits | Built in: on Linux seccomp everywhere and Landlock where the kernel has it (bubblewrap as an option), on Windows an AppContainer and a Job Object. None on macOS yet. |
| **Container** (ADR 0019) | Untrusted robots, remote agents, macOS | High: memory, pids, read-only rootfs, no capabilities, no network | Optional; requires Docker (Docker Desktop on Windows/macOS). `--sandbox docker`, §6.2. |
| **Remote agent** (ADR 0020) | Distributed tournaments, spreading load, other platforms | The agent's own `--sandbox` | `robocode-s agent`: authenticated with the agent token over TLS; fetches the jars (cached by hash), starts the robots with stdio and relays it to the server. `robocode-s battle --agents ADDR --agent-token T --wait-agents N` assigns the robots round robin to the agents with a free slot (`--slots`); the battle machine still describes them. CLI only, not the GUI's Start Battle yet. |
| *WASM (future)* | Native and new-language robots | High, plus deterministic fuel metering | wasmtime embedded in the runner (§14). |

The runner starts a local robot with `--stdio` and piped stdin and stdout (`Transport::Stdio`; `Transport::Tcp` with a token in the environment remains for robots a server reaches by address) and hands its pipes to the server; stderr is piped too, its tail kept for diagnosis.

### 6.2 Container images

- One image **per runtime**, not one polyglot image. **JVM (ADR 0019, #70):** none of our own: the runtime's official image, `eclipse-temurin:21.0.12.1_1-jre` (the pinned release of §6.1, Ubuntu), by the digest of its multi-arch index (amd64 and arm64). The robot host's `lib`, the agent and the robot's jars are mounted into it from the host (read only), so there is nothing to build, publish or keep in step with the runner. The image is pulled on first use (`docker pull`, in the same step that downloads the JRE for the other sandboxes). Python, .NET and native robots will each get an image the same way, official where there is one.
- **Warm pools:** the runner keeps robot hosts (containers too) started ahead of time, to hide container and JVM startup (a container start is about 0.4 s here; a battle of four robots takes 3 s longer than under `os`): §6.8.

### 6.3 Container network isolation

A robot must reach the server and nothing else. **Decision (ADR 0019):** a robot the runner starts talks to the server over its stdin and stdout, in a sandbox or a container alike, so it needs no network at all: containers run with `--network none`, and the OS sandbox denies every socket but Unix ones. TCP with tokens stays for remote robots and runner agents. (Until ADR 0019 the plan was a relay container on an `--internal` Docker network.)

### 6.4 One process per robot

Each robot, including each team member, gets its own process (and JVM). This keeps CPU accounting, memory limits and crash isolation simple. Team messages travel through the server.

### 6.5 Robot data files

Classic robots get a per-robot data directory with a quota (`getDataFile`, `RobocodeFileOutputStream`, default 200 KB), which classic keeps across battles (a robot such as Sonda reads statistics of earlier ones). The compat library implements the classic API on top of them; the quota is classic's own.

**Decision (#86): copy in, copy out.** The directory a battle's robot uses is in its home (`robots/.data/<package>/<Class>.data`, classic's place, where `ALWAYSUSECACHEFORDATA` sends it), which every sandbox grants it already and which goes when the battle does. The runner keeps the robot's files in the data dir, `robot-data/<lookup name with version>/` (`housekeeping::robot_data_dir`: the name with anything but letters, digits and `.-_ ` replaced by `_`; the contents of the home's `robots/.data`). `RobotLaunch::data` names that folder: `launch` (and, for a warm host, `Warm::assign`) copies it into the home before the robot starts (the robot host's `copyJars` empties `robots/` but for `.data`), and `RobotProcess::finish` copies the home's files back once the process has ended, killed at the end of a battle or not, then removes the home. So no sandbox needs a grant for the persistent folder (no Landlock rule, no AppContainer ACL entry, no mount), and a robot never touches another's files during a battle. Copying skips anything that is not a plain file or folder (a link a robot made to a file only the runner may read), and gives up (keeping the old files, logging why) above 4 MB, far over the quota, which a robot that bypasses the compat library's checks is held to. The copy-out replaces the folder (a copy beside it, then a rename). Two robots of the same class in one battle (or two battles at once) have one folder: they start with the same files and the last to end keeps its files.

Data files make a battle depend on the ones before: that is classic's behaviour, and deterministic mode keeps it (the seed fixes the battle only for the same files). `--fresh-data` / `ROBOCODES_FRESH_DATA=1` (`Environment::fresh_data`; `LocalBattle::data` is then `None`) starts robots with none and keeps none, for reproducibility tests and, later, the rumble; the live difftest does so always. A runner agent keeps its robots' files in its own data dir (`robot-data`, `--fresh-data` too): they are the agent machine's.

### 6.6 Sandboxes (ADR 0019)

`--sandbox` (`robocode-s battle`, the GUI's options) takes `os`, `bwrap`, `docker` or `none` on every platform (`runner::Sandbox`); a value the platform lacks is refused, naming the ones it has. Linux has `os` (the default), `bwrap` (where bubblewrap is installed and can make its namespaces), `docker` and `none`; Windows `os` (the default), `docker` and `none`; macOS `docker` and `none`. `docker` is there where the `docker` program is found and `docker info` answers (asked once, with a timeout; Linux containers only): otherwise it is refused with the reason. `JavaHost` carries the sandbox, so the robot host's `--describe` runs (listings, a battle's description), which load robot classes too, run in it as battles do. A battle reports it: the CLI with its header (`sandbox os (seccomp, Landlock ABI 7)`, on Windows `sandbox os (AppContainer, Job Object)`; `sandbox` in `--json`), the GUI on the loading screen and beside the options' choice.

A sandboxed robot host runs headless (`-Djava.awt.headless=true`: classic's `Container` would reach for the display), with `java.io.tmpdir` in its home and `-XX:-UsePerfData` (no `/tmp/hsperfdata`), and keeps only `PATH`, `HOME`, `LANG`, `LANGUAGE`, `TZ` and `LC_*` of the environment (the rest may hold tokens); on Windows, whatever their case, also `SystemRoot`, `windir`, `SystemDrive`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE` and `LOCALAPPDATA` (`CreateProcessW` needs it for an AppContainer: error 203 without), with `PATH` the runtime's `bin`, `System32` and the Windows folder, and `TEMP`/`TMP` the home's `tmp`. It talks over stdio; a TCP launch in a sandbox is refused. A sandboxed `--describe` run gets a 1 GB heap (`-Xmx1024m`), so that its memory is bounded as a robot's.

**Linux `os`**, in layers. Everything is prepared before the fork (the seccomp program, the Landlock ruleset with its paths opened); the child only makes system calls.
- *No new privileges*, and *rlimits*: no core dumps, files up to 64 MB (classic's data quota is 200 KB), 4096 open files. No `RLIMIT_NPROC`: it counts all the user's processes and threads, so robots would fail on a busy machine. No `RLIMIT_AS`: the JVM reserves far more than it uses; `-Xmx` bounds the heap.
- *seccomp*, allowing what it doesn't name (the JVM's set is broad). Another architecture's system calls (32-bit, x32) kill the process. Denied with an error, so Java gets an exception: `socket` but for `AF_UNIX` stream sockets (`EAFNOSUPPORT`, `EACCES`), `socketpair` but for `AF_UNIX`, and `connect` (`EPERM`: it would reach the user's D-Bus or Docker socket), so no network; `clone` with namespace flags, `unshare`, `setns`; `ptrace`, `process_vm_readv`/`writev`, `kcmp`, `pidfd_getfd`; mounts (both APIs), `pivot_root`, `chroot`; `bpf`, `perf_event_open`, `userfaultfd`, `io_uring_*`; keys, modules, `kexec_*`, `reboot`, swap, `acct`, quotas, `syslog`, `vhangup`, `fanotify_init`, file handles, setting the clock or host name, `iopl`/`ioperm`/`modify_ldt`/`uselib` (x86_64) and personalities but the default (`EPERM`); `clone3` (`ENOSYS`: its flags can't be checked, and glibc falls back to `clone`); the `TIOCSTI` and `TIOCLINUX` ioctls (typing into the terminal); signals (`kill`, `tgkill`, `rt_sigqueueinfo`, `rt_tgsigqueueinfo`) but to the process itself, the filter getting its id in the child, and `tkill`, `pidfd_send_signal`. x86_64 and aarch64; other processors have no `os`.
- *Landlock*, where the kernel has it, at the newest ABI it offers (the rights up to ABI 7 that it knows); without it the sandbox still runs and reports `filesystem not limited: no Landlock`. Granted: read and execute the Java runtime (above the real `java`'s `bin`), and read what its links lead to (a distribution's Java has its settings in `/etc`); execute the ELF interpreter `java` names (the kernel opens it to start a program); read the robot host's `lib`, the agent (deterministic battles) and the robot's jars and class folders; read and write, but not execute, the robot's home (its data files, `tmp`); read `/usr`, `/lib`, `/lib64`, `/etc/ld.so.cache`, `/etc/localtime`, `/etc/timezone`, `/etc/nsswitch.conf`, `/etc/passwd`, `/proc`, `/sys/fs/cgroup`, `/sys/devices/system/cpu`, `/dev/random` (without it `SecureRandom` seeds itself from thread timings, for seconds) and `/dev/urandom`, and write `/dev/null`. Nothing else: not the user's home or the repository, though the runtime and robot host may be in them (only their own folders are granted). From ABI 4 TCP bind and connect are denied, from ABI 6 abstract Unix sockets and signals outside the sandbox. Landlock also keeps other processes' private `/proc` files from the robot, as `ptrace`. A robot can start the ELF interpreter on a program of its own; it runs in the same sandbox.

**Linux `bwrap`:** bubblewrap with `--unshare-all` (no network, its own processes), `--die-with-parent` and `--new-session`; the grants and system paths above bound read-only, the home writable, `--proc`, `--dev` and a tmpfs `/tmp`; the rlimits, and the seccomp filter by `--seccomp` (from a memory file), which bwrap installs just before it starts `java`, so its own mounts aren't filtered (without the signal rules: it has a process namespace). Not Landlock: a Landlock domain may not mount. `/usr`'s programs can run in it. bwrap is tried once (`bwrap --unshare-all --ro-bind / / true`); missing, or without user namespaces (Ubuntu's AppArmor restricts them), it is refused with the reason.

The tests (`sandbox::linux::tests`) start the test binary again in each sandbox, which tries what a robot shouldn't: no TCP or UDP socket, Unix datagram socket or `connect`, but a Unix socket pair; reading and writing outside the grants and running `/bin/sh` fail with Landlock (and work without it), writing the home works; another process's `/proc` files, `ptrace`, `unshare` and signals fail; `clone3` gives `ENOSYS`; no core dumps.

**Windows `os`** (`sandbox::windows`): `Command` can't set an AppContainer's attributes, so the runner starts the robot host itself (`CreateProcessW` with a `STARTUPINFOEXW`), with the arguments, environment and directory put on a `Command`; it returns its pipes as `File`s and kills and waits for it as `std::process::Child` does.
- *AppContainer* `robocode-s.robot`: its profile is made once (`CreateAppContainerProfile`, else `DeriveAppContainerSidFromAppContainerName`) and kept; it fails only where there are no AppContainers, and `os` is then refused with the reason. The process is a less privileged AppContainer (`PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY`, opted out of what is granted to "all application packages", much of the system's files and registry: as one of them a robot could also listen and connect on loopback within the AppContainer, to the other robots, and send UDP). Its capabilities: no network one (no `internetClient`, no `privateNetworkClientServer`, so no sockets at all, loopback included: `WSAEACCES`); `registryRead` and `lpacCryptoServices`, made for less privileged AppContainers (without the first `jvm.dll` fails to load with "access denied" and Winsock doesn't start; without the second Java's `SecureRandom` seeds itself from thread timings, 7.5 s per JVM); and a capability of the robot's home's own, derived from its path (`DeriveCapabilitySidsFromName("robocode-s.home:<path>")`), to which only that home is granted, so robots don't reach each other's homes though they share the AppContainer. `registryRead` also opens a few of the user's keys, among them `HKCU\Environment` (the user's own environment variables; a deny entry for the AppContainer or the capability there doesn't take) and the key names in `HKCU\Software` (the applications' own keys stay closed). `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` inherits exactly the three stdio handles (the pipes, or the null device); `PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY` (`PROCESS_CREATION_CHILD_PROCESS_RESTRICTED`) refuses it processes of its own (`ERROR_CHILD_PROCESS_BLOCKED`). It is started `DETACHED_PROCESS`, without a console: a console is a second process (conhost), which the child process policy and the job refuse (the process dies with `STATUS_DLL_INIT_FAILED`).
- *ACL grants* to the AppContainer's SID, inheritable entries added once per folder (each is checked first, so a grant again changes nothing; `SetNamedSecurityInfoW` gives what the folder holds the inherited entries): read and execute (`FILE_GENERIC_READ | FILE_GENERIC_EXECUTE`, object and container inherit) the Java runtime; read (`FILE_GENERIC_READ`) the robot host's `lib`, the agent and the robot's jars and class folders. To the home's capability: the home and its folders all rights (`FILE_ALL_ACCESS`, container inherit: a folder's execute is going through it), its files all but execute (`FILE_ALL_ACCESS & !FILE_EXECUTE`, object inherit, inherit only). The runtime's and the robot host's entries stay (they grant only reading to robocode-s's own AppContainer); the homes go with the battle's folder. Grants need NTFS: on FAT or a WSL (`\\wsl$`, 9P) path they fail and the robot isn't started. Nothing else of the user's is reachable: an AppContainer reaches only what names it, its capabilities, or (opted out here) all application packages; the user's profile, other folders and other processes are denied (`ERROR_ACCESS_DENIED`). No integrity label is needed on the home: the low integrity AppContainer writes there with the grant alone.
- *Job Object*, one per robot, the process suspended until it is in it: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (the robot dies when the runner closes the job or exits), `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` 1, `JOB_OBJECT_LIMIT_PROCESS_MEMORY` of the heap plus 512 MB (the JVM's metaspace, code, threads and GC structures), `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` (no error dialog), and `JOB_OBJECT_UILIMIT_ALL` (no other processes' windows, clipboard, global atoms, desktops, display or system settings, logging off). No CPU limit, as on Linux: the turn budgets bound a robot's time.

The tests (`sandbox::windows::tests`) start a copy of the test binary in the sandbox, which tries what a robot shouldn't: TCP to the runner's loopback listener and to an outside address, listening (and connecting to itself), UDP to the runner all fail (`WSAEACCES`, nothing arrives); listing the user's profile, reading and writing a folder in the user's temporary folder fail, writing the home and its `tmp` works; starting a process (`whoami`, or itself) fails; opening the runner's process, the user's `Run` key and the clipboard fail; the job lets it commit its heap but not 512 MB more; and closing the job kills it. A deterministic battle under `os` gives the results it gives under `none` (`tests/sandbox.rs`, ignored: needs Java and the robots).

**Docker** (`sandbox::docker`): `docker run --interactive --rm --init --name robocode-s-<random> --label robocode-s=1 --network none --read-only --tmpfs /tmp:rw,noexec,nosuid,size=64m --cap-drop ALL --security-opt no-new-privileges --pids-limit 512 --memory <heap+512>m --memory-swap <same> --ulimit core=0 --ulimit nofile=4096:4096 --ulimit fsize=67108864 --log-driver none --user <uid>:<gid> --workdir /home/robot/ --env HOME=/home/robot/ --mount type=bind,source=<lib>,target=/opt/robocode-s/lib/,readonly [--mount ...agent and jars, /opt/robocode-s/read/<n>/<name>] --mount type=bind,source=<home>,target=/home/robot/ <image> java <JVM flags> ... --stdio`, and Docker's default seccomp profile. The JVM arguments are the other sandboxes' with the container's paths (`runner::container_view`), including the headless, temporary-directory and no-perf-data flags; the environment is `HOME` only (and nothing of the runner's: `docker` runs with the runner's own environment, the container with the `--env`s). `--mount` rather than `-v` because a Windows path has a colon (Docker Desktop takes `C:\...` paths). The user is the owner of the robot's home on Linux, so what the robot writes there is the user's; elsewhere a fixed `10001:10001` (Docker Desktop shows mounted files to any user). The sandboxed `--describe` runs go through the same container. A container's `docker` program is killed with `docker kill <name>` first (the container is not its child), and the container's stdin is the robot's pipe, so a container whose runner is gone stops when it reads the end of it; the stopped containers of earlier runners (label `robocode-s=1`, older than 10 minutes) are pruned when the image is prepared. The report is `docker (eclipse-temurin 21.0.12.1, no network)`.

The tests (`sandbox::docker::tests`, the real-Docker ones ignored) check the arguments (Linux and Windows path forms, read-only and writable mounts, the refusal texts, the image against `jre::RELEASE`); in a container the JRE reports the pinned release, `/sys/class/net` has only `lo`, the root and mounts are read-only, there are no capabilities, the home is writable with the user's ownership, and the container is gone after its end and after a kill. `tests/docker.rs` (ignored) fights a deterministic battle in `docker`, `os` and `none` with the same results, and leaves no container. `ROBOCODES_SANDBOX=docker live` (difftest) replays recordings identically. Windows and macOS (Docker Desktop) are untested for real: only the arguments are.

### 6.7 Installed layout and release archives (#71)

A release is one archive per platform (`robocode-s-<version>-<platform>`: Windows x64 `.zip`; Linux x64 and arm64, macOS arm64 and x64 `.tar.gz`; macOS is built on a Mac, see `tools/release/release.sh`), holding the programs and what they need:

```
robocode-s(.exe)  robocode-s-gui(.exe)
lib/robot-host/       installDist's lib: classic's jars and ours, as separate files (classic finds its modules by jar name)
lib/agent/java-determinism.jar
robots/               the sample robots' jar
share/                Linux: robocode-s.desktop, the icon, install-desktop.sh
LICENSE  THIRD-PARTY-NOTICES.txt  README.txt
```

The sounds and images are built into the GUI. `runner::local::install_dir()` is the one place that knows the layout (constants `INSTALLED_*`): the folder of the executable, its symlinks resolved (so Linux packages can put the tree in `/usr/lib/robocode-s/` and link `/usr/bin/robocode-s*`), or, in a macOS app bundle (`Contents/MacOS/`), `Contents/Resources`; it counts when `lib/robot-host` and `lib/agent` are there. `Environment::from_env` then uses these paths and the default robot folder (`robots`, also the GUI's default robot folder), unless the `ROBOCODES_*` variables or options name others; in a source tree (nothing of the layout next to the executable) the paths stay relative to the current folder, as the repository builds them. The data folder (the JRE, the robots' data files, battle homes) is then the user's, never inside the installation: `$XDG_DATA_HOME/robocode-s` (else `~/.local/share/robocode-s`) on Linux, `~/Library/Application Support/robocode-s` on macOS, `%LOCALAPPDATA%\robocode-s` on Windows (`runner::local::user_data_dir`); in a source tree it is `target/robocodes-data`. The sandboxes need nothing beyond absolute paths: Landlock grants and Docker mounts take the paths they are given (the host lib, the agent, the robots, the JRE), and the Windows ACL grants need an NTFS folder the user can change the security of (a folder the user owns, such as an archive unpacked on a drive or `%LOCALAPPDATA%\Programs`; an installation in `Program Files` needs the installer to grant the AppContainer `robocode-s.robot` read and execute there, since a user can't).

`tools/release/release.sh` is the release gate and builder, as there is no CI (checks, builds, archives, packages, `SHA256SUMS`, `gh release create`); `tools/release/notices.py` writes `THIRD-PARTY-NOTICES.txt` from `cargo metadata` and the license texts in `tools/release/licenses`. Linux binaries are built with `cargo-zigbuild` for glibc 2.31, so they run on Ubuntu 20.04, Debian 11 and later. The programs are unsigned (Windows SmartScreen, macOS Gatekeeper: the README says how to get past them); code signing is postponed indefinitely (owner decision, #78), as are Snap and Flatpak.

#### Packages (#78)

`tools/release/package.py` (Python standard library only) makes the packages from the folder `assemble` has filled for the archive, so that a package holds exactly the archive's files, and `release.sh` adds them to `target/release-dist/<version>/` and `SHA256SUMS` (the names carry the platform as the archives' do):

- **Linux `.deb` and `.rpm`** (`robocode-s-<v>-linux-<arch>.deb|rpm`, x86_64 and aarch64, from the same zigbuild binaries): `/usr/lib/robocode-s/` is the archive's tree without `share/` and the documents (the programs, `lib/robot-host`, `lib/agent`, `robots`), `/usr/bin/robocode-s` and `robocode-s-gui` are symlinks into it (`install_dir()` resolves them), `/usr/share/applications/robocode-s.desktop` (`Exec=robocode-s-gui %f`, `MimeType` for the two types below), the 256-pixel icon in `hicolor/256x256/apps`, `/usr/share/mime/packages/robocode-s.xml` (`application/x-robocode-s-recording`: `*.rsr`, `*.rec`; `application/x-robocode-s-track`: `*.track`) and `/usr/share/doc/robocode-s/` (a package `README`, `LICENSE`, `THIRD-PARTY-NOTICES.txt`; the `.deb` also `copyright` in Debian's machine-readable format, which points at the notices, and a `changelog.Debian.gz`). The only hard dependency is the C library (`libc6 (>= 2.31)`, `glibc >= 2.31`): the binaries are linked against nothing else (zig links the Rust runtime statically). The GUI `dlopen`s libvulkan, libEGL, libX11 (and -xcb, libxcb, libXcursor, libXi), libxkbcommon (and -x11) and libwayland-client and -egl (winit, wgpu), so these are `Recommends` (package names for Debian and Fedora in `package.py`), the command line works without them. There are no maintainer scripts: the desktop, MIME and icon caches are refreshed by the distributions' triggers. The `.deb` is built with `dpkg-deb` (xz). The `.rpm` is written by `package.py` itself (lead, signature header with the digests, header with a region tag, SHA-256 file digests, a `newc` cpio payload compressed with xz), because `rpmbuild` isn't installed on the build machine and `cargo-generate-rpm` can't package symlinks; it is not signed, and `rpm -K` reports only digests (`dnf install ./file.rpm` doesn't check signatures of local files by default). It lists directories it owns and the documents as `%doc`/`%license`.
- **Windows installer** (`robocode-s-<v>-windows-x86_64-setup.exe`, NSIS 3, `installer.nsi`; `makensis` is `apt install nsis`, or the Windows zip of NSIS unpacked in `~/.local/opt/nsis-3.11` and run through WSL interop, which needs no administrator). Per user (`RequestExecutionLevel user`): `%LOCALAPPDATA%\Programs\robocode-s`, so that the AppContainer ACL grants of the `os` sandbox work (design §6.6) as they do from an unpacked archive. Components: the files (always), a Start menu shortcut (on), a desktop shortcut (off), the folder on the user `PATH` (on) and the file types (on); `/S` is silent, `/NOPATH`, `/NOASSOC`, `/NOSTARTMENU`, `/DESKTOP`, `/D=` adjust it. The PATH value is read and written as `REG_EXPAND_SZ` without expanding (the user's `%VARIABLE%` entries stay); a PATH longer than NSIS's 1024-character strings is left alone with a message; the uninstaller removes the entry only if the installer added it. Registry: `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\robocode-s` (name, version, publisher, icon, install location, quiet uninstall), `HKCU\Software\robocode-s` (install folder, what was added), `HKCU\Software\Classes` (`robocode-s.recording` and `robocode-s.track` with their icons and `open` commands, `.rsr` and `.track` pointing at them, `.rec` only under `OpenWithProgids`: it is too common an extension to take over). The installer remembers the folder, so installing again upgrades in place: it first runs the old uninstaller silently (which removes exactly the files of that version, so renamed jars don't linger, and leaves what the user added to `robots\`), then installs. The uninstaller does not touch the data folder (the Java runtime) or the settings. File lists (`File`, `Delete`, `RMDir` for folders it made, deepest first, never recursive) are generated from the archive tree, so the installer, the uninstaller and the zip can't differ.
- **macOS** (`robocode-s-<v>-macos-<arch>.app.zip`, and a `.dmg` only when `release.sh` runs on a Mac, `hdiutil create -format UDZO`): `robocode-s.app` with `Contents/MacOS/{robocode-s-gui,robocode-s}` (`CFBundleExecutable` is the GUI), the rest in `Contents/Resources` (where `install_dir()` finds `lib/robot-host`, `lib/agent` and `robots`), `Info.plist` (bundle id `io.github.nhofsted.robocode-s`, written with `plistlib`; document types for `rsr`, `rec` and `track`; minimum system 11.0 on arm64, 10.13 on x86_64) and `robocode-s.icns` (the 256-pixel PNG as `ic08` and `ic13`; no resizing library is needed). A bundle needs the macOS binaries, which need the macOS SDK (see above), so on Linux the step is skipped with a message. Unsigned and unnotarized: Gatekeeper refuses a downloaded copy until quarantine is removed. winit doesn't deliver Apple's open-document event, so a double-clicked recording starts the window without opening it (opening from the window or dropping it works; an argument on the command line works as everywhere). `release.sh cask VERSION` prints a Homebrew cask (`robocode-s.rb.in`) for the zips, with the `bin` link for `robocode-s` and an `xattr` post-flight; the tap that would hold it isn't created.

`release.sh smoke` installs each kind where it can: a `.deb`/`.rpm` into containers (`debian:12`, `ubuntu:24.04`; `fedora:41`, `rockylinux:9`) with a battle under `none` and `os`, then removes it; the installer through WSL interop (silent install, a battle under `os` from the installed folder, silent uninstall, a check that files, registry keys and the PATH value are as before). A package for another architecture than the machine's is only inspected.

The GUI saves its robot folders in `gui.json` only when they differ from the default (the installation's `robots`), so a moved or upgraded installation, or one by another route, follows its own folder instead of a stale absolute path.

#### What is removed, and when (#86)

- **A battle's folder** (`battles/<stamp>-<n>-<pid>`: the description's home, the robots' homes) when the battle is over, after the robots' data files are copied out and their error output is read (`run_local_battle_with`; a runner agent removes its own at the end of each battle, and a battle prepared and not fought, as when the GUI's start is cancelled, `LocalBattle::remove_dir`). Recordings and exported results are never in it: they go where `--record`, the GUI's "Record to" and its save dialogs say. The CLI used to write `output-<i>.txt` and `stderr-<i>.txt` into the folder; it now prints a robot's error output after the results, and writes the files (in a folder that stays) only with `ROBOCODES_KEEP_BATTLES`.
- **A warm host's home** (`pool/<pid>/robot-N`) when its process has ended (`RobotProcess::finish`, also when it is dropped), and an idle host's when the pool drops it. The pool's folder goes with the last of them.
- **What a crash or a kill left:** every battle and listing starts with `housekeeping::remove_stale_runs` over `battles` and `pool` (an agent over its own): a folder named after a process (`<pid>`, `<stamp>-<n>-<pid>`) that no longer runs is removed whatever its age, one of a running process (another GUI or CLI at the same time) is kept, and one with no process in its name that is more than a day old is removed. (Listing homes, `list/<pid>-<n>`, still go after an hour.) A reused process id keeps its dead predecessor's folder until the process ends.
- **`ROBOCODES_KEEP_BATTLES=1`** keeps the battle folders and homes, for diagnosis; the robots' data files are kept in any case.
- **Windows:** the AppContainer's ACL entries sit on the home itself (design §6.6), so they go with it. Files the robot creates belong to the user, who deletes them; a deletion is retried for a second, as the process's handles may outlive it briefly.

### 6.9 Update channels (#79)

Installed copies must follow the engine version the rumble requires, so each platform's usual updater gets a channel. Each publication is outside this repository and is started by the owner, one at a time: `tools/release/release.sh publish VERSION` lists them in order and asks y/N before each (`--dry-run` shows them without doing any). Everything is prepared locally and the tools print what they run; publishing is the last step (`--for-real` in each tool).

- **apt and dnf repositories** (`tools/release/repo.py`), at `https://rumble.planethofstede.be/packages/` (an S3 bucket behind the rumble's CloudFront, §12.1). `repo.py build VERSION` adds the `.deb` and `.rpm` of `target/release-dist/VERSION` to a local tree (`target/package-repo`, synced down from the bucket first with `--pull`, so earlier versions stay) and writes the metadata itself (no `dpkg-scanpackages` or `createrepo`): `apt/dists/stable/main/binary-{amd64,arm64}/Packages(.gz)`, `Release`, `InRelease`, `Release.gpg`, the pool `apt/pool/main/r/robocode-s/robocode-s_<v>_<arch>.deb`; `rpm/{x86_64,aarch64}/` with `repodata` (`repomd.xml` and its `.asc`, primary, filelists, other, read from the `.rpm` headers); `robocode-s.asc` and `robocode-s.gpg` (the public key, armored and dearmored for apt's `signed-by`), `robocode-s.repo`, `robocode-s.sources`. Both repositories are signed with a dedicated key of the owner (`ROBOCODES_GPG_KEY`; `repo.py keygen` prints the commands: RSA 4096, no expiry, backed up off the machine). RSA, because rpm before 4.19 (RHEL 9, Rocky 9) can't verify ed25519. The `.rpm` files `package.py` writes are unsigned, and dnf with `gpgcheck=1` refuses those, so `build` adds the header and payload signatures (`RPMSIGTAG_RSA`, `RPMSIGTAG_PGP`) to the copy in the repository; users get `gpgcheck=1` and `repo_gpgcheck=1`. A `.deb` is checked through the signed `Packages` hashes, as in any apt repository. `upload` runs `aws s3 sync` and `cp` with the AWS profile `robocode-s` (ADR 0016): packages first (`Cache-Control: immutable`), then the metadata (`max-age=300`), the index files last, then a CloudFront invalidation of the index files; without `--for-real` it only prints the commands. Tested in `debian:12`, `ubuntu:24.04`, `fedora:41` and `rockylinux:9`: install from a repository served over HTTP with a throwaway key, then a newer version added to it and upgraded.
- **winget** (`tools/release/winget.py VERSION`): the three manifest files of `NickHofstede.robocode-s` (schema 1.6.0) in `target/release-dist/VERSION/winget/`: the NSIS setup from the GitHub release, `InstallerType: nullsoft`, `Scope: user`, silent switch `/S`, the checksum from `SHA256SUMS`, `ProductCode: robocode-s` (the installer's uninstall key, `installer.nsi`), EPL-2.0. Checked locally with `winget validate --manifest`. `--submit --for-real` opens the pull request to `microsoft/winget-pkgs` from the owner's fork with `gh`; Microsoft's validation (it installs the setup, with SmartScreen and antivirus checks on the unsigned installer) may ask for changes.
- **Homebrew tap** (`tools/release/tap.sh VERSION`): `target/homebrew-tap` with `Casks/robocode-s.rb` (from `release.sh cask`, so it needs the `.app.zip` files built on a Mac) and a README, prepared as a git checkout; `--for-real` creates `nhofsted/homebrew-robocode-s` and pushes. macOS only.
- Not offered: Snap and Flatpak (confinement conflicts with the robot sandboxes and Docker, ADR 0019).

### 6.8 Warm pools (#82)

A robot host takes about 0.5 to 1 s to start (JVM, classic's container and engine), a container 0.4 s more. `runner::pool` keeps hosts started ahead, each waiting on its stdin for the robot it will play, so that a battle only waits for the robot's own start-up.

- **Warm mode:** `RobotHost --stdio --warm --home DIR` takes stdio as before, starts classic's engine on its home (its settings, container and modules; the determinism agent and the deterministic JVM flags are those of the command line, as for a cold host) and then waits for a `RobotAssignment` (proto, written length-delimited before any `Hello`): the robot's lookup name, the seed, the deterministic flag (checked against how the host was started) and its jars, as paths inside the home. It moves the jars to `robots/` and goes on as a cold host does: `Hello`, `HelloAck`, then `RobotRandomness.configure` (the per-robot streams and identity hashes, ADR 0009 and 0013, depend on the name and the seed from `HelloAck` and nothing that ran before: the hooks only act on robot threads), classic's repository lookup (a repository reads the robots when asked for the robot, so the jars may come after the engine) and the proxy. Closing the pipe first ends the host. A `--stdio` host without `--warm` is unchanged.
- **Grants:** sandbox grants are fixed when the process starts (Landlock rules, the AppContainer's ACL entries, Docker's mounts), so a warm host has a home of its own (`robot-N` in the pool's folder), which every sandbox grants it for reading and writing, and the runner copies the robot's jars into its `jars/` and its data files into `robots/.data` (§6.5) before the assignment: no extra grant. The home is removed when the host's process ends, as a cold robot's is. They are copies, not hard links: the robot may write in its home, and a link would let it change the original. Class folders (read in place) are not granted to a warm host, so such robots start cold.
- **Keys:** a host can't change what it was started with, so a pool keeps hosts for one `pool::Key` at a time: sandbox, `java`, the robot host's `lib`, the agent (deterministic mode only), the heap (`memory_mb`) and the mode. Asking for another key drops the rest (idle JVMs don't pile up). A host plays one robot and is never reused (classic's statics, loaded classes and the JIT's history belong to that robot); a launch with no fitting live host starts cold, as before.
- **Filling:** starting a host is a process spawn, so it needs no thread. `Pool::warm(host, memory, mode, n)` starts up to `n` and remembers `n`; `Pool::refill` starts what was taken. A local battle refills as soon as its robots are launched, so the next battle's hosts start while this one runs; in classic mode (robots' turns are timed) after it, so as not to take its CPU. `Pool::no_refill` for a single battle.
- **Who uses it:** `robocode-s battle` warms one host per robot (`--warm N`, 0 disables) while it describes the robots, which starts their JVMs meanwhile, and keeps no more; the GUI keeps as many as the last battle had robots (at most 2, the option "Warm robots", on by default) between battles and drops them with the option, a sandbox change and at exit; `robocode-s agent` keeps `--warm N` (default `--slots`; none without slots) and refills when a battle ends. A killed runner leaves no host behind: they end when their stdin closes (a container too).
- **Cost:** an idle warm host holds about 92 MB resident (a JVM with classic's engine loaded, 68 MB of it private; independent of `-Xmx`, which is only a limit).
- **Measured** (this machine, 24 cores; medians; sample robots): battle start to first turn in a rumble-like loop of 10 1v1 battles of a known description, 0.87 s cold and 0.47 s warm under `os`, 0.92 and 0.44 s without a sandbox, 1.21 and 0.43 s under `docker` (10 battles: 13.0 s and 9.0 s under `os`, 21.9 s and 14.7 s under `docker`). The CLI's single battle of 4 robots, 3 rounds: 3.1 s and 2.7 s under `os`, 6.8 s and 6.0 s under `docker`, about 3.4 s and 3.2 s without a sandbox: only the overlap with the description (about a second) is won, and a robot's own start (finding it in its jars: about 0.4 s for the sample robots' jar, whose repository scan can't be done before the robot is known) remains.

## 7. Time budgets and determinism

### 7.1 Classic mode (default)

- Each robot gets a per-turn CPU budget derived from a "CPU constant", calibrated on the host like classic Robocode does.
- The budget is measured in **CPU time of the robot's thread or process**, measured by the runner or the trusted client library (e.g. `ThreadMXBean` for the JVM). Wall-clock time is not used, so network latency and container scheduling don't count against the robot.
- The server also applies a generous **wall-clock deadline** per turn, as a liveness guard.
- Docker CPU quotas (`--cpus`) are coarse (100 ms CFS periods). They're used only as a safety ceiling, not for fairness. Use `--cpuset-cpus` where available.

### 7.2 Deterministic mode (optional)

- There are no skipped turns based on time: the server always waits for every robot's intent, up to a hard limit. A robot that exceeds the hard limit is **disabled**. Only that pathological case can make results depend on timing.
- Robots skip turns by their **counted work** instead (ADR 0018): a robot whose work in a turn exceeds the budget (3.2 ms of virtual time, ADR 0014: how long classic really waits for a robot) answers as many turns late as classic would have run without it, with classic's skipped-turn events and disabling. The runner refuses deterministic battles with a robot host that follows other deterministic rules (an outdated build).
- All engine randomness is seeded.
- Robots' own randomness is seeded **per robot**: `Math.random()` and `new Random()` draw from streams derived from the battle seed and the robot's name (ADR 0009). For JVM robots, a Java agent (`clients/java-determinism`) does this. Classic itself can't: its `RANDOMSEED` only seeds `Math.random()` on old JVMs, as a single stream shared by all robots.
- Robot JVMs run with the inexact math intrinsics disabled (`-XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow`), so robots' own trigonometry is identical on every platform (§4.3). Otherwise volunteers on different hardware could get different results for the same seed.
- The determinism agent also gives robot objects deterministic identity hash codes, so robots iterating over identity-keyed hash collections behave the same in every run (ADR 0013).
- It counts the work of robot code and gives it a virtual clock, so robots that read the time behave the same everywhere (ADR 0014).
- Known limit: robots that read the clock to budget their own computation can still behave differently between runs.
- Under WASM, budgets become instruction fuel, which makes even the time limits deterministic.

### 7.3 Trust

Robot-side CPU measurement is only trusted when the runner is trusted: a local runner, or a remote runner operated by the tournament organizer. Untrusted remote runners only get wall-clock budgets.

A runner agent that presents the agent token (ADR 0020) is the organizer's: its robot hosts' measurements count as a local robot's (counted work decides skipped turns in deterministic mode; CPU times go to observers). In classic mode the turn deadline is the server's wall clock, so the network's round trip counts against an agent's robots. There is no untrusted-agent mode yet; the rumble's volunteers (§12.4) run whole battles instead.

### 7.4 Debug robots (#17)

A robot started by an IDE's debugger can sit at a breakpoint for minutes, so the server never times it (`RobotEntry.debug`, `Link.debug`): there is no connect timeout (the battle waits for it as long as it takes, or until it connects or the battle is stopped), no turn deadline in classic mode (the wait for a turn's `Execute`s goes on past the deadline for debug robots alone), no hard limit in deterministic mode (a debug robot is never disabled), no skipped turns by counted work (its `Execute` counts as on time whatever its work, `late_turns` isn't asked), and the robot host doesn't limit stopping at a round's end either (classic's `waitForStopThread`, which interrupts the robot's thread and stops it by force after a few seconds, waits for the thread as long as it takes). The other robots wait in lockstep: the server waits for the debug robot's `Execute` and the others' hosts wait for their next `Tick`, neither of which has a timeout, and the observers' pacing only runs between turns, so a breakpoint pauses the whole battle. A stop from an observer (`abort`) doesn't cut the wait for a debug robot short either; ending the robot's process does (the battle sees it disconnect). The only stopwatch left is a remote observer's write timeout, which counts a blocked write, not silence. A battle with a debug robot is a **debug battle**: `BattleInfo.debug` and `RecordingHeader.debug` say so, the CLI prints it in its results, and the GUI marks the battle screen and the replay. Its results aren't comparable with real ones, since the robot ran outside the sandbox with its own JVM flags. A robot from the IDE doesn't change the other robots' results in deterministic mode, which depend on their counted work only.

## 8. Java compatibility library (`clients/java-compat`)

The **robot host** runs classic Java robots unchanged (ADR 0012). It does not reimplement the `robocode.*` API. Instead it runs **classic Robocode's own robot side** (`robocode.api`, `core`, `host`, `repository`, `battle` 1.11.1 from Maven Central, EPL): the robot proxies, event manager, blocking calls, custom events, team messages, data files and painting. Only the **peer** is replaced:

- `RemotePeer` implements classic's `IRobotPeer`. Each `execute` becomes an `Execute` message; the next `Tick` becomes classic's `ExecResults`. Dead or halted robots get classic's `DeathException`, `WinException` or `AbortedException` after sending their final commands, exactly like classic's `RobotPeer.executeImpl`.
- Crashed robots (`drainEnergy`) and misbehaving ones (`punishBadBehavior`) are reported in `Execute`.
- The peer measures the robot thread's CPU time per turn (`ThreadMXBean`).
- **Same name, same object:** in classic every event carries the very same `String` instance for a robot's name, the robot's own name included, and rumble bots compare names with `==`. The host keeps one instance per name (not `intern()`: classic's names aren't interned).
- **A robot whose code ends** (crash, robot-side disable, `run()` returning) sends one final `Execute` with its current commands, `drain_energy` and `stopped`. That matches classic's `waitForBattleEndImpl`, which stores its commands once more and then only re-sends the server's copy.
- **Kotlin:** like classic 1.11.1, the host has `kotlin-stdlib` 2.4.0 on its class path, for robots written in Kotlin.
- **Transport:** with `--stdio` the host takes the process's stdin and stdout for the protocol (buffered, flushed after every message), then points `System.out` at stderr and `System.in` at an empty stream, so that classic's logging, a robot's stray `System.out` outside classic's robot streams and the JVM can't write into it; `--server` connects over TCP with a token. `--describe` is unchanged (it prints to stdout and isn't a battle).
- `RobotHost` (the main class) connects, receives its identity in `HelloAck`, configures the robot's random streams (ADR 0009, 0010), looks up the robot in its jar through classic's repository, creates classic's proxy and then drives rounds from the server's messages.
- **Class folders** (#36): a `--jar` may name a folder of class files (an IDE's output). The host makes it one of classic's development paths (`robocode.options.development.path` in the home's `robocode.properties`), so classic reads it in place and names its robots as development versions (`my.Robot*`, `my.Robot 1.0*`; not `sample.*` and `tested.*`). Every battle starts fresh JVMs in a new home, so a recompiled robot is new in the next battle; the home's `robots` folder is emptied first, so a reused home keeps no old jars or robot database. Each listing of the robots (the GUI's) has a home of its own, `data/list/<pid>-<n>`, removed afterwards (one a crash left behind goes once it is an hour old): two listings sharing one, as two GUIs at once did, broke each other. `ALWAYSUSECACHEFORDATA=true` sends such robots' data files to the home, as for jars, instead of into the folder.
- It runs with `NOSECURITY=true` (one class loader for classic's engine classes and the peer); isolation is the sandbox (§6.6).
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
- **Layout (Steam-like, `crates/gui/src/shell.rs`):** a top bar with the tabs Start Battle, Watch Battle and Run Rumble (M4) on the left and a gear for the options on the right, and a footer with the version. Pages with inputs sit in a centred column of at most 960 points; the margins show classic's floor, darkened, with a few tanks wandering as on the website's home page (`backdrop.rs`, at most 30 frames a second and still when the margins are too narrow to see). Start Battle shows the available robots (with a filter) beside the ones in the battle, then the battle's settings; while the robots are described (`runner::local::prepare`, on a background thread) and their JVMs start, a loading screen shows the contestants, or the error with a way back. Watch Battle has two sections in the same style: From File (a recording to open) and Remote Server (a battle to watch). A live battle, a watched one or a replay fills its tab with the battle screen (`screen.rs`): the battlefield, the controls below it as drawn icons (stop, pause/play, step, speed; round, turn and seed on the right; a replay adds the timeline and stepping back), the robots' output below those in a tab per robot with its debug-paint marker (a pencil, green when on) in the tab, and the robots on the right, the full height, with square energy bars (empty at 0) and scores; the energy and CPU graphs (#33) go there too. The loading screen can be cancelled: a battle that is closed stops (`ObserverControl.abort`). Icons are drawn with the painter (`icons.rs`), as egui's default fonts lack many symbols. The app is dark whatever the system's theme (egui would follow a light Windows or macOS theme, which the app's colours aren't made for), and so is the system's title bar where it draws one. Under WSLg (whose compositor crashes on winit's frame, #28) and on Windows (for a window like Steam's) the app draws its own window frame, and the top bar is its title bar: its empty parts move the window, a double click maximizes it, and thin edges resize it; `ROBOCODES_OWN_FRAME=0` (or `1`) asks for the system's frame (or ours) instead. On Windows winit's undecorated window keeps the system's caption and sizing styles and only hides the non-client area, so it keeps the drop shadow, snapping (Win+arrows, dragging to a screen edge) and the system's own move and resize loops (which hold the minimum size and end with the button's release); maximized, it covers the work area, not the taskbar. Under WSLg the compositor keeps the pointer's release after a move or resize, so the app makes one up (`window::release_after_grab`). The window is at least 720 × 480 points: the minimum goes to the compositor (`xdg_toplevel.set_min_size`), and since sizes the app asks for are not held to it (nor, maybe, every compositor's resizes), a window that has stayed smaller for 0.4 s asks for the minimum (`window::keep_min_size`; `ROBOCODES_FRAME_LOG=1` logs its sizes).
- **From your IDE** (#17, `ide.rs`): while the app is open it listens on the IDE port (127.0.0.1, 7878 by default; the options' *From your IDE* row turns it off or changes the port; if the port is taken, the options and the panel say why). A robot started by the IDE's debugger connects, describes itself and waits, as a warm host waits for its assignment. *From your IDE…*, a button above the battle list, opens a panel with the port, the run configuration to copy (a Copy button for each part, for VS Code's `launch.json` and for everything) and the robots connected now, each with *Add to battle*. A connected robot also shows as `name · IDE` in a pinned *From your IDE* group at the top of the available robots (not hidden by the filter, only there while it is connected) with a notice, "name · IDE connected from your IDE: Add to battle". Added, it is a battle entry (`@ide:ID`) showing its state, *waiting*, or *not connected* if it left before the start, and Start says why when one has. Starting hands the held connection to the battle (`runner::local::prepare_ide`, as warm hosts are assigned); the battle screen then says "Debug battle" and the replay too. After the battle the robot host ends.
- **Run Rumble** (M4 P15, `rumble.rs`, `docs/rumble-backend.md` §13.7): the tab signs in with GitHub's device flow on a thread (the code large with Copy, the browser opened, Cancel; "Signed in as ..." with `GET me`, Sign out revokes the token and removes the file), and runs a `Contributor` on a thread of its own: Start/Stop (the first Stop is graceful, then the button is "Stop now"), parallel battles, divisions (melee needs 5 slots, said inline), the sandbox from the options (`none` asks once; the consent is `rumble.unsandboxedConsent` in `gui.json`, with `rumble.server`, `rumble.parallel`, `rumble.divisions`). Threads send `Msg`s over a channel that the app polls every frame whatever tab is open, so contributing goes on elsewhere; `Model` holds what the events make of the page (running battles with round progress, session totals, all-time numbers from the posts, last results, log, waiting countdown, the problem that ended the loop) and is tested without a window. Closing the window while contributing stops gracefully and waits up to 10 s for the posts (a second close, or Quit now, quits at once). `--rumble-demo STATE` shows canned states for screenshots.
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

- **Format** (`proto`, `RecordingEntry`; gzip): a header (format version, release version, engine version (§4.4; 0 if recorded before engine versions), seed, rules, robots) followed by **every input the server gave the engine**, in order: round start and end, each turn's step, the commands each `Execute` brought, the drained, disabled and stopped robots, and an observer's abort, killed robots and mouse and keyboard input (which the engine ignores: robots' reactions are recorded as their commands, so the input is there for completeness and viewers). The server feeds the engine only through one driver (`crates/server/src/driver.rs`) that records each input before applying it, so a replay takes exactly the server's path.
- A **snapshot marker** is recorded wherever the server published a `TurnSnapshot`. It holds what the engine doesn't know (console output, debug paint, each robot's CPU time and the turn budget, and the debug properties robots set) and a 64-bit hash of the snapshot, so every replayed turn is verified. CPU times and debug properties are not battle state: the hash leaves them out (as output and paint), and a replay shows them. Recordings from before they were recorded replay with CPU times of 0 and no debug properties.
- Replay re-runs the deterministic engine. Keyframes aren't stored: the engine state is cheap to clone, so a viewer takes them in memory while it loads a recording, and scrubbing replays from the nearest one. The GUI loads on a thread of its own (`replay::Opening`), showing the progress with Cancel, so a long recording doesn't hold the window up; a website track takes the same way. A 5-round, 10-robot sample battle (4,777 turns) records to 190 KB and replays in 70 ms.
- `robocode-s battle --record FILE` records; `robocode-s replay FILE` replays, verifies and prints the results.
- A recording made with another engine version than the replaying one is still replayed, with a warning (CLI: on stderr; GUI: next to the turn readouts), since the replay may differ. Verification by snapshot hashes decides whether it did.
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
 │ GET  /api/v1/work    │──────────► │ CloudFront (rumble.planethofstede.be)        │
 │ run battles locally  │            │   ├─ /*      → S3: static site, /data/v1     │
 │ POST /api/v1/results │──────────► │   ├─ /jars/* → S3: jars bucket               │
 │ PUT recording        │──────────► │   ├─ /rec/*  → S3: recordings (jars bucket)  │
 │   (presigned S3 URL) │            │   └─ /api/v1/* → api Lambda (function URL,   │
 └──────────────────────┘            │                  OAC)                        │
 browser ──────────────────────────► │ DynamoDB: one table, no GSIs                 │
                                     │ state bucket: config, pairing state, plan    │
                                     │ EventBridge Scheduler → batch Lambda         │
                                     └──────────────────────────────────────────────┘
```

Details: `docs/rumble-backend.md` §14 (infrastructure), ADR 0021.

- **Two Lambdas in Rust** (arm64, built with cargo-zigbuild), sharing `crates/rumble-core` with the client. Rust Lambdas start quickly and use little memory, so GB-seconds stay low. `api` serves `/api/v1` (work, results, auth, submit, admin). `batch` runs on EventBridge Scheduler: **plan** hourly at :12 (folds the result log into the pairing state and writes the work plan, §12.3), **publish** every 6 hours (rankings, §12.6) and **cleanup** daily.
- **Buckets:** the site bucket, a **jars bucket** (`jars/`, `rec/`) and a private **state bucket** (config, pairing state, plan, uploads).
- **Stages:** prod, and a **dev** stage at `dev.rumble.planethofstede.be` with its own certificate, DynamoDB on-demand and its own secrets. Both are the same construct.
- **No API Gateway.** The HTTP API is only free for the first 12 months. Lambda **function URLs** sit behind CloudFront (origin access control), and everything is served from one domain per stage, `rumble.planethofstede.be`, the API under `/api/v1`. That means one certificate per stage (ACM, us-east-1), no CORS and no separate API domain.
- **Infrastructure as code:** AWS CDK (TypeScript) in `infra/`.
- **Website:** SvelteKit built with `adapter-static` on S3 + CloudFront (ADR 0015). It also hosts the user documentation. The home page and docs are prerendered; ranking, bot and pairing pages render in the browser from pre-computed JSON (`docs/rumble-data.md`). Interactive features (login, bot submission, "watch", reputation and account pages) call the `/api` Lambdas from the browser, and those Lambdas read and write DynamoDB. Unknown paths fall back to `200.html` (CloudFront custom error response), so client-side routes like `/bot/<id>` work; a CloudFront Function maps extensionless URLs to the prerendered `.html` files (`web/README.md`).
- **Deployment:** a dedicated member account `robocode-s` in the owner's AWS Organization (ADR 0016), region **eu-west-1** (Ireland). An SCP limits it to eu-west-1 and us-east-1 and to services within the free-tier plan. The CloudFront certificate needs a small us-east-1 stack next to the main eu-west-1 stack. The DNS zone `planethofstede.be` stays in the management account: the `rumble` and `dev.rumble` CNAMEs and the certificates' validation records are added there by hand, once.

### 12.2 Free-tier budget

| Service | Always-free allowance (approx.) | How we stay within it |
|---|---|---|
| Lambda | 1M requests + 400k GB-s / month | Rust on arm64 with small memory; clients fetch work and upload results in **batches**, not one request per battle |
| DynamoDB | 25 GB storage, 25 RCU + 25 WCU **provisioned** | Provisioned mode in prod (on-demand is not covered by the free tier) and no auto-scaling beyond 25; one compact log item per results post, folded into S3 (§12.6). A dev stage uses on-demand, which costs cents at test traffic and leaves the free capacity to prod |
| CloudFront | 1 TB transfer + 10M requests / month | Site, ranking JSON, jars and recordings are all served through CloudFront and cached |
| EventBridge Scheduler | 14M invocations / month | Three schedules: plan hourly, publish every 6 hours, cleanup daily |
| CloudWatch Logs | 5 GB ingestion | Low log level and short retention (7 days) |
| SSM Parameter Store | Standard parameters free | GitHub OAuth secret stored there (not Secrets Manager, which costs $0.40/month per secret) |
| ACM | Public certificates free | — |

Costs outside the free tier, kept deliberately small (the replay costs are detailed in §12.7):
- **Route 53:** none. The records live in the existing `planethofstede.be` zone, so the rumble adds no hosted zone.
- **Package repositories (§6.9):** a second bucket behind the same distribution (`packages/*`; no new DNS record or certificate). A release adds about 50 MB (four packages), so a year of releases is a few GB: cents of storage per month, and noncurrent object versions expire after 30 days. Downloads and requests count against CloudFront's free 1 TB and 10M requests; the index files are cached for 5 minutes and the packages are immutable.
- **S3 storage:** S3's free allowance only lasts 12 months. Afterwards it costs about $0.023/GB-month, so storage is capped (§12.7) at a few GB, which means cents per month.

Guardrails:
- AWS Budgets alert at $1/month.
- **Reserved concurrency** on every Lambda, so abuse can't scale costs.
- Per-token rate limits in the API.
- DynamoDB capacity fixed at the free limits. Overload causes throttling, not a bill.

### 12.3 Server-assigned work

Clients do not choose pairings themselves. Details: `docs/rumble-backend.md` §10, ADR 0021.

1. A client calls `GET /api/v1/work`. It gets a **batch** of battle assignments as a **signed batch token** (HMAC-SHA256, key in SSM): division, robot IDs and jar hashes, rounds, **seeds**, engine version and a deadline of 3 hours. Nothing is stored for an assignment, so handing out work reads and writes nothing in DynamoDB.
2. The client runs the battles in **deterministic mode** (§7.2) and downloads jars through CloudFront, verifying them by hash.
3. The client calls `POST /api/v1/results` with the whole batch: per-round scores and the robots that got a first, a battle hash and per-round track hashes, and the engine version. A batch marker makes the post idempotent. Results of another engine version than the current one are not counted.

**No hard cap on work** (owner's decision, replacing #23's threshold): every battle volunteers offer is used and directed to the most useful pairings. The `plan` job runs hourly and gives each pairing a quota, water-filled over all pairings by the expected variance reduction of one more battle (LiteRumble's shrunk variance). Pairings whose CI still contains 50 weigh ten times as much, and bots close in rank and bots with incomplete pairings get more. The `api` samples battles from the plan in memory. Verification, probation and watch work come first, for clients allowed to run them.

Clients on an older engine version can't produce current results: tokens carry the engine version, and a client refuses work for one it doesn't run.

Note: in deterministic mode slow robots skip turns by their counted work, not their time (ADR 0018). Their behavior can still differ slightly from classic on a given machine. This is accepted and documented for the rumble.

### 12.4 Trust and verification (volunteers only)

- **Authentication:** GitHub OAuth. The web uses the code flow and gets a `__Host-` session cookie; the CLI and GUI use the device flow and get a long-lived upload token (sent as `X-Rumble-Token`). The GitHub token is checked against our app and then deleted (`docs/rumble-backend.md` §7).
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

- **Failure reports** (a robot that fails to load or crashes) count only from verifiers, users above the reputation threshold; deactivating an active bot also needs 3 distinct reporters (`docs/rumble-backend.md` §12.3).
- **Verified runs don't count twice.** A wrong counted result is corrected by a -1/+1 pair of records.

Removing a user's results requires their per-battle contributions. These are the result log's records (§12.6), kept 400 days; a ban writes records with weight -1.

### 12.5 Participants and robot jars

- **Submission:** a web form with GitHub login. Authors upload a jar (via presigned S3 URL) or provide a URL to mirror. They choose divisions and can later replace the version or retire the bot. **Limits:** 5 bots per user, 5 new versions per bot per day, and a GitHub account younger than 30 days needs the owner's approval. Admins and imported bots are exempt.
- **Validation without running robots:** static checks in the submit code (`rumble_core::jar`): jar structure, one robot `.properties`, the main class, and codesize for Java weight classes, computed by parsing class files in Rust as classic's tool does (the sum of all methods' `Code` lengths). A jar that also ships a `.team` file counts as a robot.
- **Probation:** a new version first gets battles assigned to verifiers only. If it fails to load, crashes or is disabled every round, it is deactivated with a message to the author. A new version replaces the old one in the rankings only after probation, so bots don't vanish meanwhile. Imported bots skip probation.
- **Seeding:** a one-time import of all robots in the current RoboWiki participant lists, run from the owner's machine (an admin command of the rumble CLI), not in Lambda. For each jar the sources are, in order: a local directory, the list's URL, and the strangeautomata archive that classic clients also fall back on. Jars are mirrored to S3. Imported bots are attributed to their original author; the author claims them after logging in with GitHub, and the owner approves the claim.
- **Mirror and takedown policy:** jars are mirrored to keep the rumble working when original URLs die. Authors can request removal through the site or a GitHub issue, and removals are handled promptly. The import is announced on RoboWiki.
- Other-language robots (M5, M8) use the same flow with their own package format. Weight classes only apply where codesize is defined.

Details: `docs/rumble-backend.md` §12 and §16.

### 12.6 Data model and rankings

DynamoDB holds one table without GSIs; per-battle data is a compact result log, and statistics live in S3 (`docs/rumble-backend.md` §6, §9, ADR 0021).
- **Result log:** each results post writes one transaction: the batch marker, one log item with the batch's compact records (24 bytes for a 1v1 battle, 104 for a melee battle) and the user's counters. The log is kept 400 days and is the per-user record for removing a banned user's results (§12.4).
- **Pairing state:** the hourly `plan` job folds new log items into an S3 file per engine version and division (n, sum of score, sum of score squared, sum of survival per pairing). It replaces the per-pairing items and per-bot sums updated by delta that this design first had, for three reasons: a melee battle would touch 45 pairing items, the daily scan of about 600k items does not fit in a Lambda run at the free read rate, and the log already is the per-user record. The cost is that planning data is up to an hour old.
- **Users, bots and versions:** reputation, quarantine flag, counters; bot and version items with their state (probation, active, retired).
- **Melee pairings** use classic's split: each of the 45 pairs is scored by the two bots' total scores in the battle.
- **Rounds report every robot that got a first** (classic gives robots that die together a first each), so a round can have several winners; such a round fills no "rounds won by" slot.
- **Fresh data:** every rumble battle starts with no robot data files; only data shipped in the jar counts. Verification needs equal runs, so robots that learn across battles start fresh here, unlike in classic's rumble. The work item reserves `data` for server-held snapshots later.

Write cost: one transaction per results post, so at 3 battles/s the backend uses about 4 WCU/s and 1 RCU/s of the 25/25 provisioned; the free capacity carries about 15 battles/s, so throughput is limited by volunteers.

Rankings:
- **Engine versions are never mixed** (O14). Results carry the engine version they were run with. Rankings use only the current engine version. When it changes, the rumble fills the new version's pairings from scratch (fewest battles first does that by itself); the old aggregates are kept, but not ranked. The data files say which engine version they rank.
- **Every 6 hours** (owner's decision) the `publish` job computes all metrics from the pairing state file, with no table scan: APS, PWIN, survival, Glicko-2 (from scratch, 20 rating periods over the pairing matrix), ANPP, Vote and pairing matrices. It writes the data files of `docs/rumble-data.md`: per-rumble rankings and per-bot files with pairings and history. The website shows the rumble as of that run.
- **Contributors** are public (`contributors.json`, a `/contributors` page); a user can opt out on the account page.
- **History:** a snapshot of each bot's rank and score, appended to small per-bot JSON files, for trend charts; history and snapshots keep one point a day.

### 12.7 Recordings and replays

Recordings are stored per **round**, not per battle. A rumble recording is a **lossy viewer track** (ADR 0017, `crates/replay`): the round as the viewer draws it, quantized to what a screen shows, predicted and range-coded. The exact round is reproducible from its seed, bot versions, round index and engine version, which the track's header holds (ADR 0017). Measured on top bots: 3.3–3.8 bytes per turn in 1v1, so about 3.5 KB for a 900-turn round and 20 KB for a 6,000-turn one. Lossless intents would take 37–39 bytes per turn.

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
- **Bot archive:** every version of a bot (current and retired) has a download link `<id>_<version>.jar` on its bot page, and `/archive` lists all bots and versions, searchable, with downloads (`archive.json`); takedowns disappear from both. No bulk zip, no classic-style URLs.
- Contributor stats (battles run, reputation tier) and a page for submitting or managing bots.
- **User documentation** for robot authors: about the project, getting started, developing, testing, debugging and releasing a robot, and how the rumble works. Markdown in `web/src/routes/docs`, prerendered.
- **Data contract:** the `publish` job writes versioned JSON files under `/data/v1` (`docs/rumble-data.md`). The site's local dev server serves seeded synthetic data in the same format; the dev stage publishes real rankings.
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
│   ├── rumble-core/          # rumble types, tokens, records, planner, metrics, jar validation
│   ├── rumble-client/        # rumble client: HTTP, device flow, battles, import
│   ├── viewer-wasm/          # engine + renderer for the browser
│   └── cli/
├── rumble/
│   └── lambda/               # api and batch Lambdas (a workspace of its own)
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
├── tools/rumble/             # rumble tools (corpus runs, import support)
├── tools/release/            # release script, third-party notices, archive texts
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
| M3 | Isolation and distribution | Robots over stdio, built-in OS sandbox (Linux, Windows), Docker runner image as an alternative (ADR 0019), release archives per platform, remote runner agents with TLS and tokens, warm pools |
| M4 | Rumble | Free-tier AWS backend (`api` and `batch` Lambdas: work, results, auth, submit, plan, rankings), rumble client in the CLI and GUI, seeding import from RoboWiki, website with rankings, reputation-based cross-checks, dev and prod stages. Spec: `docs/rumble-backend.md`, ADR 0021 |
| M5 | Robots in Python | Python client library on the protocol (§9) plus sample robots; its sandbox and its own work counting and calibration for deterministic mode (#15) |
| M6 | Battle replays in the browser | Replay decoder (WASM) and canvas viewer; on-demand replays in the browser |
| M7 | Online editor | Write a robot in the browser, build it and try it against other robots there, then submit it to the rumble (O15) |
| M8 | Robots in .NET, Rust, C/C++ and Kotlin | Client libraries plus sample robots, each with its own work counting and calibration (#15) |
| M9+ | Extras | WASM runner, Tank Royale adapter, .NET compat |

## 17. Open questions

- **O1** ~~LiteRumble contribution~~ Decided: no; it's a non-goal.
- **O2** ~~Classic version of current rumble clients~~ Answered (2026-10-12): LiteRumble's statistics page allows 1.10.3, 1.11.0 and 1.11.1. Of the changes since 1.10.3, only 1.11.0's fix #89 (a fire command fires at most once) touches the rules; robocode-s follows 1.11.x (§13).
- **O3** ~~Container network isolation~~ Decided: relay container on an internal Docker network (§6.3).
- **O4** ~~Meaning of bit-exact~~ Decided: exact against classic with StrictMath (fdlibm) semantics (§4.3).
- **O5** ~~JRE distribution~~ Decided: a pinned Temurin JRE, downloaded on demand (§6.1).
- **O6** ~~Sample robot licensing~~ Decided: bundled under EPL-2.0. All 27 files were checked and carry the EPL-1.0 header (§13.1).
- **O7** ~~AWS account, region and domain~~ Decided: owner's account, eu-west-1, `rumble.planethofstede.be` (§12.1). The API is served under `/api/v1` on the same domain; a dev stage uses `dev.rumble.planethofstede.be`.
- **O8** ~~Cross-check fraction~~ Decided: reputation-based, verified by volunteers only (§12.4). The exact numbers (start rate, floor, curve) get tuned once real data exists.
- **O9** ~~Recording retention~~ Decided: per-round recordings, 5 rounds won by each side, for matchups involving a top-50 bot; no expiry; the rest on demand (§12.7).
- **O10** ~~Frontend framework~~ Decided: SvelteKit (static) calling the `/api` Lambdas from the browser (§12.1, ADR 0015).
- **O11** ~~Full scan for Glicko-2/ANPP~~ Decided: all metrics are computed every 6 hours from the S3 pairing state file, with no table scan (§12.6).
- **O12** ~~JDK geometry and licensing~~ Decided: independent implementation in `crates/geom`, verified against the JDK with ~24,000 edge cases and 5M random cases, all matching (ADR 0011).
- **O13** ~~`Utils.getRandom()`~~ Decided: a per-robot generator, and the oracle patches classic to match (ADR 0010, engine-spec §15.1).
- **O14** ~~Engine version (§4.4, #22)~~ Decided: one integer, `ENGINE_VERSION`. Rumble results of different engine versions are never mixed: aggregates are kept per version, only the current one is ranked, a new version starts its pairings from scratch (§12.6).
- **O15** Online editor (M7): where a robot written in the browser is built and run (in the browser, for example as WASM, or by volunteers and the rumble's clients), which languages it offers first, and how a robot goes from the editor into the rumble.

## 18. Decisions so far

| Decision | Choice |
|---|---|
| Compatibility level | Bit-exact with classic (verified by difftest) |
| Sandbox | Built-in OS sandbox by default (Linux, Windows), Docker as an alternative, plain process, remote; WASM later (ADR 0019) |
| Tank Royale | Independent protocol; adapter later |
| Determinism | Optional deterministic mode |
| Name | robocode-s |
| License | EPL-2.0 |
| Client languages | Java compat + Java/Kotlin, Python, .NET, Rust, C/C++ |
| First milestone | Headless engine + Java compat + difftest |
| Remote robots | Runner agents (`robocode-s agent`), authenticated with a token over TLS, robots' jars by hash (ADR 0020) |
| TLS | Off loopback, everywhere (observers, agents); a self-signed certificate pinned by its SHA-256 fingerprint, or a CA-issued one; rustls with ring (ADR 0020) |
| GUI scope | Setup and viewer, recording and replay, rumble client |
| Design doc location | In repo (`docs/design.md`) |
| Classic LiteRumble | Not contributing (non-goal) |
| Bit-exact definition | Against classic with StrictMath/fdlibm semantics |
| Robot connection | Robots the runner starts talk over stdin/stdout, with no network (ADR 0019) |
| Java runtime (local mode) | Pinned Temurin JRE, downloaded on demand |
| Classic sample robots | Bundled under EPL-2.0 |
| Engine version | One integer (`ENGINE_VERSION`), bumped with every change that can alter a battle's outcome; stored in replays, results and reports; rumble results of different versions are never mixed (§4.4, §12.6) |
| Rumble frontend | SvelteKit, static, with browser-side API calls; also hosts the user documentation |
| Rumble | Own distributed rumble on AWS serverless (Rust Lambdas `api` and `batch`, DynamoDB, S3, CloudFront), prod and dev stages (`docs/rumble-backend.md`, ADR 0021) |
| Rumble participants | Web form with GitHub login; seeded from RoboWiki participant lists |
| Robot jars | Mirrored to S3, with a takedown policy |
| Rumble clients | Anyone with GitHub login; server-assigned seeded work; reputation-based verification by other volunteers (new users ~20%, trusted users ~1%) with optimistic acceptance |
| IaC | AWS CDK (TypeScript) |
| AWS deployment | Own member account `robocode-s` (ADR 0016), eu-west-1; `rumble.planethofstede.be` (API under `/api/v1`; dev at `dev.rumble.planethofstede.be`) in the existing Route 53 zone of the management account |
| Replays | Per-round lossy viewer tracks (ADR 0017): 5 rounds won by each side, for matchups involving at least one top-50 bot; never expire, replaced only by new bots or versions; the rest on demand. About $0.40/month plus about $2.75 one-time |
| AWS cost | Stay within the free tier (S3 aside, which costs cents): AWS never runs battles; Lambda function URLs behind CloudFront (no API Gateway); provisioned DynamoDB at 25/25; capped S3 |
| Rumble statistics | Result log in DynamoDB, folded hourly into an S3 pairing-state file; no pairing items, no scans (§12.6, ADR 0021) |
| Rumble assignments | Signed batch tokens (HMAC), not stored rows (§12.3) |
| Rumble work | No hard cap (replaces #23's threshold); hourly plan by variance reduction, closeness to 50 and rank contest, LiteRumble-style (§12.3) |
| Rumble rankings | Published every 6 hours; contributors public with opt-out (§12.6) |
| Rumble crates | `rumble-core` and `rumble-client` in the root workspace; `rumble/lambda` a separate workspace (§15) |
| Rumble battles | Deterministic, sandboxed (the client refuses to run unsandboxed without `--unsandboxed`), fresh robot data in every battle (§12.6) |
| Rumble limits | 5 bots per user, 5 versions per bot per day, GitHub accounts under 30 days need approval; new versions on probation, imports skip it (§12.5) |
| Rumble failure reports | Only verifiers' count; deactivating an active bot needs 3 distinct reporters (§12.4) |
| Rumble melee | Classic's split: 45 pairs scored by the two bots' total scores; every robot that got a first is reported (§12.6) |
| Rumble import | RoboWiki lists; jar from a local directory, the list's URL, then the strangeautomata archive (§12.5) |
