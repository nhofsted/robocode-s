// SPDX-License-Identifier: EPL-2.0

//! Watching a battle on a server as an observer, from this process or another machine
//! (design §10: a remote GUI attached as an observer). The server's side is
//! [`crate::Observers`].

use crate::BattleOutcome;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

/// How long connecting and the server's greeting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to a battle as an observer, after the server greeted it.
pub struct Watcher {
    stream: TcpStream,
}

impl Watcher {
    /// Connects to the battle at `address` (`host:port`) with the observer `token` (empty
    /// for a server that wants none).
    pub fn connect(address: &str, token: &str) -> Result<Self, String> {
        let addresses: Vec<_> = address
            .to_socket_addrs()
            .map_err(|e| format!("{address}: {e}"))?
            .collect();
        let mut error = format!("{address}: no address");
        let mut stream = None;
        for a in addresses {
            match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => error = format!("{address}: {e}"),
            }
        }
        let mut stream = stream.ok_or(error)?;
        let io = |e: std::io::Error| format!("{address}: {e}");
        stream.set_nodelay(true).map_err(io)?;
        stream.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(io)?;
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello {
                version: Some(PROTOCOL_VERSION),
                role: v1::Role::Observer as i32,
                token: token.into(),
                ..Default::default()
            })),
        };
        write_message(&mut stream, &hello).map_err(io)?;
        match read_message::<v1::ServerMessage>(&mut stream)
            .map_err(io)?
            .and_then(|m| m.msg)
        {
            Some(server_message::Msg::HelloAck(_)) => {}
            Some(server_message::Msg::Error(e)) => {
                return Err(format!("{address} refused: {}", e.message));
            }
            _ => return Err(format!("{address}: not a robocode-s battle")),
        }
        stream.set_read_timeout(None).map_err(io)?;
        Ok(Self { stream })
    }

    /// A handle that ends the connection when shut down (from another thread).
    pub fn handle(&self) -> std::io::Result<TcpStream> {
        self.stream.try_clone()
    }

    /// Watches the battle to its end: hands every message to `observe` (`BattleInfo`
    /// first) and sends `controls` (pause, step, speed, paint, stop, kill, input) as they
    /// come. The outcome comes from the server's `BattleOver`.
    pub fn run(
        self,
        controls: Option<Receiver<v1::ObserverControl>>,
        mut observe: impl FnMut(&server_message::Msg),
    ) -> Result<BattleOutcome, String> {
        let mut reader = self.stream;
        if let Some(controls) = controls {
            let mut writer = reader.try_clone().map_err(|e| e.to_string())?;
            thread::spawn(move || {
                for control in controls {
                    let message = v1::ClientMessage {
                        msg: Some(client_message::Msg::ObserverControl(control)),
                    };
                    if let Err(e) = write_message(&mut writer, &message) {
                        crate::log_perf(format_args!("window can't send controls anymore: {e}"));
                        break;
                    }
                }
            });
        }
        loop {
            let msg = match read_message::<v1::ServerMessage>(&mut reader) {
                Ok(Some(v1::ServerMessage { msg: Some(msg) })) => msg,
                Ok(Some(_)) => continue,
                Ok(None) => return Err("the server ended the connection".into()),
                Err(e) => return Err(format!("the connection to the server broke: {e}")),
            };
            observe(&msg);
            match msg {
                server_message::Msg::BattleOver(over) => {
                    return Ok(BattleOutcome {
                        results: over.results,
                        rounds: over.rounds,
                        aborted: over.aborted,
                    });
                }
                server_message::Msg::Error(e) => return Err(e.message),
                _ => {}
            }
        }
    }
}
