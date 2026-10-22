// SPDX-License-Identifier: EPL-2.0

//! Connections over TCP, plain on loopback and TLS otherwise (design §5.1, ADR 0020).
//!
//! A [`Stream`] is used as a `TcpStream` is: blocking reads and writes, timeouts, and
//! clones for a reader thread beside the writer. A TLS session can't be split in two, so
//! the clones share it: a reader waits for the socket without holding the session, then
//! takes it to decrypt what came; a writer takes it to encrypt and send a message. One
//! reader at a time per connection.

use crate::tls::{Fingerprint, Identity, client_config, server_name};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A connection: plain TCP, or TLS over it.
#[derive(Debug)]
pub struct Stream {
    socket: TcpStream,
    tls: Option<Arc<Mutex<Session>>>,
}

#[derive(Debug)]
struct Session {
    conn: rustls::Connection,
    /// Decrypted, not yet read.
    plain: VecDeque<u8>,
    /// The peer closed the connection.
    closed: bool,
}

impl Session {
    fn new(mut conn: rustls::Connection) -> Self {
        // Messages of up to 16 MiB are written whole; the socket's own buffer holds back.
        conn.set_buffer_limit(None);
        Self {
            conn,
            plain: VecDeque::new(),
            closed: false,
        }
    }

    /// Decrypts `raw` from the socket and sends what the session has to answer.
    fn feed(&mut self, mut raw: &[u8], socket: &TcpStream) -> io::Result<()> {
        while !raw.is_empty() {
            self.conn.read_tls(&mut raw)?;
            self.process(socket)?;
        }
        self.send(socket)
    }

    /// Takes in what the session read and moves its plaintext to `plain`.
    fn process(&mut self, socket: &TcpStream) -> io::Result<()> {
        let state = match self.conn.process_new_packets() {
            Ok(state) => state,
            Err(e) => {
                // The alert that says why.
                let _ = self.send(socket);
                return Err(io::Error::new(io::ErrorKind::InvalidData, e));
            }
        };
        let mut bytes = vec![0; state.plaintext_bytes_to_read()];
        self.conn.reader().read_exact(&mut bytes)?;
        self.plain.extend(bytes);
        if state.peer_has_closed() {
            self.closed = true;
        }
        Ok(())
    }

    fn send(&mut self, mut socket: &TcpStream) -> io::Result<()> {
        while self.conn.wants_write() {
            self.conn.write_tls(&mut socket)?;
        }
        Ok(())
    }
}

impl Stream {
    /// A plain connection.
    pub fn plain(socket: TcpStream) -> Self {
        Self { socket, tls: None }
    }

    /// Whether the connection is encrypted.
    pub fn is_tls(&self) -> bool {
        self.tls.is_some()
    }

    /// The server's side of a new connection: TLS with `identity`, otherwise plain, which
    /// only a peer on loopback may have. The handshake has `timeout`.
    pub fn accept(
        socket: TcpStream,
        identity: Option<&Identity>,
        timeout: Duration,
    ) -> io::Result<Self> {
        socket.set_nonblocking(false)?;
        socket.set_nodelay(true)?;
        let Some(identity) = identity else {
            if !socket.peer_addr()?.ip().is_loopback() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "plain connections only from loopback",
                ));
            }
            return Ok(Self::plain(socket));
        };
        let conn = rustls::ServerConnection::new(identity.config.clone())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::handshake(socket, conn.into(), timeout)
    }

    fn handshake(
        socket: TcpStream,
        conn: rustls::Connection,
        timeout: Duration,
    ) -> io::Result<Self> {
        let mut session = Session::new(conn);
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;
        let mut io = &socket;
        while session.conn.is_handshaking() {
            session.conn.complete_io(&mut io)?;
        }
        // What came with the handshake's last flight (the client's first message).
        session.process(&socket)?;
        session.send(&socket)?;
        socket.set_read_timeout(None)?;
        socket.set_write_timeout(None)?;
        Ok(Self {
            socket,
            tls: Some(Arc::new(Mutex::new(session))),
        })
    }

    /// Another handle on the same connection (for a reader thread), with timeouts of its
    /// own.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            socket: self.socket.try_clone()?,
            tls: self.tls.clone(),
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_write_timeout(timeout)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.socket.read_timeout()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.socket.peer_addr()
    }

    /// Ends the connection for every handle (a TLS one says so first, if it can at once).
    pub fn shutdown(&self) -> io::Result<()> {
        if let Some(Ok(mut session)) = self.tls.as_ref().map(|t| t.try_lock()) {
            session.conn.send_close_notify();
            let _ = self
                .socket
                .set_write_timeout(Some(Duration::from_millis(100)));
            let _ = session.send(&self.socket);
        }
        self.socket.shutdown(Shutdown::Both)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(tls) = &self.tls else {
            return self.socket.read(buf);
        };
        loop {
            {
                let mut session = tls.lock().unwrap();
                if !session.plain.is_empty() {
                    return session.plain.read(buf);
                }
                if session.closed || buf.is_empty() {
                    return Ok(0);
                }
            }
            let mut raw = [0u8; 16 * 1024];
            let n = (&self.socket).read(&mut raw)?;
            let mut session = tls.lock().unwrap();
            if n == 0 {
                session.closed = true;
            } else {
                session.feed(&raw[..n], &self.socket)?;
            }
        }
    }
}

