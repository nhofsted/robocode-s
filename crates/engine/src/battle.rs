// SPDX-License-Identifier: EPL-2.0

//! The battle: rounds, the turn sequence, robots and bullets (engine-spec §4–§10).
//!
//! The code follows classic Robocode 1.11.1 closely; comments name the classic method.
//! Every random draw happens in classic's order (engine-spec §3).

use crate::commands::{Commands, TeamMessage};
use crate::events::{BulletInfo, Event, Results};
use crate::java;
use crate::rules::*;
use crate::stats::Statistics;
use crate::utils::{
    is_near, normal_absolute_angle, normal_near_absolute_angle, normal_relative_angle,
};
use robocode_s_fdlibm as m;
use robocode_s_geom::{Arc, Rect, scan_intersects};
use robocode_s_java_random::{JavaRandom, shuffle};
use std::f64::consts::PI;

/// Battle rules (classic's `BattleRules`).
#[derive(Debug, Clone, PartialEq)]
pub struct BattleRules {
    pub width: i32,
    pub height: i32,
    pub num_rounds: i32,
    pub gun_cooling_rate: f64,
    pub inactivity_time: i64,
    pub hide_enemy_names: bool,
    pub sentry_border_size: i32,
}

impl Default for BattleRules {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            num_rounds: 10,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            hide_enemy_names: false,
            sentry_border_size: 100,
        }
    }
}

