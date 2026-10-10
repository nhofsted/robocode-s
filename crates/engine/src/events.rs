// SPDX-License-Identifier: EPL-2.0

//! Events the engine generates for robots (engine-spec §12).

/// A bullet as seen in an event (classic's `robocode.Bullet`).
#[derive(Debug, Clone, PartialEq)]
pub struct BulletInfo {
    pub id: i32,
    pub owner: String,
    pub victim: Option<String>,
    pub heading: f64,
    pub x: f64,
    pub y: f64,
    pub power: f64,
    pub active: bool,
}

/// Final results of a robot, as in `BattleEndedEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    pub rank: i32,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    ScannedRobot {
        name: String,
        energy: f64,
        bearing: f64,
        distance: f64,
        heading: f64,
        velocity: f64,
        sentry: bool,
    },
    HitWall {
        bearing: f64,
    },
    HitRobot {
        name: String,
        bearing: f64,
        energy: f64,
        at_fault: bool,
    },
    HitByBullet {
        bearing: f64,
        bullet: BulletInfo,
    },
    BulletHit {
        name: String,
        energy: f64,
        bullet: BulletInfo,
    },
    BulletHitBullet {
        bullet: BulletInfo,
        hit_bullet: BulletInfo,
    },
    BulletMissed {
        bullet: BulletInfo,
    },
    RobotDeath {
        name: String,
    },
    Death,
    Win,
    RoundEnded {
        round: i32,
        turns: i64,
        total_turns: i64,
    },
    BattleEnded {
        aborted: bool,
        results: Results,
    },
    SkippedTurn {
        turn: i64,
    },
}
