# robocode-s — Design Document

Status: **Draft** · Last updated: 2026-10-10

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
8. **Our own RoboRumble**: distributed battle execution, verifiable results, a web interface with in-browser replays (§12). Contributing to classic LiteRumble is optional.

### Non-goals (for now)

- A built-in robot editor or compiler (use an IDE).
- Changing the classic game rules. Rule variants may come later behind explicit flags; the default is always classic.
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
| `rumble/*` | Rust (Lambdas) | Rumble backend: work assignment, result intake, auth, verification, rankings. |
| `web` | TypeScript + WASM | Rumble website and in-browser replay viewer. |
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

The exact order is taken from the 1.11.1 sources and pinned down by differential tests.

### 4.3 Math functions — the main bit-exactness risk

Classic Robocode calls `java.lang.Math.sin/cos/atan2/...`. On HotSpot these may run as intrinsics, which may differ by up to 1 ulp from `StrictMath` (fdlibm) and can vary between JVMs and CPUs. That means classic itself is only bit-exact per platform and JVM.

Strategy:
1. Port fdlibm (`StrictMath`) to Rust for the engine, and use it everywhere.
2. Run the differential harness with classic on a pinned JDK. Measure divergence: first exact-equality failures, then error bounds.
3. If HotSpot intrinsics cause observable divergence, decide whether "bit-exact" means "bit-exact vs StrictMath". If so, classic running with intrinsics disabled is the oracle, which is effectively fdlibm. Record the decision in an ADR.

Robot-side math runs in the robot's own runtime and is not affected.

## 5. Protocol

### 5.1 Encoding and transport

- **Schema:** Protocol Buffers (proto3). It has the broadest code generation coverage: Java, Kotlin, Python, C#, Rust (`prost`), C++ and C (`nanopb`/`upb`).
- **Framing:** a varint length prefix followed by the message, over any byte stream.
- **Transports:** TCP (default), with TLS required when the peer is not on loopback. Unix domain sockets and Windows named pipes are later optimizations for local mode.
- **Versioning:** `Hello` / `HelloAck` handshake with protocol major/minor versions and feature flags.

### 5.2 Roles

- **Runner ↔ server:** the runner registers its capabilities (runtimes, limits) and receives `LaunchRobot` requests.
- **Robot ↔ server:** a robot connects with a one-time **session token**, which the server issued to the runner at launch, and then takes part in the lockstep loop.
- **Observer ↔ server:** the GUI or other tools subscribe to the battle state stream and send battle control commands (pause, speed, enable debug paint for a robot).

### 5.3 Lockstep loop (robot role)

Per turn:

```
server → robot : TickEvent { turn, round, own_state, events[], (team messages), budget }
robot  → server: TurnIntent { set_ahead/turn/gun/radar/fire/..., adjust flags,
                              team messages, debug paint commands, custom-event results* }
```

- Blocking calls (`ahead(100)`, `turnGunRight(...)`) and `waitFor(Condition)` are implemented **in the client library**. They become a sequence of turn intents. This keeps the wire protocol purely per-turn.
- Custom events (`Condition.test()`) run client-side. Because they run during the robot's turn, firing them only affects that robot's own event queue, as in classic.
- The server sends `TickEvent` to all robots in parallel and waits up to the turn budget (§7). Robots that miss the deadline **skip the turn**, with the classic semantics: the previous intent stays in effect, a `SkippedTurnEvent` is sent, and the robot is disabled after too many skips.
- Classic runs robots one after another by default. Since robots cannot see each other's turn intents before the engine step, parallel execution produces the same results.

### 5.4 Observer stream

- Full snapshot on subscribe, then per-turn deltas: robot states, bullets, explosions, scores and debug paint.
- Debug paint is only requested from a robot while an observer has it enabled. This saves bandwidth and robot CPU time, as in classic.

## 6. Robot hosting (runners)

### 6.1 Launcher modes (pluggable)

| Mode | Use case | Isolation | Notes |
|---|---|---|---|
| **Local process** | Development, fast rumble runs with trusted robots | Low: OS process only. Memory limited through runtime flags (e.g. `-Xmx`); optionally Job Objects on Windows or cgroups on Linux. | Requires local runtimes; a JRE can be downloaded and managed by robocode-s. |
| **Container** | Untrusted robots, tournaments | High: CPU, memory, pids, read-only rootfs, no general network | Requires Docker (Docker Desktop on Windows/macOS). |
| **Remote agent** | Distributed tournaments, spreading load | Whatever the remote host's launcher provides | `runner` in agent mode, authenticated to the server. |
| *WASM (future)* | Native and new-language robots | High, plus deterministic fuel metering | wasmtime embedded in the runner (§14). |

