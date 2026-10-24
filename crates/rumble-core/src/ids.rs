// SPDX-License-Identifier: EPL-2.0

//! Identifiers (spec §4): bot ids and versions, vids, user ids, divisions, rumble ids and
//! weight classes.

use crate::config::WeightLimits;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Longest bot id.
pub const MAX_BOT_ID: usize = 100;
/// Longest robot version.
pub const MAX_VERSION: usize = 40;

/// Checks a bot id: a fully qualified class name without version, `[A-Za-z0-9_$.]`, 1..=100.
pub fn validate_bot_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_BOT_ID {
        return Err(format!("bot id must be 1 to {MAX_BOT_ID} characters"));
    }
    match id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.')))
    {
        Some(c) => Err(format!(
            "bot id contains {c:?}; allowed are letters, digits, _ $ and ."
        )),
        None => Ok(()),
    }
}

/// Checks a robot version: 1..=40 printable ASCII characters, no comma.
pub fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty() || version.len() > MAX_VERSION {
        return Err(format!("version must be 1 to {MAX_VERSION} characters"));
    }
    match version
        .chars()
        .find(|c| !(' '..='~').contains(c) || *c == ',')
    {
        Some(c) => Err(format!(
            "version contains {c:?}; printable ASCII without a comma only"
        )),
        None => Ok(()),
    }
}

/// A vid in base 36 (lowercase, no padding), as used in DynamoDB keys.
pub fn vid36(vid: u32) -> String {
    if vid == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    let mut rest = vid;
    while rest > 0 {
        digits.push(char::from_digit(rest % 36, 36).unwrap());
        rest /= 36;
    }
    digits.iter().rev().collect()
}

/// Parses [`vid36`]'s output (upper case is accepted too).
pub fn parse_vid36(text: &str) -> Result<u32, String> {
    if text.is_empty() {
        return Err("empty vid".to_string());
    }
    u32::from_str_radix(text, 36).map_err(|e| format!("bad vid {text:?}: {e}"))
}

/// The user id of a GitHub account: `g<numeric id>`.
pub fn uid(github_id: u64) -> String {
    format!("g{github_id}")
}

/// The GitHub numeric id of a user id.
pub fn parse_uid(uid: &str) -> Result<u64, String> {
    uid.strip_prefix('g')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(|| format!("bad user id {uid:?}"))
}

/// A battle division.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Division {
    OneVsOne,
    Melee,
}

impl Division {
    /// Every division built in M4.
    pub const ALL: [Division; 2] = [Division::OneVsOne, Division::Melee];

    pub fn as_str(self) -> &'static str {
        match self {
            Division::OneVsOne => "1v1",
            Division::Melee => "melee",
        }
    }
}

impl fmt::Display for Division {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Division {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "1v1" => Ok(Division::OneVsOne),
            "melee" => Ok(Division::Melee),
            _ => Err(format!("unknown division {text:?}")),
        }
    }
}

impl Serialize for Division {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Division {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A weight class. `Mega` has no limit and no suffix: it is the division's main rumble.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WeightClass {
    Nano,
    Micro,
    Mini,
    Mega,
}

impl WeightClass {
    pub const ALL: [WeightClass; 4] = [
        WeightClass::Nano,
        WeightClass::Micro,
        WeightClass::Mini,
        WeightClass::Mega,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            WeightClass::Nano => "nano",
            WeightClass::Micro => "micro",
            WeightClass::Mini => "mini",
            WeightClass::Mega => "mega",
        }
    }

    /// The exclusive codesize limit; `None` for mega.
    pub fn limit(self, limits: &WeightLimits) -> Option<u32> {
        match self {
            WeightClass::Nano => Some(limits.nano),
            WeightClass::Micro => Some(limits.micro),
            WeightClass::Mini => Some(limits.mini),
            WeightClass::Mega => None,
        }
    }

    /// Whether a bot of this codesize belongs to the class (strictly below the limit).
    /// A nano bot is also a micro, mini and mega one.
    pub fn contains(self, codesize: u32, limits: &WeightLimits) -> bool {
        self.limit(limits).is_none_or(|limit| codesize < limit)
    }

    /// The lightest class the codesize fits.
    pub fn of(codesize: u32, limits: &WeightLimits) -> WeightClass {
        WeightClass::ALL
            .into_iter()
            .find(|class| class.contains(codesize, limits))
            .unwrap_or(WeightClass::Mega)
    }
}

impl fmt::Display for WeightClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WeightClass {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        WeightClass::ALL
            .into_iter()
            .find(|class| class.as_str() == text)
            .ok_or_else(|| format!("unknown weight class {text:?}"))
    }
}

impl Serialize for WeightClass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WeightClass {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A rumble: a division in a weight class. Written `1v1`, `1v1-mini`, `melee-nano`, ...
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RumbleId {
    pub division: Division,
    pub class: WeightClass,
}

impl RumbleId {
    pub fn new(division: Division, class: WeightClass) -> RumbleId {
        RumbleId { division, class }
    }

