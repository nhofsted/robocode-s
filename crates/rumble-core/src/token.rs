// SPDX-License-Identifier: EPL-2.0

//! Signed tokens (spec §5.4, §7.1): `base64url(payload JSON) "." base64url(HMAC-SHA256)`,
//! no padding. The payload is a JSON object carrying `k`, the id of the signing key.
//! Batch tokens and the OAuth `state` use the same format.

use crate::api::BattleKind;
use crate::ids::Division;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

const KEY_BYTES: usize = 32;

/// Why a token was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    /// Not `payload.signature`, not base64url, or the payload isn't the expected JSON.
    BadFormat(String),
    BadSignature,
    /// The `k` of the payload names a key that isn't in the key set.
    UnknownKey(String),
    /// `exp` has passed.
    Expired,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::BadFormat(why) => write!(f, "malformed token: {why}"),
            TokenError::BadSignature => f.write_str("bad token signature"),
            TokenError::UnknownKey(kid) => write!(f, "unknown signing key {kid:?}"),
            TokenError::Expired => f.write_str("token expired"),
        }
    }
}

impl std::error::Error for TokenError {}

struct Key {
    kid: String,
    bytes: [u8; KEY_BYTES],
}

/// The signing keys: the SSM parameter's `kid:64hex` lines. The newest (last) line signs,
/// all lines verify; keys rotate by appending a line.
pub struct Keys {
    keys: Vec<Key>,
}

// No key material in logs.
impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kids: Vec<&str> = self.keys.iter().map(|key| key.kid.as_str()).collect();
        f.debug_struct("Keys").field("kids", &kids).finish()
    }
}

fn hex_decode(text: &str) -> Option<[u8; KEY_BYTES]> {
    if text.len() != 2 * KEY_BYTES {
        return None;
    }
    let mut bytes = [0; KEY_BYTES];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        *byte = (high * 16 + low) as u8;
    }
    Some(bytes)
}

impl Keys {
    /// Parses `kid:hexkey` lines (blank lines are skipped). A key id is 1 to 32 characters
    /// of `[A-Za-z0-9_-]`, the key 64 hex digits (32 bytes), key ids are unique.
    pub fn parse(text: &str) -> Result<Keys, String> {
        let mut keys: Vec<Key> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let number = number + 1;
            let (kid, hex) = line
                .split_once(':')
                .ok_or_else(|| format!("signing keys line {number}: expected kid:hexkey"))?;
            if kid.is_empty()
                || kid.len() > 32
                || !kid
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err(format!("signing keys line {number}: bad key id"));
            }
            let bytes = hex_decode(hex).ok_or_else(|| {
                format!("signing keys line {number}: the key must be 64 hex digits")
            })?;
            if keys.iter().any(|key| key.kid == kid) {
                return Err(format!(
                    "signing keys line {number}: duplicate key id {kid:?}"
                ));
            }
            keys.push(Key {
                kid: kid.to_string(),
                bytes,
            });
        }
        if keys.is_empty() {
            return Err("no signing keys".to_string());
        }
        Ok(Keys { keys })
    }

    /// The id of the key that signs.
    pub fn signing_kid(&self) -> &str {
        &self.signing().kid
    }

    fn signing(&self) -> &Key {
        self.keys.last().expect("Keys is never empty")
    }

    fn find(&self, kid: &str) -> Option<&Key> {
        self.keys.iter().find(|key| key.kid == kid)
    }
}

fn mac(key: &Key, message: &[u8]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(&key.bytes).expect("HMAC takes any key length");
    mac.update(message);
    mac
}

