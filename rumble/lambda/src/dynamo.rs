// SPDX-License-Identifier: EPL-2.0

//! [`DynamoStore`]: the core's `Store` on the one DynamoDB table of spec §9 (`pk`/`sk`
//! strings, `ttl`). Each method is one request (or the transaction its documentation in
//! `rumble_core::store` names), driven with [`block_on`] from the request's blocking
//! thread. Conditional check failures become `StoreError::ConditionFailed`, throttling
//! after the SDK's retries `StoreError::Busy`.
//!
//! Item layouts follow §9.2 with these additions (all documented on the core's types):
//! `ver` on users (reputation read-modify-write), `bo` (owned bots, a list), `hid`, `tr`,
//! `ghc`; `vid` on version items; `sd` (seeds) on log items; `i` and `v` (the robot order) in slot entries. Lists of
//! strings that may be empty are `L`, not `SS` (DynamoDB has no empty sets).

use crate::block_on;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::{
    AttributeValue, Delete, Put, ReturnValue, TransactWriteItem, Update,
};
use rumble_core::api::{BotColors, Rejected};
use rumble_core::ids::{Division, WeightClass, parse_vid36, vid36};
use rumble_core::store::{
    Alert, BatchMarker, BotItem, BotSource, BotState, LogItem, Reputation, ResultsTransaction, Run,
    SlotEntry, SlotItem, SlotStatus, Store, StoreError, StoreResult, TokenItem, TokenKind, User,
    UserPatch, UserState, VerifyItem, VerifyKind, VersionItem, VersionState,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

type Item = HashMap<String, AttributeValue>;

/// The table, with an instance cache of full slot items (they never change, §9.3).
pub struct DynamoStore {
    client: Client,
    table: String,
    full_slots: Mutex<HashMap<String, SlotItem>>,
}

// ---- attribute helpers ----

fn s(text: impl Into<String>) -> AttributeValue {
    AttributeValue::S(text.into())
}

fn n(number: impl ToString) -> AttributeValue {
    AttributeValue::N(number.to_string())
}

fn b(bytes: Vec<u8>) -> AttributeValue {
    AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(bytes))
}

fn strings(list: &[String]) -> AttributeValue {
    AttributeValue::L(list.iter().map(|x| s(x.clone())).collect())
}

fn numbers<T: std::fmt::Display>(list: &[T]) -> AttributeValue {
    AttributeValue::L(list.iter().map(n).collect())
}

fn divisions(list: &[Division]) -> AttributeValue {
    if list.is_empty() {
        AttributeValue::L(Vec::new())
    } else {
        AttributeValue::Ss(list.iter().map(|d| d.as_str().to_string()).collect())
    }
}

fn bad(name: &str) -> StoreError {
    StoreError::Other(format!("malformed attribute {name}"))
}

fn get_s(item: &Item, name: &str) -> StoreResult<String> {
    opt_s(item, name)?.ok_or_else(|| bad(name))
}

fn opt_s(item: &Item, name: &str) -> StoreResult<Option<String>> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(None),
        Some(AttributeValue::S(text)) => Ok(Some(text.clone())),
        Some(_) => Err(bad(name)),
    }
}

fn get_n<T: std::str::FromStr>(item: &Item, name: &str) -> StoreResult<T> {
    opt_n(item, name)?.ok_or_else(|| bad(name))
}

fn opt_n<T: std::str::FromStr>(item: &Item, name: &str) -> StoreResult<Option<T>> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(None),
        Some(AttributeValue::N(text)) => text.parse().map(Some).map_err(|_| bad(name)),
        Some(_) => Err(bad(name)),
    }
}

fn n_or<T: std::str::FromStr>(item: &Item, name: &str, default: T) -> StoreResult<T> {
    Ok(opt_n(item, name)?.unwrap_or(default))
}

fn get_bool(item: &Item, name: &str) -> StoreResult<bool> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(false),
        Some(AttributeValue::Bool(value)) => Ok(*value),
        Some(_) => Err(bad(name)),
    }
}

fn get_b(item: &Item, name: &str) -> StoreResult<Vec<u8>> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(Vec::new()),
        Some(AttributeValue::B(blob)) => Ok(blob.as_ref().to_vec()),
        Some(_) => Err(bad(name)),
    }
}

fn get_strings(item: &Item, name: &str) -> StoreResult<Vec<String>> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(Vec::new()),
        Some(AttributeValue::Ss(list)) => Ok(list.clone()),
        Some(AttributeValue::L(list)) => list
            .iter()
            .map(|v| v.as_s().cloned().map_err(|_| bad(name)))
            .collect(),
        Some(_) => Err(bad(name)),
    }
}

fn get_numbers<T: std::str::FromStr>(item: &Item, name: &str) -> StoreResult<Vec<T>> {
    match item.get(name) {
        None | Some(AttributeValue::Null(_)) => Ok(Vec::new()),
        Some(AttributeValue::L(list)) => list
            .iter()
            .map(|v| {
                v.as_n()
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .ok_or_else(|| bad(name))
            })
            .collect(),
        Some(AttributeValue::Ns(list)) => list
            .iter()
            .map(|t| t.parse().map_err(|_| bad(name)))
            .collect(),
        Some(_) => Err(bad(name)),
    }
}

fn get_divisions(item: &Item, name: &str) -> StoreResult<Vec<Division>> {
    let mut list: Vec<Division> = get_strings(item, name)?
        .iter()
        .map(|d| d.parse().map_err(|_| bad(name)))
        .collect::<StoreResult<_>>()?;
    list.sort_unstable();
    Ok(list)
}

fn parse<T>(result: Result<T, String>) -> StoreResult<T> {
    result.map_err(StoreError::Other)
}

/// Maps an SDK error: conditional failures (also inside a cancelled transaction) and
/// throttling.
fn map_err<E, R>(e: SdkError<E, R>) -> StoreError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug,
{
    let code = e.code().unwrap_or_default().to_string();
    let message = e.message().unwrap_or_default().to_string();
    match code.as_str() {
        "ConditionalCheckFailedException" => StoreError::ConditionFailed,
        "TransactionCanceledException" if message.contains("ConditionalCheckFailed") => {
            StoreError::ConditionFailed
        }
        "TransactionCanceledException" if message.contains("Throttl") => StoreError::Busy,
        "ProvisionedThroughputExceededException"
        | "ThrottlingException"
        | "RequestLimitExceeded"
        | "TransactionConflictException" => StoreError::Busy,
        _ => StoreError::Other(format!(
            "{code}: {message} ({})",
            aws_sdk_dynamodb::error::DisplayErrorContext(&e)
        )),
    }
}

