// SPDX-License-Identifier: EPL-2.0

//! CloudFront invalidations (spec §8, §14.6), behind [`Cdn`] (sync, like [`crate::s3`]):
//! `publish` invalidates `/data/v1/*`, `cleanup` the jars and recordings of a takedown.

use crate::block_on;
use aws_sdk_cloudfront::types::{InvalidationBatch, Paths};
use std::sync::Mutex;

/// Paths per invalidation request (CloudFront's limit is 3,000 for exact paths).
pub const MAX_PATHS: usize = 1000;

pub trait Cdn: Send + Sync {
    /// One `CreateInvalidation` per [`MAX_PATHS`] paths. `reference` makes the request
    /// idempotent (CloudFront's `CallerReference`).
    fn invalidate(&self, paths: &[String], reference: &str) -> Result<(), String>;
}

impl<C: Cdn + ?Sized> Cdn for std::sync::Arc<C> {
    fn invalidate(&self, paths: &[String], reference: &str) -> Result<(), String> {
        (**self).invalidate(paths, reference)
    }
}

/// The distribution of a stage.
pub struct CloudFront {
    client: aws_sdk_cloudfront::Client,
    distribution: String,
}

impl CloudFront {
    pub fn new(client: aws_sdk_cloudfront::Client, distribution: &str) -> CloudFront {
        CloudFront {
            client,
            distribution: distribution.to_string(),
        }
    }
}

impl Cdn for CloudFront {
    fn invalidate(&self, paths: &[String], reference: &str) -> Result<(), String> {
        for (i, chunk) in paths.chunks(MAX_PATHS).enumerate() {
            let paths = Paths::builder()
                .quantity(chunk.len() as i32)
                .set_items(Some(chunk.to_vec()))
                .build()
                .map_err(|e| e.to_string())?;
            let batch = InvalidationBatch::builder()
                .paths(paths)
                .caller_reference(format!("{reference}.{i}"))
                .build()
                .map_err(|e| e.to_string())?;
            block_on(
                self.client
                    .create_invalidation()
                    .distribution_id(&self.distribution)
                    .invalidation_batch(batch)
                    .send(),
            )
            .map_err(|e| {
                format!(
                    "CloudFront invalidation: {}",
                    aws_sdk_cloudfront::error::DisplayErrorContext(&e)
                )
            })?;
        }
        Ok(())
    }
}

/// Records invalidations, for tests (and a stage without a distribution).
#[derive(Default)]
pub struct MemCdn {
    pub invalidations: Mutex<Vec<Vec<String>>>,
}

impl MemCdn {
    pub fn new() -> MemCdn {
        MemCdn::default()
    }

    /// Every path invalidated so far.
    pub fn paths(&self) -> Vec<String> {
        self.invalidations.lock().unwrap().concat()
    }
}

impl Cdn for MemCdn {
    fn invalidate(&self, paths: &[String], _reference: &str) -> Result<(), String> {
        if !paths.is_empty() {
            self.invalidations.lock().unwrap().push(paths.to_vec());
        }
        Ok(())
    }
}
