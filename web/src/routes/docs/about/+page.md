---
title: About robocode-s
description: What robocode-s is, why it exists, and how it relates to classic Robocode and Robocode Tank Royale.
---

robocode-s rebuilds [Robocode](https://robocode.sourceforge.io/) around three ideas:

- **The engine is separate from the robots.** The battle engine is written in Rust. Robots don't run inside it: every robot is its own process, which talks to the engine over a small, open protocol. A robot that crashes, hangs or eats memory only takes itself down.
- **Classic robots keep working.** Existing Java robot jars run unmodified, and a battle with the same seed plays out turn for turn exactly as in classic Robocode 1.11.1. That is verified by replaying recorded classic battles, including RoboRumble's top bots, and comparing every turn.
- **Robots can be written in any language.** Because the protocol is language-neutral, robots can also be written in Kotlin, Python, C#, Rust or C/C++, and run locally, in a container or on another machine.

On top of that come a GPU-accelerated desktop viewer, a deterministic mode where the same seed always gives the same battle, and [our own rumble](/docs/rumble) with a modern website and battle replays in the browser.

## Classic Robocode, Tank Royale and robocode-s

**Classic Robocode** (Mathew A. Nelson, Flemming N. Larsen and contributors) is the original: a Java application in which robots run as threads in the same JVM. Its RoboRumble has ranked community bots for two decades. robocode-s treats classic as the reference: same rules, same physics, same events, down to the last bit of every floating-point number.

**[Robocode Tank Royale](https://github.com/robocode-dev/tank-royale)** is the classic team's own next generation. Like robocode-s it runs robots out of process, in several languages. Its game API is different, though, so classic robots don't run on it. robocode-s puts running the existing robots exactly first. A bridge between the two protocols is on the wish list.

## Where the project stands

| Milestone | What | State |
|---|---|---|
| M0 | Repository, CI on Linux, Windows and macOS, design | Done |
| M1 | Headless engine, protocol, server, classic Java robots, bit-exact | In progress: engine, server and robot host fight real RoboRumble robots identically to classic; the command-line battle runner is next |
| M2 | Desktop GUI: battle setup, viewer, debug painting, replay | Planned |
| M3 | Isolation: Docker images, remote runners | Planned |
| M4 | Native APIs for Python, .NET, Rust, C/C++ and Java/Kotlin | Planned |
| M5 | The rumble: backend, volunteer client, rankings website | Planned (this website is its start) |
| M5b | Battle replays in the browser | Planned |

## License and contributing

robocode-s is open source under the [Eclipse Public License 2.0](https://www.eclipse.org/legal/epl-2.0/). The classic sample robots are bundled under the same license, with their original copyright.

The code, issues and design documents are on [GitHub](https://github.com/nhofsted/robocode-s). Bug reports and ideas are welcome there.
