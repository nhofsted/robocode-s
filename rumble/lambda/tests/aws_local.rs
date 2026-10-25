// SPDX-License-Identifier: EPL-2.0

//! The `api` router on the real AWS implementations against local stand-ins: DynamoDB
//! Local (`amazon/dynamodb-local`) and an S3 mock (`adobe/s3mock`; MinIO's images are no
//! longer published), started with Docker. Ignored by default; run with
//! `cargo test --test aws_local -- --ignored` where `docker info` works.
//!
//! work → results → recording uploads (presigned PUTs) → recordings/done → repost, plus a
//! verification round trip, the device-flow token, `me` and a submission, so every
//! `DynamoStore` operation of the api runs against DynamoDB's real condition semantics.

mod common;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
    TimeToLiveSpecification,
};
use common::*;
use rumble_core::api::{
    BattleKind, BotColors, RecordingsDoneRequest, ResultsResponse, WorkResponse,
};
use rumble_core::ids::Division;
use rumble_core::store::{Run, SlotStatus, Store, StoreError, VERIFY_TTL, VerifyItem, VerifyKind};
use rumble_lambda::dynamo::DynamoStore;
use rumble_lambda::github::FakeGithub;
use rumble_lambda::http::Request;
use rumble_lambda::routes::{App, Services, Settings};
use rumble_lambda::s3::{Bucket, Objects, S3Objects};
use rumble_lambda::{block_on, now_ms};
use serde_json::json;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const DYNAMODB_IMAGE: &str = "amazon/dynamodb-local:3.3.1";
const S3_IMAGE: &str = "adobe/s3mock:4.9.1";
const TABLE: &str = "rumble-test";

/// A container removed on drop.
struct Container {
    id: String,
    port: u16,
}

impl Container {
    fn start(image: &str, port: u16, args: &[&str]) -> Container {
        let mut command = Command::new("docker");
        command.args(["run", "-d", "--rm", "-p", &format!("127.0.0.1::{port}")]);
        command.args(args).arg(image);
        let output = command.output().expect("docker");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let id = String::from_utf8(output.stdout).unwrap().trim().to_string();
        let mapped = Command::new("docker")
            .args(["port", &id, &port.to_string()])
            .output()
            .expect("docker port");
        let text = String::from_utf8(mapped.stdout).unwrap();
        let port = text
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or_else(|| panic!("no port mapping in {text:?}"));
        Container { id, port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.id]).output();
    }
}

async fn sdk_config(endpoint: &str) -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "local", "local", None, None, "test",
        ))
        .endpoint_url(endpoint)
        .load()
        .await
}

