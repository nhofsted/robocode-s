// SPDX-License-Identifier: EPL-2.0

//! TLS for connections from other machines (design §5.1, ADR 0020): observers and runner
//! agents. The server has a self-signed certificate made once and kept in the data
//! folder, or a CA-issued one (`--tls-cert`, `--tls-key`). Clients pin the self-signed
//! one by its SHA-256 fingerprint, as SSH pins host keys; without a pin they verify a
//! CA-issued one against the Mozilla roots (`webpki-roots`). TLS 1.3 only, ring.

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// The SHA-256 of a certificate (its DER), shown and given as `sha256:AB:CD:...`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(der: &[u8]) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, der);
        let mut bytes = [0; 32];
        bytes.copy_from_slice(digest.as_ref());
        Self(bytes)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::str::FromStr for Fingerprint {
    type Err = String;

    /// `sha256:` and the 32 bytes in hex, in either case, with or without colons; the
    /// prefix may be left out.
    fn from_str(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let hex = match trimmed.get(..7) {
            Some(p) if p.eq_ignore_ascii_case("sha256:") => &trimmed[7..],
            _ => trimmed,
        };
        let digits: Vec<u8> = hex
            .bytes()
            .filter(|b| *b != b':' && !b.is_ascii_whitespace())
            .collect();
        let bad = || format!("not a sha256: fingerprint (64 hex digits): {text}");
        if digits.len() != 64 {
            return Err(bad());
        }
        let mut bytes = [0; 32];
        for (i, pair) in digits.chunks(2).enumerate() {
            let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
            bytes[i] = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
        }
        Ok(Self(bytes))
    }
}

/// The server's certificate and key.
#[derive(Clone)]
pub struct Identity {
    pub(crate) config: Arc<rustls::ServerConfig>,
    /// Of the first certificate in the chain.
    pub fingerprint: Fingerprint,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identity({})", self.fingerprint)
    }
}

/// Where in the data folder the self-signed certificate and its key are kept.
const CERT_FILE: &str = "tls/server-cert.der";
const KEY_FILE: &str = "tls/server-key.der";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

impl Identity {
    /// The self-signed certificate in the data folder `data`, made the first time.
    pub fn load_or_create(data: &Path) -> Result<Self, String> {
        let (cert_path, key_path) = (data.join(CERT_FILE), data.join(KEY_FILE));
        let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
        if !cert_path.is_file() || !key_path.is_file() {
            let (cert, key) = self_signed()?;
            let dir = cert_path.parent().unwrap_or(data);
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            write_private(&key_path, &key)?;
            std::fs::write(&cert_path, &cert)
                .map_err(|e| format!("{}: {e}", cert_path.display()))?;
        }
        let cert = read(&cert_path)?;
        let key = read(&key_path)?;
        Self::new(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(|e| format!("{}: {e}", cert_path.display()))
    }

    /// A certificate chain and its key from PEM files (a CA-issued certificate).
    pub fn from_pem(cert: &Path, key: &Path) -> Result<Self, String> {
        let chain = CertificateDer::pem_file_iter(cert)
            .and_then(|i| i.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("{}: {e}", cert.display()))?;
        if chain.is_empty() {
            return Err(format!("{}: no certificate", cert.display()));
        }
        let key_der =
            PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
        Self::new(chain, key_der).map_err(|e| format!("{}: {e}", cert.display()))
    }

    fn new(
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Result<Self, String> {
        let fingerprint = Fingerprint::of(&chain[0]);
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            config: Arc::new(config),
            fingerprint,
        })
    }
}

/// Writes the key so that only the user can read it.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(error)
}

/// How a client trusts the server's certificate.
pub(crate) fn client_config(pin: Option<Fingerprint>) -> Result<Arc<rustls::ClientConfig>, String> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?;
    let config = match pin {
        Some(pin) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned {
                pin,
                provider: provider(),
            }))
            .with_no_client_auth(),
        None => builder
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// The name a client checks a CA-issued certificate for: the host of `host:port`.
pub(crate) fn server_name(address: &str) -> Result<ServerName<'static>, String> {
    let host = match address.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => host,
        _ => address,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(host.to_string()).map_err(|e| format!("{address}: {e}"))
}

