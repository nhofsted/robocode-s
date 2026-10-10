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

## Repository layout

- `crates/`: Rust workspace (engine, fdlibm, java-random, protocol, server, runner, cli)
- `proto/`: wire protocol schema
- `clients/java-compat/`: drop-in replacement for classic `robocode.jar`
- `clients/java-determinism/`: Java agent for per-robot seeded randomness
- `robots/samples/`: classic sample robots (EPL-2.0)
- `tools/difftest/`: oracle that records classic battles for differential testing
- `docs/`: [design](docs/design.md) and [architecture decision records](docs/adr/README.md)

## License

[Eclipse Public License 2.0](LICENSE)