### 6.2 Container images

- One image **per runtime**, not one polyglot image: `robocode-s-runner-jvm`, `-python`, `-dotnet`, `-native`.
- Each image contains a small launcher shim: it receives the robot package and starts it with the compat or client library on the classpath or path.
- Multi-arch builds (amd64 and arm64), published to GHCR.
- **Warm pools:** the runner keeps N containers per image started ahead of time, to hide container and JVM startup.

### 6.3 Container network isolation

A robot must reach the server and nothing else. Proposal (open question O3): put robot containers on an `--internal` Docker network that also contains a tiny relay container. The relay forwards only to the server's robot port. This works the same on all three platforms and needs no host firewall rules.

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
- All engine randomness is seeded.
- The JVM compat runner seeds `java.util.Random` and `Math.random()` deterministically per robot, as classic does with `-DRANDOMSEED`.
- Under WASM, budgets become instruction fuel, which makes even the time limits deterministic.

### 7.3 Trust

Robot-side CPU measurement is only trusted when the runner is trusted: a local runner, or a remote runner operated by the tournament organizer. Untrusted remote runners only get wall-clock budgets.

## 8. Java compatibility library (`clients/java-compat`)

- Published as a jar that provides the **exact public API of `robocode.jar` 1.11.1**: `robocode.*`, `robocode.util.*`, `robocode.robotinterfaces.*` and `robocode.robotinterfaces.peer.*`.
- **Architecture mirrors classic:** classic robots talk to their engine through the `I*RobotPeer` interfaces. We implement those peers on top of the protocol, so `Robot`, `AdvancedRobot`, `TeamRobot`, `RateControlRobot`, `Droid`, `JuniorRobot` and robots implementing the interfaces directly all work unchanged.
- The robot runs on its own thread, as in classic. The peer blocks that thread inside `execute()` until the next tick.
- **Robot loading:** reads robot `.properties` from jars (including team `.team` files) and uses a class loader per robot.
- **`getGraphics()` / `onPaint`:** a `Graphics2D` implementation that records drawing operations as protocol paint commands.
- **Team messages:** `Serializable` objects are serialized with Java serialization and sent as opaque bytes through the server. Only robots in the same JVM language family can read them, as in classic.
- **Sandboxing:** none in-process. `SecurityManager` is gone in modern JDKs, so isolation is done by the container or process (§6).
- **Packaging:** robots built against classic `robocode.jar` link against our jar without recompiling.

## 9. Other client libraries

Native APIs for new robots, all generated from the same `.proto`:

- **Java/Kotlin** (modern API, non-legacy), **Python**, **C#/.NET**, **Rust**, **C/C++**.
- Each language provides two layers: a low-level **turn-loop API** (receive tick, send intent) and an ergonomic **robot API** (event handlers, blocking helpers) modeled on classic concepts.
- Classic .NET robots (`robocode.dotnet`) may get a compat layer later, like the Java one.

## 10. GUI (`gui`)

- **Stack:** `wgpu` (Vulkan / Metal / DX12 / GL fallback) for the battlefield, `egui` for panels and dialogs. Bevy is not used, to avoid ECS overhead for a simple 2D scene and to keep the engine independent.
- **Features (scope v1):** battle setup (robot selection, rules, number of rounds, seed, launcher mode), live viewer with speed control, pause and single-step, robot debug painting per robot, robot console output, results table, recording and replay with scrubbing.
- **Headless:** the same battles run from `cli` without a GUI, or with a remote GUI attached as an observer.
- The battlefield is drawn with instanced sprites (tank body, gun, radar), bullets, explosions and vector debug paint. All of it is cheap on the GPU even at very high turn rates. Turns that can't be displayed are dropped, so rendering never slows down the simulation.

## 11. Recording and replay

- **Format:** a header (battle config, robot identities and hashes, engine version, seed) followed by per-turn **intents** of all robots, plus a **state keyframe** every N turns.
- Replay re-runs the deterministic engine from the nearest keyframe. This makes recordings small and allows scrubbing.
- Debug paint and console output are stored in an optional side stream.
- Classic `.br` recordings can be imported if feasible (low priority).

