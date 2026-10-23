// SPDX-License-Identifier: EPL-2.0

//! robocode-s wire protocol: generated message types (`proto/robocode_s/v1`) and framing.
//!
//! Every message on a connection is a [`v1::ClientMessage`] or [`v1::ServerMessage`],
//! preceded by its length as a protobuf varint ([`write_message`], [`read_message`]).

use prost::Message;
use std::io::{self, Read, Write};

/// Version 1 of the protocol.
pub mod v1 {
    #![allow(clippy::all, clippy::pedantic, missing_docs)]
    include!(concat!(env!("OUT_DIR"), "/robocode_s.v1.rs"));
}

/// The protocol version implemented by this crate.
pub const PROTOCOL_VERSION: v1::ProtocolVersion = v1::ProtocolVersion { major: 1, minor: 2 };

/// Messages larger than this are refused (16 MiB).
pub const MAX_MESSAGE_SIZE: usize = 16 << 20;

/// Writes one length-prefixed message.
pub fn write_message<M: Message>(writer: &mut impl Write, message: &M) -> io::Result<()> {
    writer.write_all(&message.encode_length_delimited_to_vec())
}

/// Reads one length-prefixed message. Returns `None` if the stream ends cleanly before a
/// new message starts; a stream ending inside a message is an error.
pub fn read_message<M: Message + Default>(reader: &mut impl Read) -> io::Result<Option<M>> {
    let Some(length) = read_length(reader)? else {
        return Ok(None);
    };
    if length > MAX_MESSAGE_SIZE as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message of {length} bytes exceeds the limit of {MAX_MESSAGE_SIZE}"),
        ));
    }
    let mut buffer = vec![0; length as usize];
    reader.read_exact(&mut buffer)?;
    M::decode(buffer.as_slice())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Reads a varint length prefix, or `None` at a clean end of stream.
fn read_length(reader: &mut impl Read) -> io::Result<Option<u64>> {
    let mut value: u64 = 0;
    for i in 0..10 {
        let mut byte = [0u8; 1];
        if reader.read(&mut byte)? == 0 {
            if i == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended inside a length prefix",
            ));
        }
        value |= u64::from(byte[0] & 0x7f) << (7 * i);
        if byte[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "length prefix longer than 10 bytes",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use v1::{
        ClientMessage, Event, Hello, Role, ServerMessage, Tick, client_message, event,
        server_message,
    };

    fn tick() -> ServerMessage {
        ServerMessage {
            msg: Some(server_message::Msg::Tick(Tick {
                turn: 42,
                status: Some(v1::RobotStatus {
                    energy: Some(99.5),
                    x: Some(18.000000000000004),
                    time: 42,
                    ..Default::default()
                }),
                events: vec![
                    Event {
                        kind: Some(event::Kind::ScannedRobot(v1::ScannedRobot {
                            name: "sample.Fire (2)".into(),
                            bearing: Some(-std::f64::consts::PI),
                            ..Default::default()
                        })),
                    },
                    Event {
                        kind: Some(event::Kind::Death(v1::Death {})),
                    },
                ],
                team_messages: vec![v1::TeamMessage {
                    sender: "leader".into(),
                    recipient: None,
                    payload: vec![0xac, 0xed, 0, 5],
                }],
                ..Default::default()
            })),
        }
    }

    #[test]
    fn messages_roundtrip_back_to_back() {
        let hello = ClientMessage {
            msg: Some(client_message::Msg::Hello(Hello {
                version: Some(PROTOCOL_VERSION),
                role: Role::Robot as i32,
                token: "token".into(),
                ..Default::default()
            })),
        };
        let mut stream = Vec::new();
        write_message(&mut stream, &tick()).unwrap();
        write_message(&mut stream, &tick()).unwrap();
        let mut reader = stream.as_slice();
        assert_eq!(
            read_message::<ServerMessage>(&mut reader).unwrap(),
            Some(tick())
        );
        assert_eq!(
            read_message::<ServerMessage>(&mut reader).unwrap(),
            Some(tick())
        );
        assert_eq!(read_message::<ServerMessage>(&mut reader).unwrap(), None);

        let mut stream = Vec::new();
        write_message(&mut stream, &hello).unwrap();
        assert_eq!(
            read_message::<ClientMessage>(&mut stream.as_slice()).unwrap(),
            Some(hello)
        );
    }

    #[test]
    fn runner_messages_roundtrip() {
        let launch = ServerMessage {
            msg: Some(server_message::Msg::LaunchRobot(v1::LaunchRobot {
                robot_id: "2".into(),
                session_token: "t".into(),
                robot_name: "sample.Fire (2)".into(),
                lookup_name: "sample.Fire".into(),
                battle_seed: -7,
                deterministic: true,
                memory_limit_mb: 512,
                jars: vec![v1::JarRef {
                    name: "samples.jar".into(),
                    sha256: "ab".repeat(32),
                    size: 47_612,
                }],
                ..Default::default()
            })),
        };
        let part = ServerMessage {
            msg: Some(server_message::Msg::JarData(v1::JarData {
                sha256: "ab".repeat(32),
                offset: 1 << 20,
                data: vec![0x50, 0x4b, 3, 4],
                last: true,
            })),
        };
        let hello = ClientMessage {
            msg: Some(client_message::Msg::Hello(Hello {
                version: Some(PROTOCOL_VERSION),
                role: Role::Runner as i32,
                token: "agent".into(),
                runner: Some(v1::RunnerInfo {
                    engine_version: 1,
                    deterministic_rules: 1,
                    platform: "JVM 21".into(),
                    slots: 4,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        };
        let mut stream = Vec::new();
        write_message(&mut stream, &launch).unwrap();
        write_message(&mut stream, &part).unwrap();
        let mut reader = stream.as_slice();
        assert_eq!(read_message(&mut reader).unwrap(), Some(launch));
        assert_eq!(read_message(&mut reader).unwrap(), Some(part));
        let mut stream = Vec::new();
        write_message(&mut stream, &hello).unwrap();
        assert_eq!(read_message(&mut stream.as_slice()).unwrap(), Some(hello));
    }

    #[test]
    fn doubles_are_exact() {
        let mut stream = Vec::new();
        write_message(&mut stream, &tick()).unwrap();
        let Some(ServerMessage {
            msg: Some(server_message::Msg::Tick(t)),
        }) = read_message(&mut stream.as_slice()).unwrap()
        else {
            panic!("expected a tick");
        };
        assert_eq!(
            t.status.unwrap().x.unwrap().to_bits(),
            18.000000000000004f64.to_bits()
        );
    }

    /// Doubles are `optional` in the proto because implicit presence loses -0.0 in prost.
    #[test]
    fn negative_zero_survives() {
        let message = v1::HitWall {
            bearing: Some(-0.0),
        };
        let decoded = v1::HitWall::decode(message.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.bearing.unwrap().to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn truncated_message_is_an_error() {
        let mut stream = Vec::new();
        write_message(&mut stream, &tick()).unwrap();
        stream.pop();
        let error = read_message::<ServerMessage>(&mut stream.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        let error = read_message::<ServerMessage>(&mut [0x80u8].as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_and_malformed_messages_are_refused() {
        // A length prefix of 1 GiB.
        let huge = [0x80, 0x80, 0x80, 0x80, 0x04];
        let error = read_message::<ServerMessage>(&mut huge.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let endless = [0xffu8; 11];
        let error = read_message::<ServerMessage>(&mut endless.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // Three bytes that are not a valid message (field 1 as a varint without value).
        let garbage = [3, 0x08, 0xff, 0xff];
        let error = read_message::<ServerMessage>(&mut garbage.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
