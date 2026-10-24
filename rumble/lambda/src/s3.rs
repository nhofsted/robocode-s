// SPDX-License-Identifier: EPL-2.0

//! S3 (spec §8): the objects the Lambdas read and write, behind [`Objects`] (sync, like the
//! core's `Store`), with the S3 implementation and an in-memory one for tests. Conditional
//! GETs by ETag (plan and config reloads), presigned PUTs with `Content-Length` and
//! `x-amz-checksum-sha256` signed (uploads, recordings), HeadObject with `ChecksumMode`
//! (`recordings/done`).

use crate::block_on;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rumble_core::results::Presigned;
use rumble_core::store::rfc3339;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

/// Cache-Control of jars and recordings (§8, §12.5).
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
pub const JAR_TYPE: &str = "application/java-archive";
pub const TRACK_TYPE: &str = "application/octet-stream";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// Served by CloudFront: `jars/`, `rec/`.
    Jars,
    /// Private: `state/`, `uploads/`.
    State,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fetched {
    Missing,
    /// The ETag given still matches.
    NotModified,
    Found {
        bytes: Vec<u8>,
        etag: String,
    },
}

/// What a presigned PUT signs besides the key.
#[derive(Clone, Debug, PartialEq)]
pub struct PutSpec {
    pub size: u64,
    /// The SHA-256 of the body, base64.
    pub sha256_b64: String,
    pub content_type: String,
    pub cache_control: Option<String>,
}

pub type S3Result<T> = Result<T, String>;

pub trait Objects: Send + Sync {
    /// GET, conditional on `if_none_match` if given.
    fn get(&self, bucket: Bucket, key: &str, if_none_match: Option<&str>) -> S3Result<Fetched>;
    /// PUT with the body's SHA-256 checksum.
    fn put(
        &self,
        bucket: Bucket,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: Option<&str>,
    ) -> S3Result<()>;
    /// HeadObject: whether the object exists.
    fn exists(&self, bucket: Bucket, key: &str) -> S3Result<bool>;
    /// HeadObject with `ChecksumMode=ENABLED`: `None` if missing, else its `ChecksumSHA256`.
    fn head_checksum(&self, bucket: Bucket, key: &str) -> S3Result<Option<Option<String>>>;
    fn delete(&self, bucket: Bucket, key: &str) -> S3Result<()>;
    /// A presigned PUT valid `expires_in` seconds from `now`.
    fn presign_put(
        &self,
        bucket: Bucket,
        key: &str,
        spec: &PutSpec,
        expires_in: u64,
        now: u64,
    ) -> S3Result<Presigned>;
}

impl<O: Objects + ?Sized> Objects for std::sync::Arc<O> {
    fn get(&self, bucket: Bucket, key: &str, if_none_match: Option<&str>) -> S3Result<Fetched> {
        (**self).get(bucket, key, if_none_match)
    }
    fn put(
        &self,
        bucket: Bucket,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: Option<&str>,
    ) -> S3Result<()> {
        (**self).put(bucket, key, bytes, content_type, cache_control)
    }
    fn exists(&self, bucket: Bucket, key: &str) -> S3Result<bool> {
        (**self).exists(bucket, key)
    }
    fn head_checksum(&self, bucket: Bucket, key: &str) -> S3Result<Option<Option<String>>> {
        (**self).head_checksum(bucket, key)
    }
    fn delete(&self, bucket: Bucket, key: &str) -> S3Result<()> {
        (**self).delete(bucket, key)
    }
    fn presign_put(
        &self,
        bucket: Bucket,
        key: &str,
        spec: &PutSpec,
        expires_in: u64,
        now: u64,
    ) -> S3Result<Presigned> {
        (**self).presign_put(bucket, key, spec, expires_in, now)
    }
}

/// The SHA-256 of bytes in S3's checksum form.
pub fn sha256_b64(bytes: &[u8]) -> String {
    STANDARD.encode(Sha256::digest(bytes))
}

/// S3 itself.
pub struct S3Objects {
    client: aws_sdk_s3::Client,
    jars: String,
    state: String,
}

