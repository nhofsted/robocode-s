// SPDX-License-Identifier: EPL-2.0

//! The `batch` Lambda's scenario (`common::batch_scenario`) on the in-memory store and
//! objects. The same scenario runs against DynamoDB Local and the S3 mock in `aws_local`.

mod common;

use common::batch_scenario::{World, run};
use rumble_core::store::MemStore;
use rumble_lambda::s3::MemObjects;
use std::path::Path;
use std::sync::Arc;

#[test]
fn plan_publish_cleanup_in_memory() {
    let world = World::new(Arc::new(MemStore::new()), Arc::new(MemObjects::new()));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("batch-data-mem");
    run(&world, &dir);
}