## 12. robocode-s Rumble

We run our own distributed rumble. Volunteers run battles with the robocode-s rumble client, and a serverless AWS backend collects results, computes rankings and serves a web interface for browsing results and replaying battles. Taking part in classic LiteRumble stays an optional extra (O1).

### 12.1 Architecture

```
 rumble clients (cli / gui)                AWS
 ┌──────────────────────┐   HTTPS   ┌────────────────────────────────────────────┐
 │ GET  /work           │──────────►│ API Gateway (HTTP API)                     │
 │ run battles locally  │           │   ├─ work Lambda      ─┐                   │
 │ POST /results        │──────────►│   ├─ results Lambda   ─┼─► DynamoDB        │
 │ PUT recording (S3    │──────────►│   ├─ auth Lambda       │   (bots, pairings,│
 │     presigned URL)   │           │   └─ submit Lambda    ─┘    battles, users)│
 └──────────────────────┘           │ S3: robot jars, recordings, site, rankings │
                                    │ EventBridge schedule → rankings Lambda     │
 browser ──────────────────────────►│ CloudFront → static site + ranking JSON    │
                                    │ verify Lambda (container image: engine +   │
                                    │   JVM runner) for validation & spot checks │
                                    └────────────────────────────────────────────┘
```

- **Lambdas in Rust** (`cargo-lambda`), so they share the `engine`, `protocol` and recording code with the rest of the project.
- **Infrastructure as code:** AWS CDK (TypeScript) in `infra/`.
- **Storage:** DynamoDB, on-demand, single-table design: bots, per-pairing statistics, battle records, users and tokens. S3 holds mirrored robot jars (keyed by SHA-256), recordings (lifecycle-pruned) and the generated ranking JSON.
- **Website:** a static single-page app on S3 + CloudFront. It reads pre-computed ranking JSON and doesn't hit a database for page views.

### 12.2 Server-assigned work

Unlike LiteRumble, clients do not choose pairings themselves:

1. A client calls `GET /work`. The work Lambda returns a batch of battle assignments: division, robot IDs and jar hashes, number of rounds, **seed** and assignment ID. Pairings with the fewest battles come first, as in LiteRumble.
2. The client runs the battles in **deterministic mode** (§7.2). It downloads and verifies jars by hash, from the S3 mirror.
3. The client calls `POST /results` with per-round scores, the recording hash and engine version. The recording is uploaded through a presigned S3 URL.
4. The results Lambda checks that the assignment exists, is unexpired and belongs to this user, then updates the pairing statistics incrementally.

Note: in deterministic mode slow robots never skip turns, so their rumble behavior may differ slightly from classic. This is accepted and documented for the rumble.

### 12.3 Trust and verification

- **Authentication:** GitHub OAuth. The auth Lambda exchanges the code and issues a long-lived upload token for the client and a session for the web UI. Cognito is not used because GitHub isn't a standard OIDC provider for Cognito.
- **Cross-checks:** because battles are deterministic, a configurable fraction of assignments (e.g. 2–5%) goes to two different users. Any mismatch flags both submissions.
- **Spot checks:** the verify Lambda re-runs sampled battles itself. Robots run inside the Lambda container (Firecracker isolation, time limited) with the local-process runner.
- **Consequences:** users with mismatches are quarantined. Their results are excluded until re-verified, and they can be banned and their results purged.

### 12.4 Participants and robot jars

- **Submission:** a web form with GitHub login. Authors upload a jar or provide a URL; it's mirrored to S3. They choose divisions (1v1, melee, teams, twin duel) and can later replace the version or retire the bot.
- **Validation:** the verify Lambda checks the jar structure and `.properties`, computes codesize for Java weight classes (nano/micro/mini/mega), and runs a short smoke battle. Only then is the bot activated.
- **Seeding:** a one-time import of all robots in the current RoboWiki participant lists. Their jars are mirrored to S3. Imported bots are attributed to their original author and are claimable by that author after logging in with GitHub.
- **Mirror and takedown policy:** jars are mirrored to keep the rumble working when original URLs die. Authors can request removal through the site or a GitHub issue, and removals are handled promptly. The import is announced on RoboWiki.
- Other-language robots (M4) use the same flow with their own package format. Weight classes only apply where codesize is defined.

### 12.5 Rankings

