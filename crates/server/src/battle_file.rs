// SPDX-License-Identifier: EPL-2.0

//! Classic's `.battle` files: Java properties with the battle's rules and robots, as
//! classic's "Save battle" writes them (`robocode.battle.*`, `robocode.battleField.*`).
//! Keys classic has that robocode-s doesn't use (initial positions, ...) are ignored.

use robocode_s_engine::BattleRules;

/// The battle in a `.battle` file.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleFile {
    pub rules: BattleRules,
    /// The robots and teams, by name with version, in battle order.
    pub robots: Vec<String>,
}

const WIDTH: &str = "robocode.battleField.width";
const HEIGHT: &str = "robocode.battleField.height";
const ROUNDS: &str = "robocode.battle.numRounds";
const GUN_COOLING: &str = "robocode.battle.gunCoolingRate";
const INACTIVITY: &str = "robocode.battle.rules.inactivityTime";
const HIDE_NAMES: &str = "robocode.battle.hideEnemyNames";
const SENTRY_BORDER: &str = "robocode.battle.sentryBorderSize";
const ROBOTS: &str = "robocode.battle.selectedRobots";

/// Reads a `.battle` file's text; rules it doesn't set keep their value in `rules`.
pub fn parse(text: &str, mut rules: BattleRules) -> Result<BattleFile, String> {
    let mut robots = Vec::new();
    for (key, value) in properties(text) {
        let value = value.trim();
        fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
            value
                .parse()
                .map_err(|_| format!("{key}: not a valid number: {value}"))
        }
        match key.as_str() {
            WIDTH => rules.width = number(&key, value)?,
            HEIGHT => rules.height = number(&key, value)?,
            ROUNDS => rules.num_rounds = number(&key, value)?,
            GUN_COOLING => rules.gun_cooling_rate = number(&key, value)?,
            INACTIVITY => rules.inactivity_time = number(&key, value)?,
            HIDE_NAMES => rules.hide_enemy_names = value.eq_ignore_ascii_case("true"),
            SENTRY_BORDER => rules.sentry_border_size = number(&key, value)?,
            ROBOTS => {
                robots = value
                    .split(',')
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(String::from)
                    .collect();
            }
            _ => {}
        }
    }
    // As classic's `BattlefieldSpecification`, which refuses other sizes (smaller ones
    // leave no room to place the robots).
    for (key, size) in [(WIDTH, rules.width), (HEIGHT, rules.height)] {
        if !(400..=5000).contains(&size) {
            return Err(format!("{key}: {size} is not from 400 to 5000"));
        }
    }
    Ok(BattleFile { rules, robots })
}

/// The `.battle` file for `battle`, as classic writes it.
pub fn write(battle: &BattleFile) -> String {
    let r = &battle.rules;
    let robots = battle.robots.join(",");
    let lines = [
        "#Battle Properties".to_string(),
        format!("{WIDTH}={}", r.width),
        format!("{HEIGHT}={}", r.height),
        format!("{ROUNDS}={}", r.num_rounds),
        format!("{GUN_COOLING}={}", r.gun_cooling_rate),
        format!("{INACTIVITY}={}", r.inactivity_time),
        format!("{HIDE_NAMES}={}", r.hide_enemy_names),
        format!("{SENTRY_BORDER}={}", r.sentry_border_size),
        format!("{ROBOTS}={}", escape(&robots)),
    ];
    lines.join("\n") + "\n"
}

/// The key-value pairs of a Java properties text: `#` and `!` comments, `=`, `:` or
/// white space between key and value, backslash escapes and continued lines.
fn properties(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let mut logical = line.trim_start().to_string();
        if logical.is_empty() || logical.starts_with('#') || logical.starts_with('!') {
            continue;
        }
        // A line ending in an odd number of backslashes continues on the next one.
        while logical.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1 {
            logical.pop();
            match lines.next() {
                Some(next) => logical.push_str(next.trim_start()),
                None => break,
            }
        }
        let (mut key, mut value, mut in_key, mut escaped) =
            (String::new(), String::new(), true, false);
        let mut chars = logical.chars().peekable();
        while let Some(c) = chars.next() {
            if escaped {
                let c = match c {
                    't' => '\t',
                    'n' => '\n',
                    'r' => '\r',
                    'f' => '\u{c}',
                    other => other,
                };
                if in_key { &mut key } else { &mut value }.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if in_key && (c == '=' || c == ':' || c.is_whitespace()) {
                in_key = false;
                // White space around the separator, and one `=` or `:` after white space.
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if c.is_whitespace() && chars.peek().is_some_and(|&c| c == '=' || c == ':') {
                    chars.next();
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
            } else if in_key {
                key.push(c);
            } else {
                value.push(c);
            }
        }
        pairs.push((key, value));
    }
    pairs
}

/// A property value with the characters Java escapes escaped.
fn escape(value: &str) -> String {
    let mut out = String::new();
    for (i, c) in value.chars().enumerate() {
        match c {
            '\\' | '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            ' ' if i == 0 => out.push_str("\\ "),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_classic_battle_file() {
        let text = "#Battle Properties\n\
                    robocode.battleField.width=1000\n\
                    robocode.battleField.height=1000\n\
                    robocode.battle.numRounds=35\n\
                    robocode.battle.gunCoolingRate=0.2\n\
                    robocode.battle.rules.inactivityTime=900\n\
                    robocode.battle.hideEnemyNames=true\n\
                    robocode.battle.sentryBorderSize=150\n\
                    robocode.battle.initialPositions=(50,50,0),(?,?,?)\n\
                    robocode.battle.selectedRobots=sample.Corners,jk.mega.DrussGT 3.1.16,\\\n    \
                    sample.Fire\n";
        let b = parse(text, BattleRules::default()).unwrap();
        assert_eq!(
            (b.rules.width, b.rules.height, b.rules.num_rounds),
            (1000, 1000, 35)
        );
        assert_eq!(b.rules.gun_cooling_rate, 0.2);
        assert_eq!(b.rules.inactivity_time, 900);
        assert!(b.rules.hide_enemy_names);
        assert_eq!(b.rules.sentry_border_size, 150);
        assert_eq!(
            b.robots,
            ["sample.Corners", "jk.mega.DrussGT 3.1.16", "sample.Fire"]
        );
    }

    #[test]
    fn missing_rules_keep_their_value() {
        let b = parse("robocode.battle.numRounds = 3", BattleRules::default()).unwrap();
        assert_eq!(b.rules.num_rounds, 3);
        assert_eq!(b.rules.width, 800);
        assert!(b.robots.is_empty());
        assert!(parse("robocode.battle.numRounds=x", BattleRules::default()).is_err());
        let small = parse("robocode.battleField.width=100", BattleRules::default());
        assert!(small.unwrap_err().contains("400 to 5000"));
    }

    #[test]
    fn round_trips() {
        let battle = BattleFile {
            rules: BattleRules {
                width: 1200,
                height: 900,
                num_rounds: 7,
                gun_cooling_rate: 0.15,
                inactivity_time: 600,
                hide_enemy_names: true,
                sentry_border_size: 120,
            },
            robots: vec!["sample.Fire".into(), "a.B 1.0".into(), "sample.Fire".into()],
        };
        let text = write(&battle);
        assert!(text.contains("robocode.battle.selectedRobots=sample.Fire,a.B 1.0,sample.Fire"));
        assert_eq!(parse(&text, BattleRules::default()).unwrap(), battle);
    }
}