/// A robot taking part in the battle.
#[derive(Debug, Clone, PartialEq)]
pub struct RobotSetup {
    /// Unique name, e.g. `jk.mega.DrussGT 3.1.16` or `sample.Fire (2)`.
    pub name: String,
    /// Full class name without version or suffix (team message recipients match on it).
    pub full_class_name: String,
    pub advanced: bool,
    pub team_robot: bool,
    pub droid: bool,
    pub sentry: bool,
    pub leader: bool,
    /// Team index, if the robot is part of a team.
    pub team: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobotState {
    Active,
    HitWall,
    HitRobot,
    Dead,
}

impl RobotState {
    /// Classic's enum name.
    pub fn name(self) -> &'static str {
        match self {
            RobotState::Active => "ACTIVE",
            RobotState::HitWall => "HIT_WALL",
            RobotState::HitRobot => "HIT_ROBOT",
            RobotState::Dead => "DEAD",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulletState {
    Fired,
    Moving,
    HitVictim,
    HitBullet,
    HitWall,
    Exploded,
    Inactive,
}

impl BulletState {
    pub fn name(self) -> &'static str {
        match self {
            BulletState::Fired => "FIRED",
            BulletState::Moving => "MOVING",
            BulletState::HitVictim => "HIT_VICTIM",
            BulletState::HitBullet => "HIT_BULLET",
            BulletState::HitWall => "HIT_WALL",
            BulletState::Exploded => "EXPLODED",
            BulletState::Inactive => "INACTIVE",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, BulletState::Fired | BulletState::Moving)
    }
}

/// One robot's engine state (classic's `RobotPeer`).
#[derive(Debug, Clone)]
pub struct Robot {
    pub setup: RobotSetup,
    pub index: usize,
    pub state: RobotState,
    pub energy: f64,
    pub velocity: f64,
    pub x: f64,
    pub y: f64,
    pub body_heading: f64,
    pub gun_heading: f64,
    pub radar_heading: f64,
    pub gun_heat: f64,
    pub bbox: Rect,
    /// The stored command state; also classic's `currentCommands` during a turn.
    pub commands: Commands,
    pub stats: Statistics,
    pub events: Vec<Event>,
    /// Status updates of this robot's own bullets, in the order the engine made them.
    pub bullet_updates: Vec<BulletStatus>,
    /// Team messages delivered to this robot.
    pub team_inbox: Vec<TeamMessage>,
    last_heading: f64,
    last_gun_heading: f64,
    last_radar_heading: f64,
    scan: bool,
    turned_radar_with_gun: bool,
    in_collision: bool,
    is_over_driving: bool,
    is_io_robot: bool,
    is_winner: bool,
    halted: bool,
    /// The robot's thread has finished (death, halt or abort); it gains no more energy.
    exec_finished_and_disabled: bool,
    energy_drained: bool,
    /// The robot's thread still runs and executes after each turn.
    running: bool,
}

impl Robot {
    fn new(setup: RobotSetup, index: usize) -> Self {
        Self {
            setup,
            index,
            state: RobotState::Active,
            energy: 0.0,
            velocity: 0.0,
            x: 0.0,
            y: 0.0,
            body_heading: 0.0,
            gun_heading: 0.0,
            radar_heading: 0.0,
            gun_heat: 0.0,
            bbox: Rect::EMPTY,
            commands: Commands::default(),
            stats: Statistics::default(),
            events: Vec::new(),
            bullet_updates: Vec::new(),
            team_inbox: Vec::new(),
            last_heading: 0.0,
            last_gun_heading: 0.0,
            last_radar_heading: 0.0,
            scan: false,
            turned_radar_with_gun: false,
            in_collision: false,
            is_over_driving: false,
            is_io_robot: false,
            is_winner: false,
            halted: false,
            exec_finished_and_disabled: false,
            energy_drained: false,
            running: false,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.state != RobotState::Dead
    }

    pub fn is_dead(&self) -> bool {
        self.state == RobotState::Dead
    }

    /// Whether the robot still executes after each turn: not dead, halted or disabled.
    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    pub fn is_winner(&self) -> bool {
        self.is_winner
    }

    pub fn current_score(&self) -> f64 {
        self.stats.current_score(self.setup.sentry)
    }

    pub fn total_score(&self) -> f64 {
        self.stats.total_score()
    }

    fn update_bounding_box(&mut self) {
        self.bbox = Rect::robot(self.x, self.y);
    }

    fn add_event(&mut self, event: Event) {
        self.events.push(event);
    }

    // ---------------------------------------------------------------- movement (engine-spec §6)

    /// `updateHeading`.
    fn update_heading(&mut self) {
        let mut normalize = true;
        let turn_rate = java::min(
            self.commands.max_turn_rate,
            (0.4 + 0.6 * (1.0 - (self.velocity.abs() / MAX_VELOCITY))) * MAX_TURN_RATE_RADIANS,
        );
        let c = &mut self.commands;
        let r = c.body_turn_remaining;
        if r > 0.0 {
            if r < turn_rate {
                self.body_heading += r;
                self.gun_heading += r;
                self.radar_heading += r;
                if c.adjust_gun_for_body_turn {
                    c.gun_turn_remaining -= r;
                }
                if c.adjust_radar_for_body_turn {
                    c.radar_turn_remaining -= r;
                }
                c.body_turn_remaining = 0.0;
            } else {
                self.body_heading += turn_rate;
                self.gun_heading += turn_rate;
                self.radar_heading += turn_rate;
                c.body_turn_remaining -= turn_rate;
                if c.adjust_gun_for_body_turn {
                    c.gun_turn_remaining -= turn_rate;
                }
                if c.adjust_radar_for_body_turn {
                    c.radar_turn_remaining -= turn_rate;
                }
            }
        } else if r < 0.0 {
            if r > -turn_rate {
                self.body_heading += r;
                self.gun_heading += r;
                self.radar_heading += r;
                if c.adjust_gun_for_body_turn {
                    c.gun_turn_remaining -= r;
                }
                if c.adjust_radar_for_body_turn {
                    c.radar_turn_remaining -= r;
                }
                c.body_turn_remaining = 0.0;
            } else {
                self.body_heading -= turn_rate;
                self.gun_heading -= turn_rate;
                self.radar_heading -= turn_rate;
                c.body_turn_remaining += turn_rate;
                if c.adjust_gun_for_body_turn {
                    c.gun_turn_remaining += turn_rate;
                }
                if c.adjust_radar_for_body_turn {
                    c.radar_turn_remaining += turn_rate;
                }
            }
        } else {
            normalize = false;
        }
        if normalize {
            self.body_heading = if self.commands.body_turn_remaining == 0.0 {
                normal_near_absolute_angle(self.body_heading)
            } else {
                normal_absolute_angle(self.body_heading)
            };
        }
    }

    /// `updateGunHeading`.
    fn update_gun_heading(&mut self) {
        let c = &mut self.commands;
        let r = c.gun_turn_remaining;
        let rate = GUN_TURN_RATE_RADIANS;
        if r > 0.0 {
            if r < rate {
                self.gun_heading += r;
                self.radar_heading += r;
                if c.adjust_radar_for_gun_turn {
                    c.radar_turn_remaining -= r;
                }
                c.gun_turn_remaining = 0.0;
            } else {
                self.gun_heading += rate;
                self.radar_heading += rate;
                c.gun_turn_remaining -= rate;
                if c.adjust_radar_for_gun_turn {
                    c.radar_turn_remaining -= rate;
                }
            }
        } else if r < 0.0 {
            if r > -rate {
                self.gun_heading += r;
                self.radar_heading += r;
                if c.adjust_radar_for_gun_turn {
                    c.radar_turn_remaining -= r;
                }
                c.gun_turn_remaining = 0.0;
            } else {
                self.gun_heading -= rate;
                self.radar_heading -= rate;
                c.gun_turn_remaining += rate;
                if c.adjust_radar_for_gun_turn {
                    c.radar_turn_remaining += rate;
                }
            }
        }
        self.gun_heading = normal_absolute_angle(self.gun_heading);
    }

    /// `updateRadarHeading`.
    fn update_radar_heading(&mut self) {
        let c = &mut self.commands;
        let r = c.radar_turn_remaining;
        let rate = RADAR_TURN_RATE_RADIANS;
        if r > 0.0 {
            if r < rate {
                self.radar_heading += r;
                c.radar_turn_remaining = 0.0;
            } else {
                self.radar_heading += rate;
                c.radar_turn_remaining -= rate;
            }
        } else if r < 0.0 {
            if r > -rate {
                self.radar_heading += r;
                c.radar_turn_remaining = 0.0;
            } else {
                self.radar_heading -= rate;
                c.radar_turn_remaining += rate;
            }
        }
        self.radar_heading = normal_absolute_angle(self.radar_heading);
    }

    /// `updateMovement`.
    fn update_movement(&mut self) {
        let mut distance = self.commands.distance_remaining;
        if distance.is_nan() {
            distance = 0.0;
        }
        self.velocity = self.new_velocity(self.velocity, distance);
        if is_near(self.velocity, 0.0) && self.is_over_driving {
            self.commands.distance_remaining = 0.0;
            distance = 0.0;
            self.is_over_driving = false;
        }
        if java::signum(distance * self.velocity) != -1.0 {
            self.is_over_driving =
                self.distance_traveled_until_stop(self.velocity) > distance.abs();
        }
        self.commands.distance_remaining = distance - self.velocity;
        if self.velocity != 0.0 {
            self.x += self.velocity * m::sin(self.body_heading);
            self.y += self.velocity * m::cos(self.body_heading);
            self.update_bounding_box();
        }
    }

    fn distance_traveled_until_stop(&self, velocity: f64) -> f64 {
        let mut distance = 0.0;
        let mut v = velocity.abs();
        while v > 0.0 {
            v = self.new_velocity(v, 0.0);
            distance += v;
        }
        distance
    }

    /// `getNewVelocity`.
    fn new_velocity(&self, velocity: f64, distance: f64) -> f64 {
        if distance < 0.0 {
            return -self.new_velocity(-velocity, -distance);
        }
        let goal = if distance == f64::INFINITY {
            self.commands.max_velocity
        } else {
            java::min(max_velocity_for(distance), self.commands.max_velocity)
        };
        if velocity >= 0.0 {
            java::max(
                velocity - DECELERATION,
                java::min(goal, velocity + ACCELERATION),
            )
        } else {
            java::max(
                velocity - ACCELERATION,
                java::min(goal, velocity + max_decel(-velocity)),
            )
        }
    }

    /// `checkWallCollision`.
    fn check_wall_collision(&mut self, width: i32, height: i32) {
        let min_x = f64::from(ROBOT_HALF_SIZE);
        let min_y = f64::from(ROBOT_HALF_SIZE);
        let max_x = f64::from(width - ROBOT_HALF_SIZE);
        let max_y = f64::from(height - ROBOT_HALF_SIZE);
        let mut hit = false;
        let (mut adjust_x, mut adjust_y) = (0.0, 0.0);
        let mut angle = 0.0;
        if self.x < min_x {
            hit = true;
            adjust_x = min_x - self.x;
            angle = normal_relative_angle(3.0 * PI / 2.0 - self.body_heading);
        } else if self.x > max_x {
            hit = true;
            adjust_x = max_x - self.x;
            angle = normal_relative_angle(PI / 2.0 - self.body_heading);
        }
        if self.y < min_y {
            hit = true;
            adjust_y = min_y - self.y;
            angle = normal_relative_angle(PI - self.body_heading);
        } else if self.y > max_y {
            hit = true;
            adjust_y = max_y - self.y;
            angle = normal_relative_angle(-self.body_heading);
        }
        if !hit {
            return;
        }
        self.add_event(Event::HitWall { bearing: angle });
        let (adjust_x, adjust_y) = self.slide(adjust_x, adjust_y);
        self.x += adjust_x;
        self.y += adjust_y;
        if self.x < min_x {
            self.x = min_x;
        } else if self.x > max_x {
            self.x = max_x;
        }
        if self.y < min_y {
            self.y = min_y;
        } else if self.y > max_y {
            self.y = max_y;
        }
        self.after_wall_hit();
    }

    /// Projects the correction onto the direction of travel (shared by wall and sentry border).
    /// Two branches are identical; kept as in classic.
    #[allow(clippy::if_same_then_else)]
    fn slide(&self, mut adjust_x: f64, mut adjust_y: f64) -> (f64, f64) {
        if self.body_heading % (PI / 2.0) != 0.0 {
            let tan_heading = m::tan(self.body_heading);
            if adjust_x == 0.0 {
                adjust_x = adjust_y * tan_heading;
            } else if adjust_y == 0.0 {
                adjust_y = adjust_x / tan_heading;
            } else if (adjust_x / tan_heading).abs() > adjust_y.abs() {
                adjust_y = adjust_x / tan_heading;
            } else if (adjust_y * tan_heading).abs() > adjust_x.abs() {
                adjust_x = adjust_y * tan_heading;
            }
        }
        (adjust_x, adjust_y)
    }

    fn after_wall_hit(&mut self) {
        if self.setup.advanced {
            let damage = wall_hit_damage(self.velocity);
            self.set_energy_raw(self.energy - damage);
        }
        self.update_bounding_box();
        self.commands.distance_remaining = 0.0;
        self.velocity = 0.0;
        self.state = RobotState::HitWall;
    }

    /// `checkSentryOutsideBorder`.
    fn check_sentry_outside_border(&mut self, width: i32, height: i32, range: i32) {
        let min_x = f64::from(range - ROBOT_HALF_SIZE);
        let min_y = f64::from(range - ROBOT_HALF_SIZE);
        let max_x = f64::from(width - range + ROBOT_HALF_SIZE);
        let max_y = f64::from(height - range + ROBOT_HALF_SIZE);
        let mut hit = false;
        let (mut adjust_x, mut adjust_y) = (0.0, 0.0);
        let mut angle = 0.0;
        let inside = self.x > min_x && self.x < max_x && self.y > min_y && self.y < max_y;
        if inside {
            if (self.x - min_x) <= MAX_VELOCITY {
                hit = true;
                adjust_x = min_x - self.x;
                angle = normal_relative_angle(PI / 2.0 - self.body_heading);
            } else if (max_x - self.x) <= MAX_VELOCITY {
                hit = true;
                adjust_x = max_x - self.x;
                angle = normal_relative_angle(3.0 * PI / 2.0 - self.body_heading);
            } else if (self.y - min_y) <= MAX_VELOCITY {
                hit = true;
                adjust_y = min_y - self.y;
                angle = normal_relative_angle(-self.body_heading);
            } else if (max_y - self.y) <= MAX_VELOCITY {
                hit = true;
                adjust_y = max_y - self.y;
                angle = normal_relative_angle(PI - self.body_heading);
            }
        }
        if !hit {
            return;
        }
        self.add_event(Event::HitWall { bearing: angle });
        let (adjust_x, adjust_y) = self.slide(adjust_x, adjust_y);
        self.x += adjust_x;
        self.y += adjust_y;
        if inside {
            if (self.x - min_x) <= MAX_VELOCITY {
                self.x = min_x;
            } else if (max_x - self.x) <= MAX_VELOCITY {
                self.x = max_x;
            }
            if (self.y - min_y) <= MAX_VELOCITY {
                self.y = min_y;
            } else if (max_y - self.y) <= MAX_VELOCITY {
                self.y = max_y;
            }
        }
        self.after_wall_hit();
    }

    /// `setEnergy(e, false)`: without touching the inactivity counter.
    fn set_energy_raw(&mut self, energy: f64) {
        self.energy = energy;
        if self.energy < 0.01 {
            self.energy = 0.0;
            self.commands.distance_remaining = 0.0;
            self.commands.body_turn_remaining = 0.0;
        }
    }
}

/// `getMaxVelocity(distance)`.
fn max_velocity_for(distance: f64) -> f64 {
    let decel_time = java::max(
        1.0,
        ((((4.0 * 2.0 / DECELERATION) * distance + 1.0).sqrt() - 1.0) / 2.0).ceil(),
    );
    if decel_time == f64::INFINITY {
        return MAX_VELOCITY;
    }
    let decel_dist = (decel_time / 2.0) * (decel_time - 1.0) * DECELERATION;
    ((decel_time - 1.0) * DECELERATION) + ((distance - decel_dist) / decel_time)
}

/// `maxDecel(speed)`.
fn max_decel(speed: f64) -> f64 {
    let decel_time = speed / DECELERATION;
    let accel_time = 1.0 - decel_time;
    java::min(1.0, decel_time) * DECELERATION + java::max(0.0, accel_time) * ACCELERATION
}

/// An update of a robot's own bullet (classic's `BulletStatus`).
#[derive(Debug, Clone, PartialEq)]
pub struct BulletStatus {
    pub bullet_id: i32,
    pub x: f64,
    pub y: f64,
    pub victim: Option<String>,
    pub active: bool,
}

/// What a robot learns about itself after a turn (classic's `RobotStatus`).
#[derive(Debug, Clone, PartialEq)]
pub struct RobotStatus {
    pub energy: f64,
    pub x: f64,
    pub y: f64,
    pub body_heading: f64,
    pub gun_heading: f64,
    pub radar_heading: f64,
    pub velocity: f64,
    pub body_turn_remaining: f64,
    pub radar_turn_remaining: f64,
    pub gun_turn_remaining: f64,
    pub distance_remaining: f64,
    pub gun_heat: f64,
    pub others: i32,
    pub num_sentries: i32,
    pub round: i32,
    pub num_rounds: i32,
    pub time: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulletKind {
    Bullet,
    /// A robot explosion (`ExplosionPeer`): follows its owner, never collides.
    Explosion,
}

/// A bullet or explosion (classic's `BulletPeer`, engine-spec §8).
#[derive(Debug, Clone)]
pub struct Bullet {
    key: u64,
    pub kind: BulletKind,
    pub owner: usize,
    pub id: i32,
    pub state: BulletState,
    pub heading: f64,
    pub x: f64,
    pub y: f64,
    last_x: f64,
    last_y: f64,
    pub power: f64,
    pub victim: Option<usize>,
    pub frame: i32,
    /// The segment travelled in the last update.
    line: (f64, f64, f64, f64),
}

impl Bullet {
    fn explosion_length(&self) -> i32 {
        match self.kind {
            BulletKind::Bullet => BULLET_EXPLOSION_LENGTH,
            BulletKind::Explosion => ROBOT_EXPLOSION_LENGTH,
        }
    }