fn key(pk: impl Into<String>, sk: impl Into<String>) -> Item {
    let mut item = Item::new();
    item.insert("pk".into(), s(pk));
    item.insert("sk".into(), s(sk));
    item
}

fn user_key(uid: &str) -> Item {
    key(format!("U#{uid}"), "P")
}

fn token_key(hash: &str) -> Item {
    key(format!("T#{hash}"), "T")
}

fn token_index_key(uid: &str, hash: &str) -> Item {
    key(
        format!("U#{uid}"),
        format!("T#{}", &hash[..hash.len().min(12)]),
    )
}

fn bot_key(id: &str) -> Item {
    key(format!("BOT#{id}"), "M")
}

fn version_key(vid: u32) -> Item {
    key("REG", format!("V#{}", vid36(vid)))
}

fn marker_key(batch: &str) -> Item {
    key(format!("B#{batch}"), "B")
}

fn slot_pk(engine: u32, division: Division, slot_key: &str) -> String {
    format!("S#{engine}#{}#{slot_key}", division.as_str())
}

// ---- items ----

fn user_item(user: &User) -> Item {
    let mut item = user_key(&user.uid);
    item.insert("login".into(), s(&user.login));
    item.insert("name".into(), s(&user.name));
    item.insert("country".into(), s(&user.country));
    item.insert("ghid".into(), n(user.github_id));
    if let Some(created) = user.github_created {
        item.insert("ghc".into(), n(created));
    }
    item.insert("adm".into(), AttributeValue::Bool(user.admin));
    item.insert("tr".into(), AttributeValue::Bool(user.trusted));
    item.insert("hid".into(), AttributeValue::Bool(user.hidden));
    item.insert("bo".into(), strings(&user.bots));
    item.insert("bat".into(), n(user.battles));
    if let Some(last) = user.last_post {
        item.insert("lp".into(), n(last));
    }
    item.insert("cr".into(), n(user.created));
    reputation_attributes(&mut item, &user.reputation, user.reputation.ver);
    item
}

fn reputation_attributes(item: &mut Item, reputation: &Reputation, ver: u64) {
    item.insert("ver".into(), n(ver));
    item.insert("rep".into(), n(reputation.rep));
    item.insert("st".into(), s(reputation.state.as_str()));
    item.insert("vm".into(), n(reputation.verified_matches));
    item.insert("mm".into(), n(reputation.mismatches));
    item.insert("mmt".into(), numbers(&reputation.mismatch_times));
    item.insert("qm".into(), n(reputation.quarantine_matches));
}

fn user_from(item: &Item) -> StoreResult<User> {
    let pk = get_s(item, "pk")?;
    Ok(User {
        uid: pk.strip_prefix("U#").ok_or_else(|| bad("pk"))?.to_string(),
        login: opt_s(item, "login")?.unwrap_or_default(),
        name: opt_s(item, "name")?.unwrap_or_default(),
        country: opt_s(item, "country")?.unwrap_or_default(),
        github_id: n_or(item, "ghid", 0)?,
        github_created: opt_n(item, "ghc")?,
        admin: get_bool(item, "adm")?,
        trusted: get_bool(item, "tr")?,
        hidden: get_bool(item, "hid")?,
        bots: get_strings(item, "bo")?,
        battles: n_or(item, "bat", 0)?,
        last_post: opt_n(item, "lp")?,
        created: n_or(item, "cr", 0)?,
        reputation: Reputation {
            ver: n_or(item, "ver", 0)?,
            rep: n_or(item, "rep", 0)?,
            state: match opt_s(item, "st")? {
                Some(st) => parse(UserState::parse(&st))?,
                None => UserState::New,
            },
            verified_matches: n_or(item, "vm", 0)?,
            mismatches: n_or(item, "mm", 0)?,
            mismatch_times: get_numbers(item, "mmt")?,
            quarantine_matches: n_or(item, "qm", 0)?,
        },
    })
}

fn token_attributes(item: &mut Item, token: &TokenItem) {
    item.insert("k".into(), s(token.kind.as_str()));
    item.insert("lbl".into(), s(&token.label));
    item.insert("cr".into(), n(token.created));
    item.insert("lu".into(), n(token.last_used));
    if let Some(expires) = token.expires {
        item.insert("ttl".into(), n(expires));
    }
}

fn token_from(item: &Item, hash: String, uid: String) -> StoreResult<TokenItem> {
    Ok(TokenItem {
        hash,
        uid,
        kind: parse(TokenKind::parse(&get_s(item, "k")?))?,
        label: opt_s(item, "lbl")?.unwrap_or_default(),
        created: n_or(item, "cr", 0)?,
        last_used: n_or(item, "lu", 0)?,
        expires: opt_n(item, "ttl")?,
    })
}

fn bot_item(bot: &BotItem) -> Item {
    let mut item = bot_key(&bot.id);
    item.insert("au".into(), s(&bot.author));
    if let Some(owner) = &bot.owner {
        item.insert("uid".into(), s(owner));
    }
    if let Some(homepage) = &bot.homepage {
        item.insert("hp".into(), s(homepage));
    }
    if let Some(description) = &bot.description {
        item.insert("ds".into(), s(description));
    }
    item.insert("dv".into(), divisions(&bot.divisions));
    item.insert("vs".into(), numbers(&bot.versions));
    if let Some(current) = bot.current {
        item.insert("cur".into(), n(current));
    }
    item.insert("st".into(), s(bot.state.as_str()));
    item.insert("cr".into(), n(bot.created));
    item.insert("src".into(), s(bot.source.as_str()));
    item
}

fn bot_from(item: &Item) -> StoreResult<BotItem> {
    let pk = get_s(item, "pk")?;
    Ok(BotItem {
        id: pk
            .strip_prefix("BOT#")
            .ok_or_else(|| bad("pk"))?
            .to_string(),
        author: opt_s(item, "au")?.unwrap_or_default(),
        owner: opt_s(item, "uid")?,
        homepage: opt_s(item, "hp")?,
        description: opt_s(item, "ds")?,
        divisions: get_divisions(item, "dv")?,
        versions: get_numbers(item, "vs")?,
        current: opt_n(item, "cur")?,
        state: parse(BotState::parse(&get_s(item, "st")?))?,
        created: n_or(item, "cr", 0)?,
        source: parse(BotSource::parse(&get_s(item, "src")?))?,
    })
}

