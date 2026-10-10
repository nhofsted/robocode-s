# 0004. Protocol Buffers with length-delimited framing

- Status: Accepted
- Date: 2026-10-10

## Context

Robots are written in Java, Kotlin, Python, C#, Rust and C/C++, and the lockstep loop exchanges small messages every turn.

## Decision

The protocol is defined in proto3 (`proto/`) and sent as varint-length-delimited messages over TCP (TLS when not on loopback). The protocol is independent of Robocode Tank Royale; an adapter may be added later (design §5).

## Consequences

Code generation exists for every target language. JSON tooling is not available on the wire; the GUI and other tools use the same protobuf messages.