/// Signs any serializable object, adding `k`. For callers with their own payloads.
pub fn sign_state<T: Serialize>(state: &T, keys: &Keys) -> Result<String, String> {
    let mut value = serde_json::to_value(state).map_err(|e| e.to_string())?;
    let object = value
        .as_object_mut()
        .ok_or("a token payload must be a JSON object")?;
    let key = keys.signing();
    object.insert("k".to_string(), Value::String(key.kid.clone()));
    let payload = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    let signature = mac(key, &payload).finalize().into_bytes();
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// Checks the signature (with the key the payload names) and, if the payload has an `exp`
/// (unix seconds), that `now` is before it. Returns the payload.
pub fn verify_state<T: DeserializeOwned>(
    token: &str,
    keys: &Keys,
    now: u64,
) -> Result<T, TokenError> {
    let bad = |why: &str| TokenError::BadFormat(why.to_string());
    let (payload_text, signature_text) = token.split_once('.').ok_or_else(|| bad("no dot"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_text)
        .map_err(|_| bad("payload is not base64url"))?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_text)
        .map_err(|_| bad("signature is not base64url"))?;
    let value: Value = serde_json::from_slice(&payload).map_err(|_| bad("payload is not JSON"))?;
    let kid = value
        .get("k")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("no key id"))?;
    let key = keys
        .find(kid)
        .ok_or_else(|| TokenError::UnknownKey(kid.to_string()))?;
    mac(key, &payload)
        .verify_slice(&signature)
        .map_err(|_| TokenError::BadSignature)?;
    if let Some(exp) = value.get("exp") {
        let exp = exp.as_u64().ok_or_else(|| bad("exp is not a number"))?;
        if now >= exp {
            return Err(TokenError::Expired);
        }
    }
    serde_json::from_value(value).map_err(|e| TokenError::BadFormat(e.to_string()))
}

/// One battle of a batch: `[division, kind, seed, rounds, [vid...], ref?]`. `ref` is the
/// verification item's sort key for `verify` and `probation` battles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Value>", into = "Vec<Value>")]
pub struct BatchBattle {
    pub division: Division,
    pub kind: BattleKind,
    pub seed: i64,
    pub rounds: u32,
    pub vids: Vec<u32>,
    pub item: Option<String>,
}

impl From<BatchBattle> for Vec<Value> {
    fn from(battle: BatchBattle) -> Vec<Value> {
        let mut array = vec![
            serde_json::to_value(battle.division).unwrap(),
            serde_json::to_value(battle.kind).unwrap(),
            Value::from(battle.seed),
            Value::from(battle.rounds),
            Value::from(battle.vids),
        ];
        if let Some(item) = battle.item {
            array.push(Value::String(item));
        }
        array
    }
}

impl TryFrom<Vec<Value>> for BatchBattle {
    type Error = String;

    fn try_from(array: Vec<Value>) -> Result<BatchBattle, String> {
        if !(5..=6).contains(&array.len()) {
            return Err(format!(
                "a batch battle has 5 or 6 elements, not {}",
                array.len()
            ));
        }
        let mut elements = array.into_iter();
        let mut next = || elements.next().unwrap();
        let division = serde_json::from_value(next()).map_err(|e| e.to_string())?;
        let kind = serde_json::from_value(next()).map_err(|e| e.to_string())?;
        let seed = serde_json::from_value(next()).map_err(|e| e.to_string())?;
        let rounds = serde_json::from_value(next()).map_err(|e| e.to_string())?;
        let vids = serde_json::from_value(next()).map_err(|e| e.to_string())?;
        let item = match elements.next() {
            None | Some(Value::Null) => None,
            Some(value) => Some(serde_json::from_value(value).map_err(|e| e.to_string())?),
        };
        Ok(BatchBattle {
            division,
            kind,
            seed,
            rounds,
            vids,
            item,
        })
    }
}

/// The payload of a batch token: the assignment itself (§5.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchPayload {
    /// Payload version, 1.
    pub v: u32,
    /// The batch id.
    pub id: String,
    pub uid: String,
    /// The engine version.
    pub e: u32,
    /// Issued at, unix seconds.
    pub iat: u64,
    /// Expires at (the batch deadline), unix seconds.
    pub exp: u64,
    /// The battles, in index order.
    pub b: Vec<BatchBattle>,
    /// The signing key's id: set by [`sign`], ignored on the way in.
    #[serde(default)]
    pub k: String,
}

/// Signs a batch token with the newest key.
pub fn sign(payload: &BatchPayload, keys: &Keys) -> Result<String, String> {
    sign_state(payload, keys)
}

/// Verifies a batch token: format, key, signature, then expiry.
pub fn verify(token: &str, keys: &Keys, now: u64) -> Result<BatchPayload, TokenError> {
    verify_state(token, keys, now)
}

