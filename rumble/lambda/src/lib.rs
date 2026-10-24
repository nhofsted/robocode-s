// SPDX-License-Identifier: EPL-2.0

//! The rumble backend's Lambdas (spec §2, §14.2): the I/O around `rumble-core`. The core is
//! synchronous (its `Store` trait in particular), so every request runs on a
//! `tokio::task::spawn_blocking` thread and the AWS and GitHub calls inside it are driven
//! with [`block_on`]. The `api` binary routes every `/api/v1` endpoint ([`routes`]); the
//! `batch` binary runs the [`batch`] jobs on the same [`dynamo`] and [`s3`] implementations
//! (and [`cloudfront`]).

pub mod batch;
pub mod cloudfront;
pub mod dynamo;
pub mod fetch;
pub mod github;
pub mod http;
pub mod limits;
pub mod routes;
pub mod s3;
pub mod ssm;

use std::future::Future;

pub use rumble_core::config::DETERMINISTIC_RULES;

/// The engine version of this build: the default of `engineVersion` (spec §3).
pub const ENGINE_VERSION: u32 = robocode_s_engine::ENGINE_VERSION;

/// Runs a future to completion from a blocking thread of the Lambda's runtime (the threads
/// of `spawn_blocking` are inside the runtime's context). Must not be called from async
/// code.
pub fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(future)
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 32 bytes from the operating system's generator.
pub fn random32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random generator");
    bytes
}

/// A required environment variable.
pub fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))
}