async fn create_table(client: &aws_sdk_dynamodb::Client) {
    for attempt in 0.. {
        match client.list_tables().send().await {
            Ok(_) => break,
            Err(_) if attempt < 60 => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(e) => panic!("DynamoDB Local not ready: {e:?}"),
        }
    }
    // As CDK creates it (spec §9.1, §14.2): pk/sk strings, TTL on `ttl`, no indexes.
    client
        .create_table()
        .table_name(TABLE)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("create table");
    client
        .update_time_to_live()
        .table_name(TABLE)
        .time_to_live_specification(
            TimeToLiveSpecification::builder()
                .attribute_name("ttl")
                .enabled(true)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("ttl");
}

async fn wait_for_s3(client: &aws_sdk_s3::Client, buckets: usize) {
    for attempt in 0.. {
        match client.list_buckets().send().await {
            Ok(output) if output.buckets().len() >= buckets => return,
            _ if attempt < 120 => tokio::time::sleep(Duration::from_millis(500)).await,
            other => panic!("S3 mock not ready: {other:?}"),
        }
    }
}

fn put_presigned(url: &str, headers: &std::collections::BTreeMap<String, String>, body: Vec<u8>) {
    let client = reqwest::Client::new();
    let mut request = client.put(url).body(body);
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("content-length") {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    let response = block_on(request.send()).expect("PUT");
    assert!(
        response.status().is_success(),
        "PUT {url}: {} {}",
        response.status(),
        block_on(response.text()).unwrap_or_default()
    );
}

#[test]
#[ignore = "needs Docker: cargo test --test aws_local -- --ignored"]
fn api_against_dynamodb_local_and_s3() {
    let dynamo = Container::start(DYNAMODB_IMAGE, 8000, &[]);
    let s3 = Container::start(
        S3_IMAGE,
        9090,
        &[
            "-e",
            "COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS=jars,state",
            "-e",
            "initialBuckets=jars,state",
        ],
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (dynamo_config, s3_config) = runtime.block_on(async {
        let dynamo_config = sdk_config(&dynamo.url()).await;
        let s3_config = sdk_config(&s3.url()).await;
        create_table(&aws_sdk_dynamodb::Client::new(&dynamo_config)).await;
        wait_for_s3(&S3Objects::client(&s3_config, true), 2).await;
        (dynamo_config, s3_config)
    });
    runtime
        .block_on(async {
            tokio::task::spawn_blocking(move || scenario(&dynamo_config, &s3_config)).await
        })
        .unwrap();
}

fn scenario(dynamo_config: &aws_config::SdkConfig, s3_config: &aws_config::SdkConfig) {
    let store = Arc::new(DynamoStore::new(
        aws_sdk_dynamodb::Client::new(dynamo_config),
        TABLE,
    ));
    let objects = Arc::new(S3Objects::new(
        S3Objects::client(s3_config, true),
        "jars",
        "state",
    ));
    let github = Arc::new(FakeGithub::new());
    let start = now_ms();
    let clock = Arc::new(AtomicU64::new(start));
    let counter = Arc::new(AtomicU64::new(start));
    objects
        .put(
            Bucket::State,
            "state/config.json",
            &serde_json::to_vec(
                &json!({"engineVersion": 1, "divisions": {"1v1": {"rounds": 3}},
                // No random checks: the verification below is the only one.
                "rep": {"startRate": 0.0, "floorRate": 0.0}}),
            )
            .unwrap(),
            "application/json",
            None,
        )
        .unwrap();
    let plan = plan_1v1(1);
    objects
        .put(
            Bucket::State,
            "state/e1/1v1/plan.json.gz",
            &plan.to_bytes(),
            "application/gzip",
            None,
        )
        .unwrap();
    for vid in 1..=3 {
        // Seeded vids come from the counter, as registrations take them.
        assert_eq!(store.next_vid().unwrap(), vid);
        store.put_version(&version(vid)).unwrap();
    }
    // A version's colours are set once: the first confirmed ones stay.
    let black = BotColors {
        body: Some("#000000".into()),
        ..Default::default()
    };
    store.set_version_colors(3, &black).unwrap();
    let white = BotColors {
        body: Some("#ffffff".into()),
        ..Default::default()
    };
    assert_eq!(
        store.set_version_colors(3, &white),
        Err(StoreError::ConditionFailed)
    );
    assert_eq!(
        store.set_version_colors(999, &white),
        Err(StoreError::ConditionFailed)
    );
    assert_eq!(store.get_version(3).unwrap().unwrap().colors, Some(black));
    let app = App::new(
        Services {
            store: Box::new(store.clone()),
            objects: Box::new(objects.clone()),
            github: Box::new(github.clone()),
            fetcher: Box::new(Arc::new(FakeFetcher::default())),
            clock: {
                let clock = clock.clone();
                Box::new(move || clock.load(Ordering::SeqCst))
            },
            random: Box::new(move || {
                use sha2::Digest;
                sha2::Sha256::digest(counter.fetch_add(1, Ordering::SeqCst).to_le_bytes()).into()
            }),
        },
        keys(),
        Settings {
            site_origin: SITE.into(),
            github_client_id: "Ov23test".into(),
        },
    );
    let advance = |ms: u64| {
        clock.fetch_add(ms, Ordering::SeqCst);
    };
    let call = |request: Request| app.handle(&request.header("X-Rumble-Client", CLIENT));
    let get = |path: &str, token: &str| {
        call(Request::new("GET", &format!("/api/v1/{path}")).header("X-Rumble-Token", token))
    };
    let post = |path: &str, token: &str, body: &dyn erased::Body| {
        let mut request =
            Request::new("POST", &format!("/api/v1/{path}")).header("X-Rumble-Token", token);
        request.body = body.bytes();
        call(request)
    };

    // The device flow: a GitHub token for an upload token (user upsert, token transaction).
    github.add_token("gho_local", profile(501, "recorder"));
    let response = call(
        Request::new("POST", "/api/v1/auth/token")
            .json(&json!({"githubToken": "gho_local", "label": "it"})),
    );
    assert_eq!(response.status, 200, "{:?}", response.value());
    let token = response.value()["token"].as_str().unwrap().to_string();
    // A recorder: rep 30 (admin update through the store's reputation write).
    let reputation = rumble_core::store::Reputation {
        rep: 30,
        ..Default::default()
    };
    let user = store.get_user("g501").unwrap().unwrap();
    store
        .set_reputation("g501", user.reputation.ver, &reputation)
        .unwrap();
    assert_eq!(
        store.set_reputation("g501", user.reputation.ver, &reputation),
        Err(rumble_core::store::StoreError::ConditionFailed)
    );
    let me = get("me", &token);
    assert_eq!(
        (
            me.value()["login"].as_str(),
            me.value()["reputation"].as_i64()
        ),
        (Some("recorder"), Some(30))
    );
    assert_eq!(
        get("me/tokens", &token).value()["tokens"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // work → results.
    let response = get("work?engine=1&max=8&divisions=1v1", &token);
    assert_eq!(response.status, 200, "{:?}", response.value());
    let batch: WorkResponse = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(batch.battles.len(), 8);
    let post_body = results(&batch);
    let response = post("results", &token, &post_body);
    assert_eq!(response.status, 200, "{:?}", response.value());
    let outcome: ResultsResponse = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(outcome.accepted.len(), 8);
    assert_eq!(outcome.user.battles, 8);
    assert!(
        !outcome.uploads.is_empty(),
        "battles with the top bot claim slots"
    );
    let marker = store.get_marker(&batch.batch_id).unwrap().unwrap();
    assert_eq!(marker.slots.len(), outcome.uploads.len());
    let hour = rumble_core::store::hour_key(clock.load(Ordering::SeqCst) / 1000);
    let log = store.query_log(1, hour).unwrap();
    assert_eq!((log.len(), log[0].battles, log[0].seeds.len()), (1, 8, 8));

    // Upload the tracks to the presigned URLs, then confirm.
    for upload in &outcome.uploads {
        let bytes = track(&batch.batch_id, upload.index, upload.round);
        put_presigned(&upload.url, &upload.headers, bytes);
    }
    let slots: Vec<String> = outcome.uploads.iter().map(|u| u.slot.clone()).collect();
    advance(1_000);
    let done = post(
        "recordings/done",
        &token,
        &RecordingsDoneRequest {
            batch: batch.batch.clone(),
            slots: slots.clone(),
        },
    );
    assert_eq!(done.status, 200, "{:?}", done.value());
    assert_eq!(
        done.value()["filled"].as_array().unwrap().len(),
        slots.len(),
        "{:?}",
        done.value()
    );
    for slot in &slots {
        let id = rumble_core::slots::SlotId::parse(slot).unwrap();
        let item = store
            .get_slots(1, Division::OneVsOne, &id.group.key())
            .unwrap()
            .unwrap();
        assert_eq!(item.entries[slot].status, SlotStatus::Filled);
    }

    // A repost is a duplicate: nothing counted again, no uploads left to do.
    advance(5_000);
    let again: ResultsResponse =
        serde_json::from_slice(&post("results", &token, &post_body).body).unwrap();
    assert!(again.duplicate);
    assert!(again.uploads.is_empty());
    assert_eq!(store.query_log(1, hour).unwrap().len(), 1);
    assert_eq!(store.get_user("g501").unwrap().unwrap().battles, 8);

    // Verification: a verifier claims a check, runs it, and matches.
    let seed = 777;
    let battle_hash = sha_hex(format!("battle {seed} 0").as_bytes());
    store
        .put_verify(&VerifyItem {
            engine: 1,
            sort: format!("{:013}#b-old.0", clock.load(Ordering::SeqCst)),
            division: Division::OneVsOne,
            seed,
            bots: vec![2, 3],
            rounds: 3,
            runs: vec![Run {
                uid: "g501".into(),
                hash: Some(battle_hash),
                record: vec![],
                held: false,
                colors: Vec::new(),
            }],
            uids: vec!["g501".into()],
            stage: 1,
            claimant: None,
            claim_until: None,
            kind: VerifyKind::Check,
            ttl: clock.load(Ordering::SeqCst) / 1000 + VERIFY_TTL,
        })
        .unwrap();
    let verifier = user_with_token(
        store.as_ref(),
        502,
        60,
        9,
        clock.load(Ordering::SeqCst) / 1000,
    );
    let batch = get("work?engine=1&max=2&divisions=1v1", &verifier);
    let batch: WorkResponse = serde_json::from_slice(&batch.body).unwrap();
    assert_eq!(batch.battles[0].kind, BattleKind::Verify);
    assert_eq!(batch.battles[0].seed, seed);
    let response = post("results", &verifier, &results(&batch));
    assert_eq!(response.status, 200, "{:?}", response.value());
    assert!(
        store.query_verify(1, None, 10).unwrap().is_empty(),
        "matched and deleted"
    );
    assert_eq!(store.get_user("g502").unwrap().unwrap().reputation.rep, 61);
    assert_eq!(store.get_user("g501").unwrap().unwrap().reputation.rep, 31);

    // A submission: presigned upload to the state bucket, then submit (vid counter,
    // registration transaction, owned bots).
    let bytes = jar("it.Bot", Some("1.0"));
    let sha = sha_hex(&bytes);
    advance(1_000);
    let url = post(
        "bots/upload-url",
        &token,
        &json!({"sha256": sha, "size": bytes.len()}),
    );
    assert_eq!(url.status, 200, "{:?}", url.value());
    let upload: rumble_core::api::UploadUrlResponse = serde_json::from_slice(&url.body).unwrap();
    put_presigned(&upload.url, &upload.headers, bytes.clone());
    advance(1_000);
    let submitted = post(
        "bots/submit",
        &token,
        &json!({"source": "upload", "sha256": sha, "divisions": ["1v1"]}),
    );
    assert_eq!(submitted.status, 200, "{:?}", submitted.value());
    assert_eq!(submitted.value()["vid"], 4);
    assert!(
        objects
            .exists(Bucket::Jars, &format!("jars/{sha}.jar"))
            .unwrap()
    );
    assert!(
        !objects
            .exists(Bucket::State, &format!("uploads/g501/{sha}.jar"))
            .unwrap()
    );
    advance(1_000);
    let mine = get("me/bots", &token).value();
    assert_eq!(mine["bots"][0]["versions"][0]["state"], "probation");
    advance(1_000);
    let again = post(
        "bots/submit",
        &token,
        &json!({"source": "upload", "sha256": sha, "divisions": ["1v1"]}),
    );
    assert_eq!(again.status, 400, "the upload is gone after a submission");
}

/// The `batch` Lambda's scenario (`common::batch_scenario`) on DynamoDB Local and the S3
/// mock: fold → plan → publish of a 50-bot rumble, a crash rerun, a ban and a takedown.
#[test]
#[ignore = "needs Docker: cargo test --test aws_local -- --ignored"]
fn batch_against_dynamodb_local_and_s3() {
    let dynamo = Container::start(DYNAMODB_IMAGE, 8000, &[]);
    let s3 = Container::start(
        S3_IMAGE,
        9090,
        &[
            "-e",
            "COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS=jars,state,site",
            "-e",
            "initialBuckets=jars,state,site",
        ],
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (dynamo_config, s3_config) = runtime.block_on(async {
        let dynamo_config = sdk_config(&dynamo.url()).await;
        let s3_config = sdk_config(&s3.url()).await;
        create_table(&aws_sdk_dynamodb::Client::new(&dynamo_config)).await;
        wait_for_s3(&S3Objects::client(&s3_config, true), 3).await;
        (dynamo_config, s3_config)
    });
    runtime
        .block_on(async {
            tokio::task::spawn_blocking(move || {
                let store = Arc::new(DynamoStore::new(
                    aws_sdk_dynamodb::Client::new(&dynamo_config),
                    TABLE,
                ));
                let objects = Arc::new(
                    S3Objects::new(S3Objects::client(&s3_config, true), "jars", "state")
                        .with_site("site"),
                );
                let world = common::batch_scenario::World::new(store, objects);
                let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("batch-data-aws");
                common::batch_scenario::run(&world, &dir);
            })
            .await
        })
        .unwrap();
}

/// A `&dyn` stand-in for `impl Serialize` in the closures above.
mod erased {
    pub trait Body {
        fn bytes(&self) -> Vec<u8>;
    }

    impl<T: serde::Serialize> Body for T {
        fn bytes(&self) -> Vec<u8> {
            serde_json::to_vec(self).unwrap()
        }
    }
}
