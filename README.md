# robocode-s

A reimplementation of [Robocode](https://robocode.sourceforge.io/) with a Rust battle engine, a GPU-accelerated battlefield viewer, and a client/server architecture where robots run out of process: locally, in containers, or on remote hosts.

- **Bit-exact compatibility** with classic Java robots (`robocode.*` API, reference version 1.11.1)
- **Robots in any language**: Java/Kotlin, Python, C#/.NET, Rust, C/C++
- **Isolation & resource limits** through pluggable launchers (local process, Docker, remote)
- Runs on **Windows, Linux and macOS**
- **Distributed RoboRumble** with a serverless AWS backend and a web interface that replays battles in the browser

> Status: early design phase. See [docs/design.md](docs/design.md).

## Building

Requirements: Rust (stable, via [rustup](https://rustup.rs)), a C toolchain, and JDK 21.

```sh
cargo test --workspace     # engine, protocol, server, runner, cli
./gradlew build            # Java libraries, sample robots and difftest oracle
```

The website (`web/`) needs Node 24 and pnpm; see [web/README.md](web/README.md).

## Running a battle

```sh
./gradlew :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
cargo run --release -p robocode-s-cli -- battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
```

Every robot runs in its own JVM; a pinned JRE is downloaded on first use. Battles are deterministic by default: the same seed gives the same battle. `--classic` uses classic's timing instead, where slow robots skip turns. Other robots: `--jar path/to/robot.jar`. All options: `robocode-s battle --help`.

`--record FILE` records a battle (`--paint` includes the robots' debug painting); `robocode-s replay FILE` re-runs it, checks every turn and prints the results.

## The GUI

```sh
cargo run --release -p robocode-s-gui                 # battle setup, then the live battle
cargo run --release -p robocode-s-gui -- battle.rsr   # replay a recording
```

The battle setup lists the robots in the jars (`ROBOCODES_ROBOT_JARS`). A live battle can be paused, stepped and sped up, with each robot's debug painting and console output; a replay can be scrubbed through.

## Repository layout

- `crates/`: Rust workspace (engine, fdlibm, java-random, geom, protocol, server, runner, difftest, cli)
- `proto/`: wire protocol schema
- `clients/java-compat/`: the robot host for classic Java robots (classic's robot side over the robocode-s protocol)
- `clients/java-determinism/`: Java agent for per-robot seeded randomness
- `robots/samples/`: classic sample robots (EPL-2.0)
- `tools/difftest/`: oracle that records classic battles for differential testing
- `web/`: the rumble website and user documentation (SvelteKit, static)
- `docs/`: [design](docs/design.md), [engine specification](docs/engine-spec.md) and [architecture decision records](docs/adr/README.md); the rumble's [data files](docs/rumble-data.md)

## License

[Eclipse Public License 2.0](LICENSE)
