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
| `rumble/*` | Rust (Lambdas) | Rumble backend: work assignment, result intake, auth, submission, rankings. AWS never runs battles. |
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
- **Deployment:** the owner's AWS account, region **eu-west-1** (Ireland), DNS zone `planethofstede.be` in Route 53. The alias record is created by CDK via `HostedZone.fromLookup`. The CloudFront certificate needs a small us-east-1 stack next to the main eu-west-1 stack.

### 12.2 Free-tier budget

| Service | Always-free allowance (approx.) | How we stay within it |
|---|---|---|
| Lambda | 1M requests + 400k GB-s / month | Rust on arm64 with small memory; clients fetch work and upload results in **batches**, not one request per battle |
| DynamoDB | 25 GB storage, 25 RCU + 25 WCU **provisioned** | Provisioned mode (on-demand is not covered by the free tier) and no auto-scaling beyond 25; aggregates instead of per-battle rows (§12.6) |
| CloudFront | 1 TB transfer + 10M requests / month | Site, ranking JSON, jars and recordings are all served through CloudFront and cached |
| EventBridge Scheduler | 14M invocations / month | A few scheduled ranking runs per day |
| CloudWatch Logs | 5 GB ingestion | Low log level and short retention (7 days) |
| SSM Parameter Store | Standard parameters free | GitHub OAuth secret stored there (not Secrets Manager, which costs $0.40/month per secret) |
| ACM | Public certificates free | — |

Costs outside the free tier, kept deliberately small (the replay costs are detailed in §12.7):
- **Route 53:** the hosted zone already exists ($0.50/month); alias queries to CloudFront are free.
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

Note: in deterministic mode slow robots never skip turns, so their rumble behavior may differ slightly from classic. This is accepted and documented for the rumble.

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
- **Cheap metrics** (APS, PWIN, survival) come from per-bot aggregates. A scheduled Lambda writes them as JSON to S3 a few times per day.
- **Metrics that need all pairings** (Glicko-2, ANPP, Vote, pairing matrices) come from a slow, rate-limited scan once a day, kept inside the free read capacity. The output is per-division JSON files plus per-bot pairing files.
- **History:** a daily snapshot of each bot's rank and score, appended to small per-bot JSON files, for trend charts.

### 12.7 Recordings and replays

Recordings are stored per **round**, not per battle. A recording starts from a state keyframe at the beginning of a round and contains the intents of that round. Estimated size is 10–20 KB for a 1v1 round; to be measured in M1.

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
| Stored rounds | about 520k (1v1) + other divisions → **about 8–10 GB** → about $0.20–0.25/month |
| Initial fill | about 550k PUTs, **one-time about $2.75** |
| Ongoing uploads | new bot outside the top 50: about 500 rounds; new bot entering the top 50: about 11k rounds. At a few dozen new versions/month: **about $0.10–0.50/month** |
| Downloads | Served through CloudFront and cached; within CloudFront's free allowance |

### 12.8 Web interface

- Rankings tables per division and weight class, sortable by every metric, with filters and search.
- Bot pages: metadata, author, versions, rating history chart, best and worst pairings, and a full pairing matrix with heatmap.
- Pairing pages: per-pairing statistics and a "watch a battle" button (§12.7).
- **Battle replay in the browser:** the engine and viewer are compiled to **WebAssembly** (WebGPU, with WebGL2 fallback) and play the recordings. Scrubbing works the same as in the desktop GUI (§11).
- Contributor stats (battles run, reputation tier) and a page for submitting or managing bots.

### 12.9 Classic LiteRumble (optional)

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
│   ├── api/                  # work, results, auth, submit, admin
│   ├── codesize/             # class-file parser for weight classes
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
| M5 | Rumble | Free-tier AWS backend (work/results/auth/submit/rankings), rumble client, seeding import from RoboWiki, website with rankings, reputation-based cross-checks |
| M5b | Web replay | Engine + viewer compiled to WASM; on-demand replays in the browser |
| M6+ | Extras | WASM runner, Tank Royale adapter, .NET compat |

## 17. Open questions

- **O1** Will classic LiteRumble accept results from robocode-s? This is now optional, since we run our own rumble.
- **O2** Which classic version do current rumble clients use? Are there rule-relevant differences from 1.11.1?
- **O3** Container network isolation: is the relay approach acceptable, or are there simpler cross-platform options?
- **O4** `Math` vs `StrictMath` divergence (§4.3): what exactly does "bit-exact" mean?
- **O5** JRE distribution in local mode: bundle via `jlink`, download on demand, or require a user-installed Java?
- **O6** Licensing of bundled classic sample robots. They are EPL-1.0, which permits distribution under later EPL versions; this needs confirmation for each file.
- **O7** ~~AWS account, region and domain~~ Decided: owner's account, eu-west-1, `rumble.planethofstede.be` (§12.1). The API is served under `/api` on the same domain.
- **O8** ~~Cross-check fraction~~ Decided: reputation-based, verified by volunteers only (§12.4). The exact numbers (start rate, floor, curve) get tuned once real data exists.
- **O9** ~~Recording retention~~ Decided: per-round recordings, 5 rounds won by each side, for matchups involving a top-50 bot; no expiry; the rest on demand (§12.7).
- **O11** Is the daily full scan for Glicko-2/ANPP enough, or should those metrics be updated incrementally? Revisit once the bot count is known.
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
| Rumble clients | Anyone with GitHub login; server-assigned seeded work; reputation-based verification by other volunteers (new users ~20%, trusted users ~1%) with optimistic acceptance |
| IaC | AWS CDK (TypeScript) |
| AWS deployment | eu-west-1; `rumble.planethofstede.be` (API under `/api`) in the existing Route 53 zone |
| Replays | Per-round recordings: 5 rounds won by each side, for matchups involving at least one top-50 bot; never expire, replaced only by new bots or versions; the rest on demand. About $0.50/month plus about $2.75 one-time |
| AWS cost | Stay within the free tier (S3 aside, which costs cents): AWS never runs battles; Lambda function URLs behind CloudFront (no API Gateway); provisioned DynamoDB at 25/25; capped S3 |