fn version_item(version: &VersionItem) -> Item {
    let mut item = version_key(version.vid);
    item.insert("vid".into(), n(version.vid));
    item.insert("bot".into(), s(&version.bot));
    item.insert("ver".into(), s(&version.version));
    item.insert("sha".into(), s(&version.sha256));
    item.insert("sz".into(), n(version.size));
    if let Some(codesize) = version.codesize {
        item.insert("cs".into(), n(codesize));
    }
    item.insert("wc".into(), s(version.class.as_str()));
    item.insert("dv".into(), divisions(&version.divisions));
    item.insert("st".into(), s(version.state.as_str()));
    item.insert("pb".into(), n(version.probation_battles));
    item.insert("pf".into(), n(version.probation_failures));
    item.insert("cls".into(), s(&version.class_name));
    item.insert("cr".into(), n(version.created));
    if let Some(retired) = version.retired {
        item.insert("ret".into(), n(retired));
    }
    if let Some(url) = &version.url {
        item.insert("url".into(), s(url));
    }
    if let Some(colors) = &version.colors {
        item.insert("col".into(), s(json(colors)));
    }
    if let Some(released) = version.released {
        item.insert("rd".into(), n(released));
    }
    if let Some(reason) = &version.nondeterministic {
        item.insert("nd".into(), s(reason));
    }
    if version.disagreements > 0 {
        item.insert("ndn".into(), n(version.disagreements));
    }
    item
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializable")
}

/// A JSON attribute; absent is `T::default()`.
fn opt_json<T: serde::de::DeserializeOwned + Default>(item: &Item, name: &str) -> StoreResult<T> {
    match opt_s(item, name)? {
        Some(text) => serde_json::from_str(&text).map_err(|_| bad(name)),
        None => Ok(T::default()),
    }
}

fn version_from(item: &Item) -> StoreResult<VersionItem> {
    let sk = get_s(item, "sk")?;
    let vid = match opt_n(item, "vid")? {
        Some(vid) => vid,
        None => parse(parse_vid36(sk.strip_prefix("V#").ok_or_else(|| bad("sk"))?))?,
    };
    Ok(VersionItem {
        vid,
        bot: get_s(item, "bot")?,
        version: get_s(item, "ver")?,
        sha256: get_s(item, "sha")?,
        size: n_or(item, "sz", 0)?,
        codesize: opt_n(item, "cs")?,
        class: get_s(item, "wc")?
            .parse::<WeightClass>()
            .map_err(StoreError::Other)?,
        divisions: get_divisions(item, "dv")?,
        state: parse(VersionState::parse(&get_s(item, "st")?))?,
        probation_battles: n_or(item, "pb", 0)?,
        probation_failures: n_or(item, "pf", 0)?,
        class_name: opt_s(item, "cls")?.unwrap_or_default(),
        created: n_or(item, "cr", 0)?,
        retired: opt_n(item, "ret")?,
        url: opt_s(item, "url")?,
        colors: opt_json(item, "col")?,
        released: opt_n(item, "rd")?,
        nondeterministic: opt_s(item, "nd")?,
        disagreements: n_or(item, "ndn", 0)?,
    })
}

fn u32s_to_bytes(list: &[u32]) -> Vec<u8> {
    list.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

fn marker_item(marker: &BatchMarker) -> Item {
    let mut item = marker_key(&marker.batch_id);
    item.insert("uid".into(), s(&marker.uid));
    item.insert("acc".into(), b(u32s_to_bytes(&marker.accepted)));
    item.insert(
        "rej".into(),
        s(serde_json::to_string(&marker.rejected).expect("serializable")),
    );
    if !marker.slots.is_empty() {
        item.insert("slots".into(), AttributeValue::Ss(marker.slots.clone()));
    }
    item.insert("ttl".into(), n(marker.ttl));
    item
}

fn marker_from(item: &Item) -> StoreResult<BatchMarker> {
    let pk = get_s(item, "pk")?;
    let rejected: Vec<Rejected> = match opt_s(item, "rej")? {
        Some(json) => serde_json::from_str(&json).map_err(|_| bad("rej"))?,
        None => Vec::new(),
    };
    let mut slots = get_strings(item, "slots")?;
    slots.sort();
    Ok(BatchMarker {
        batch_id: pk.strip_prefix("B#").ok_or_else(|| bad("pk"))?.to_string(),
        uid: get_s(item, "uid")?,
        accepted: bytes_to_u32s(&get_b(item, "acc")?),
        rejected,
        slots,
        ttl: n_or(item, "ttl", 0)?,
    })
}

fn log_item(log: &LogItem) -> Item {
    let mut item = key(format!("R#{}#{}", log.engine, log.hour), &log.sort);
    item.insert("uid".into(), s(&log.uid));
    item.insert("recs".into(), b(log.records.clone()));
    item.insert(
        "sd".into(),
        b(log.seeds.iter().flat_map(|s| s.to_le_bytes()).collect()),
    );
    item.insert("nb".into(), n(log.battles));
    item.insert("held".into(), AttributeValue::Bool(log.held));
    item.insert("ttl".into(), n(log.ttl));
    item
}

fn log_from(item: &Item) -> StoreResult<LogItem> {
    let pk = get_s(item, "pk")?;
    let mut parts = pk.split('#');
    let (Some("R"), Some(engine), Some(hour)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad("pk"));
    };
    Ok(LogItem {
        engine: engine.parse().map_err(|_| bad("pk"))?,
        hour: hour.parse().map_err(|_| bad("pk"))?,
        sort: get_s(item, "sk")?,
        uid: get_s(item, "uid")?,
        records: get_b(item, "recs")?,
        seeds: get_b(item, "sd")?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| i64::from_le_bytes(*c))
            .collect(),
        battles: n_or(item, "nb", 0)?,
        held: get_bool(item, "held")?,
        ttl: n_or(item, "ttl", 0)?,
    })
}

fn run_value(run: &Run) -> AttributeValue {
    let mut map = Item::new();
    map.insert("uid".into(), s(&run.uid));
    if let Some(hash) = &run.hash {
        map.insert("h".into(), s(hash));
    }
    map.insert("rec".into(), b(run.record.clone()));
    map.insert("held".into(), AttributeValue::Bool(run.held));
    if !run.colors.is_empty() {
        map.insert("col".into(), s(json(&run.colors)));
    }
    AttributeValue::M(map)
}