impl S3Objects {
    pub fn new(client: aws_sdk_s3::Client, jars_bucket: &str, state_bucket: &str) -> S3Objects {
        S3Objects {
            client,
            jars: jars_bucket.to_string(),
            state: state_bucket.to_string(),
        }
    }

    /// The S3 client configuration the Lambdas use: checksums only when an operation
    /// requires them (otherwise presigned PUTs would also sign a CRC of an empty body).
    pub fn client(config: &aws_config::SdkConfig, path_style: bool) -> aws_sdk_s3::Client {
        let s3 = aws_sdk_s3::config::Builder::from(config)
            .request_checksum_calculation(
                aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
            )
            .response_checksum_validation(
                aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired,
            )
            .force_path_style(path_style)
            .build();
        aws_sdk_s3::Client::from_conf(s3)
    }

    fn bucket(&self, bucket: Bucket) -> &str {
        match bucket {
            Bucket::Jars => &self.jars,
            Bucket::State => &self.state,
        }
    }
}

fn describe<E: std::error::Error>(e: &E) -> String {
    aws_sdk_s3::error::DisplayErrorContext(e).to_string()
}

fn raw_status<E>(
    e: &aws_sdk_s3::error::SdkError<E, aws_sdk_s3::config::http::HttpResponse>,
) -> Option<u16> {
    e.raw_response().map(|r| r.status().as_u16())
}

impl Objects for S3Objects {
    fn get(&self, bucket: Bucket, key: &str, if_none_match: Option<&str>) -> S3Result<Fetched> {
        block_on(async {
            let request = self
                .client
                .get_object()
                .bucket(self.bucket(bucket))
                .key(key)
                .set_if_none_match(if_none_match.map(str::to_string));
            match request.send().await {
                Ok(output) => {
                    let etag = output.e_tag.clone().unwrap_or_default();
                    let bytes = output
                        .body
                        .collect()
                        .await
                        .map_err(|e| format!("S3 GET {key}: {e}"))?
                        .into_bytes()
                        .to_vec();
                    Ok(Fetched::Found { bytes, etag })
                }
                Err(e) => match raw_status(&e) {
                    Some(304) => Ok(Fetched::NotModified),
                    Some(404) => Ok(Fetched::Missing),
                    _ if e.as_service_error().is_some_and(|s| s.is_no_such_key()) => {
                        Ok(Fetched::Missing)
                    }
                    _ => Err(format!("S3 GET {key}: {}", describe(&e))),
                },
            }
        })
    }

    fn put(
        &self,
        bucket: Bucket,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: Option<&str>,
    ) -> S3Result<()> {
        block_on(async {
            self.client
                .put_object()
                .bucket(self.bucket(bucket))
                .key(key)
                .content_type(content_type)
                .set_cache_control(cache_control.map(str::to_string))
                .checksum_sha256(sha256_b64(bytes))
                .body(ByteStream::from(bytes.to_vec()))
                .send()
                .await
                .map(|_| ())
                .map_err(|e| format!("S3 PUT {key}: {}", describe(&e)))
        })
    }

    fn exists(&self, bucket: Bucket, key: &str) -> S3Result<bool> {
        Ok(self.head_checksum(bucket, key)?.is_some())
    }

    fn head_checksum(&self, bucket: Bucket, key: &str) -> S3Result<Option<Option<String>>> {
        block_on(async {
            match self
                .client
                .head_object()
                .bucket(self.bucket(bucket))
                .key(key)
                .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
                .send()
                .await
            {
                Ok(output) => Ok(Some(output.checksum_sha256)),
                Err(e) if raw_status(&e) == Some(404) => Ok(None),
                Err(e) if e.as_service_error().is_some_and(|s| s.is_not_found()) => Ok(None),
                Err(e) => Err(format!("S3 HEAD {key}: {}", describe(&e))),
            }
        })
    }

