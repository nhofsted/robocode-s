// SPDX-License-Identifier: EPL-2.0

//! Scoring of one robot (classic's `RobotStatistics`, engine-spec §10).

use crate::events::Results;
use crate::rules::{ROBOT_HIT_BONUS, ROBOT_HIT_DAMAGE};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Statistics {
    pub rank: i32,
    is_active: bool,
    pub is_in_round: bool,

    survival: f64,
    last_survivor_bonus: f64,
    bullet_damage: f64,
    bullet_kill_bonus: f64,
    ramming_damage: f64,
    ramming_kill_bonus: f64,
    /// Damage dealt to each robot (by name) this round.
    robot_damage: HashMap<String, f64>,

    total_score: f64,
    total_survival: f64,
    total_last_survivor_bonus: f64,
    total_bullet_damage: f64,
    total_bullet_kill_bonus: f64,
    total_ramming_damage: f64,
    total_ramming_kill_bonus: f64,
    total_firsts: i32,
    total_seconds: i32,
    total_thirds: i32,
}

impl Statistics {
    /// Start of a round (`reset`).
    pub fn reset(&mut self) {
        self.reset_scores();
        self.is_active = true;
        self.is_in_round = true;
    }

    fn reset_scores(&mut self) {
        self.robot_damage.clear();
        self.survival = 0.0;
        self.last_survivor_bonus = 0.0;
        self.bullet_damage = 0.0;
        self.bullet_kill_bonus = 0.0;
        self.ramming_damage = 0.0;
        self.ramming_kill_bonus = 0.0;
    }

    /// Disabled robots score nothing more (`setInactive`).
    pub fn set_inactive(&mut self) {
        self.reset_scores();
        self.is_active = false;
    }

    pub fn generate_totals(&mut self, sentry: bool) {
        self.total_survival += self.survival;
        self.total_last_survivor_bonus += self.last_survivor_bonus;
        self.total_bullet_damage += self.bullet_damage;
        self.total_bullet_kill_bonus += self.bullet_kill_bonus;
        self.total_ramming_damage += self.ramming_damage;
        self.total_ramming_kill_bonus += self.ramming_kill_bonus;
        self.total_score = if sentry {
            0.0
        } else {
            self.total_bullet_damage
                + self.total_ramming_damage
                + self.total_survival
                + self.total_ramming_kill_bonus
                + self.total_bullet_kill_bonus
                + self.total_last_survivor_bonus
        };
        self.is_in_round = false;
    }

    pub fn total_score(&self) -> f64 {
        self.total_score
    }

    pub fn current_score(&self, sentry: bool) -> f64 {
        if sentry {
            0.0
        } else {
            self.bullet_damage
                + self.ramming_damage
                + self.survival
                + self.ramming_kill_bonus
                + self.bullet_kill_bonus
                + self.last_survivor_bonus
        }
    }

    pub fn score_survival(&mut self, sentry: bool) {
        if self.is_active && !sentry {
            self.survival += 50.0;
        }
    }

    /// `enemy_count` = robots in the battle minus this robot and its teammates.
    /// Increments firsts for robots without a team and for team leaders.
    pub fn score_last_survivor(&mut self, sentry: bool, enemy_count: i32, counts_first: bool) {
        if self.is_active && !sentry {
            self.last_survivor_bonus += f64::from(10 * enemy_count);
            if counts_first {
                self.total_firsts += 1;
            }
        }
    }

    pub fn score_bullet_damage(&mut self, robot: &str, damage: f64) {
        if self.is_active {
            self.increment_robot_damage(robot, damage);
            self.bullet_damage += damage;
        }
    }

    /// Adds and returns the kill bonus; `team_damage` is the damage this robot's team
    /// (or the robot alone) dealt to the victim this round.
    pub fn score_bullet_kill(&mut self, team_damage: f64) -> f64 {
        if self.is_active {
            let bonus = team_damage;
            self.bullet_kill_bonus += bonus;
            bonus
        } else {
            0.0
        }
    }

    pub fn score_ramming_damage(&mut self, robot: &str) {
        if self.is_active {
            self.increment_robot_damage(robot, ROBOT_HIT_DAMAGE);
            self.ramming_damage += ROBOT_HIT_BONUS;
        }
    }

    pub fn score_ramming_kill(&mut self, team_damage: f64) -> f64 {
        if self.is_active {
            let bonus = team_damage;
            self.ramming_kill_bonus += bonus;
            bonus
        } else {
            0.0
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn score_robot_death(&mut self, sentry: bool, winner: bool, enemies_remaining: i32) {
        if !sentry {
            match enemies_remaining {
                0 => {
                    if !winner {
                        self.total_firsts += 1;
                    }
                }
                1 => self.total_seconds += 1,
                2 => self.total_thirds += 1,
                _ => {}
            }
        }
    }

    pub fn score_firsts(&mut self, sentry: bool) {
        if self.is_active && !sentry {
            self.total_firsts += 1;
        }
    }

    pub fn robot_damage(&self, robot: &str) -> f64 {
        self.robot_damage.get(robot).copied().unwrap_or(0.0)
    }

    fn increment_robot_damage(&mut self, robot: &str, damage: f64) {
        let d = self.robot_damage(robot) + damage;
        self.robot_damage.insert(robot.to_string(), d);
    }

    pub fn final_results(&self) -> Results {
        Results {
            rank: self.rank,
            score: self.total_score,
            survival: self.total_survival,
            last_survivor_bonus: self.total_last_survivor_bonus,
            bullet_damage: self.total_bullet_damage,
            bullet_damage_bonus: self.total_bullet_kill_bonus,
            ram_damage: self.total_ramming_damage,
            ram_damage_bonus: self.total_ramming_kill_bonus,
            firsts: self.total_firsts,
            seconds: self.total_seconds,
            thirds: self.total_thirds,
        }
    }
}