fn run_from(value: &AttributeValue) -> StoreResult<Run> {
    let map = value.as_m().map_err(|_| bad("runs"))?;
    Ok(Run {
        uid: get_s(map, "uid")?,
        hash: opt_s(map, "h")?,
        record: get_b(map, "rec")?,
        held: get_bool(map, "held")?,
        colors: opt_json(map, "col")?,
    })
}

fn verify_item(verify: &VerifyItem) -> Item {
    let mut item = key(format!("V#{}", verify.engine), &verify.sort);
    item.insert("div".into(), s(verify.division.as_str()));
    item.insert("seed".into(), n(verify.seed));
    item.insert("bots".into(), numbers(&verify.bots));
    item.insert("rounds".into(), n(verify.rounds));
    item.insert(
        "runs".into(),
        AttributeValue::L(verify.runs.iter().map(run_value).collect()),
    );
    if !verify.uids.is_empty() {
        item.insert("uids".into(), AttributeValue::Ss(verify.uids.clone()));
    }
    item.insert("stage".into(), n(verify.stage));
    if let Some(claimant) = &verify.claimant {
        item.insert("cl".into(), s(claimant));
    }
    if let Some(until) = verify.claim_until {
        item.insert("cu".into(), n(until));
    }
    item.insert("kind".into(), s(verify.kind.as_str()));
    item.insert("ttl".into(), n(verify.ttl));
    item
}

fn verify_from(item: &Item) -> StoreResult<VerifyItem> {
    let pk = get_s(item, "pk")?;
    let runs = match item.get("runs") {
        Some(AttributeValue::L(list)) => list.iter().map(run_from).collect::<StoreResult<_>>()?,
        _ => Vec::new(),
    };
    let mut uids = get_strings(item, "uids")?;
    uids.sort();
    Ok(VerifyItem {
        engine: pk
            .strip_prefix("V#")
            .and_then(|e| e.parse().ok())
            .ok_or_else(|| bad("pk"))?,
        sort: get_s(item, "sk")?,
        division: get_s(item, "div")?.parse().map_err(StoreError::Other)?,
        seed: get_n(item, "seed")?,
        bots: get_numbers(item, "bots")?,
        rounds: get_n(item, "rounds")?,
        runs,
        uids,
        stage: n_or(item, "stage", 1)?,
        claimant: opt_s(item, "cl")?,
        claim_until: opt_n(item, "cu")?,
        kind: parse(VerifyKind::parse(&get_s(item, "kind")?))?,
        ttl: n_or(item, "ttl", 0)?,
    })
}

fn slot_value(entry: &SlotEntry) -> AttributeValue {
    let mut map = Item::new();
    map.insert("st".into(), s(entry.status.as_str()));
    map.insert("b".into(), s(&entry.batch));
    map.insert("i".into(), n(entry.index));
    map.insert("r".into(), n(entry.round));
    map.insert("h".into(), s(&entry.sha256_b64));
    map.insert("n".into(), n(entry.size));
    map.insert("x".into(), n(entry.claim_until));
    map.insert("sd".into(), n(entry.seed));
    if !entry.order.is_empty() {
        map.insert("v".into(), numbers(&entry.order));
    }
    AttributeValue::M(map)
}

fn slot_from(value: &AttributeValue) -> StoreResult<SlotEntry> {
    let map = value.as_m().map_err(|_| bad("m"))?;
    Ok(SlotEntry {
        status: parse(SlotStatus::parse(&get_s(map, "st")?))?,
        batch: get_s(map, "b")?,
        index: n_or(map, "i", 0)?,
        round: get_n(map, "r")?,
        sha256_b64: get_s(map, "h")?,
        size: get_n(map, "n")?,
        claim_until: get_n(map, "x")?,
        seed: get_n(map, "sd")?,
        order: get_numbers(map, "v")?,
    })
}

fn slots_from(item: &Item) -> StoreResult<SlotItem> {
    let mut entries = BTreeMap::new();
    if let Some(AttributeValue::M(map)) = item.get("m") {
        for (id, value) in map {
            entries.insert(id.clone(), slot_from(value)?);
        }
    }
    Ok(SlotItem {
        entries,
        full: get_bool(item, "full")?,
    })
}

fn alert_from(item: &Item) -> StoreResult<Alert> {
    Ok(Alert {
        sort: get_s(item, "sk")?,
        kind: get_s(item, "kind")?,
        text: opt_s(item, "text")?.unwrap_or_default(),
        reference: opt_s(item, "ref")?.unwrap_or_default(),
        ttl: n_or(item, "ttl", 0)?,
    })
}