    fn delete(&self, bucket: Bucket, key: &str) -> S3Result<()> {
        block_on(async {
            self.client
                .delete_object()
                .bucket(self.bucket(bucket))
                .key(key)
                .send()
                .await
                .map(|_| ())
                .map_err(|e| format!("S3 DELETE {key}: {}", describe(&e)))
        })
    }

    fn presign_put(
        &self,
        bucket: Bucket,
        key: &str,
        spec: &PutSpec,
        expires_in: u64,
        now: u64,
    ) -> S3Result<Presigned> {
        block_on(async {
            let config = PresigningConfig::builder()
                .start_time(std::time::UNIX_EPOCH + Duration::from_secs(now))
                .expires_in(Duration::from_secs(expires_in))
                .build()
                .map_err(|e| e.to_string())?;
            let request = self
                .client
                .put_object()
                .bucket(self.bucket(bucket))
                .key(key)
                .content_length(spec.size as i64)
                .checksum_sha256(&spec.sha256_b64)
                .content_type(&spec.content_type)
                .set_cache_control(spec.cache_control.clone())
                .presigned(config)
                .await
                .map_err(|e| format!("presigning {key}: {}", describe(&e)))?;
            let mut headers: BTreeMap<String, String> = request
                .headers()
                .map(|(name, value)| (canonical_header(name), value.to_string()))
                .collect();
            headers
                .entry("Content-Length".to_string())
                .or_insert_with(|| spec.size.to_string());
            Ok(Presigned {
                url: request.uri().to_string(),
                headers,
                expires: rfc3339(now + expires_in),
            })
        })
    }
}

/// `content-length` → `Content-Length`; `x-amz-*` headers stay lower case (§5.4 example).
fn canonical_header(name: &str) -> String {
    if name.starts_with("x-amz-") {
        return name.to_string();
    }
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// An object of [`MemObjects`].
#[derive(Clone, Debug, PartialEq)]
pub struct MemObject {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub cache_control: Option<String>,
    pub sha256_b64: String,
    pub etag: String,
}

/// In-memory objects for tests. ETags are the hex SHA-256 of the body.
#[derive(Default)]
pub struct MemObjects {
    pub objects: Mutex<HashMap<(Bucket, String), MemObject>>,
    /// Presigned PUTs handed out: (bucket, key, spec).
    pub presigned: Mutex<Vec<(Bucket, String, PutSpec)>>,
    /// Number of GETs, for the caching tests.
    pub gets: Mutex<u32>,
}

impl MemObjects {
    pub fn new() -> MemObjects {
        MemObjects::default()
    }

    /// What a client's PUT to a presigned URL would store.
    pub fn upload(&self, bucket: Bucket, key: &str, bytes: &[u8]) {
        self.put(bucket, key, bytes, TRACK_TYPE, None).unwrap();
    }

    pub fn object(&self, bucket: Bucket, key: &str) -> Option<MemObject> {
        self.objects
            .lock()
            .unwrap()
            .get(&(bucket, key.to_string()))
            .cloned()
    }

    pub fn keys(&self, bucket: Bucket) -> Vec<String> {
        let mut keys: Vec<String> = self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|(b, _)| *b == bucket)
            .map(|(_, k)| k.clone())
            .collect();
        keys.sort();
        keys
    }
}

impl Objects for MemObjects {
    fn get(&self, bucket: Bucket, key: &str, if_none_match: Option<&str>) -> S3Result<Fetched> {
        *self.gets.lock().unwrap() += 1;
        Ok(match self.object(bucket, key) {
            None => Fetched::Missing,
            Some(object) if Some(object.etag.as_str()) == if_none_match => Fetched::NotModified,
            Some(object) => Fetched::Found {
                bytes: object.bytes,
                etag: object.etag,
            },
        })
    }

    fn put(
        &self,
        bucket: Bucket,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: Option<&str>,
    ) -> S3Result<()> {
        let object = MemObject {
            bytes: bytes.to_vec(),
            content_type: content_type.to_string(),
            cache_control: cache_control.map(str::to_string),
            sha256_b64: sha256_b64(bytes),
            etag: rumble_core::jar::sha256_hex(bytes),
        };
        self.objects
            .lock()
            .unwrap()
            .insert((bucket, key.to_string()), object);
        Ok(())
    }