    /// The division's rumbles, mega first.
    pub fn of_division(division: Division) -> [RumbleId; 4] {
        [
            WeightClass::Mega,
            WeightClass::Mini,
            WeightClass::Micro,
            WeightClass::Nano,
        ]
        .map(|class| RumbleId { division, class })
    }

    /// The rumbles of a division that a bot of this codesize is a member of.
    pub fn memberships(division: Division, codesize: u32, limits: &WeightLimits) -> Vec<RumbleId> {
        RumbleId::of_division(division)
            .into_iter()
            .filter(|rumble| rumble.class.contains(codesize, limits))
            .collect()
    }
}

impl fmt::Display for RumbleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.class {
            WeightClass::Mega => write!(f, "{}", self.division),
            class => write!(f, "{}-{}", self.division, class),
        }
    }
}

impl FromStr for RumbleId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (division, class) = match text.split_once('-') {
            Some((division, class)) => (division, class),
            None => (text, "mega"),
        };
        let class: WeightClass = class.parse()?;
        if text.contains('-') && class == WeightClass::Mega {
            return Err(format!("unknown rumble {text:?}"));
        }
        Ok(RumbleId {
            division: division.parse()?,
            class,
        })
    }
}

impl Serialize for RumbleId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RumbleId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_ids() {
        assert!(validate_bot_id("jk.mega.DrussGT").is_ok());
        assert!(validate_bot_id("a.B$1_").is_ok());
        assert!(validate_bot_id("").is_err());
        assert!(validate_bot_id("a b").is_err());
        assert!(validate_bot_id("a/b").is_err());
        assert!(validate_bot_id(&"a".repeat(100)).is_ok());
        assert!(validate_bot_id(&"a".repeat(101)).is_err());
    }

    #[test]
    fn versions() {
        assert!(validate_version("3.1.16").is_ok());
        assert!(validate_version("1.0 beta").is_ok());
        assert!(validate_version("").is_err());
        assert!(validate_version("1,0").is_err());
        assert!(validate_version("1.0\n").is_err());
        assert!(validate_version("1.0é").is_err());
        assert!(validate_version(&"1".repeat(40)).is_ok());
        assert!(validate_version(&"1".repeat(41)).is_err());
    }

    #[test]
    fn vids() {
        assert_eq!(vid36(0), "0");
        assert_eq!(vid36(35), "z");
        assert_eq!(vid36(36), "10");
        assert_eq!(vid36(812), "mk");
        for vid in [0, 1, 77, 812, 913, u32::MAX] {
            assert_eq!(parse_vid36(&vid36(vid)), Ok(vid));
        }
        assert!(parse_vid36("").is_err());
        assert!(parse_vid36("-1").is_err());
        assert!(parse_vid36("zzzzzzzz").is_err());
    }

    #[test]
    fn uids() {
        assert_eq!(uid(583231), "g583231");
        assert_eq!(parse_uid("g583231"), Ok(583231));
        assert!(parse_uid("583231").is_err());
        assert!(parse_uid("g").is_err());
        assert!(parse_uid("g+1").is_err());
    }

    #[test]
    fn divisions_and_rumbles() {
        assert_eq!(
            serde_json::to_string(&Division::OneVsOne).unwrap(),
            "\"1v1\""
        );
        assert_eq!(
            serde_json::from_str::<Division>("\"melee\"").unwrap(),
            Division::Melee
        );
        assert!(serde_json::from_str::<Division>("\"teams\"").is_err());
        for division in Division::ALL {
            for rumble in RumbleId::of_division(division) {
                assert_eq!(rumble.to_string().parse(), Ok(rumble));
                let json = serde_json::to_string(&rumble).unwrap();
                assert_eq!(serde_json::from_str::<RumbleId>(&json).unwrap(), rumble);
            }
        }
        assert_eq!(
            RumbleId::new(Division::Melee, WeightClass::Nano).to_string(),
            "melee-nano"
        );
        assert_eq!(
            RumbleId::new(Division::OneVsOne, WeightClass::Mega).to_string(),
            "1v1"
        );
        assert!("1v1-mega".parse::<RumbleId>().is_err());
        assert!("1v1-big".parse::<RumbleId>().is_err());
    }

    #[test]
    fn weight_classes() {
        let limits = WeightLimits::default();
        assert_eq!(WeightClass::of(249, &limits), WeightClass::Nano);
        assert_eq!(WeightClass::of(250, &limits), WeightClass::Micro);
        assert_eq!(WeightClass::of(749, &limits), WeightClass::Micro);
        assert_eq!(WeightClass::of(750, &limits), WeightClass::Mini);
        assert_eq!(WeightClass::of(1499, &limits), WeightClass::Mini);
        assert_eq!(WeightClass::of(1500, &limits), WeightClass::Mega);
        let nano = RumbleId::memberships(Division::OneVsOne, 100, &limits);
        assert_eq!(nano.len(), 4);
        let mini = RumbleId::memberships(Division::Melee, 1000, &limits);
        assert_eq!(
            mini.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["melee", "melee-mini"]
        );
    }
}
