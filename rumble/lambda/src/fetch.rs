// SPDX-License-Identifier: EPL-2.0

//! Fetching a submitted jar from a URL (spec §12.2): `http`/`https`, 10 s, the size limit,
//! at most 3 redirects. Every hop's host must resolve to public addresses only, and the
//! connection is pinned to the address checked, so a submission can't reach the Lambda's
//! runtime API on localhost or anything else private.

use crate::block_on;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub trait Fetcher: Send + Sync {
    /// The body of `url`, at most `limit` bytes. `Err` is a message for the submitter;
    /// `Err` starting with `too large` means the limit was exceeded.
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String>;
}

impl<F: Fetcher + ?Sized> Fetcher for std::sync::Arc<F> {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        (**self).fetch(url, limit)
    }
}

const MAX_REDIRECTS: usize = 3;

/// Whether an address is one a public web server can have.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (18..20).contains(&b))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && v6.segments()[1] == 0xdb8)
        }
    }
}

/// The real fetcher.
pub struct HttpFetcher;

impl HttpFetcher {
    async fn fetch_async(url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut current = url::Url::parse(url).map_err(|e| format!("bad URL: {e}"))?;
        for _ in 0..=MAX_REDIRECTS {
            if !matches!(current.scheme(), "http" | "https") {
                return Err("only http and https URLs".to_string());
            }
            let host = current
                .host_str()
                .ok_or("the URL has no host")?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let port = current.port_or_known_default().unwrap_or(80);
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|e| format!("cannot resolve {host}: {e}"))?
                .collect();
            let Some(&address) = addresses.first() else {
                return Err(format!("cannot resolve {host}"));
            };
            if !addresses.iter().all(|a| is_public(a.ip())) {
                return Err(format!("{host} is not a public address"));
            }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .user_agent("robocode-s-rumble")
                .resolve(&host, address)
                .build()
                .map_err(|e| e.to_string())?;
            let mut response = client
                .get(current.clone())
                .send()
                .await
                .map_err(|e| format!("download failed: {e}"))?;
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|l| l.to_str().ok())
                    .ok_or("a redirect without a location")?;
                current = current
                    .join(location)
                    .map_err(|e| format!("bad redirect: {e}"))?;
                continue;
            }
            if !status.is_success() {
                return Err(format!("download failed: HTTP {status}"));
            }
            if response.content_length().is_some_and(|n| n > limit) {
                return Err(format!("too large: over {limit} bytes"));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("download failed: {e}"))?
            {
                body.extend_from_slice(&chunk);
                if body.len() as u64 > limit {
                    return Err(format!("too large: over {limit} bytes"));
                }
            }
            return Ok(body);
        }
        Err(format!("more than {MAX_REDIRECTS} redirects"))
    }
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        block_on(HttpFetcher::fetch_async(url, limit))
    }
}

/// URLs and their bodies, in memory (for tests); other URLs answer HTTP 404.
#[derive(Default)]
pub struct MemFetcher {
    bodies: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    calls: std::sync::Mutex<Vec<String>>,
}

impl MemFetcher {
    pub fn new() -> MemFetcher {
        MemFetcher::default()
    }

    pub fn set(&self, url: &str, body: impl Into<Vec<u8>>) {
        self.bodies
            .lock()
            .unwrap()
            .insert(url.to_string(), body.into());
    }

    pub fn remove(&self, url: &str) {
        self.bodies.lock().unwrap().remove(url);
    }

    /// The URLs fetched so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Fetcher for MemFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        self.calls.lock().unwrap().push(url.to_string());
        match self.bodies.lock().unwrap().get(url) {
            Some(body) if body.len() as u64 > limit => {
                Err(format!("too large: over {limit} bytes"))
            }
            Some(body) => Ok(body.clone()),
            None => Err("download failed: HTTP 404".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "140.82.112.3", "2606:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn refuses_private_hosts_and_schemes() {
        let local = HttpFetcher::fetch_async("http://127.0.0.1:9001/2018-06-01/runtime", 10).await;
        assert!(local.unwrap_err().contains("not a public address"));
        let file = HttpFetcher::fetch_async("file:///etc/passwd", 10).await;
        assert!(file.unwrap_err().contains("http"));
        assert!(HttpFetcher::fetch_async("not a url", 10).await.is_err());
    }
}
