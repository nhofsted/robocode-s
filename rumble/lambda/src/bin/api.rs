// SPDX-License-Identifier: EPL-2.0

//! The `api` Lambda (spec §14.2): every `/api/v1` route behind the function URL. Cold start
//! reads the environment and the SSM secrets; each request runs the synchronous router on
//! a blocking thread.

use lambda_http::{Error, Request, service_fn};
use rumble_lambda::dynamo::DynamoStore;
use rumble_lambda::fetch::HttpFetcher;
use rumble_lambda::github::HttpGithub;
use rumble_lambda::routes::{App, Services, Settings};
use rumble_lambda::s3::S3Objects;
use rumble_lambda::{env, http, now_ms, random32, ssm};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let table = env("TABLE")?;
    let jars = env("JARS_BUCKET")?;
    let state = env("STATE_BUCKET")?;
    let site_origin = env("SITE_ORIGIN")?;
    let github_client_id = env("GITHUB_CLIENT_ID")?;
    let prefix = env("SSM_PREFIX")?;

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let secrets = ssm::load(&aws_sdk_ssm::Client::new(&config), &prefix).await?;
    let services = Services {
        store: Box::new(DynamoStore::new(
            aws_sdk_dynamodb::Client::new(&config),
            &table,
        )),
        objects: Box::new(S3Objects::new(
            S3Objects::client(&config, false),
            &jars,
            &state,
        )),
        github: Box::new(HttpGithub::new(
            &github_client_id,
            &secrets.github_client_secret,
        )),
        fetcher: Box::new(HttpFetcher),
        clock: Box::new(now_ms),
        random: Box::new(random32),
    };
    let app = Arc::new(App::new(
        services,
        secrets.keys,
        Settings {
            site_origin: site_origin.trim_end_matches('/').to_string(),
            github_client_id,
        },
    ));
    lambda_http::run(service_fn(move |request: Request| {
        let app = Arc::clone(&app);
        async move {
            let request = http::from_lambda(request);
            let response = tokio::task::spawn_blocking(move || app.handle(&request)).await?;
            Ok::<_, Error>(http::to_lambda(response))
        }
    }))
    .await
}