/// Accepts exactly the certificate with the pinned fingerprint, whatever its name, issuer
/// or dates; its key must still sign the handshake.
#[derive(Debug)]
struct Pinned {
    pin: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = Fingerprint::of(end_entity);
        if presented == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "the server's certificate is {presented}, not the pinned {}",
                self.pin
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A new self-signed certificate (DER) and its ECDSA P-256 key (PKCS #8 DER), for
/// `CN=robocode-s`, valid from 2025 without end (RFC 5280's 99991231235959Z). rustls has
/// no certificate maker; this one is a few lines of DER.
pub fn self_signed() -> Result<(Vec<u8>, Vec<u8>), String> {
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
        .map_err(|_| "making a key failed".to_string())?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, key.as_ref(), &rng)
        .map_err(|_| "reading the new key failed".to_string())?;
    let mut serial = [0u8; 16];
    rng.fill(&mut serial)
        .map_err(|_| "no randomness".to_string())?;
    serial[0] = (serial[0] & 0x7f) | 0x40; // positive, and no leading zero byte
    // ecdsa-with-SHA256 (1.2.840.10045.4.3.2), without parameters.
    let algorithm = der(
        0x30,
        &der(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02]),
    );
    // CN (2.5.4.3) = robocode-s, as a UTF8String.
    let cn = [der(0x06, &[0x55, 0x04, 0x03]), der(0x0c, b"robocode-s")].concat();
    let name = der(0x30, &der(0x31, &der(0x30, &cn)));
    let validity = der(
        0x30,
        &[der(0x17, b"250101000000Z"), der(0x18, b"99991231235959Z")].concat(),
    );
    // id-ecPublicKey (1.2.840.10045.2.1), prime256v1 (1.2.840.10045.3.1.7).
    let spki_algorithm = der(
        0x30,
        &[
            der(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]),
            der(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]),
        ]
        .concat(),
    );
    let spki = der(
        0x30,
        &[spki_algorithm, bit_string(pair.public_key().as_ref())].concat(),
    );
    let tbs = der(
        0x30,
        &[
            der(0xa0, &der(0x02, &[2])), // v3
            der(0x02, &serial),
            algorithm.clone(),
            name.clone(),
            validity,
            name,
            spki,
        ]
        .concat(),
    );
    let signature = pair
        .sign(&rng, &tbs)
        .map_err(|_| "signing the certificate failed".to_string())?;
    let cert = der(
        0x30,
        &[tbs, algorithm, bit_string(signature.as_ref())].concat(),
    );
    Ok((cert, key.as_ref().to_vec()))
}

/// A DER element: tag, length, contents.
fn der(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = contents.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n
            .to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(contents);
    out
}

fn bit_string(bits: &[u8]) -> Vec<u8> {
    der(0x03, &[&[0u8][..], bits].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_read_what_they_show() {
        let f = Fingerprint::of(b"certificate");
        let shown = f.to_string();
        assert!(shown.starts_with("sha256:"));
        assert_eq!(shown.len(), 7 + 32 * 3 - 1);
        assert_eq!(shown.parse::<Fingerprint>().unwrap(), f);
        let lower = shown.to_lowercase();
        assert_eq!(lower.parse::<Fingerprint>().unwrap(), f);
        let bare: String = shown[7..].chars().filter(|c| *c != ':').collect();
        assert_eq!(bare.parse::<Fingerprint>().unwrap(), f);
        assert!("sha256:AB:CD".parse::<Fingerprint>().is_err());
        assert!(format!("{}ZZ", &bare[2..]).parse::<Fingerprint>().is_err());
    }

    #[test]
    fn a_self_signed_identity_is_made_once() {
        let dir = std::env::temp_dir().join(format!("robocodes-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = Identity::load_or_create(&dir).unwrap();
        let again = Identity::load_or_create(&dir).unwrap();
        assert_eq!(first.fingerprint, again.fingerprint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the user reads the key");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn der_lengths() {
        assert_eq!(der(0x04, &[1, 2]), [0x04, 2, 1, 2]);
        let long = der(0x04, &[0; 300]);
        assert_eq!(&long[..4], &[0x04, 0x82, 0x01, 0x2c]);
    }

    #[test]
    fn server_names_come_from_the_host() {
        assert!(matches!(
            server_name("example.org:7654").unwrap(),
            ServerName::DnsName(_)
        ));
        assert!(matches!(
            server_name("[::1]:7654").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("10.0.0.2:1").unwrap(),
            ServerName::IpAddress(_)
        ));
    }
}
