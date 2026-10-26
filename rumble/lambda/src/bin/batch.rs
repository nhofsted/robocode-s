// SPDX-License-Identifier: EPL-2.0

//! The `batch` Lambda (spec §14.2): invoked by EventBridge Scheduler with
//! `{"mode": "plan" | "publish" | "cleanup" | "sync"}`. Environment: `TABLE`, `JARS_BUCKET`,
//! `STATE_BUCKET`, `SITE_BUCKET`, `DISTRIBUTION_ID`. A failed run logs
//! `ALERT batch-error <mode> ...` (the alarm's metric filter) and fails the invocation.

use lambda_runtime::{Error, LambdaEvent, service_fn};
use rumble_lambda::batch::{Batch, Event, Services};
use rumble_lambda::cloudfront::CloudFront;
use rumble_lambda::dynamo::DynamoStore;
use rumble_lambda::fetch::HttpFetcher;
use rumble_lambda::s3::S3Objects;
use rumble_lambda::{env, now_ms, random32};
use serde_json::Value;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let table = env("TABLE")?;
    let jars = env("JARS_BUCKET")?;
    let state = env("STATE_BUCKET")?;
    let site = env("SITE_BUCKET")?;
    let distribution = env("DISTRIBUTION_ID")?;

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let batch = Arc::new(Batch::new(Services {
        store: Box::new(DynamoStore::new(
            aws_sdk_dynamodb::Client::new(&config),
            &table,
        )),
        objects: Box::new(
            S3Objects::new(S3Objects::client(&config, false), &jars, &state).with_site(&site),
        ),
        cdn: Box::new(CloudFront::new(
            aws_sdk_cloudfront::Client::new(&config),
            &distribution,
        )),
        fetcher: Box::new(HttpFetcher),
        clock: Box::new(now_ms),
        sleep: Box::new(std::thread::sleep),
        random: Box::new(random32),
    }));
    lambda_runtime::run(service_fn(move |event: LambdaEvent<Value>| {
        let batch = Arc::clone(&batch);
        async move {
            let Event { mode } = serde_json::from_value(event.payload)
                .map_err(|e| format!("bad event (want {{\"mode\": ...}}): {e}"))?;
            let result = tokio::task::spawn_blocking(move || batch.run(mode)).await?;
            match result {
                Ok(report) => {
                    println!("{mode:?}: {report}");
                    Ok::<Value, Error>(report)
                }
                Err(e) => {
                    println!("ALERT batch-error {mode:?}: {e}");
                    Err(e.into())
                }
            }
        }
    }))
    .await
}
