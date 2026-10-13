// SPDX-License-Identifier: EPL-2.0

//! The results table, as classic shows it, and as JSON.

pub use robocode_s_server::results::{Contestant, contestants};

/// Classic's ordinal: 1st, 2nd, 3rd, 4th, ..., 11th, 12th, 13th, 21st.
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Classic rounds results for display: `(int) (value + 0.5)`.
fn round(v: f64) -> i64 {
    (v + 0.5) as i64
}

/// The results table with classic's columns.
pub fn table(rows: &[Contestant]) -> String {
    let total: f64 = rows.iter().map(|c| c.score).sum();
    let header = [
        "Rank",
        "Robot Name",
        "Total Score",
        "Survival",
        "Surv Bonus",
        "Bullet Dmg",
        "Bullet Bonus",
        "Ram Dmg * 2",
        "Ram Bonus",
        "1sts",
        "2nds",
        "3rds",
    ];
    let mut lines: Vec<Vec<String>> = vec![header.iter().map(|h| h.to_string()).collect()];
    for c in rows {
        let percent = if total > 0.0 {
            round(100.0 * c.score / total)
        } else {
            0
        };
        lines.push(vec![
            ordinal(c.rank),
            c.name.clone(),
            format!("{} ({percent}%)", round(c.score)),
            round(c.survival).to_string(),
            round(c.last_survivor_bonus).to_string(),
            round(c.bullet_damage).to_string(),
            round(c.bullet_damage_bonus).to_string(),
            round(c.ram_damage).to_string(),
            round(c.ram_damage_bonus).to_string(),
            c.firsts.to_string(),
            c.seconds.to_string(),
            c.thirds.to_string(),
        ]);
    }
    let widths: Vec<usize> = (0..header.len())
        .map(|col| {
            lines
                .iter()
                .map(|l| l[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for line in &lines {
        let cells: Vec<String> = line
            .iter()
            .enumerate()
            .map(|(col, cell)| {
                // Names left-aligned, numbers right-aligned.
                if col == 1 {
                    format!("{cell:<w$}", w = widths[col])
                } else {
                    format!("{cell:>w$}", w = widths[col])
                }
            })
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// The results as JSON, one object per contestant in rank order.
pub fn json(rows: &[Contestant]) -> serde_json::Value {
    serde_json::Value::Array(
        rows.iter()
            .map(|c| {
                serde_json::json!({
                    "rank": c.rank,
                    "name": c.name,
                    "score": c.score,
                    "survival": c.survival,
                    "lastSurvivorBonus": c.last_survivor_bonus,
                    "bulletDamage": c.bullet_damage,
                    "bulletDamageBonus": c.bullet_damage_bonus,
                    "ramDamage": c.ram_damage,
                    "ramDamageBonus": c.ram_damage_bonus,
                    "firsts": c.firsts,
                    "seconds": c.seconds,
                    "thirds": c.thirds,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use robocode_s_protocol::v1;

    fn robot(name: &str, team: Option<(&str, i32)>) -> v1::RobotStatics {
        v1::RobotStatics {
            name: name.into(),
            team_name: team.map(|t| t.0.into()),
            team_index: team.map_or(-1, |t| t.1),
            ..Default::default()
        }
    }

    fn result(rank: i32, score: f64, firsts: i32) -> v1::Results {
        v1::Results {
            rank,
            score: Some(score),
            survival: Some(score / 2.0),
            firsts,
            ..Default::default()
        }
    }

    #[test]
    fn table_has_classic_columns_and_percentages() {
        let statics = [robot("sample.Fire", None), robot("sample.Walls", None)];
        let rows = contestants(&statics, &[result(2, 250.0, 1), result(1, 750.0, 9)]);
        let t = table(&rows);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("Rank  Robot Name"));
        assert!(lines[0].ends_with("1sts  2nds  3rds"));
        assert!(lines[1].starts_with(" 1st  sample.Walls"));
        assert!(lines[1].contains("750 (75%)"));
        assert!(lines[2].contains("250 (25%)"));
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }
}
