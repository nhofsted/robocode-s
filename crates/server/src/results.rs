// SPDX-License-Identifier: EPL-2.0

//! Battle results per contestant, as classic shows them: a team is one contestant with
//! the sums of its members' results.

use robocode_s_protocol::v1;

/// One row of the results table.
#[derive(Debug, Clone, PartialEq)]
pub struct Contestant {
    pub name: String,
    pub rank: usize,
    pub score: f64,
    pub survival: f64,
    pub last_survivor_bonus: f64,
    pub bullet_damage: f64,
    pub bullet_damage_bonus: f64,
    pub ram_damage: f64,
    pub ram_damage_bonus: f64,
    pub firsts: i32,
    pub seconds: i32,
    pub thirds: i32,
}

/// The results table's columns, as classic names them.
pub const COLUMNS: [&str; 12] = [
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

/// Classic's ordinal: 1st, 2nd, 3rd, 4th, ..., 11th, 12th, 13th, 21st.
pub fn ordinal(n: usize) -> String {
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
pub fn round(v: f64) -> i64 {
    (v + 0.5) as i64
}

/// The cells of the results table, one row per contestant under [`COLUMNS`], formatted as
/// classic shows them: the total score with its share of all scores, e.g. `750 (75%)`.
pub fn cells(rows: &[Contestant]) -> Vec<[String; 12]> {
    let total: f64 = rows.iter().map(|c| c.score).sum();
    rows.iter()
        .map(|c| {
            let percent = if total > 0.0 {
                round(100.0 * c.score / total)
            } else {
                0
            };
            [
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
            ]
        })
        .collect()
}

/// Groups per-robot `results` (by robot index) into contestants in battle order, then
/// ranks them by total score; equal scores keep the robots' own ranking.
pub fn contestants(statics: &[v1::RobotStatics], results: &[v1::Results]) -> Vec<Contestant> {
    let mut rows: Vec<(Contestant, i32)> = Vec::new();
    let mut team_rows: Vec<(i32, usize)> = Vec::new();
    for (s, r) in statics.iter().zip(results) {
        let row = match team_rows
            .iter()
            .find(|(t, _)| *t == s.team_index && *t >= 0)
        {
            Some(&(_, row)) => row,
            None => {
                let name = s.team_name.clone().unwrap_or_else(|| s.name.clone());
                if s.team_index >= 0 {
                    team_rows.push((s.team_index, rows.len()));
                }
                rows.push((
                    Contestant {
                        name,
                        rank: 0,
                        score: 0.0,
                        survival: 0.0,
                        last_survivor_bonus: 0.0,
                        bullet_damage: 0.0,
                        bullet_damage_bonus: 0.0,
                        ram_damage: 0.0,
                        ram_damage_bonus: 0.0,
                        firsts: 0,
                        seconds: 0,
                        thirds: 0,
                    },
                    i32::MAX,
                ));
                rows.len() - 1
            }
        };
        let (c, best_rank) = &mut rows[row];
        c.score += r.score.unwrap_or_default();
        c.survival += r.survival.unwrap_or_default();
        c.last_survivor_bonus += r.last_survivor_bonus.unwrap_or_default();
        c.bullet_damage += r.bullet_damage.unwrap_or_default();
        c.bullet_damage_bonus += r.bullet_damage_bonus.unwrap_or_default();
        c.ram_damage += r.ram_damage.unwrap_or_default();
        c.ram_damage_bonus += r.ram_damage_bonus.unwrap_or_default();
        c.firsts += r.firsts;
        c.seconds += r.seconds;
        c.thirds += r.thirds;
        *best_rank = (*best_rank).min(r.rank);
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        rows[b]
            .0
            .score
            .total_cmp(&rows[a].0.score)
            .then(rows[a].1.cmp(&rows[b].1))
    });
    for (rank, &i) in order.iter().enumerate() {
        rows[i].0.rank = rank + 1;
    }
    let mut ranked: Vec<Contestant> = rows.into_iter().map(|(c, _)| c).collect();
    ranked.sort_by_key(|c| c.rank);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_team_is_one_contestant_with_summed_results() {
        let team = Some(("sampleteam.MyFirstTeam (1)", 0));
        let statics = [
            robot("sampleteam.MyFirstLeader", team),
            robot("sample.Walls", None),
            robot("sampleteam.MyFirstDroid (1)", team),
            robot("sample.SpinBot", None),
        ];
        let results = [
            result(3, 600.0, 0),
            result(1, 3353.0, 5),
            result(4, 1070.0, 0),
            result(2, 927.0, 0),
        ];
        let rows = contestants(&statics, &results);
        let names: Vec<&str> = rows.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "sample.Walls",
                "sampleteam.MyFirstTeam (1)",
                "sample.SpinBot"
            ]
        );
        assert_eq!(rows[1].score, 1670.0);
        assert_eq!(rows[1].survival, 835.0);
        assert_eq!(rows.iter().map(|c| c.rank).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn equal_scores_keep_the_engine_ranking() {
        let statics = [robot("a", None), robot("b", None)];
        let rows = contestants(&statics, &[result(2, 0.0, 0), result(1, 0.0, 0)]);
        assert_eq!(rows[0].name, "b");
    }
}
