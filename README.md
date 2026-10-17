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

Every robot runs in its own JVM; a pinned JRE is downloaded on first use. Battles are deterministic by default: the same seed gives the same battle. `--classic` uses classic's timing instead, where slow robots skip turns. Other robots: `--jar path/to/robot.jar`, or `--jar path/to/classes` for a folder of class files such as an IDE's output (`bin`, `build/classes/java/main`), read in place as classic's development folders are: its robots are named with classic's `*` (`my.Robot*`, or `my.Robot 1.0*` with a version in its `.properties`), and a recompiled robot is new in the next battle. `ROBOCODES_ROBOT_JARS` (a path list) takes jars and class folders too. A classic `.battle` file gives the robots and rules: `--battle FILE`. All options: `robocode-s battle --help`.

`--record FILE` records a battle (`--paint` includes the robots' debug painting); `robocode-s replay FILE` re-runs it, checks every turn and prints the results.

`--observe PORT` lets the GUI watch the battle, from this machine or another (Start Battle, Watch a battle, or `robocode-s-gui --watch HOST:PORT`): the battle starts once the first observer connected, and observers control it as in a live battle (pause, speed, stop, kill, input). A port alone listens on 127.0.0.1 only; to listen on other addresses (`--observe 0.0.0.0:7654`) observers need a token, `--observe-token TOKEN` (or `ROBOCODES_OBSERVE_TOKEN`; the GUI's Token field, or `--token`). The connection is not encrypted, so use a trusted network, or an SSH tunnel to the loopback port (`ssh -L 7654:localhost:7654 host`). Every process on the machine reaches a loopback port, so on a shared machine give it a token too (the battle's own connections, its robots and the built-in observer, get fresh secret tokens each battle).

## The GUI

```sh
cargo run --release -p robocode-s-gui                 # battle setup, then the live battle
cargo run --release -p robocode-s-gui -- battle.rsr   # replay a recording
```

The window has tabs: Start Battle (the setup, then the live battle), Replay Battle and, later, Run Rumble; the gear opens the options: the robot folders (scanned for `.jar` files; a folder holding class files, such as an IDE's output, is also read in place as a development folder, so a recompiled robot is new in the next battle), what the battlefield shows (scan arcs, coloured bullets, robot names and energy, explosions, the floor, the sentry border, the turns and frames per second) and sound (classic's gunshot, hit, explosion and collision sounds, with a volume; off by default, as in classic), and the defaults for new battles: mode, rounds, battlefield size and the other rules (gun cooling rate, inactivity time, sentry border, hidden enemy names). They are saved in `~/.config/robocode-s/gui.json` (the platform's config directory; `ROBOCODES_GUI_CONFIG` names another file). The battle setup lists the robots in the robot folders and in `ROBOCODES_ROBOT_JARS` (listed again whenever the setup shows, and with Rescan in the options) beside the ones in the battle (double-click to add or remove; a robot may be in it more than once; a click shows its author, description, web page and the Robocode version it was built for from its `.properties`, with its language and platform, Java on the robot host's JVM, as Tank Royale shows them), with a game type (Classic, 1v1, Melee or Teams, each setting its rounds, battlefield size and the default rules; Custom otherwise) and the rules (rounds, battlefield size, gun cooling rate, inactivity time, sentry border, hidden enemy names) and seed below, and a classic `.battle` file to open or save them to. Below them, Watch a battle connects to a battle another `robocode-s` runs with `--observe` (its address and token, if it has one; see above) and shows it on the same battle screen, with its controls; closing it leaves that battle running. While a battle starts, a loading screen shows the contestants (with Cancel), or why the battle couldn't start. The battle screen, live or replayed, has the battlefield with its controls below it (stop, pause and play, single steps, the speed, and the round, turn and seed), the robots' output below those in a tab per robot (the pencil in a tab turns that robot's debug painting on and off; the buttons right of the tabs copy, save and clear the open one, and in a live battle kill the robot, after a second click to confirm; the debug properties a robot set with `setDebugProperty` show beside its output), and the robots with their energy and score on the right. In a live battle, interactive robots (such as `sample.Interactive`) get the mouse over the battlefield and the keys while it has focus (the pointer is over it, or it was clicked last), as in classic. The battlefield zooms and pans for large fields: Ctrl+wheel zooms around the pointer and Ctrl+drag pans (in a replay, where no robot reads the mouse, the plain wheel and drags too, and a double-click fits it again); while zoomed in, a button in its top right corner shows the zoom and fits the whole field again. The view keeps the field filling it, so it cannot be lost, and interactive robots still get battlefield coordinates. The results window copies its table, and saves it as CSV (or as text, to a `.txt` file). Stop ends a live battle with the results so far, as classic's stop; a replay can be scrubbed through. Replay Battle opens (with Open, the folder button, the command line or a file dropped on the window) both a full recording, re-run exactly with debug paint and output, and a website replay: one round's lossy viewer track (ADR 0017), which plays what the viewer draws (positions, headings, energy and bullets) without names, scores, output or debug paint. They are told apart by their first bytes (gzip for a recording). `replay-size --write DIR` writes tracks from oracle recordings.

## Repository layout

- `crates/`: Rust workspace (engine, fdlibm, java-random, geom, protocol, server, runner, difftest, cli, gui, replay)
- `proto/`: wire protocol schema
- `clients/java-compat/`: the robot host for classic Java robots (classic's robot side over the robocode-s protocol)
- `clients/java-determinism/`: Java agent for per-robot seeded randomness
- `robots/samples/`: classic sample robots (EPL-2.0)
- `tools/difftest/`: oracle that records classic battles for differential testing
- `web/`: the rumble website and user documentation (SvelteKit, static)
- `docs/`: [design](docs/design.md), [engine specification](docs/engine-spec.md) and [architecture decision records](docs/adr/README.md); the rumble's [data files](docs/rumble-data.md)

## License

[Eclipse Public License 2.0](LICENSE)
