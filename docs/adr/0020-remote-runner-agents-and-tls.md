# 0020. Remote runner agents and TLS

- Status: Accepted
- Date: 2026-10-22

## Context

A battle's robots should be able to run on other machines (#72): to spread a melee's JVMs over several computers, or to run robots on another platform. Since ADR 0019 a robot the runner starts talks over its stdio and has no network, so the robots on another machine need something there that starts them and carries their stdio to the server. Observers from elsewhere (`--observe`) used plain TCP: a token and the whole battle in the clear, off loopback.

The protocol already had a runner role (`ROLE_RUNNER`, `RunnerInfo`, `LaunchRobot`, `LaunchResult`, `StopRobot`, `RobotExited`), designed for this and never implemented.

## Decision

- **TLS off loopback.** Every listener that isn't on loopback uses TLS (rustls, TLS 1.3, ring: both were in the tree, through `ureq`), and a plain one off loopback is refused (`net::check_listener`). A plain connection is accepted only from a loopback peer, and a client connects plain only to a loopback address without a fingerprint. That covers the observers' listener (`--observe`, the GUI's Watch tab), the new agent listener, and the agents' robots. The in-process robots' listener stays plain on loopback.
- **The server's certificate:** a self-signed one (ECDSA P-256, `CN=robocode-s`, no expiry), made once and kept in the data folder (`tls/server-cert.der`, and the key `tls/server-key.der`, readable by the user only). rustls has no certificate maker and `rcgen` isn't in the tree, so `tls::self_signed` writes the few lines of DER itself and signs them with ring. Its SHA-256 fingerprint is printed at start (`certificate sha256:AB:CD:...`). Clients pin it (`--fingerprint`, the GUI's Fingerprint field), as SSH pins host keys: exactly that certificate is accepted, whatever its name or dates. `--tls-cert`/`--tls-key` take a CA-issued certificate (PEM) instead, which a client without a fingerprint verifies against the Mozilla roots (`webpki-roots`, also in the tree). `--tls` uses TLS on loopback too.
- **The runner role is the agent's role**, rather than a new `ROLE_AGENT` (a deviation from #72's plan: one role per concept, and the messages fit). Additions only, protocol 1.1: `RunnerInfo` gets `engine_version`, `deterministic_rules`, `platform`, `slots`; `LaunchRobot` gets `jars` (`JarRef`: name, SHA-256, size) and `lookup_name`; `robot_id` is the robot's index as decimal text; `JarRequest` and `JarData` are new. `RobotPackage` and `server_address` stay unused.
- **The agent** (`robocode-s agent --server HOST:PORT --token T [--fingerprint F] [--sandbox S] [--slots N]`) connects and says `Hello` as a runner with the agent token, its engine version, the deterministic rules of its robot host (from its `--describe` listing, which now reports them too), its platform and its slots. Then it waits.
  - For each `LaunchRobot` it fetches the jars it doesn't have (`JarRequest`, then `JarData` in 1 MiB parts), checks their size and hash, and caches them by hash (`agent/jars/<sha256>/<name>`).
  - It starts the robot in its sandbox with stdio, as a local runner does. It reads the robot's `Hello`, puts in the robot's one-time token, and says it to the server on a TLS connection of its own to the same listener. It then relays bytes both ways, flushing every write (lockstep), and answers `LaunchResult`.
  - On `StopRobot` it gives the robot a moment to end (its stdin closed with the battle), kills it, and sends its stderr tail in `RobotExited`.
  - It connects again after a battle and after a dropped connection, with backoff up to 30 s. A refusal (token, versions, certificate) ends it.
- **The server refuses** an agent with the wrong token (after a delay), another engine version or other deterministic rules, with an `Error` that says which.
- **The battle side** (`robocode-s battle --agents ADDR --agent-token T [--wait-agents N]`) prepares the battle as before: it has the jars and describes the robots in its own sandbox.
  - It waits up to `--agent-wait` (60 s) for N agents and assigns the robots round robin over the agents with a free slot.
  - It tells the listener which tokens to expect, sends the `LaunchRobot`s, serves only the battle's jars (any other hash is refused and the agent cut off), and waits for every `LaunchResult` before the battle starts. Robots that connect earlier are queued.
  - At the end it sends `StopRobot`s and collects the stderr tails for the battle's report. The report says which agent each robot ran on.
  - Class folders can't go to agents.
- **A dropped agent** ends its robots' connections: they are then like crashed robots (`RobotDisconnected`), and the battle goes on.
- **Trust (design §7.3):** an agent that presents the agent token is the organizer's, so its robot hosts' own measurements count as a local robot's: their counted work decides skipped turns in deterministic mode, and their CPU times go to the observers. In classic mode the turn deadline is the server's wall clock, so the network's round trip counts against an agent's robots. There is no untrusted-agent mode.
- **TLS streams** (`net::Stream`) behave like `TcpStream`s (timeouts, clones for a reader thread): the clones share the session. A reader waits on the socket without holding it and then decrypts, and a writer encrypts and sends each message whole. The 10 s `Hello` timeout covers the TLS handshake too.
- The GUI's Start Battle page doesn't use agents yet (CLI only).

## Consequences

- Robots on another machine, on another platform (a Windows agent in an AppContainer for a Linux battle), give the battle the same outcome as local robots in deterministic mode.
- Observers off loopback need the fingerprint (or a CA's certificate), and an observer's token no longer travels in the clear.
- The certificate is long-lived and self-signed: deleting `tls/` in the data folder makes a new one, which clients must pin again.
- No new crates: rustls, ring and webpki-roots were already compiled in for the JRE download.
- Warm pools of robot processes (#82) are left for later.