impl Write for Stream {
    /// Takes all of `buf`: a message written with one call goes out whole, even with
    /// writers on two threads.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(tls) = &self.tls else {
            return self.socket.write(buf);
        };
        let mut session = tls.lock().unwrap();
        session.conn.writer().write_all(buf)?;
        session.send(&self.socket)?;
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match &self.tls {
            None => self.socket.write_all(buf),
            Some(_) => self.write(buf).map(|_| ()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

/// Whether every address `address` (`host:port`) stands for is a loopback one.
pub fn is_loopback(address: &str) -> Result<bool, String> {
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .map_err(|e| format!("{address}: {e}"))?
        .collect();
    Ok(!addresses.is_empty() && addresses.iter().all(|a| a.ip().is_loopback()))
}

/// A listener that takes plain connections must be on loopback: elsewhere it needs TLS.
pub fn check_listener(listener: &TcpListener, identity: Option<&Identity>) -> io::Result<()> {
    let address = listener.local_addr()?;
    if identity.is_none() && !address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{address} is reachable from other machines: it needs TLS"),
        ));
    }
    Ok(())
}

/// Connects to `address` (`host:port`): TLS when `pin` is given or the server isn't on
/// loopback, else plain. TLS trusts the certificate with the fingerprint `pin`, else a
/// CA-issued one for the host (the Mozilla roots). Connecting and the TLS handshake each
/// have `timeout`.
pub fn connect(
    address: &str,
    pin: Option<Fingerprint>,
    timeout: Duration,
) -> Result<Stream, String> {
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .map_err(|e| format!("{address}: {e}"))?
        .collect();
    let mut error = format!("{address}: no address");
    let mut socket = None;
    for a in addresses {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                socket = Some(s);
                break;
            }
            Err(e) => error = format!("{address}: {e}"),
        }
    }
    let socket = socket.ok_or(error)?;
    let io = |e: io::Error| format!("{address}: {e}");
    socket.set_nodelay(true).map_err(io)?;
    let loopback = socket.peer_addr().map_err(io)?.ip().is_loopback();
    if pin.is_none() && loopback {
        return Ok(Stream::plain(socket));
    }
    let config = client_config(pin)?;
    let conn = rustls::ClientConnection::new(config, server_name(address)?)
        .map_err(|e| format!("{address}: {e}"))?;
    Stream::handshake(socket, conn.into(), timeout).map_err(|e| {
        let hint = if pin.is_none() && e.to_string().contains("UnknownIssuer") {
            ": a self-signed certificate needs its fingerprint (the server prints it), \
             --fingerprint sha256:..."
        } else {
            ""
        };
        format!("{address}: TLS: {e}{hint}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// A fresh identity, in a folder of its own: tests running at once each make one.
    fn identity() -> Identity {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("robocodes-net-{}-{n}", std::process::id()));
        let identity = Identity::load_or_create(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        identity
    }

    #[test]
    fn plain_only_on_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(check_listener(&listener, None).is_ok());
        assert!(is_loopback("localhost:1").unwrap());
        assert!(is_loopback("127.0.0.1:1").unwrap());
        assert!(!is_loopback("0.0.0.0:1").unwrap());
        assert!(!is_loopback("10.1.2.3:1").unwrap());
        if let Ok(open) = TcpListener::bind("0.0.0.0:0") {
            assert!(check_listener(&open, None).is_err());
            assert!(check_listener(&open, Some(&identity())).is_ok());
        }
    }

    #[test]
    fn tls_carries_messages_both_ways_between_clones() {
        let identity = identity();
        let pin = identity.fingerprint;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut stream =
                Stream::accept(socket, Some(&identity), Duration::from_secs(5)).unwrap();
            assert!(stream.is_tls());
            let mut reader = stream.try_clone().unwrap();
            // A message larger than a TLS record, and one each way at once.
            let echo = thread::spawn(move || {
                let mut got = vec![0; 100_000];
                reader.read_exact(&mut got).unwrap();
                got
            });
            stream.write_all(&vec![7u8; 50_000]).unwrap();
            echo.join().unwrap()
        });
        let mut client = connect(&address, Some(pin), Duration::from_secs(5)).unwrap();
        assert!(client.is_tls());
        let message: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        client.write_all(&message).unwrap();
        let mut back = vec![0; 50_000];
        client.read_exact(&mut back).unwrap();
        assert!(back.iter().all(|b| *b == 7));
        assert_eq!(server.join().unwrap(), message);
        // Without a pin, a loopback connection is plain.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let plain = connect(
            &listener.local_addr().unwrap().to_string(),
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!plain.is_tls());
    }

    #[test]
    fn a_wrong_pin_or_no_pin_for_a_self_signed_certificate_is_refused() {
        let identity = identity();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        thread::spawn(move || {
            for _ in 0..2 {
                let (socket, _) = listener.accept().unwrap();
                let _ = Stream::accept(socket, Some(&identity), Duration::from_secs(5));
            }
        });
        let wrong = Fingerprint([0xab; 32]);
        let e = connect(&address, Some(wrong), Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("not the pinned"), "{e}");
        // `localhost` resolves to loopback: plain. A CA check is what a pin-less TLS
        // connection does, so test it against the certificate directly.
        let config = client_config(None).unwrap();
        let conn = rustls::ClientConnection::new(config, server_name(&address).unwrap()).unwrap();
        let socket = TcpStream::connect(&address).unwrap();
        let e = Stream::handshake(socket, conn.into(), Duration::from_secs(5)).unwrap_err();
        assert!(e.to_string().contains("UnknownIssuer"), "{e}");
    }
}