impl DynamoStore {
    pub fn new(client: Client, table: &str) -> DynamoStore {
        DynamoStore {
            client,
            table: table.to_string(),
            full_slots: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: Item) -> StoreResult<Option<Item>> {
        block_on(
            self.client
                .get_item()
                .table_name(&self.table)
                .set_key(Some(key))
                .send(),
        )
        .map(|output| output.item)
        .map_err(map_err)
    }

    fn put(&self, item: Item, condition: Option<&str>) -> StoreResult<()> {
        block_on(
            self.client
                .put_item()
                .table_name(&self.table)
                .set_item(Some(item))
                .set_condition_expression(condition.map(str::to_string))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    /// Query of a partition, all pages, optionally `sk > after` or `begins_with(sk, ..)`.
    fn query(
        &self,
        pk: &str,
        sk_condition: Option<(&str, &str)>,
        limit: Option<usize>,
    ) -> StoreResult<Vec<Item>> {
        let mut items = Vec::new();
        let mut start: Option<Item> = None;
        loop {
            let mut request = self
                .client
                .query()
                .table_name(&self.table)
                .expression_attribute_values(":pk", s(pk));
            let mut condition = "pk = :pk".to_string();
            if let Some((expression, value)) = sk_condition {
                condition.push_str(" AND ");
                condition.push_str(expression);
                request = request.expression_attribute_values(":sk", s(value));
            }
            request = request.key_condition_expression(condition);
            if let Some(limit) = limit {
                let left = limit.saturating_sub(items.len()).min(1000) as i32;
                if left == 0 {
                    break;
                }
                request = request.limit(left);
            }
            let output =
                block_on(request.set_exclusive_start_key(start.take()).send()).map_err(map_err)?;
            items.extend(output.items.unwrap_or_default());
            match output.last_evaluated_key {
                Some(last) if !last.is_empty() => start = Some(last),
                _ => break,
            }
        }
        Ok(items)
    }

    fn put_tx(&self, item: Item, condition: Option<&str>) -> StoreResult<TransactWriteItem> {
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(item))
            .set_condition_expression(condition.map(str::to_string))
            .build()
            .map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(TransactWriteItem::builder().put(put).build())
    }

    fn transact(&self, items: Vec<TransactWriteItem>) -> StoreResult<()> {
        block_on(
            self.client
                .transact_write_items()
                .set_transact_items(Some(items))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn slot_update(
        &self,
        pk: &str,
        expected: &[(String, Option<SlotEntry>)],
        set: &[(String, SlotEntry)],
        full: bool,
    ) -> StoreResult<()> {
        let mut names: HashMap<String, String> = HashMap::new();
        fn name_of(id: &str, names: &mut HashMap<String, String>) -> String {
            let placeholder = format!("#s{}", names.len());
            names.entry(id.to_string()).or_insert(placeholder).clone()
        }
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, "S")));
        let mut sets = Vec::new();
        for (i, (id, entry)) in set.iter().enumerate() {
            let name = name_of(id, &mut names);
            sets.push(format!("m.{name} = :v{i}"));
            request = request.expression_attribute_values(format!(":v{i}"), slot_value(entry));
        }
        sets.push("#full = :full".to_string());
        request = request
            .expression_attribute_values(":full", AttributeValue::Bool(full))
            .expression_attribute_names("#full", "full");
        let mut conditions = vec!["attribute_exists(m)".to_string()];
        for (i, (id, entry)) in expected.iter().enumerate() {
            let name = name_of(id, &mut names);
            match entry {
                None => conditions.push(format!("attribute_not_exists(m.{name})")),
                Some(entry) => {
                    conditions.push(format!("m.{name} = :e{i}"));
                    request =
                        request.expression_attribute_values(format!(":e{i}"), slot_value(entry));
                }
            }
        }
        for (id, placeholder) in names {
            request = request.expression_attribute_names(placeholder, id);
        }
        block_on(
            request
                .update_expression(format!("SET {}", sets.join(", ")))
                .condition_expression(conditions.join(" AND "))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }
}

impl Store for DynamoStore {
    fn get_user(&self, uid: &str) -> StoreResult<Option<User>> {
        self.get(user_key(uid))?.as_ref().map(user_from).transpose()
    }

    fn create_user(&self, user: &User) -> StoreResult<()> {
        self.put(user_item(user), Some("attribute_not_exists(pk)"))
    }

    fn patch_user(&self, uid: &str, patch: &UserPatch) -> StoreResult<()> {
        let mut sets = Vec::new();
        let mut values = Item::new();
        let mut add = |name: &str, value: AttributeValue| {
            sets.push(format!("#{name} = :{name}"));
            values.insert(format!(":{name}"), value);
        };
        if let Some(login) = &patch.login {
            add("login", s(login));
        }
        if let Some(name) = &patch.name {
            add("name", s(name));
        }
        if let Some(country) = &patch.country {
            add("country", s(country));
        }
        if let Some(created) = patch.github_created {
            add("ghc", n(created));
        }
        if let Some(admin) = patch.admin {
            add("adm", AttributeValue::Bool(admin));
        }
        if let Some(trusted) = patch.trusted {
            add("tr", AttributeValue::Bool(trusted));
        }
        if let Some(hidden) = patch.hidden {
            add("hid", AttributeValue::Bool(hidden));
        }
        if let Some(bots) = &patch.bots {
            add("bo", strings(bots));
        }
        if sets.is_empty() {
            // Nothing to change: only the existence check.
            return match self.get(user_key(uid))? {
                Some(_) => Ok(()),
                None => Err(StoreError::ConditionFailed),
            };
        }
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(user_key(uid)))
            .condition_expression("attribute_exists(pk)");
        for set in &sets {
            let name = set.split(' ').next().unwrap();
            request = request.expression_attribute_names(name, &name[1..]);
        }
        block_on(
            request
                .update_expression(format!("SET {}", sets.join(", ")))
                .set_expression_attribute_values((!values.is_empty()).then_some(values))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn set_reputation(
        &self,
        uid: &str,
        expected_ver: u64,
        reputation: &Reputation,
    ) -> StoreResult<()> {
        let mut values = Item::new();
        reputation_attributes(&mut values, reputation, expected_ver + 1);
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(user_key(uid)));
        let mut sets = Vec::new();
        for (name, value) in values {
            sets.push(format!("#{name} = :{name}"));
            request = request
                .expression_attribute_names(format!("#{name}"), &name)
                .expression_attribute_values(format!(":{name}"), value);
        }
        let condition = if expected_ver == 0 {
            "attribute_exists(pk) AND (attribute_not_exists(#ver) OR #ver = :old)"
        } else {
            "#ver = :old"
        };
        block_on(
            request
                .expression_attribute_values(":old", n(expected_ver))
                .update_expression(format!("SET {}", sets.join(", ")))
                .condition_expression(condition)
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn put_token(&self, token: &TokenItem) -> StoreResult<()> {
        let mut main = token_key(&token.hash);
        main.insert("uid".into(), s(&token.uid));
        token_attributes(&mut main, token);
        let mut index = token_index_key(&token.uid, &token.hash);
        index.insert("h".into(), s(&token.hash));
        token_attributes(&mut index, token);
        self.transact(vec![
            self.put_tx(main, Some("attribute_not_exists(pk)"))?,
            self.put_tx(index, None)?,
        ])
    }

    fn get_token(&self, hash: &str) -> StoreResult<Option<TokenItem>> {
        match self.get(token_key(hash))? {
            Some(item) => {
                let uid = get_s(&item, "uid")?;
                token_from(&item, hash.to_string(), uid).map(Some)
            }
            None => Ok(None),
        }
    }

    fn list_tokens(&self, uid: &str) -> StoreResult<Vec<TokenItem>> {
        let items = self.query(
            &format!("U#{uid}"),
            Some(("begins_with(sk, :sk)", "T#")),
            None,
        )?;
        let mut tokens = items
            .iter()
            .map(|item| token_from(item, get_s(item, "h")?, uid.to_string()))
            .collect::<StoreResult<Vec<_>>>()?;
        tokens.sort_by(|a, b| a.id().cmp(b.id()));
        Ok(tokens)
    }

    fn touch_token(&self, hash: &str, last_used: u64, expires: Option<u64>) -> StoreResult<()> {
        let update = |key: Item, condition: bool| {
            let mut request = self
                .client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(key))
                .expression_attribute_values(":lu", n(last_used))
                .return_values(ReturnValue::AllNew);
            let mut expression = "SET lu = :lu".to_string();
            if let Some(expires) = expires {
                expression.push_str(", #ttl = :ttl");
                request = request
                    .expression_attribute_names("#ttl", "ttl")
                    .expression_attribute_values(":ttl", n(expires));
            }
            if condition {
                request = request.condition_expression("attribute_exists(pk)");
            }
            block_on(request.update_expression(expression).send()).map_err(map_err)
        };
        let output = update(token_key(hash), true)?;
        let uid = output
            .attributes
            .as_ref()
            .map(|item| get_s(item, "uid"))
            .transpose()?
            .ok_or_else(|| bad("uid"))?;
        // The index item exists alongside (put in one transaction).
        update(token_index_key(&uid, hash), true).map(|_| ())
    }

    fn delete_token(&self, uid: &str, hash: &str) -> StoreResult<()> {
        let main = Delete::builder()
            .table_name(&self.table)
            .set_key(Some(token_key(hash)))
            .condition_expression("attribute_not_exists(pk) OR uid = :uid")
            .expression_attribute_values(":uid", s(uid))
            .build()
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let index = Delete::builder()
            .table_name(&self.table)
            .set_key(Some(token_index_key(uid, hash)))
            .build()
            .map_err(|e| StoreError::Other(e.to_string()))?;
        match self.transact(vec![
            TransactWriteItem::builder().delete(main).build(),
            TransactWriteItem::builder().delete(index).build(),
        ]) {
            // Another user's token: nothing to delete.
            Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn next_vid(&self) -> StoreResult<u32> {
        let output = block_on(
            self.client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(key("CTR", "vid")))
                .update_expression("ADD #n :one")
                .expression_attribute_names("#n", "n")
                .expression_attribute_values(":one", n(1))
                .return_values(ReturnValue::UpdatedNew)
                .send(),
        )
        .map_err(map_err)?;
        output
            .attributes
            .as_ref()
            .map(|item| get_n(item, "n"))
            .transpose()?
            .ok_or_else(|| bad("n"))
    }

    fn get_bot(&self, id: &str) -> StoreResult<Option<BotItem>> {
        self.get(bot_key(id))?.as_ref().map(bot_from).transpose()
    }

    fn get_version(&self, vid: u32) -> StoreResult<Option<VersionItem>> {
        self.get(version_key(vid))?
            .as_ref()
            .map(version_from)
            .transpose()
    }

    fn list_versions(&self) -> StoreResult<Vec<VersionItem>> {
        let mut versions = self
            .query("REG", Some(("begins_with(sk, :sk)", "V#")), None)?
            .iter()
            .map(version_from)
            .collect::<StoreResult<Vec<_>>>()?;
        versions.sort_by_key(|v| v.vid);
        Ok(versions)
    }

    fn register_version(
        &self,
        bot: &BotItem,
        expected_versions: Option<&[u32]>,
        version: &VersionItem,
    ) -> StoreResult<()> {
        let version_put = self.put_tx(version_item(version), Some("attribute_not_exists(pk)"))?;
        let bot_put = match expected_versions {
            None => self.put_tx(bot_item(bot), Some("attribute_not_exists(pk)"))?,
            Some(expected) => {
                let put = Put::builder()
                    .table_name(&self.table)
                    .set_item(Some(bot_item(bot)))
                    .condition_expression("vs = :vs")
                    .expression_attribute_values(":vs", numbers(expected))
                    .build()
                    .map_err(|e| StoreError::Other(e.to_string()))?;
                TransactWriteItem::builder().put(put).build()
            }
        };
        self.transact(vec![version_put, bot_put])
    }

    fn put_bot(&self, bot: &BotItem) -> StoreResult<()> {
        self.put(bot_item(bot), None)
    }

    fn put_version(&self, version: &VersionItem) -> StoreResult<()> {
        self.put(version_item(version), None)
    }

    fn update_version(&self, version: &VersionItem, expected: VersionState) -> StoreResult<()> {
        block_on(
            self.client
                .put_item()
                .table_name(&self.table)
                .set_item(Some(version_item(version)))
                .condition_expression("#st = :expected")
                .expression_attribute_names("#st", "st")
                .expression_attribute_values(":expected", s(expected.as_str()))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn set_version_colors(&self, vid: u32, colors: &BotColors) -> StoreResult<()> {
        block_on(
            self.client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(version_key(vid)))
                .update_expression("SET col = :col")
                .condition_expression("attribute_exists(pk) AND attribute_not_exists(col)")
                .expression_attribute_values(":col", s(json(colors)))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn set_version_nondeterministic(&self, vid: u32, reason: Option<&str>) -> StoreResult<()> {
        let update = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(version_key(vid)))
            .condition_expression("attribute_exists(pk)");
        let update = match reason {
            Some(reason) => update
                .update_expression("SET nd = :nd")
                .expression_attribute_values(":nd", s(reason)),
            None => update.update_expression("REMOVE nd"),
        };
        block_on(update.send()).map(|_| ()).map_err(map_err)
    }

    fn add_version_disagreement(&self, vid: u32) -> StoreResult<u32> {
        let output = block_on(
            self.client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(version_key(vid)))
                .update_expression("ADD ndn :one")
                .condition_expression("attribute_exists(pk)")
                .expression_attribute_values(":one", n(1))
                .return_values(ReturnValue::UpdatedNew)
                .send(),
        )
        .map_err(map_err)?;
        output
            .attributes
            .as_ref()
            .map(|item| get_n(item, "ndn"))
            .transpose()?
            .ok_or_else(|| bad("ndn"))
    }

    fn get_marker(&self, batch_id: &str) -> StoreResult<Option<BatchMarker>> {
        self.get(marker_key(batch_id))?
            .as_ref()
            .map(marker_from)
            .transpose()
    }

    fn commit_results(&self, tx: &ResultsTransaction) -> StoreResult<()> {
        let mut items =
            vec![self.put_tx(marker_item(&tx.marker), Some("attribute_not_exists(pk)"))?];
        if let Some(log) = &tx.log {
            items.push(self.put_tx(log_item(log), Some("attribute_not_exists(pk)"))?);
        }
        let update = Update::builder()
            .table_name(&self.table)
            .set_key(Some(user_key(&tx.uid)))
            .update_expression("ADD bat :bat SET lp = :lp")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_values(":bat", n(tx.battles))
            .expression_attribute_values(":lp", n(tx.last_post))
            .build()
            .map_err(|e| StoreError::Other(e.to_string()))?;
        items.push(TransactWriteItem::builder().update(update).build());
        self.transact(items)
    }

    fn add_marker_slots(&self, batch_id: &str, slots: &[String]) -> StoreResult<()> {
        if slots.is_empty() {
            return Ok(());
        }
        block_on(
            self.client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(marker_key(batch_id)))
                .update_expression("ADD slots :slots")
                .condition_expression("attribute_exists(pk)")
                .expression_attribute_values(":slots", AttributeValue::Ss(slots.to_vec()))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn append_log(&self, item: &LogItem) -> StoreResult<()> {
        self.put(log_item(item), Some("attribute_not_exists(pk)"))
    }

    fn query_log(&self, engine: u32, hour: u32) -> StoreResult<Vec<LogItem>> {
        self.query(&format!("R#{engine}#{hour}"), None, None)?
            .iter()
            .map(log_from)
            .collect()
    }

    fn put_verify(&self, item: &VerifyItem) -> StoreResult<()> {
        self.put(verify_item(item), Some("attribute_not_exists(pk)"))
    }

    fn get_verify(&self, engine: u32, sort: &str) -> StoreResult<Option<VerifyItem>> {
        self.get(key(format!("V#{engine}"), sort))?
            .as_ref()
            .map(verify_from)
            .transpose()
    }

    fn query_verify(
        &self,
        engine: u32,
        after: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<VerifyItem>> {
        let pk = format!("V#{engine}");
        let condition = after.map(|after| ("sk > :sk", after));
        self.query(&pk, condition, Some(limit))?
            .iter()
            .map(verify_from)
            .collect()
    }

    fn claim_verify(
        &self,
        engine: u32,
        sort: &str,
        uid: &str,
        until: u64,
        now: u64,
    ) -> StoreResult<()> {
        block_on(
            self.client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(key(format!("V#{engine}"), sort)))
                .update_expression("SET cl = :uid, cu = :until")
                .condition_expression(
                    "attribute_exists(pk) AND NOT contains(uids, :uid) AND \
                     (attribute_not_exists(cl) OR attribute_not_exists(cu) OR cu <= :now)",
                )
                .expression_attribute_values(":uid", s(uid))
                .expression_attribute_values(":until", n(until))
                .expression_attribute_values(":now", n(now))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn replace_verify(&self, item: &VerifyItem, claimant: &str) -> StoreResult<()> {
        block_on(
            self.client
                .put_item()
                .table_name(&self.table)
                .set_item(Some(verify_item(item)))
                .condition_expression("cl = :claimant")
                .expression_attribute_values(":claimant", s(claimant))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn delete_verify(&self, engine: u32, sort: &str, claimant: Option<&str>) -> StoreResult<()> {
        let mut request = self
            .client
            .delete_item()
            .table_name(&self.table)
            .set_key(Some(key(format!("V#{engine}"), sort)));
        request = match claimant {
            Some(claimant) => request
                .condition_expression("attribute_exists(pk) AND cl = :claimant")
                .expression_attribute_values(":claimant", s(claimant)),
            None => request.condition_expression("attribute_exists(pk)"),
        };
        block_on(request.send()).map(|_| ()).map_err(map_err)
    }

    fn get_slots(
        &self,
        engine: u32,
        division: Division,
        slot_key: &str,
    ) -> StoreResult<Option<SlotItem>> {
        let pk = slot_pk(engine, division, slot_key);
        if let Some(full) = self.full_slots.lock().unwrap().get(&pk) {
            return Ok(Some(full.clone()));
        }
        let item = self
            .get(key(&pk, "S"))?
            .as_ref()
            .map(slots_from)
            .transpose()?;
        if let Some(item) = item.as_ref().filter(|i| i.full) {
            let mut cache = self.full_slots.lock().unwrap();
            if cache.len() > 50_000 {
                cache.clear();
            }
            cache.insert(pk, item.clone());
        }
        Ok(item)
    }

    fn update_slots(
        &self,
        engine: u32,
        division: Division,
        slot_key: &str,
        expected: &[(String, Option<SlotEntry>)],
        set: &[(String, SlotEntry)],
        full: bool,
    ) -> StoreResult<()> {
        let pk = slot_pk(engine, division, slot_key);
        match self.slot_update(&pk, expected, set, full) {
            // A nested SET needs the map: if the item is missing (the condition requires
            // `m`), create it empty and try again. If it existed, the failure was real.
            Err(StoreError::ConditionFailed) if expected.iter().all(|(_, e)| e.is_none()) => {
                let mut item = key(&pk, "S");
                item.insert("m".into(), AttributeValue::M(Item::new()));
                item.insert("full".into(), AttributeValue::Bool(false));
                self.put(item, Some("attribute_not_exists(pk)"))?;
                self.slot_update(&pk, expected, set, full)?;
            }
            other => other?,
        }
        if full {
            self.full_slots.lock().unwrap().remove(&pk);
        }
        Ok(())
    }

    fn put_alert(&self, alert: &Alert) -> StoreResult<()> {
        let mut item = key("ALERT", &alert.sort);
        item.insert("kind".into(), s(&alert.kind));
        item.insert("text".into(), s(&alert.text));
        item.insert("ref".into(), s(&alert.reference));
        item.insert("ttl".into(), n(alert.ttl));
        self.put(item, None)
    }

    fn list_alerts(&self) -> StoreResult<Vec<Alert>> {
        self.query("ALERT", None, None)?
            .iter()
            .map(alert_from)
            .collect()
    }

    fn remove_slots(
        &self,
        engine: u32,
        division: Division,
        slot_key: &str,
        remove: &[(String, SlotEntry)],
    ) -> StoreResult<()> {
        if remove.is_empty() {
            return Ok(());
        }
        let pk = slot_pk(engine, division, slot_key);
        self.full_slots.lock().unwrap().remove(&pk);
        // The condition names the status and the track hash, which identify the entry
        // (older entries may lack attributes a full comparison would trip over).
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(key(&pk, "S")))
            .expression_attribute_names("#full", "full");
        let mut removes = Vec::new();
        let mut conditions = vec!["attribute_exists(m)".to_string()];
        for (i, (id, entry)) in remove.iter().enumerate() {
            request = request
                .expression_attribute_names(format!("#s{i}"), id)
                .expression_attribute_values(format!(":st{i}"), s(entry.status.as_str()))
                .expression_attribute_values(format!(":h{i}"), s(&entry.sha256_b64));
            removes.push(format!("m.#s{i}"));
            conditions.push(format!("m.#s{i}.st = :st{i} AND m.#s{i}.h = :h{i}"));
        }
        block_on(
            request
                .expression_attribute_values(":no", AttributeValue::Bool(false))
                .update_expression(format!("REMOVE {} SET #full = :no", removes.join(", ")))
                .condition_expression(conditions.join(" AND "))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }

    fn delete_slots(&self, engine: u32, division: Division, slot_key: &str) -> StoreResult<()> {
        let pk = slot_pk(engine, division, slot_key);
        self.full_slots.lock().unwrap().remove(&pk);
        block_on(
            self.client
                .delete_item()
                .table_name(&self.table)
                .set_key(Some(key(&pk, "S")))
                .send(),
        )
        .map(|_| ())
        .map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_round_trip() {
        let user = User {
            uid: "g1".into(),
            login: "ann".into(),
            github_id: 1,
            github_created: Some(5),
            bots: vec!["a.B".into()],
            battles: 9,
            reputation: Reputation {
                ver: 3,
                rep: 61,
                state: UserState::Quarantined,
                mismatch_times: vec![1, 2],
                ..Reputation::default()
            },
            ..User::default()
        };
        assert_eq!(user_from(&user_item(&user)).unwrap(), user);
        let bot = BotItem {
            id: "a.B".into(),
            author: "Ann".into(),
            owner: Some("g1".into()),
            homepage: None,
            description: Some("d".into()),
            divisions: vec![Division::OneVsOne, Division::Melee],
            versions: vec![3, 1],
            current: Some(1),
            state: BotState::Delisted,
            created: 7,
            source: BotSource::Import,
        };
        assert_eq!(bot_from(&bot_item(&bot)).unwrap(), bot);
        let version = VersionItem {
            vid: 1000,
            bot: "a.B".into(),
            version: "1.0".into(),
            sha256: "ab".repeat(32),
            size: 9,
            codesize: Some(200),
            class: WeightClass::Nano,
            divisions: vec![Division::Melee],
            state: VersionState::Probation,
            probation_battles: 2,
            probation_failures: 1,
            class_name: "a.B".into(),
            created: 1,
            retired: None,
            url: Some("https://x".into()),
            colors: None,
            released: Some(1_792_843_200_123),
            nondeterministic: Some("threads".into()),
            disagreements: 2,
        };
        let item = version_item(&version);
        assert_eq!(item["sk"], s("V#rs"));
        assert!(!item.contains_key("col"));
        assert_eq!(item["rd"], n(1_792_843_200_123u64));
        assert_eq!(version_from(&item).unwrap(), version);
        let delisted = VersionItem {
            state: VersionState::Delisted,
            released: None,
            nondeterministic: None,
            disagreements: 0,
            ..version.clone()
        };
        let item = version_item(&delisted);
        assert_eq!(
            (item["st"].clone(), item.contains_key("rd")),
            (s("delisted"), false)
        );
        assert_eq!(version_from(&item).unwrap(), delisted);
        let colored = VersionItem {
            colors: Some(BotColors {
                body: Some("#000000".into()),
                gun: None,
                radar: Some("#ffffff".into()),
            }),
            ..version
        };
        let item = version_item(&colored);
        assert_eq!(item["col"], s(r##"{"body":"#000000","radar":"#ffffff"}"##));
        assert_eq!(version_from(&item).unwrap(), colored);
        let marker = BatchMarker {
            batch_id: "b-1".into(),
            uid: "g1".into(),
            accepted: vec![0, 2, 70_000],
            rejected: vec![Rejected {
                index: 1,
                error: "invalid".into(),
                message: "m".into(),
            }],
            slots: vec!["1v1.1.2.a.0".into()],
            ttl: 9,
        };
        assert_eq!(marker_from(&marker_item(&marker)).unwrap(), marker);
        let log = LogItem {
            engine: 1,
            hour: 2_026_102_412,
            sort: "0001#b-1".into(),
            uid: "g1".into(),
            records: vec![1, 2, 3],
            seeds: vec![-1, i64::MAX],
            battles: 2,
            held: true,
            ttl: 4,
        };
        assert_eq!(log_from(&log_item(&log)).unwrap(), log);
        let verify = VerifyItem {
            engine: 1,
            sort: "0001#b-1.0".into(),
            division: Division::OneVsOne,
            seed: i64::MIN,
            bots: vec![2, 1],
            rounds: 35,
            runs: vec![Run {
                uid: "g1".into(),
                hash: None,
                record: vec![9],
                held: false,
                colors: Vec::new(),
            }],
            uids: vec!["g1".into()],
            stage: 2,
            claimant: Some("g2".into()),
            claim_until: Some(5),
            kind: VerifyKind::Recheck,
            ttl: 6,
        };
        assert_eq!(verify_from(&verify_item(&verify)).unwrap(), verify);
        let mut colored = verify.clone();
        colored.runs[0].colors = vec![
            None,
            Some(BotColors {
                gun: Some("#ff0000".into()),
                ..Default::default()
            }),
        ];
        assert_eq!(verify_from(&verify_item(&colored)).unwrap(), colored);
        let entry = SlotEntry {
            status: SlotStatus::Claimed,
            batch: "b-1".into(),
            index: 3,
            round: 7,
            sha256_b64: "abc=".into(),
            size: 10,
            claim_until: 11,
            seed: -5,
            order: vec![3, 4, 5],
        };
        assert_eq!(slot_from(&slot_value(&entry)).unwrap(), entry);
        // Entries written before M6 have no `v`.
        let mut old = slot_value(&entry);
        if let AttributeValue::M(map) = &mut old {
            map.remove("v");
        }
        assert!(slot_from(&old).unwrap().order.is_empty());
    }
}
