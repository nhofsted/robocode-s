# robocode-s

A reimplementation of [Robocode](https://robocode.sourceforge.io/) with a Rust battle engine, a GPU-accelerated battlefield viewer, and a client/server architecture where robots run out of process: locally, in containers, or on remote hosts.

- **Bit-exact compatibility** with classic Java robots (`robocode.*` API, reference version 1.11.1)
- **Robots in any language**: Java/Kotlin, Python, C#/.NET, Rust, C/C++
- **Isolation & resource limits** through pluggable launchers (local process, Docker, remote)
- Runs on **Windows, Linux and macOS**
- **Distributed RoboRumble** with a serverless AWS backend and a web interface that replays battles in the browser

> Status: early design phase. See [docs/design.md](docs/design.md).

## Installing

Release archives (one per platform, from the [releases page](https://github.com/nhofsted/robocode-s/releases)) hold both programs, the robot host, the determinism agent and the sample robots: unpack one anywhere and run `robocode-s-gui` or `robocode-s` from it; the programs find their files next to themselves, from any current folder (and through symlinks). The Java runtime for the robots is downloaded on first use into the user's data folder (`~/.local/share/robocode-s`, `~/Library/Application Support/robocode-s`, `%LOCALAPPDATA%\robocode-s`), never into the unpacked folder. The archives' `README.txt` says more: the sandbox of each platform, the unsigned programs (Windows SmartScreen, macOS `xattr -dr com.apple.quarantine .`), the Linux menu entry (`share/install-desktop.sh`). Linux binaries need glibc 2.31 or newer. `tools/release/release.sh build` makes the archives (design §6.7). The rest of this page is about building from the source tree.

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

Every robot runs in its own JVM; a pinned JRE is downloaded on first use. Battles are deterministic by default: the same seed gives the same battle. `--classic` uses classic's timing instead, where slow robots skip turns. Other robots: `--jar path/to/robot.jar`, or `--jar path/to/classes` for a folder of class files such as an IDE's output (`bin`, `build/classes/java/main`), read in place as classic's development folders are: its robots are named with classic's `*` (`my.Robot*`, or `my.Robot 1.0*` with a version in its `.properties`), and a recompiled robot is new in the next battle. `ROBOCODES_ROBOT_JARS` (a path list) takes jars and class folders too. A classic `.battle` file gives the robots and rules: `--battle FILE`. Robots are code from anyone, so they run in a sandbox: `--sandbox os`, the default on Linux and Windows, is on Linux seccomp and, where the kernel has it, Landlock, on Windows an AppContainer and a Job Object (no network, no files but the robot's own; the battle's header says which), `--sandbox bwrap` uses bubblewrap where it is installed, `--sandbox docker` runs each robot in a container of the official Temurin JRE image (no network, a read-only root; on every platform, but only if Docker is installed and running: nothing else needs it, and the image is pulled the first time), and `--sandbox none` is a plain process (design §6.6). All options: `robocode-s battle --help`.

`--record FILE` records a battle (`--paint` includes the robots' debug painting; a battle stopped before its first round, such as a start cancelled in the GUI, leaves no file); `robocode-s replay FILE` re-runs it, checks every turn and prints the results. A recording keeps the engine version it ran with (`robocode-s --version` shows it); a replay by another version warns.

`--observe PORT` lets the GUI watch the battle, from this machine or another (Watch Battle, Remote Server, or `robocode-s-gui --watch HOST:PORT`): the battle starts once the first observer connected, and observers control it as in a live battle (pause, speed, stop, kill, input). A port alone listens on 127.0.0.1 only; to listen on other addresses (`--observe 0.0.0.0:7654`) observers need a token, `--observe-token TOKEN` (or `ROBOCODES_OBSERVE_TOKEN`; the GUI's Token field, or `--token`). The connection is not encrypted, so use a trusted network, or an SSH tunnel to the loopback port (`ssh -L 7654:localhost:7654 host`). Every process on the machine reaches a loopback port, so on a shared machine give it a token too (the battle's own connections, its robots and the built-in observer, get fresh secret tokens each battle).

## The GUI

```sh
cargo run --release -p robocode-s-gui                 # battle setup, then the live battle
cargo run --release -p robocode-s-gui -- battle.rsr   # replay a recording
```

The window has tabs: Start Battle (the setup, then the live battle), Watch Battle (a recording from a file, or a battle on a server) and, later, Run Rumble; the gear opens the options: the robot folders (scanned for `.jar` files; a folder holding class files, such as an IDE's output, is also read in place as a development folder, so a recompiled robot is new in the next battle) and the sandbox robots run in (as `--sandbox`; the loading screen shows it), what the battlefield shows (scan arcs, coloured bullets, robot names and energy, explosions, the floor, the sentry border, the turns and frames per second) and sound (classic's gunshot, hit, explosion and collision sounds, with a volume; off by default, as in classic), and the defaults for new battles: mode, rounds, battlefield size and the other rules (gun cooling rate, inactivity time, sentry border, hidden enemy names). They are saved in `~/.config/robocode-s/gui.json` (the platform's config directory; `ROBOCODES_GUI_CONFIG` names another file). The battle setup lists the robots in the robot folders and in `ROBOCODES_ROBOT_JARS` (listed again whenever the setup shows, and with Rescan in the options) beside the ones in the battle (double-click to add or remove; a robot may be in it more than once; a click shows its author, description, web page and the Robocode version it was built for from its `.properties`, with its language and platform, Java on the robot host's JVM, as Tank Royale shows them), with a game type (Classic, 1v1, Melee or Teams, each setting its rounds, battlefield size and the default rules; Custom otherwise) and the rules (rounds, battlefield size, gun cooling rate, inactivity time, sentry border, hidden enemy names) and seed below, and a classic `.battle` file to open or save them to. While a battle starts, a loading screen shows the contestants (with Cancel), or why the battle couldn't start. The battle screen, live or replayed, has the battlefield with its controls below it (stop, pause and play, single steps, the speed, and the round, turn and seed), the robots' output below those in a tab per robot (the pencil in a tab turns that robot's debug painting on and off; the buttons right of the tabs copy, save and clear the open one, and in a live battle kill the robot, after a second click to confirm; the debug properties a robot set with `setDebugProperty` show beside its output), and the robots with their energy and score on the right. In a live battle, interactive robots (such as `sample.Interactive`) get the mouse over the battlefield and the keys while it has focus (the pointer is over it, or it was clicked last), as in classic. The battlefield zooms and pans for large fields: Ctrl+wheel zooms around the pointer and Ctrl+drag pans (in a replay, where no robot reads the mouse, the plain wheel and drags too, and a double-click fits it again); while zoomed in, a button in its top right corner shows the zoom and fits the whole field again. The view keeps the field filling it, so it cannot be lost, and interactive robots still get battlefield coordinates. The results window copies its table, and saves it as CSV (or as text, to a `.txt` file). Stop ends a live battle with the results so far, as classic's stop; a replay can be scrubbed through. Watch Battle has two sections. From File opens (with Open, the folder button, the command line or a file dropped on the window) both a full recording, re-run exactly with debug paint and output, and a website replay: one round's lossy viewer track (ADR 0017), which plays what the viewer draws (positions, headings, energy and bullets) without names, scores, output or debug paint. They are told apart by their first bytes (gzip for a recording). A recording is replayed once as it opens, which takes a moment for a long battle: meanwhile the tab shows how far it got, with Cancel. Remote Server connects to a battle another `robocode-s` runs with `--observe` (its address and token, if it has one; see above) and shows it on the same battle screen, with its controls; Leave (or Done at its end) leaves that battle running and goes back to the Watch Battle page. A watched battle and a replay take turns in the tab: the one opened last replaces the other, while a battle started on Start Battle runs on beside them. `replay-size --write DIR` writes tracks from oracle recordings.

On Windows (and under WSLg) the app draws its own window frame, as Steam does: the top bar is the title bar (drag it to move the window, double-click it to maximize), with the minimize, maximize and close buttons on the right; the window keeps the system's drop shadow and snapping (Win+arrows, dragging it to a screen edge), without Windows 11's 1px border. `ROBOCODES_OWN_FRAME=0` gives the system's frame instead. The app is dark whatever the system's theme. A release build opens no console window; started from a terminal (Command Prompt, PowerShell) it writes `--help`, its errors and the `ROBOCODES_*_LOG` diagnostics there, though the prompt comes back at once (`start /wait robocode-s-gui.exe ...` waits for it; a debug build is a console program).

The app's icon is classic Robocode's, as the website's favicon (`crates/gui/assets/README.md`): the window has it at run time (taskbar, Alt+Tab, X11), and the Windows program has it as its file icon (Explorer, Start menu) with its version information. Wayland desktops, WSLg among them, take a window's icon from the `.desktop` file named by its app id, `robocode-s`; to have it there, install `crates/gui/robocode-s.desktop` and the icon for your user (with `Path=` set to the folder to start in, see below, if the program isn't started from the repository):

```sh
install -Dm644 crates/gui/robocode-s.desktop ~/.local/share/applications/robocode-s.desktop
install -Dm644 web/static/favicon-256.png ~/.local/share/icons/hicolor/256x256/apps/robocode-s.png
# and robocode-s-gui on the PATH (Exec=robocode-s-gui), e.g. ~/.cargo/bin via cargo install --path crates/gui
```

To build the Windows program from Linux or WSL, with the `x86_64-pc-windows-gnu` target and MinGW-w64 (`rustup target add x86_64-pc-windows-gnu`, `apt install mingw-w64`):

```sh
CARGO_TARGET_DIR=target/windows cargo build --release -p robocode-s-gui --target x86_64-pc-windows-gnu
# target/windows/x86_64-pc-windows-gnu/release/robocode-s-gui.exe
```

A release archive has all this next to the program (see Installing); a program built by hand finds the robot host and the sample robots relative to the folder it is started in, as from the repository: copy `clients/java-compat/build/install/`, `clients/java-determinism/build/libs/` and `robots/samples/build/libs/` there in the same layout (or set `ROBOCODES_HOST_LIB`, `ROBOCODES_AGENT_JAR` and the robot folders). The pinned JRE is downloaded into `target\robocodes-data` there on first use. The `os` sandbox grants its robots the JRE, the robot host, the agent, the robots' jars and folders and their homes (in the data folder) by ACL entries, so these must be on NTFS: not on FAT, and not in WSL's files (`\\wsl$\...`) when the Windows program is started from WSL. The entries on the JRE and the robot host's folders stay (they let only robocode-s's own AppContainer, `robocode-s.robot`, read them); the homes' go with them.

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
