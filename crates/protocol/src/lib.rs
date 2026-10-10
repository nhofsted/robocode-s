// SPDX-License-Identifier: EPL-2.0

//! robocode-s wire protocol: generated message types (see `proto/`) and framing.

/// Version 1 of the protocol.
pub mod v1 {
    #![allow(clippy::all, missing_docs)]
    include!(concat!(env!("OUT_DIR"), "/robocode_s.v1.rs"));
}

/// The protocol version implemented by this crate.
pub const PROTOCOL_VERSION: (u32, u32) = (1, 0);

#[cfg(test)]
mod tests {
    use super::v1::{ClientMessage, Hello, Role, client_message::Msg};
    use prost::Message;

    #[test]
    fn hello_roundtrips_length_delimited() {
        let msg = ClientMessage {
            msg: Some(Msg::Hello(Hello {
                role: Role::Robot as i32,
                session_token: "token".into(),
                ..Default::default()
            })),
        };
        let bytes = msg.encode_length_delimited_to_vec();
        assert_eq!(
            ClientMessage::decode_length_delimited(&bytes[..]).unwrap(),
            msg
        );
    }
}