- Computed incrementally on upload (pairing statistics) plus a scheduled rankings Lambda that writes JSON to S3 per division.
- **Metrics (LiteRumble-compatible):** APS (average percentage score), PWIN, survival, ANPP, Vote and Glicko-2. Each is per division and weight class.
- **History:** a daily snapshot of each bot's rank and score, for trend charts.

### 12.6 Web interface

- Rankings tables per division and weight class, sortable by every metric, with filters and search.
- Bot pages: metadata, author, versions, rating history chart, best and worst pairings, and a full pairing matrix with heatmap.
- Pairing pages: per-pairing statistics and a list of battles.
- **Battle replay in the browser:** the engine and viewer are compiled to **WebAssembly** (WebGPU, with WebGL2 fallback) and play the uploaded recordings. Scrubbing works the same as in the desktop GUI (§11).
- Contributor stats (battles run per user) and a page for submitting or managing bots.

### 12.7 Classic LiteRumble (optional)

Contributing to classic LiteRumble from robocode-s still depends on community acceptance (O1). The plan is unchanged: approach the community only after difftest shows bit-exact agreement over a large rumble corpus.

## 13. Compatibility reference and differential testing

- **Reference version:** classic Robocode **1.11.1** (released 2026-07-13, the latest at the time of writing). Before 1.10, the 1.9.x line received bug fixes and Java-version support, not rule changes. Still, the version currently used by most RoboRumble clients must be confirmed (O2). If it differs, the harness also runs against that version.
- **Harness (`tools/difftest`):**
  1. Runs a battle in classic Robocode via the `robocode.control` API (`RobocodeEngine`, `BattleListener`, `ITurnSnapshot`) with `RANDOMSEED`.
  2. Runs the same battle in robocode-s, deterministic mode, with the same seed.
  3. Compares per-turn snapshots field by field (positions, headings, energy, gun heat, bullets, events) and reports the first divergence with a diff of both states.
- **Corpus:** the classic sample robots first, then a growing set of RoboRumble robots (1v1, melee, teams).
- **CI:** the sample corpus runs on every PR, and the large corpus runs nightly.

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
│   ├── fdlibm/               # StrictMath port
│   ├── protocol/             # generated + framing
│   ├── server/
│   ├── runner/
│   ├── gui/
│   ├── viewer-wasm/          # engine + renderer for the browser
│   └── cli/
├── rumble/                   # Lambda functions (Rust, cargo-lambda)
│   ├── api/                  # work, results, auth, submit
│   ├── verify/               # validation & spot-check runner (container image)
│   └── rankings/
├── web/                      # rumble website (static SPA)
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
| M5 | Rumble | AWS backend (work/results/auth/rankings), rumble client, seeding import from RoboWiki, website with rankings, cross-checks and spot checks |
| M5b | Web replay | Engine + viewer compiled to WASM; replays in the browser |
| M6+ | Extras | WASM runner, Tank Royale adapter, .NET compat |

## 17. Open questions

- **O1** Will classic LiteRumble accept results from robocode-s? This is now optional, since we run our own rumble.
- **O2** Which classic version do current rumble clients use? Are there rule-relevant differences from 1.11.1?
- **O3** Container network isolation: is the relay approach acceptable, or are there simpler cross-platform options?
- **O4** `Math` vs `StrictMath` divergence (§4.3): what exactly does "bit-exact" mean?
- **O5** JRE distribution in local mode: bundle via `jlink`, download on demand, or require a user-installed Java?
- **O6** Licensing of bundled classic sample robots. They are EPL-1.0, which permits distribution under later EPL versions; this needs confirmation for each file.
- **O7** AWS account, region and domain name for the rumble site.
- **O8** Cross-check fraction and spot-check rate. These trade compute cost against detecting cheating.
- **O9** Recording retention: keep all, keep the latest N per pairing, or sample? This drives S3 cost.
- **O10** Frontend framework for `web/` (e.g. Svelte or React with Vite).

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
| Rumble | Own distributed rumble on AWS serverless (Rust Lambdas, DynamoDB, S3, CloudFront) |
| Rumble participants | Web form with GitHub login; seeded from RoboWiki participant lists |
| Robot jars | Mirrored to S3, with a takedown policy |
| Rumble clients | Anyone with GitHub login; server-assigned seeded work, cross-checks and spot checks |
| IaC | AWS CDK (TypeScript) |
