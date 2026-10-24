// SPDX-License-Identifier: EPL-2.0

//! Requests and responses as the router sees them, independent of `lambda_http` (so the
//! router's tests need no Lambda events), and their mapping to and from the function URL's
//! events: JSON bodies, error bodies (§5.2), cookies, `Retry-After`, the viewer address
//! from `CloudFront-Viewer-Address` (§5.1). No CORS headers: the site and the API share an
//! origin.

use rumble_core::api::{ApiError, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Bodies above this get 413 (§5.1).
pub const MAX_BODY: usize = 1 << 20;

pub const SESSION_COOKIE: &str = "__Host-rs_session";
pub const OAUTH_COOKIE: &str = "__Host-rs_oauth";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The full path, e.g. `/api/v1/work`.
    pub path: String,
    /// The raw query string without `?`.
    pub query: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, path: &str) -> Request {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        Request {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            ..Request::default()
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn json<T: Serialize>(mut self, body: &T) -> Request {
        self.body = serde_json::to_vec(body).expect("serializable");
        self
    }

    /// The first value of a header (`name` in lower case).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// A cookie of the `Cookie` header(s).
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == "cookie")
            .flat_map(|(_, v)| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v)
    }

    /// A query parameter, percent-decoded.
    pub fn param(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.into_owned())
    }

    /// The viewer's IP from `CloudFront-Viewer-Address` (`ip:port`).
    pub fn viewer_ip(&self) -> String {
        match self.get("cloudfront-viewer-address") {
            Some(address) => address
                .rsplit_once(':')
                .map_or(address, |(ip, _)| ip)
                .to_string(),
            None => "unknown".to_string(),
        }
    }

    /// The body as JSON of `T`: 400 `bad_request` if it isn't.
    pub fn body_json<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| ApiError::new(ErrorCode::BadRequest, format!("body: {e}")))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json<T: Serialize>(status: u16, body: &T) -> Response {
        Response {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: serde_json::to_vec(body).expect("serializable"),
        }
    }

    pub fn ok<T: Serialize>(body: &T) -> Response {
        Response::json(200, body)
    }

    pub fn empty(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn redirect(location: &str) -> Response {
        Response::empty(302).with_header("location", location)
    }

    pub fn error(error: &ApiError) -> Response {
        let response = Response::json(error.code.status(), &error.body());
        match error.code {
            ErrorCode::Busy => response.with_header("retry-after", "30"),
            _ => response,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Adds a `Set-Cookie` for one of our `__Host-` cookies (`Secure; HttpOnly;
    /// SameSite=Lax; Path=/`); `max_age` 0 clears it.
    pub fn with_cookie(self, name: &str, value: &str, max_age: u64) -> Response {
        self.with_header(
            "set-cookie",
            &format!("{name}={value}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"),
        )
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON (for tests).
    pub fn value(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// A 429 with `Retry-After`.
pub fn rate_limited(retry_after: u64) -> Response {
    Response::error(&ApiError::new(
        ErrorCode::RateLimited,
        format!("too many requests; retry in {retry_after} s"),
    ))
    .with_header("retry-after", &retry_after.to_string())
}

/// From the function URL's event (`lambda_http` decodes base64 bodies and folds the
/// `cookies` array into a `Cookie` header).
pub fn from_lambda(request: lambda_http::Request) -> Request {
    let (parts, body) = request.into_parts();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_string(),
            ))
        })
        .collect();
    Request {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or("").to_string(),
        headers,
        body: body.to_vec(),
    }
}

/// To the function URL's response (`lambda_http` moves `Set-Cookie` headers to `cookies`).
pub fn to_lambda(response: Response) -> lambda_http::Response<lambda_http::Body> {
    let mut builder = lambda_http::http::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let body = if response.body.is_empty() {
        lambda_http::Body::Empty
    } else {
        match String::from_utf8(response.body) {
            Ok(text) => lambda_http::Body::Text(text),
            Err(e) => lambda_http::Body::Binary(e.into_bytes()),
        }
    };
    builder.body(body).unwrap_or_else(|_| {
        let mut fallback = lambda_http::Response::new(lambda_http::Body::Empty);
        *fallback.status_mut() = lambda_http::http::StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// The release in `X-Rumble-Client: robocode-s/<release> (engine <E>; <os>-<arch>)`.
pub fn client_release(header: &str) -> Option<&str> {
    let rest = header.trim().strip_prefix("robocode-s/")?;
    Some(rest.split_whitespace().next().unwrap_or(rest))
}

/// Compares two releases `major.minor.patch[-pre]` numerically; a pre-release sorts below
/// its release. Unparseable versions compare as `None`.
pub fn compare_releases(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn parse(text: &str) -> Option<([u64; 3], bool)> {
        let (core, pre) = match text.split_once('-') {
            Some((core, _)) => (core, true),
            None => (text.split('+').next()?, false),
        };
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for number in &mut numbers {
            *number = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some((numbers, pre))
    }
    let (a, a_pre) = parse(a)?;
    let (b, b_pre) = parse(b)?;
    Some(a.cmp(&b).then(b_pre.cmp(&a_pre)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn requests() {
        let request = Request::new("GET", "/api/v1/auth/login?return=%2Faccount&x=1")
            .header("Cookie", "a=1; __Host-rs_oauth=abc")
            .header("CloudFront-Viewer-Address", "2001:db8::1:443");
        assert_eq!(request.query, "return=%2Faccount&x=1");
        assert_eq!(request.param("return").as_deref(), Some("/account"));
        assert_eq!(request.cookie(OAUTH_COOKIE), Some("abc"));
        assert_eq!(request.cookie("b"), None);
        assert_eq!(request.viewer_ip(), "2001:db8::1");
        assert_eq!(Request::new("GET", "/").viewer_ip(), "unknown");
    }

    #[test]
    fn responses() {
        let busy = Response::error(&ApiError::new(ErrorCode::Busy, "x"));
        assert_eq!(busy.status, 503);
        assert_eq!(busy.get("Retry-After"), Some("30"));
        assert_eq!(busy.value()["error"], "busy");
        let limited = rate_limited(7);
        assert_eq!(
            (limited.status, limited.get("retry-after")),
            (429, Some("7"))
        );
        let cookie = Response::empty(204).with_cookie(SESSION_COOKIE, "rss_x", 0);
        assert_eq!(
            cookie.get("set-cookie"),
            Some("__Host-rs_session=rss_x; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
        );
    }

    #[test]
    fn releases() {
        assert_eq!(
            client_release("robocode-s/0.2.0 (engine 1; linux-x86_64)"),
            Some("0.2.0")
        );
        assert_eq!(client_release("curl/8"), None);
        assert_eq!(compare_releases("0.2.0", "0.2.0"), Some(Ordering::Equal));
        assert_eq!(compare_releases("0.1.9", "0.2.0"), Some(Ordering::Less));
        assert_eq!(compare_releases("0.10.0", "0.2.0"), Some(Ordering::Greater));
        assert_eq!(
            compare_releases("0.2.0-rc.1", "0.2.0"),
            Some(Ordering::Less)
        );
        assert_eq!(compare_releases("x", "0.2.0"), None);
    }

    #[test]
    fn lambda_mapping() {
        let request = lambda_http::http::Request::builder()
            .method("POST")
            .uri("https://x.lambda-url.eu-west-1.on.aws/api/v1/results?a=b")
            .header("X-Rumble-Token", "rsu_t")
            .body(lambda_http::Body::Text("{}".into()))
            .unwrap();
        let request = from_lambda(request);
        assert_eq!(request.path, "/api/v1/results");
        assert_eq!(request.query, "a=b");
        assert_eq!(request.get("x-rumble-token"), Some("rsu_t"));
        assert_eq!(request.body, b"{}");
        let response = to_lambda(Response::ok(&serde_json::json!({"a": 1})));
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
    }
}