/// Verifies a batch token's format, key and signature but not its expiry: for
/// `recordings/done`, which may come after the deadline (the slot claims have their own
/// expiry, §12.5).
pub fn verify_signature(token: &str, keys: &Keys) -> Result<BatchPayload, TokenError> {
    verify_state(token, keys, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY1: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const KEY2: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn keys(text: &str) -> Keys {
        Keys::parse(text).unwrap()
    }

    fn payload() -> BatchPayload {
        BatchPayload {
            v: 1,
            id: "b-01JB2".to_string(),
            uid: "g583231".to_string(),
            e: 1,
            iat: 1_000,
            exp: 2_000,
            b: vec![
                BatchBattle {
                    division: Division::OneVsOne,
                    kind: BattleKind::Normal,
                    seed: i64::MIN,
                    rounds: 35,
                    vids: vec![812, 77],
                    item: None,
                },
                BatchBattle {
                    division: Division::Melee,
                    kind: BattleKind::Verify,
                    seed: 42,
                    rounds: 35,
                    vids: (1..=10).collect(),
                    item: Some("VER#0001".to_string()),
                },
            ],
            k: String::new(),
        }
    }

    fn parts(token: &str) -> (Vec<u8>, Vec<u8>) {
        let (p, s) = token.split_once('.').unwrap();
        (
            URL_SAFE_NO_PAD.decode(p).unwrap(),
            URL_SAFE_NO_PAD.decode(s).unwrap(),
        )
    }

    fn join(payload: &[u8], signature: &[u8]) -> String {
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        )
    }

    #[test]
    fn sign_and_verify() {
        let keys = keys(&format!("a:{KEY1}\n"));
        let token = sign(&payload(), &keys).unwrap();
        assert!(!token.contains('=') && !token.contains('+') && !token.contains('/'));
        assert_eq!(token.matches('.').count(), 1);
        let (payload_bytes, signature) = parts(&token);
        assert_eq!(signature.len(), 32);
        let value: Value = serde_json::from_slice(&payload_bytes).unwrap();
        assert_eq!(value["k"], "a");
        assert_eq!(
            value["b"][0],
            json!(["1v1", "normal", i64::MIN, 35, [812, 77]])
        );
        assert_eq!(value["b"][1][5], "VER#0001");
        let verified = verify(&token, &keys, 1_999).unwrap();
        assert_eq!(verified.k, "a");
        assert_eq!(
            BatchPayload {
                k: String::new(),
                ..verified
            },
            payload()
        );
    }

    #[test]
    fn signatures_depend_on_the_payload() {
        let keys = keys(&format!("a:{KEY1}"));
        let one = sign(&payload(), &keys).unwrap();
        let mut other = payload();
        other.id = "b-other".to_string();
        let two = sign(&other, &keys).unwrap();
        assert_ne!(parts(&one).1, parts(&two).1);
        assert_eq!(
            sign(&payload(), &keys).unwrap(),
            one,
            "signing is deterministic"
        );
    }

    #[test]
    fn tampered_payload() {
        let keys = keys(&format!("a:{KEY1}"));
        let token = sign(&payload(), &keys).unwrap();
        let (payload_bytes, signature) = parts(&token);
        let mut value: Value = serde_json::from_slice(&payload_bytes).unwrap();
        value["uid"] = json!("g1");
        let forged = join(&serde_json::to_vec(&value).unwrap(), &signature);
        assert_eq!(verify(&forged, &keys, 1_500), Err(TokenError::BadSignature));
    }

    #[test]
    fn tampered_signature() {
        let keys = keys(&format!("a:{KEY1}"));
        let token = sign(&payload(), &keys).unwrap();
        let (payload_bytes, mut signature) = parts(&token);
        signature[0] ^= 1;
        assert_eq!(
            verify(&join(&payload_bytes, &signature), &keys, 1_500),
            Err(TokenError::BadSignature)
        );
        assert_eq!(
            verify(&join(&payload_bytes, &signature[..31]), &keys, 1_500),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn expiry() {
        let keys = keys(&format!("a:{KEY1}"));
        let token = sign(&payload(), &keys).unwrap();
        assert!(verify(&token, &keys, 1_999).is_ok());
        assert_eq!(verify(&token, &keys, 2_000), Err(TokenError::Expired));
        assert_eq!(verify(&token, &keys, 9_999), Err(TokenError::Expired));
        // The signature alone still checks out after the deadline.
        assert_eq!(verify_signature(&token, &keys).unwrap().id, "b-01JB2");
        let mut forged = token.clone();
        forged.push('A');
        assert!(verify_signature(&forged, &keys).is_err());
    }

    #[test]
    fn key_rotation() {
        let old = keys(&format!("a:{KEY1}"));
        let rotated = keys(&format!("a:{KEY1}\nb:{KEY2}\n"));
        assert_eq!(rotated.signing_kid(), "b");
        let old_token = sign(&payload(), &old).unwrap();
        let new_token = sign(&payload(), &rotated).unwrap();
        // The old key still verifies, the new key signs.
        assert_eq!(verify(&old_token, &rotated, 1_500).unwrap().k, "a");
        assert_eq!(verify(&new_token, &rotated, 1_500).unwrap().k, "b");
        // A server that doesn't know the new key can't verify its tokens.
        assert_eq!(
            verify(&new_token, &old, 1_500),
            Err(TokenError::UnknownKey("b".to_string()))
        );
        // And a retired key no longer verifies.
        let retired = keys(&format!("b:{KEY2}"));
        assert_eq!(
            verify(&old_token, &retired, 1_500),
            Err(TokenError::UnknownKey("a".to_string()))
        );
    }

    #[test]
    fn a_kid_cannot_be_swapped_for_another_key() {
        let rotated = keys(&format!("a:{KEY1}\nb:{KEY2}"));
        let token = sign(&payload(), &rotated).unwrap();
        let (payload_bytes, signature) = parts(&token);
        let mut value: Value = serde_json::from_slice(&payload_bytes).unwrap();
        value["k"] = json!("a");
        let forged = join(&serde_json::to_vec(&value).unwrap(), &signature);
        assert_eq!(
            verify(&forged, &rotated, 1_500),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn bad_formats() {
        let keys = keys(&format!("a:{KEY1}"));
        for token in ["", "abc", ".", "a.b.c", "!!.!!", "e30.e30"] {
            assert!(
                matches!(verify(token, &keys, 0), Err(TokenError::BadFormat(_))),
                "{token:?}"
            );
        }
        // Padding is not accepted.
        let token = sign(&payload(), &keys).unwrap();
        let (p, s) = token.split_once('.').unwrap();
        assert!(matches!(
            verify(&format!("{p}==.{s}"), &keys, 0),
            Err(TokenError::BadFormat(_))
        ));
    }

    #[test]
    fn state_tokens() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct State {
            n: String,
            r: String,
            exp: u64,
        }
        let keys = keys(&format!("a:{KEY1}"));
        let state = State {
            n: "nonce".to_string(),
            r: "/account".to_string(),
            exp: 600,
        };
        let token = sign_state(&state, &keys).unwrap();
        assert_eq!(verify_state::<State>(&token, &keys, 10), Ok(state));
        assert_eq!(
            verify_state::<State>(&token, &keys, 600),
            Err(TokenError::Expired)
        );
        // Without exp there is no expiry check.
        let open = sign_state(&json!({"n": "x"}), &keys).unwrap();
        assert!(verify_state::<Value>(&open, &keys, u64::MAX).is_ok());
        assert!(sign_state(&json!([1]), &keys).is_err());
    }

    #[test]
    fn key_parsing() {
        assert!(Keys::parse("").is_err());
        assert!(Keys::parse("\n\n").is_err());
        assert!(Keys::parse("a").is_err());
        assert!(Keys::parse("a:abc").is_err());
        assert!(Keys::parse(&format!("a:{}", KEY1.replace('0', "g"))).is_err());
        assert!(Keys::parse(&format!(":{KEY1}")).is_err());
        assert!(Keys::parse(&format!("a b:{KEY1}")).is_err());
        assert!(Keys::parse(&format!("a:{KEY1}\na:{KEY2}")).is_err());
        let parsed =
            Keys::parse(&format!("k1:{KEY1}\r\n\n  k-2:{}  \n", KEY2.to_uppercase())).unwrap();
        assert_eq!(parsed.signing_kid(), "k-2");
        assert!(!format!("{parsed:?}").contains(KEY1));
    }
}
