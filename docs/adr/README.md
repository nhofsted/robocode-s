# Architecture decision records

| # | Decision |
|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions |
| [0002](0002-out-of-process-robots.md) | Robots run out of process |
| [0003](0003-bit-exact-classic-compatibility.md) | Bit-exact compatibility with classic Robocode, against StrictMath |
| [0004](0004-protobuf-protocol.md) | Protocol Buffers with length-delimited framing |
| [0005](0005-deterministic-mode.md) | Optional deterministic mode |
| [0006](0006-cpu-time-budgets.md) | Time budgets measured in robot CPU time |
| [0007](0007-own-rumble-on-aws-free-tier.md) | Own distributed rumble on the AWS free tier |
| [0008](0008-licensing.md) | EPL-2.0 license; classic samples bundled under EPL-2.0 |
| [0009](0009-per-robot-random-streams.md) | Per-robot random streams via a Java agent |
| [0010](0010-per-robot-utils-random.md) | `Utils.getRandom()` returns a per-robot generator |
| [0011](0011-independent-geometry.md) | Independent implementation of the JDK geometry, verified against the JDK |
| [0012](0012-classic-robot-side.md) | Java robots run on classic Robocode's own robot side |
| [0013](0013-deterministic-identity-hash-codes.md) | Deterministic identity hash codes for robots |
| [0014](0014-work-metering-and-virtual-clock.md) | Work metering and a virtual clock for robots |
| [0015](0015-static-sveltekit-site.md) | One static SvelteKit site for the rumble and the user documentation |
| [0016](0016-own-aws-account.md) | The rumble runs in its own AWS account |
| [0017](0017-lossy-viewer-replays.md) | Rumble replays are lossy viewer tracks, not re-simulated intents |
| [0018](0018-deterministic-skipped-turns.md) | Deterministic skipped turns from counted work |
| [0019](0019-built-in-robot-sandbox.md) | Robots run in a built-in OS sandbox; Docker is an alternative |
| [0020](0020-remote-runner-agents-and-tls.md) | Remote runner agents and TLS |
| [0021](0021-rumble-backend.md) | Rumble backend: a result log folded into S3, signed batches, no work cap |