    /// `updateBulletState`.
    fn update_state(&mut self) {
        match self.state {
            BulletState::Fired => {
                if self.frame > 0 {
                    self.state = BulletState::Moving;
                }
            }
            BulletState::HitBullet
            | BulletState::HitVictim
            | BulletState::HitWall
            | BulletState::Exploded
                if self.frame >= self.explosion_length() =>
            {
                self.state = BulletState::Inactive;
            }
            _ => {}
        }
    }

    /// Robocode's own segment intersection test (`BulletPeer.intersect`).
    fn intersects(&self, other: (f64, f64, f64, f64)) -> bool {
        let (x1, y1, x2, y2) = other;
        let (x3, y3, x4, y4) = self.line;
        let (dx13, dx21, dx43) = (x1 - x3, x2 - x1, x4 - x3);
        let (dy13, dy21, dy43) = (y1 - y3, y2 - y1, y4 - y3);
        let dn = dy43 * dx21 - dx43 * dy21;
        let ua = (dx43 * dy13 - dy43 * dx13) / dn;
        let ub = (dx21 * dy13 - dy21 * dx13) / dn;
        (0.0..=1.0).contains(&ua) && (0.0..=1.0).contains(&ub)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contestant {
    Robot(usize),
    Team(usize),
}

/// A battle in progress.
#[derive(Debug, Clone)]
pub struct Battle {
    pub rules: BattleRules,
    pub robots: Vec<Robot>,
    pub bullets: Vec<Bullet>,
    random: JavaRandom,
    pub round: i32,
    pub time: i64,
    pub total_turns: i64,
    end_timer: i32,
    inactive_turn_count: i64,
    inactivity_energy: f64,
    active_participants: i32,
    active_sentries: i32,
    death_robots: Vec<usize>,
    pub aborted: bool,
    /// The results when the battle was aborted (see [`Battle::results`]).
    results_at_abort: Option<Vec<Results>>,
    /// Members of each team, in robot index order (the first is the leader).
    teams: Vec<Vec<usize>>,
    contestants: Vec<Contestant>,
    next_key: u64,
}

impl Battle {
    /// `Battle.setup`: robots get their indices in the given order.
    pub fn new(rules: BattleRules, setups: Vec<RobotSetup>, seed: i64) -> Self {
        let mut teams: Vec<Vec<usize>> = Vec::new();
        let mut contestants = Vec::new();
        let mut robots = Vec::new();
        for (index, setup) in setups.into_iter().enumerate() {
            match setup.team {
                Some(team) => {
                    if teams.len() <= team {
                        teams.resize(team + 1, Vec::new());
                    }
                    if teams[team].is_empty() {
                        contestants.push(Contestant::Team(team));
                    }
                    teams[team].push(index);
                }
                None => contestants.push(Contestant::Robot(index)),
            }
            robots.push(Robot::new(setup, index));
        }
        Self {
            rules,
            robots,
            bullets: Vec::new(),
            random: JavaRandom::new(seed),
            round: 0,
            time: 0,
            total_turns: 0,
            end_timer: 0,
            inactive_turn_count: 0,
            inactivity_energy: 0.0,
            active_participants: 0,
            active_sentries: 0,
            death_robots: Vec::new(),
            aborted: false,
            results_at_abort: None,
            teams,
            contestants,
            next_key: 0,
        }
    }

    fn robots_at_random(&mut self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.robots.len()).collect();
        shuffle(&mut order, &mut self.random);
        order
    }

    fn bullets_at_random(&mut self) -> Vec<u64> {
        let mut order: Vec<u64> = self.bullets.iter().map(|b| b.key).collect();
        shuffle(&mut order, &mut self.random);
        order
    }

    fn bullet_index(&self, key: u64) -> Option<usize> {
        self.bullets.iter().position(|b| b.key == key)
    }

    fn new_key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    // ---------------------------------------------------------------- rounds (engine-spec §4)

    /// `preloadRound` and `initializeRound`: places the robots and starts the round.
    pub fn start_round(&mut self) {
        self.compute_active_robots();
        for i in 0..self.robots.len() {
            self.initialize_robot_round(i);
        }
        self.compute_active_robots();
        self.end_timer = 0;
        self.time = 0;
        self.inactive_turn_count = 0;
        for i in self.robots_at_random() {
            self.robots[i].stats.reset();
            self.robots[i].running = true;
        }
    }

    /// `RobotPeer.initializeRound` (random placement; explicit positions are not supported yet).
    fn initialize_robot_round(&mut self, i: usize) {
        let (w, h) = (self.rules.width, self.rules.height);
        let border = self.rules.sentry_border_size;
        let max_width = f64::from(w - ROBOT_SIZE);
        let max_height = f64::from(h - ROBOT_SIZE);
        let half = f64::from(ROBOT_SIZE / 2);
        let move_width = border - ROBOT_SIZE;
        let move_height = border - ROBOT_SIZE;
        let sentry = self.robots[i].setup.sentry;
        for _ in 0..1000 {
            let rnd_x = self.random.next_double();
            let rnd_y = self.random.next_double();
            let (x, y);
            if sentry {
                let horizontal = self.random.next_double() <= f64::from(w) / f64::from(w + h);
                if horizontal {
                    x = half + rnd_x * max_width;
                    y = half
                        + (f64::from(move_height) * (rnd_y * 2.0 - 1.0) + max_height) % max_height;
                } else {
                    y = half + rnd_y * max_height;
                    x = half
                        + (f64::from(move_width) * (rnd_x * 2.0 - 1.0) + max_width) % max_width;
                }
            } else if self.active_sentries > 0 {
                let safe_width = w - 2 * border;
                let safe_height = h - 2 * border;
                x = f64::from(border + ROBOT_SIZE) + rnd_x * f64::from(safe_width - 2 * ROBOT_SIZE);
                y = f64::from(border + ROBOT_SIZE)
                    + rnd_y * f64::from(safe_height - 2 * ROBOT_SIZE);
            } else {
                x = f64::from(ROBOT_SIZE) + rnd_x * f64::from(w - 2 * ROBOT_SIZE);
                y = f64::from(ROBOT_SIZE) + rnd_y * f64::from(h - 2 * ROBOT_SIZE);
            }
            let heading = 2.0 * PI * self.random.next_double();
            let r = &mut self.robots[i];
            r.x = x;
            r.y = y;
            r.body_heading = heading;
            r.gun_heading = heading;
            r.radar_heading = heading;
            r.update_bounding_box();
            if self.valid_spot(i) {
                break;
            }
        }
        let r = &mut self.robots[i];
        r.state = RobotState::Active;
        r.is_winner = false;
        r.velocity = 0.0;
        r.energy = 100.0;
        if r.setup.sentry {
            r.energy += 400.0;
        }
        if r.setup.leader {
            r.energy += 100.0;
        }
        if r.setup.droid {
            r.energy += 20.0;
        }
        r.gun_heat = 3.0;
        r.halted = false;
        r.exec_finished_and_disabled = false;
        r.energy_drained = false;
        r.scan = false;
        r.in_collision = false;
        r.events.clear();
        r.bullet_updates.clear();
        r.team_inbox.clear();
        r.commands = Commands::default();
    }

    /// `validSpot`: no overlap with any other robot's current box (placed or not).
    fn valid_spot(&self, i: usize) -> bool {
        let bbox = self.robots[i].bbox;
        self.robots
            .iter()
            .all(|o| o.index == i || !bbox.intersects(&o.bbox))
    }

    /// `isRoundOver`.
    pub fn is_round_over(&self) -> bool {
        self.end_timer > 5 * TURNS_DISPLAYED_AFTER_ENDING
    }

    /// Each robot's results so far: the completed rounds, or for an aborted battle the
    /// results at the abort, ranked.
    pub fn results(&self) -> Vec<Results> {
        match &self.results_at_abort {
            Some(results) => results.clone(),
            None => self
                .robots
                .iter()
                .map(|r| r.stats.final_results())
                .collect(),
        }
    }

    pub fn is_last_round(&self) -> bool {
        self.round + 1 == self.rules.num_rounds
    }

    /// `finalizeRound`: bullets are cleared; then the next round.
    pub fn finish_round(&mut self) {
        self.bullets.clear();
        self.round += 1;
    }

    // ---------------------------------------------------------------- commands

    /// A robot executed: its new command state, validated as classic does on receipt.
    pub fn set_commands(&mut self, i: usize, mut commands: Commands) {
        commands.validate();
        self.robots[i].commands = commands;
    }

    /// Takes the robot's bullet updates since the last call.
    pub fn take_bullet_updates(&mut self, i: usize) -> Vec<BulletStatus> {
        std::mem::take(&mut self.robots[i].bullet_updates)
    }

    /// Takes the team messages delivered to the robot since the last call.
    pub fn take_team_messages(&mut self, i: usize) -> Vec<TeamMessage> {
        std::mem::take(&mut self.robots[i].team_inbox)
    }

    /// The robot's status (`publishStatus` / `startRound`).
    pub fn status(&self, i: usize) -> RobotStatus {
        let r = &self.robots[i];
        let others = if self.time == 0 {
            // `startRound`: alive robots don't count themselves.
            self.active_participants - i32::from(r.is_alive())
        } else {
            self.active_participants - i32::from(!(r.is_dead() || r.setup.sentry))
        };
        RobotStatus {
            energy: r.energy,
            x: r.x,
            y: r.y,
            body_heading: r.body_heading,
            gun_heading: r.gun_heading,
            radar_heading: r.radar_heading,
            velocity: r.velocity,
            body_turn_remaining: r.commands.body_turn_remaining,
            radar_turn_remaining: r.commands.radar_turn_remaining,
            gun_turn_remaining: r.commands.gun_turn_remaining,
            distance_remaining: r.commands.distance_remaining,
            gun_heat: r.gun_heat,
            others,
            num_sentries: self.active_sentries,
            round: self.round,
            num_rounds: self.rules.num_rounds,
            time: self.time,
        }
    }

    /// Disables a robot (`punishBadBehavior`), e.g. after too many skipped turns: it dies,
    /// scores nothing more this round, and its thread stops.
    pub fn disable(&mut self, i: usize) {
        self.kill(i);
        self.robots[i].stats.set_inactive();
        self.robots[i].halted = true;
    }

    /// The robot's code crashed (`drainEnergy`): energy 0, and it gains no energy anymore.
    pub fn drain_energy(&mut self, i: usize) {
        self.set_energy(i, 0.0);
        self.robots[i].energy_drained = true;
    }

    /// The robot's thread ended: it no longer executes, its last commands stay in effect.
    pub fn stop(&mut self, i: usize) {
        self.robots[i].running = false;
    }

    /// The battle is stopped (classic's `AbortCommand`): from the next turn every robot
    /// loses 5 energy a turn and its thread stops, the round runs out, and robots get a
    /// `BattleEndedEvent` with `aborted` set.
    pub fn abort(&mut self) {
        self.aborted = true;
    }

    /// Takes the events generated for a robot since the last call.
    pub fn take_events(&mut self, i: usize) -> Vec<Event> {
        std::mem::take(&mut self.robots[i].events)
    }

    // ---------------------------------------------------------------- the turn (engine-spec §5)

    /// `Battle.runTurn`.
    pub fn step(&mut self) {
        self.time += 1;
        self.total_turns += 1;
        for i in 0..self.robots.len() {
            self.load_commands(i);
        }
        self.update_bullets();
        self.update_robots();
        self.handle_dead_robots();
        if self.aborted || self.one_team_remaining() {
            self.shutdown_turn();
        }
        self.inactive_turn_count += 1;
        self.compute_active_robots();
        self.wakeup_robots();
    }

    /// `performLoadCommands`.
    fn load_commands(&mut self, i: usize) {
        self.fire_bullets(i);
        let r = &mut self.robots[i];
        if r.commands.scan {
            r.scan = true;
        }
        if r.commands.io {
            r.is_io_robot = true;
        }
    }

    /// `fireBullets`.
    fn fire_bullets(&mut self, i: usize) {
        let mut new_bullet = None;
        for c in 0..self.robots[i].commands.bullets.len() {
            let cmd = self.robots[i].commands.bullets[c].clone();
            if cmd.power.is_nan() || cmd.consumed {
                continue;
            }
            let (gun_heat, energy) = (self.robots[i].gun_heat, self.robots[i].energy);
            if gun_heat > 0.0 || energy == 0.0 {
                // (quirk) A bullet created earlier in this loop is never added.
                return;
            }
            let power = java::min(
                energy,
                java::min(java::max(cmd.power, MIN_BULLET_POWER), MAX_BULLET_POWER),
            );
            self.update_energy(i, -power);
            let key = self.new_key();
            let r = &mut self.robots[i];
            r.gun_heat += crate::rules::gun_heat(power);
            let heading = if !r.turned_radar_with_gun || !cmd.assist_valid || r.setup.advanced {
                r.gun_heading
            } else {
                cmd.assist_angle
            };
            new_bullet = Some(Bullet {
                key,
                kind: BulletKind::Bullet,
                owner: i,
                id: cmd.id,
                state: BulletState::Fired,
                heading,
                x: r.x,
                y: r.y,
                last_x: r.x,
                last_y: r.y,
                power,
                victim: None,
                frame: 0,
                line: (0.0, 0.0, 0.0, 0.0),
            });
            r.commands.bullets[c].consumed = true;
        }
        if let Some(b) = new_bullet {
            self.bullets.push(b);
        }
    }

    /// `updateBullets`.
    fn update_bullets(&mut self) {
        for key in self.bullets_at_random() {
            let robots = self.robots_at_random();
            let bullets = self.bullets_at_random();
            self.update_bullet(key, &robots, &bullets);
            if let Some(b) = self.bullet_index(key) {
                if self.bullets[b].state == BulletState::Inactive {
                    self.bullets.remove(b);
                }
            }
        }
    }

    /// `BulletPeer.update` / `ExplosionPeer.update`.
    fn update_bullet(&mut self, key: u64, robots: &[usize], bullets: &[u64]) {
        let Some(b) = self.bullet_index(key) else {
            return;
        };
        if self.bullets[b].kind == BulletKind::Explosion {
            let owner = self.bullets[b].owner;
            let (x, y) = (self.robots[owner].x, self.robots[owner].y);
            let bullet = &mut self.bullets[b];
            bullet.frame += 1;
            bullet.x = x;
            bullet.y = y;
            bullet.update_state();
            return;
        }
        self.bullets[b].frame += 1;
        if self.bullets[b].state.is_active() {
            {
                let bullet = &mut self.bullets[b];
                bullet.last_x = bullet.x;
                bullet.last_y = bullet.y;
                let v = bullet_speed(bullet.power);
                bullet.x += v * m::sin(bullet.heading);
                bullet.y += v * m::cos(bullet.heading);
                bullet.line = (bullet.last_x, bullet.last_y, bullet.x, bullet.y);
            }
            self.check_bullet_wall(key);
            if self.bullet(key).state.is_active() {
                self.check_bullet_robots(key, robots);
            }
            if self.bullet(key).state.is_active() {
                self.check_bullet_bullets(key, bullets);
            }
        }
        if let Some(b) = self.bullet_index(key) {
            self.bullets[b].update_state();
        }
        // `owner.addBulletStatus(createStatus())`: only while the owner is alive.
        let b = self.bullet(key);
        let owner = b.owner;
        if self.robots[owner].is_alive() {
            let status = BulletStatus {
                bullet_id: b.id,
                x: b.x,
                y: b.y,
                victim: b.victim.map(|v| self.bullet_name_for_event(owner, v)),
                active: b.state.is_active(),
            };
            self.robots[owner].bullet_updates.push(status);
        }
    }

    fn bullet(&self, key: u64) -> &Bullet {
        &self.bullets[self.bullet_index(key).expect("bullet")]
    }

    fn bullet_mut(&mut self, key: u64) -> &mut Bullet {
        let i = self.bullet_index(key).expect("bullet");
        &mut self.bullets[i]
    }

    fn check_bullet_wall(&mut self, key: u64) {
        let (w, h) = (f64::from(self.rules.width), f64::from(self.rules.height));
        let b = self.bullet(key);
        if (b.x - BULLET_RADIUS <= 0.0)
            || (b.y - BULLET_RADIUS <= 0.0)
            || (b.x + BULLET_RADIUS >= w)
            || (b.y + BULLET_RADIUS >= h)
        {
            let b = self.bullet_mut(key);
            b.state = BulletState::HitWall;
            b.frame = 0;
            let owner = b.owner;
            let info = self.bullet_info(key, false);
            self.robots[owner].add_event(Event::BulletMissed { bullet: info });
        }
    }

    /// `BulletPeer.checkRobotCollision`.
    fn check_bullet_robots(&mut self, key: u64, robots: &[usize]) {
        let (owner, line) = {
            let b = self.bullet(key);
            (b.owner, b.line)
        };
        for &v in robots {
            if v == owner
                || self.robots[v].is_dead()
                || !self.robots[v]
                    .bbox
                    .intersects_line(line.0, line.1, line.2, line.3)
            {
                continue;
            }
            let power = {
                let b = self.bullet_mut(key);
                b.state = BulletState::HitVictim;
                b.frame = 0;
                b.victim = Some(v);
                b.power
            };
            let (bx, by) = (self.bullet(key).x, self.bullet(key).y);
            let mut damage = bullet_damage(power);
            if self.robots[owner].setup.sentry {
                if self.robots[v].setup.sentry {
                    damage = 0.0;
                } else {
                    let range = f64::from(self.rules.sentry_border_size);
                    if bx > range
                        && bx < (f64::from(self.rules.width) - range)
                        && by > range
                        && by < (f64::from(self.rules.height) - range)
                    {
                        damage = 0.0;
                    }
                }
            }
            let mut score = damage;
            if score > self.robots[v].energy {
                score = self.robots[v].energy;
            }
            self.update_energy(v, -damage);
            let team_fire = self.same_team(owner, v);
            let victim_sentry = self.robots[v].setup.sentry;
            let victim_name = self.robots[v].setup.name.clone();
            if !team_fire && !victim_sentry {
                self.robots[owner]
                    .stats
                    .score_bullet_damage(&victim_name, score);
            }
            if self.robots[v].energy <= 0.0 && self.robots[v].is_alive() {
                self.kill(v);
                if !team_fire && !victim_sentry {
                    let bonus = self.kill_bonus(owner, &victim_name, 0.20);
                    self.robots[owner].stats.score_bullet_kill(bonus);
                }
            }
            if !victim_sentry {
                self.update_energy(owner, bullet_hit_bonus(power));
            }
            let heading = self.bullet(key).heading;
            let bearing = normal_relative_angle(heading + PI - self.robots[v].body_heading);
            let victim_view = self.bullet_info(key, true);
            self.robots[v].add_event(Event::HitByBullet {
                bearing,
                bullet: victim_view,
            });
            let name = self.name_for_event(owner, v);
            let energy = self.robots[v].energy;
            let owner_view = self.bullet_info(key, false);
            self.robots[owner].add_event(Event::BulletHit {
                name,
                energy,
                bullet: owner_view,
            });
            let (last_x, last_y) = (self.bullet(key).last_x, self.bullet(key).last_y);
            if self.robots[v].bbox.contains(last_x, last_y) {
                let b = self.bullet_mut(key);
                b.x = last_x;
                b.y = last_y;
            }
            break;
        }
    }

    /// The kill bonus: `factor` times the damage dealt to the victim this round, by the
    /// killer or (for team robots) summed over its team.
    fn kill_bonus(&self, killer: usize, victim: &str, factor: f64) -> f64 {
        match self.robots[killer].setup.team {
            None => self.robots[killer].stats.robot_damage(victim) * factor,
            Some(team) => {
                let mut bonus = 0.0;
                for &mate in &self.teams[team] {
                    bonus += self.robots[mate].stats.robot_damage(victim) * factor;
                }
                bonus
            }
        }
    }

    /// `BulletPeer.checkBulletCollision`.
    fn check_bullet_bullets(&mut self, key: u64, bullets: &[u64]) {
        for &other in bullets {
            if other == key {
                continue;
            }
            let Some(oi) = self.bullet_index(other) else {
                continue;
            };
            let (owner, x, y) = {
                let b = self.bullet(key);
                (b.owner, b.x, b.y)
            };
            let o = &self.bullets[oi];
            if o.owner == owner || !o.state.is_active() || !self.bullet(key).intersects(o.line) {
                continue;
            }
            if self.robots[owner].setup.sentry || self.robots[o.owner].setup.sentry {
                let border = f64::from(self.rules.sentry_border_size);
                if x > border
                    && x < (f64::from(self.rules.width) - border)
                    && y > border
                    && y < (f64::from(self.rules.height) - border)
                {
                    continue;
                }
            }
            let other_owner = o.owner;
            for k in [key, other] {
                let b = self.bullet_mut(k);
                b.state = BulletState::HitBullet;
                b.frame = 0;
                b.x = b.last_x;
                b.y = b.last_y;
            }
            let (mine, theirs_hidden) =
                (self.bullet_info(key, false), self.bullet_info(other, true));
            self.robots[owner].add_event(Event::BulletHitBullet {
                bullet: mine,
                hit_bullet: theirs_hidden,
            });
            let (theirs, mine_hidden) =
                (self.bullet_info(other, false), self.bullet_info(key, true));
            self.robots[other_owner].add_event(Event::BulletHitBullet {
                bullet: theirs,
                hit_bullet: mine_hidden,
            });
            break;
        }
    }

    /// `BulletPeer.createBullet(hideOwnerName)`.
    fn bullet_info(&self, key: u64, hide_owner_name: bool) -> BulletInfo {
        let b = self.bullet(key);
        let owner = b.owner;
        let owner_name = if hide_owner_name {
            self.bullet_name_for_event(owner, owner)
        } else {
            self.robots[owner].setup.name.clone()
        };
        let victim = b.victim.map(|v| {
            if hide_owner_name {
                self.robots[v].setup.name.clone()
            } else {
                self.bullet_name_for_event(owner, v)
            }
        });
        BulletInfo {
            id: b.id,
            owner: owner_name,
            victim,
            heading: b.heading,
            x: b.x,
            y: b.y,
            power: b.power,
            active: b.state.is_active(),
        }
    }

    /// `BulletPeer.getNameForEvent`: hidden unless the robot is the bullet owner's teammate.
    fn bullet_name_for_event(&self, owner: usize, other: usize) -> String {
        if self.rules.hide_enemy_names && !self.is_team_mate(owner, other) {
            anonymous_name(other)
        } else {
            self.robots[other].setup.name.clone()
        }
    }

    /// `RobotPeer.getNameForEvent`: how `viewer` sees `other`.
    fn name_for_event(&self, viewer: usize, other: usize) -> String {
        if self.rules.hide_enemy_names && !self.is_team_mate(viewer, other) {
            anonymous_name(other)
        } else {
            self.robots[other].setup.name.clone()
        }
    }

    /// `isTeamMate`: both in the same team (a robot is its own teammate).
    fn is_team_mate(&self, a: usize, b: usize) -> bool {
        matches!(self.robots[a].setup.team, Some(t) if self.teams[t].contains(&b))
    }

    fn same_team(&self, a: usize, b: usize) -> bool {
        self.robots[a].setup.team.is_some()
            && self.robots[a].setup.team == self.robots[b].setup.team
    }

    /// `updateRobots`.
    fn update_robots(&mut self) {
        let zap = self.inactive_turn_count > self.rules.inactivity_time;
        let zap_energy = if self.aborted {
            5.0
        } else if zap {
            0.1
        } else {
            0.0
        };
        for i in self.robots_at_random() {
            let others = self.robots_at_random();
            self.perform_move(i, &others, zap_energy);
        }
        for r in &mut self.robots {
            if r.state == RobotState::HitRobot {
                r.update_bounding_box();
            }
        }
        for i in self.robots_at_random() {
            let others = self.robots_at_random();
            self.perform_scan(i, &others);
        }
    }

    /// `performMove`.
    fn perform_move(&mut self, i: usize, others: &[usize], zap_energy: f64) {
        if self.robots[i].is_dead() {
            return;
        }
        let (width, height, border, cooling) = (
            self.rules.width,
            self.rules.height,
            self.rules.sentry_border_size,
            self.rules.gun_cooling_rate,
        );
        let (last_x, last_y);
        {
            let r = &mut self.robots[i];
            r.state = RobotState::Active;
            r.gun_heat -= cooling;
            if r.gun_heat < 0.0 {
                r.gun_heat = 0.0;
            }
            r.last_heading = r.body_heading;
            r.last_gun_heading = r.gun_heading;
            r.last_radar_heading = r.radar_heading;
            last_x = r.x;
            last_y = r.y;
            if !r.in_collision {
                r.update_heading();
            }
            r.update_gun_heading();
            r.update_radar_heading();
            r.update_movement();
            r.check_wall_collision(width, height);
            if r.setup.sentry {
                r.check_sentry_outside_border(width, height, border);
            }
        }
        self.check_robot_collision(i, others);
        let r = &mut self.robots[i];
        if !r.scan {
            r.scan = r.last_heading != r.body_heading
                || r.last_gun_heading != r.gun_heading
                || r.last_radar_heading != r.radar_heading
                || last_x != r.x
                || last_y != r.y;
        }
        if r.is_dead() {
            return;
        }
        if zap_energy != 0.0 {
            self.zap(i, zap_energy);
        }
    }

    /// `RobotPeer.checkRobotCollision`.
    fn check_robot_collision(&mut self, i: usize, others: &[usize]) {
        self.robots[i].in_collision = false;
        for &o in others {
            if o == i
                || self.robots[o].is_dead()
                || !self.robots[i].bbox.intersects(&self.robots[o].bbox)
            {
                continue;
            }
            let (x, y, heading, velocity) = {
                let r = &self.robots[i];
                (r.x, r.y, r.body_heading, r.velocity)
            };
            let angle = m::atan2(self.robots[o].x - x, self.robots[o].y - y);
            let moved_x = velocity * m::sin(heading);
            let moved_y = velocity * m::cos(heading);
            let bearing = normal_relative_angle(angle - heading);
            let at_fault = (velocity > 0.0 && bearing > -PI / 2.0 && bearing < PI / 2.0)
                || (velocity < 0.0 && (bearing < -PI / 2.0 || bearing > PI / 2.0));
            if !at_fault {
                continue;
            }
            {
                let r = &mut self.robots[i];
                r.in_collision = true;
                r.velocity = 0.0;
                r.commands.distance_remaining = 0.0;
                r.x -= moved_x;
                r.y -= moved_y;
            }
            let team_fire = self.same_team(i, o);
            let other_sentry = self.robots[o].setup.sentry;
            let other_name = self.robots[o].setup.name.clone();
            if !team_fire && !other_sentry {
                self.robots[i].stats.score_ramming_damage(&other_name);
            }
            self.update_energy(i, -ROBOT_HIT_DAMAGE);
            self.update_energy(o, -ROBOT_HIT_DAMAGE);
            if self.robots[o].energy == 0.0 && self.robots[o].is_alive() {
                self.kill(o);
                if !team_fire && !other_sentry {
                    let bonus = self.kill_bonus(i, &other_name, 0.30);
                    self.robots[i].stats.score_ramming_kill(bonus);
                }
            }
            let name = self.name_for_event(i, o);
            let other_energy = self.robots[o].energy;
            self.robots[i].add_event(Event::HitRobot {
                name,
                bearing: normal_relative_angle(angle - heading),
                energy: other_energy,
                at_fault: true,
            });
            let name = self.name_for_event(o, i);
            let energy = self.robots[i].energy;
            let other_heading = self.robots[o].body_heading;
            self.robots[o].add_event(Event::HitRobot {
                name,
                bearing: normal_relative_angle(PI + angle - other_heading),
                energy,
                at_fault: false,
            });
        }
        if self.robots[i].in_collision {
            self.robots[i].state = RobotState::HitRobot;
        }
    }

    /// `performScan`.
    fn perform_scan(&mut self, i: usize, others: &[usize]) {
        if self.robots[i].is_dead() {
            return;
        }
        self.robots[i].turned_radar_with_gun = false;
        if self.robots[i].scan {
            if !self.robots[i].setup.droid {
                self.scan(i, others);
            }
            let r = &mut self.robots[i];
            r.turned_radar_with_gun =
                (r.last_gun_heading == r.last_radar_heading) && (r.gun_heading == r.radar_heading);
            r.scan = false;
        }
        self.dispatch_team_messages(i);
        let r = &mut self.robots[i];
        r.last_heading = -1.0;
        r.last_gun_heading = -1.0;
        r.last_radar_heading = -1.0;
    }

    /// Team messages from this turn's commands go to the matching teammates
    /// (`performScan`, engine-spec §7).
    fn dispatch_team_messages(&mut self, i: usize) {
        let Some(team) = self.robots[i].setup.team else {
            return;
        };
        if !self.robots[i].setup.team_robot {
            return;
        }
        let messages = self.robots[i].commands.messages.clone();
        for message in messages {
            for member in self.teams[team].clone() {
                if self.dispatch_to(i, member, message.recipient.as_deref()) {
                    self.robots[member].team_inbox.push(message.clone());
                }
            }
        }
    }

    /// `checkDispatchToMember`: alive, and either a broadcast to others or the
    /// recipient is a prefix of the member's name or full class name.
    fn dispatch_to(&self, sender: usize, member: usize, recipient: Option<&str>) -> bool {
        let m = &self.robots[member];
        if !m.is_alive() {
            return false;
        }
        match recipient {
            None => member != sender,
            Some(r) => m.setup.name.starts_with(r) || m.setup.full_class_name.starts_with(r),
        }
    }

    /// `RobotPeer.scan` (engine-spec §7).
    fn scan(&mut self, i: usize, others: &[usize]) {
        let (x, y, radar, last_radar, body) = {
            let r = &self.robots[i];
            (
                r.x,
                r.y,
                r.radar_heading,
                r.last_radar_heading,
                r.body_heading,
            )
        };
        let mut start_angle = last_radar;
        let mut scan_radians = radar - start_angle;
        if scan_radians < -PI {
            scan_radians += 2.0 * PI;
        } else if scan_radians > PI {
            scan_radians -= 2.0 * PI;
        }
        start_angle -= PI / 2.0;
        start_angle = normal_absolute_angle(start_angle);
        let arc = Arc {
            x: x - RADAR_SCAN_RADIUS,
            y: y - RADAR_SCAN_RADIUS,
            w: 2.0 * RADAR_SCAN_RADIUS,
            h: 2.0 * RADAR_SCAN_RADIUS,
            start: 180.0 * start_angle / PI,
            extent: 180.0 * scan_radians / PI,
        };
        for &o in others {
            if o == i || self.robots[o].is_dead() || !scan_intersects(&arc, &self.robots[o].bbox) {
                continue;
            }
            let other = &self.robots[o];
            let dx = other.x - x;
            let dy = other.y - y;
            let angle = m::atan2(dx, dy);
            let distance = m::hypot(dx, dy);
            let event = Event::ScannedRobot {
                name: self.name_for_event(i, o),
                energy: other.energy,
                bearing: normal_relative_angle(angle - body),
                distance,
                heading: other.body_heading,
                velocity: other.velocity,
                sentry: other.setup.sentry,
            };
            self.robots[i].add_event(event);
        }
    }

    /// `zap`: inactivity energy loss.
    fn zap(&mut self, i: usize, amount: f64) {
        if self.robots[i].energy == 0.0 {
            self.kill(i);
            return;
        }
        let r = &mut self.robots[i];
        r.energy -= amount.abs();
        if r.energy < 0.1 {
            r.energy = 0.0;
            r.commands.distance_remaining = 0.0;
            r.commands.body_turn_remaining = 0.0;
        }
    }

    /// `updateEnergy`.
    fn update_energy(&mut self, i: usize, delta: f64) {
        let r = &self.robots[i];
        if (!r.exec_finished_and_disabled && !r.energy_drained) || delta < 0.0 {
            let energy = r.energy + delta;
            self.set_energy(i, energy);
        }
    }

    /// `setEnergy(e, true)`.
    fn set_energy(&mut self, i: usize, energy: f64) {
        let old = self.robots[i].energy;
        if old != energy {
            self.reset_inactive_turn_count(old - energy);
        }
        self.robots[i].set_energy_raw(energy);
    }

    /// `resetInactiveTurnCount`: every 10 energy lost resets the inactivity counter.
    fn reset_inactive_turn_count(&mut self, energy_loss: f64) {
        if energy_loss < 0.0 {
            return;
        }
        self.inactivity_energy += energy_loss;
        while self.inactivity_energy >= 10.0 {
            self.inactivity_energy -= 10.0;
            self.inactive_turn_count = 0;
        }
    }

    /// `kill` (engine-spec §9.4).
    fn kill(&mut self, i: usize) {
        self.reset_inactive_turn_count(10.0);
        if self.robots[i].is_alive() {
            self.robots[i].add_event(Event::Death);
            if self.robots[i].setup.leader {
                if let Some(team) = self.robots[i].setup.team {
                    for mate in self.teams[team].clone() {
                        if self.robots[mate].is_alive() && mate != i {
                            self.update_energy(mate, -30.0);
                            let key = self.new_key();
                            let (mx, my) = (self.robots[mate].x, self.robots[mate].y);
                            self.bullets.push(Bullet {
                                key,
                                kind: BulletKind::Bullet,
                                owner: i,
                                id: 0,
                                state: BulletState::HitVictim,
                                heading: 0.0,
                                x: mx,
                                y: my,
                                last_x: mx,
                                last_y: my,
                                power: 4.0,
                                victim: Some(mate),
                                frame: 0,
                                line: (0.0, 0.0, 0.0, 0.0),
                            });
                        }
                    }
                }
            }
            self.death_robots.push(i);
            let key = self.new_key();
            let (x, y) = (self.robots[i].x, self.robots[i].y);
            self.bullets.push(Bullet {
                key,
                kind: BulletKind::Explosion,
                owner: i,
                id: 0,
                state: BulletState::Exploded,
                heading: 0.0,
                x,
                y,
                last_x: 0.0,
                last_y: 0.0,
                power: 1.0,
                victim: Some(i),
                frame: 0,
                line: (0.0, 0.0, 0.0, 0.0),
            });
        }
        let energy = self.robots[i].energy;
        self.update_energy(i, -energy);
        self.robots[i].state = RobotState::Dead;
    }

    /// `handleDeadRobots`.
    fn handle_dead_robots(&mut self) {
        let mut dead = self.death_robots.clone();
        shuffle(&mut dead, &mut self.random);
        for d in dead {
            let scores = match self.robots[d].setup.team {
                None => true,
                Some(team) => !self.teams[team].iter().any(|&m| self.robots[m].is_alive()),
            };
            if scores {
                let remaining = self.active_contestant_count(d);
                let r = &mut self.robots[d];
                r.stats
                    .score_robot_death(r.setup.sentry, r.is_winner, remaining);
            }
            for r in self.robots_at_random() {
                if self.robots[r].is_alive() {
                    let name = self.name_for_event(r, d);
                    self.robots[r].add_event(Event::RobotDeath { name });
                    let other_team = self.robots[r].setup.team.is_none()
                        || self.robots[r].setup.team != self.robots[d].setup.team;
                    if other_team {
                        let sentry = self.robots[r].setup.sentry;
                        self.robots[r].stats.score_survival(sentry);
                    }
                }
            }
        }
        self.death_robots.clear();
    }

    /// `getActiveContestantCount`.
    fn active_contestant_count(&self, peer: usize) -> i32 {
        let mut count = 0;
        for c in &self.contestants {
            match *c {
                Contestant::Robot(r) => {
                    if !self.robots[r].setup.sentry && self.robots[r].is_alive() {
                        count += 1;
                    }
                }
                Contestant::Team(t) => {
                    if Some(t) != self.robots[peer].setup.team
                        && self.teams[t]
                            .iter()
                            .any(|&m| !self.robots[m].setup.sentry && self.robots[m].is_alive())
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn compute_active_robots(&mut self) {
        self.active_participants = 0;
        self.active_sentries = 0;
        for r in &self.robots {
            if r.is_alive() {
                if r.setup.sentry {
                    self.active_sentries += 1;
                } else {
                    self.active_participants += 1;
                }
            }
        }
    }

    /// `oneTeamRemaining` (alive sentries take part, engine-spec §4.5).
    fn one_team_remaining(&self) -> bool {
        if self.active_participants <= 1 {
            return true;
        }
        let mut found = false;
        let mut current: Option<usize> = None;
        for r in &self.robots {
            if r.is_alive() {
                if !found {
                    found = true;
                    current = r.setup.team;
                } else {
                    if current.is_none() && r.setup.team.is_none() {
                        return false;
                    }
                    if current != r.setup.team {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// `shutdownTurn`.
    fn shutdown_turn(&mut self) {
        if self.end_timer == 0 {
            if self.aborted {
                self.robots_at_random();
            } else if self.one_team_remaining() {
                let mut leader_firsts = false;
                let mut winning_team = None;
                let round_ended = Event::RoundEnded {
                    round: self.round,
                    turns: self.time,
                    total_turns: self.total_turns,
                };
                let count = self.robots.len() as i32;
                for i in self.robots_at_random() {
                    self.robots[i].add_event(round_ended.clone());
                    let r = &self.robots[i];
                    if r.is_alive() && !r.is_winner && !r.setup.sentry {
                        let team_size = r.setup.team.map_or(0, |t| self.teams[t].len() as i32);
                        let enemies = if r.setup.team.is_some() {
                            count - 1 - (team_size - 1)
                        } else {
                            count - 1
                        };
                        let counts_first = r.setup.team.is_none() || r.setup.leader;
                        let (sentry, leader, team) = (r.setup.sentry, r.setup.leader, r.setup.team);
                        let r = &mut self.robots[i];
                        r.stats.score_last_survivor(sentry, enemies, counts_first);
                        r.is_winner = true;
                        r.add_event(Event::Win);
                        if team.is_some() {
                            if leader {
                                leader_firsts = true;
                            } else {
                                winning_team = team;
                            }
                        }
                    }
                    let sentry = self.robots[i].setup.sentry;
                    self.robots[i].stats.generate_totals(sentry);
                }
                if !leader_firsts {
                    if let Some(team) = winning_team {
                        let leader = self.teams[team][0];
                        let sentry = self.robots[leader].setup.sentry;
                        self.robots[leader].stats.score_firsts(sentry);
                    }
                }
            }
            if self.aborted || self.is_last_round() {
                let mut ordered: Vec<usize> = (0..self.robots.len()).collect();
                ordered.sort_by(|&a, &b| self.compare(a, b));
                ordered.reverse();
                for (rank, &i) in ordered.iter().enumerate() {
                    self.robots[i].stats.rank = rank as i32 + 1;
                    let results = self.robots[i].stats.final_results();
                    let aborted = self.aborted;
                    self.robots[i].add_event(Event::BattleEnded { aborted, results });
                }
            }
            if self.aborted {
                // Not in classic, which reports no results for an aborted battle: the
                // results so far, with the aborted round up to here, so they match the
                // ranks just given (`compare` counted the round's scores too). Robots that
                // die of the abort's energy loss later don't change them.
                for r in &mut self.robots {
                    let sentry = r.setup.sentry;
                    r.stats.generate_totals(sentry);
                }
                let results = self.robots.iter().map(|r| r.stats.final_results());
                self.results_at_abort = Some(results.collect());
            }
        }
        if self.end_timer > 4 * TURNS_DISPLAYED_AFTER_ENDING {
            for r in &mut self.robots {
                r.halted = true;
            }
        }
        self.end_timer += 1;
    }

    /// `RobotPeer.compareTo`: by total score, plus the current score while in a round
    /// (classic uses the first robot's in-round flag for both).
    fn compare(&self, a: usize, b: usize) -> std::cmp::Ordering {
        let (ra, rb) = (&self.robots[a], &self.robots[b]);
        let mut mine = ra.total_score();
        let mut his = rb.total_score();
        if ra.stats.is_in_round {
            mine += ra.current_score();
            his += rb.current_score();
        }
        if mine < his {
            std::cmp::Ordering::Less
        } else if mine > his {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }

    /// `wakeupRobots`: robots execute in a shuffled order. A robot that is dead, halted or
    /// in an aborted battle finishes its thread on that execute.
    fn wakeup_robots(&mut self) {
        let order = self.robots_at_random();
        let aborted = self.aborted;
        for i in order {
            let r = &mut self.robots[i];
            if r.running && (r.is_dead() || r.halted || aborted) {
                r.exec_finished_and_disabled = true;
                r.running = false;
            }
        }
    }
}

fn anonymous_name(index: usize) -> String {
    format!("#{index}")
}