    fn exists(&self, bucket: Bucket, key: &str) -> S3Result<bool> {
        Ok(self.object(bucket, key).is_some())
    }

    fn head_checksum(&self, bucket: Bucket, key: &str) -> S3Result<Option<Option<String>>> {
        Ok(self.object(bucket, key).map(|o| Some(o.sha256_b64)))
    }

    fn delete(&self, bucket: Bucket, key: &str) -> S3Result<()> {
        self.objects
            .lock()
            .unwrap()
            .remove(&(bucket, key.to_string()));
        Ok(())
    }

    fn presign_put(
        &self,
        bucket: Bucket,
        key: &str,
        spec: &PutSpec,
        expires_in: u64,
        now: u64,
    ) -> S3Result<Presigned> {
        self.presigned
            .lock()
            .unwrap()
            .push((bucket, key.to_string(), spec.clone()));
        let mut headers = BTreeMap::new();
        headers.insert("Content-Length".to_string(), spec.size.to_string());
        headers.insert("x-amz-checksum-sha256".to_string(), spec.sha256_b64.clone());
        headers.insert("Content-Type".to_string(), spec.content_type.clone());
        if let Some(cache) = &spec.cache_control {
            headers.insert("Cache-Control".to_string(), cache.clone());
        }
        let bucket = match bucket {
            Bucket::Jars => "jars",
            Bucket::State => "state",
        };
        Ok(Presigned {
            url: format!("https://{bucket}.s3.example/{key}?X-Amz-Expires={expires_in}"),
            headers,
            expires: rfc3339(now + expires_in),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names() {
        assert_eq!(canonical_header("content-length"), "Content-Length");
        assert_eq!(canonical_header("cache-control"), "Cache-Control");
        assert_eq!(
            canonical_header("x-amz-checksum-sha256"),
            "x-amz-checksum-sha256"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn presigned_puts_sign_length_and_checksum() {
        let config = aws_config::SdkConfig::builder()
            .region(aws_sdk_s3::config::Region::new("eu-west-1"))
            .credentials_provider(aws_sdk_s3::config::SharedCredentialsProvider::new(
                aws_sdk_s3::config::Credentials::new("AKIDEXAMPLE", "secret", None, None, "test"),
            ))
            .behavior_version(aws_config::BehaviorVersion::latest())
            .build();
        let objects = S3Objects::new(S3Objects::client(&config, false), "jars-b", "state-b");
        let spec = PutSpec {
            size: 3401,
            sha256_b64: sha256_b64(b"track"),
            content_type: TRACK_TYPE.to_string(),
            cache_control: Some(IMMUTABLE.to_string()),
        };
        let presigned = tokio::task::spawn_blocking(move || {
            objects.presign_put(
                Bucket::Jars,
                "rec/e1/1v1/a/b/a-0.track",
                &spec,
                3600,
                1_792_843_200,
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(
            presigned
                .url
                .starts_with("https://jars-b.s3.eu-west-1.amazonaws.com/rec/e1/1v1/a/b/a-0.track?"),
            "{}",
            presigned.url
        );
        let url = url::Url::parse(&presigned.url).unwrap();
        let signed: String = url
            .query_pairs()
            .find(|(k, _)| k == "X-Amz-SignedHeaders")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        let signed: Vec<&str> = signed.split(';').collect();
        for header in [
            "content-length",
            "x-amz-checksum-sha256",
            "content-type",
            "cache-control",
            "host",
        ] {
            assert!(signed.contains(&header), "{header} not in {signed:?}");
        }
        assert_eq!(presigned.headers["Content-Length"], "3401");
        assert_eq!(
            presigned.headers["x-amz-checksum-sha256"],
            sha256_b64(b"track")
        );
        assert_eq!(presigned.headers["Cache-Control"], IMMUTABLE);
        assert_eq!(presigned.expires, "2026-10-24T13:00:00Z");
        assert!(presigned.url.contains("X-Amz-Expires=3600"));
    }
}
