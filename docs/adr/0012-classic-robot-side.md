# 0012. Java robots run on classic Robocode's own robot side

- Status: Accepted
- Date: 2026-10-11

## Context

Classic Java robots must run unchanged and behave exactly as in classic (ADR 0003). The robot-side API is large and subtle:
- `Robot`, `AdvancedRobot`, `TeamRobot`, `JuniorRobot`, `RateControlRobot`, `Droid`;
- blocking calls, custom events, event priorities and interruption;
- team messages, data files, painting.

Classic implements it in robot proxies that talk to the engine through one interface, `IRobotPeer` (`executeImpl`).

## Decision

The robot host (`clients/java-compat`) runs **classic's own robot side**: `robocode.api`, `core`, `host` and `repository` 1.11.1 from Maven Central (EPL), and `battle` for the container setup. It replaces only the peer, with `RemotePeer`, which speaks the robocode-s protocol and follows classic's `RobotPeer.executeImpl`:
- the commands are sent first;
- dead or halted robots then get `DeathException`, `WinException` or `AbortedException`;
- otherwise the next `Tick` becomes classic's `ExecResults`.

Crashes (`drainEnergy`) and misbehavior (`punishBadBehavior`) are reported in `Execute` (`drain_energy`, `stopped`, `disable`). The peer measures the robot thread's CPU time per turn.

The host runs with `NOSECURITY=true`, so classic's engine classes and our peer share one class loader. Classic's package and thread checks are therefore off. robocode-s isolates robots by process and container instead (design §6).

## Consequences

- **Verified:** every recorded classic battle is identical when fought live through the runner, robot host, server and engine. That covers 9 sample battles (melee, teams, sentry, JuniorRobot) and 4 top-bot battles, 124,786 turns.
- **Robot side tied to classic 1.11.1:** updating it means updating these jars.
- **Deviation:** a robot that would violate classic's security checks is not disabled for it.
