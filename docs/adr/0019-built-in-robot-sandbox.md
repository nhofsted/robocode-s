# 0019. Robots run in a built-in OS sandbox; Docker is an alternative

- Status: Accepted
- Date: 2026-10-19

## Context

Rumble volunteers run robot jars that anyone can submit, so robots are untrusted code. Classic contained them with Java's SecurityManager; Java 21 has none left to use, and robocode-s runs classic's robot side with `NOSECURITY=true` (§8). The design had Docker as the sandbox (§6.1, §6.3), but on Windows and macOS that means Docker Desktop (a large install, free only for personal use and small companies, WSL2 on Windows), which would keep many volunteers out.

The operating systems have sandboxes of their own that need no install: Landlock and seccomp on Linux (Chrome and bubblewrap use the same), AppContainers and Job Objects on Windows (Edge, Office). What makes them hard to use is a robot's network: it has to reach the server and nothing else.

## Decision

- **Robots talk to the server over their stdin and stdout** when the runner starts them: the same length-delimited protocol messages as over TCP, on pipes the runner creates. A robot then needs no network at all, so every sandbox can deny all of it. The pipe pair is the robot's identity: no token. TCP with tokens stays for remote robots and remote runner agents. The robot host keeps the pipes to itself and sends anything written to `System.out` outside classic's robot streams to stderr.
- **Built-in sandbox (`--sandbox os`)**, applied by the runner when it starts a robot process:
  - **Linux:** no new privileges; Landlock limits the filesystem to reading the JRE, the robot host, the agent and the robot's jars and writing the robot's home and data folder, and denies TCP bind and connect (ABI 4) and abstract Unix sockets and signals outside the sandbox (ABI 6); seccomp denies sockets other than Unix ones and syscalls a robot has no use for (ptrace, mount, keyctl, bpf, ...); rlimits and the JVM's heap limit bound memory, processes and files. It needs no user namespaces (which Ubuntu restricts) and no root. A kernel without Landlock (before 5.13, or with Landlock off) has no OS sandbox.
  - **Windows:** an AppContainer process with no capabilities (no network, no access to the user's files) and the JRE, robot host, jars and the robot's home granted to it; a Job Object limits memory, CPU and processes and kills the robot with the battle.
  - **macOS:** no OS sandbox for now (`sandbox-exec` is deprecated); Docker, or none.
- **Docker (`--sandbox docker`)** is the alternative, for remote runner agents, organisers who prefer it and macOS: `robocode-s-runner-jvm` with the pinned JRE, robot host and agent, started with `docker run -i --network none --read-only`, the robot's jars mounted read-only and its home writable, with CPU, memory and pids limits. Its stdio carries the protocol too, so it needs no relay container (replacing §6.3's decision).
- **`--sandbox none`** is today's plain local process. The default is `os` where the platform has it, otherwise `none`. The rumble client requires `os` or `docker`.

## Consequences

- No hard Docker dependency; Linux and Windows volunteers need nothing beyond robocode-s.
- The runner has a sandbox per OS to maintain, and it is ours, less tried than Docker's: everything is denied unless a robot needs it, and tests check that a sandboxed robot can't open a socket or read outside its grants.
- Robots in a sandbox can't open network connections even to localhost, and can't read outside their grants; classic's SecurityManager denied the same to rumble robots.
