// SPDX-License-Identifier: EPL-2.0

//! The `batch` Lambda (spec §10.5, §15, §12.4, §11.3): the I/O around the core's `batch`,
//! `publish` and `cleanup` modules. One invocation runs one mode:
//! - `plan` (hourly): fold the result log into the pairing states, write the plans, update
//!   the versions, issue probation work ([`plan`]);
//! - `publish` (every 6 hours): `plan`, then the data files, snapshots and contributors
//!   ([`publish`]);
//! - `cleanup` (daily): recordings, slot items and jars of dead versions, ban removals
//!   ([`cleanup`]).
//!
//! Reserved concurrency 1 makes it the only writer of the state files. Like the `api`, it
//! runs on a blocking thread and drives the SDK with [`crate::block_on`].

pub mod cleanup;
pub mod plan;
pub mod publish;

use crate::cloudfront::Cdn;
use crate::routes::{CONFIG_KEY, config_from};
use crate::s3::{Bucket, Fetched, Objects};
use rumble_core::Config;
use rumble_core::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Reads of the table per second the jobs allow themselves (of 25 provisioned, §9.3).
pub const READ_RATE: f64 = 10.0;
/// Writes per second of the ban removal (§11.3).
pub const WRITE_RATE: f64 = 5.0;
/// Of the Lambda's 900 s, the time after which a job stops starting new work (the ban
/// removal and the bot files resume in the next run).
pub const BUDGET_MS: u64 = 720_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Plan,
    Publish,
    Cleanup,
}

/// The invocation's input: `{"mode": "plan" | "publish" | "cleanup"}`.
#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub mode: Mode,
}

pub struct Services {
    pub store: Box<dyn Store + Send + Sync>,
    pub objects: Box<dyn Objects>,
    pub cdn: Box<dyn Cdn>,
    /// Milliseconds since the epoch.
    pub clock: Box<dyn Fn() -> u64 + Send + Sync>,
    pub sleep: Box<dyn Fn(Duration) + Send + Sync>,
    pub random: Box<dyn Fn() -> [u8; 32] + Send + Sync>,
}

pub struct Batch {
    pub services: Services,
    /// See [`BUDGET_MS`].
    pub budget_ms: u64,
}

/// A JSON object to put, with its content type and cache control.
pub(crate) fn json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("data files serialize")
}

impl Batch {
    pub fn new(services: Services) -> Batch {
        Batch {
            services,
            budget_ms: BUDGET_MS,
        }
    }

    pub fn store(&self) -> &dyn Store {
        self.services.store.as_ref()
    }

    pub fn objects(&self) -> &dyn Objects {
        self.services.objects.as_ref()
    }

    pub fn now_ms(&self) -> u64 {
        (self.services.clock)()
    }

    /// The configuration (`state/config.json` over the defaults, as the `api` reads it).
    pub fn config(&self) -> Result<Config, String> {
        match self.objects().get(Bucket::State, CONFIG_KEY, None)? {
            Fetched::Found { bytes, .. } => {
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|e| format!("{CONFIG_KEY}: {e}"))?;
                config_from(&value)
            }
            _ => config_from(&Value::Object(Default::default())),
        }
    }

    /// Reads and parses a JSON object of the state bucket; `None` if missing.
    pub fn read_json<T: for<'de> Deserialize<'de>>(
        &self,
        bucket: Bucket,
        key: &str,
    ) -> Result<Option<T>, String> {
        match self.objects().get(bucket, key, None)? {
            Fetched::Found { bytes, .. } => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("{key}: {e}")),
            _ => Ok(None),
        }
    }

    /// Writes a JSON object of the state bucket.
    pub fn write_state_json<T: Serialize>(&self, key: &str, value: &T) -> Result<(), String> {
        self.objects().put(
            Bucket::State,
            key,
            &json_bytes(value),
            "application/json",
            None,
        )
    }

    /// Runs one mode and returns its report (logged and returned as the invocation's
    /// result).
    pub fn run(&self, mode: Mode) -> Result<Value, String> {
        let config = self.config()?;
        match mode {
            Mode::Plan => plan::run(self, &config).map(|r| serde_json::to_value(r).unwrap()),
            Mode::Publish => publish::run(self, &config).map(|r| serde_json::to_value(r).unwrap()),
            Mode::Cleanup => cleanup::run(self, &config).map(|r| serde_json::to_value(r).unwrap()),
        }
    }
}

/// Keeps a job's use of the table's capacity at `rate` units per second (DynamoDB's burst
/// capacity absorbs short peaks; this keeps the long runs from eating the `api`'s share).
pub struct Pace<'a> {
    batch: &'a Batch,
    rate: f64,
    start_ms: u64,
    used: f64,
}

impl<'a> Pace<'a> {
    pub fn new(batch: &'a Batch, rate: f64) -> Pace<'a> {
        Pace {
            batch,
            rate,
            start_ms: batch.now_ms(),
            used: 0.0,
        }
    }

    /// Accounts `units` of capacity, sleeping first if the rate is exceeded.
    pub fn take(&mut self, units: f64) {
        let due = self.start_ms + (self.used / self.rate * 1000.0) as u64;
        let now = self.batch.now_ms();
        if due > now {
            (self.batch.services.sleep)(Duration::from_millis(due - now));
        }
        self.used += units;
    }
}

/// Read units of an eventually consistent read of `bytes` (0.5 per started 4 KB).
pub fn read_units(bytes: usize) -> f64 {
    0.5 * bytes.div_ceil(4096).max(1) as f64
}
