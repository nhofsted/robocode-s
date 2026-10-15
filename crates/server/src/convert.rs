// SPDX-License-Identifier: EPL-2.0

//! Conversions between engine types and protocol messages.

use robocode_s_engine as engine;
use robocode_s_protocol::v1;

/// The engine's view of a robot, from the statics sent to it.
pub fn robot_setup(statics: &v1::RobotStatics) -> engine::RobotSetup {
    let t = statics.r#type.unwrap_or_default();
    engine::RobotSetup {
        name: statics.name.clone(),
        full_class_name: statics.full_class_name.clone(),
        advanced: t.advanced,
        team_robot: t.team,
        droid: t.droid,
        sentry: t.sentry,
        leader: statics.team_leader,
        team: usize::try_from(statics.team_index).ok(),
    }
}

pub fn rules(rules: &engine::BattleRules, deterministic: bool) -> v1::BattleRules {
    v1::BattleRules {
        battlefield_width: rules.width,
        battlefield_height: rules.height,
        num_rounds: rules.num_rounds,
        gun_cooling_rate: Some(rules.gun_cooling_rate),
        inactivity_time: rules.inactivity_time,
        hide_enemy_names: rules.hide_enemy_names,
        sentry_border_size: rules.sentry_border_size,
        deterministic,
    }
}

/// The engine's rules from the protocol's (the reverse of [`rules`]).
pub fn rules_in(r: &v1::BattleRules) -> engine::BattleRules {
    engine::BattleRules {
        width: r.battlefield_width,
        height: r.battlefield_height,
        num_rounds: r.num_rounds,
        gun_cooling_rate: r.gun_cooling_rate.unwrap_or(0.1),
        inactivity_time: r.inactivity_time,
        hide_enemy_names: r.hide_enemy_names,
        sentry_border_size: r.sentry_border_size,
    }
}

/// Parts of the command state the engine doesn't use but robots and observers do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Passthrough {
    pub colors: Option<v1::Colors>,
    pub adjust_radar_for_body_turn_set: bool,
}

/// A robot's command state from `Execute`.
pub fn commands(c: &v1::RobotCommands) -> (engine::Commands, Passthrough) {
    let commands = engine::Commands {
        body_turn_remaining: c.body_turn_remaining.unwrap_or_default(),
        gun_turn_remaining: c.gun_turn_remaining.unwrap_or_default(),
        radar_turn_remaining: c.radar_turn_remaining.unwrap_or_default(),
        distance_remaining: c.distance_remaining.unwrap_or_default(),
        adjust_gun_for_body_turn: c.adjust_gun_for_body_turn,
        adjust_radar_for_gun_turn: c.adjust_radar_for_gun_turn,
        adjust_radar_for_body_turn: c.adjust_radar_for_body_turn,
        max_turn_rate: c.max_turn_rate.unwrap_or_default(),
        max_velocity: c.max_velocity.unwrap_or_default(),
        scan: c.scan,
        io: c.io_robot,
        bullets: c
            .bullets
            .iter()
            .map(|b| engine::BulletCommand {
                id: b.bullet_id,
                power: b.power.unwrap_or_default(),
                assist_valid: b.fire_assist_valid,
                assist_angle: b.fire_assist_angle.unwrap_or_default(),
                consumed: false,
            })
            .collect(),
        messages: c.team_messages.iter().map(team_message_in).collect(),
    };
    let passthrough = Passthrough {
        colors: c.colors,
        adjust_radar_for_body_turn_set: c.adjust_radar_for_body_turn_set,
    };
    (commands, passthrough)
}

/// The server's copy of the command state, as returned in `Tick` (no one-shot parts).
pub fn commands_copy(c: &engine::Commands, passthrough: &Passthrough) -> v1::RobotCommands {
    v1::RobotCommands {
        body_turn_remaining: Some(c.body_turn_remaining),
        gun_turn_remaining: Some(c.gun_turn_remaining),
        radar_turn_remaining: Some(c.radar_turn_remaining),
        distance_remaining: Some(c.distance_remaining),
        adjust_gun_for_body_turn: c.adjust_gun_for_body_turn,
        adjust_radar_for_gun_turn: c.adjust_radar_for_gun_turn,
        adjust_radar_for_body_turn: c.adjust_radar_for_body_turn,
        adjust_radar_for_body_turn_set: passthrough.adjust_radar_for_body_turn_set,
        max_turn_rate: Some(c.max_turn_rate),
        max_velocity: Some(c.max_velocity),
        colors: passthrough.colors,
        ..Default::default()
    }
}

fn team_message_in(m: &v1::TeamMessage) -> engine::TeamMessage {
    engine::TeamMessage {
        sender: m.sender.clone(),
        recipient: m.recipient.clone(),
        bytes: m.payload.clone(),
    }
}

pub fn team_message(m: &engine::TeamMessage) -> v1::TeamMessage {
    v1::TeamMessage {
        sender: m.sender.clone(),
        recipient: m.recipient.clone(),
        payload: m.bytes.clone(),
    }
}

pub fn status(s: &engine::RobotStatus) -> v1::RobotStatus {
    v1::RobotStatus {
        energy: Some(s.energy),
        x: Some(s.x),
        y: Some(s.y),
        body_heading: Some(s.body_heading),
        gun_heading: Some(s.gun_heading),
        radar_heading: Some(s.radar_heading),
        velocity: Some(s.velocity),
        body_turn_remaining: Some(s.body_turn_remaining),
        radar_turn_remaining: Some(s.radar_turn_remaining),
        gun_turn_remaining: Some(s.gun_turn_remaining),
        distance_remaining: Some(s.distance_remaining),
        gun_heat: Some(s.gun_heat),
        others: s.others,
        num_sentries: s.num_sentries,
        round: s.round,
        num_rounds: s.num_rounds,
        time: s.time,
    }
}

pub fn bullet_status(b: &engine::BulletStatus) -> v1::BulletStatus {
    v1::BulletStatus {
        bullet_id: b.bullet_id,
        x: Some(b.x),
        y: Some(b.y),
        victim: b.victim.clone(),
        active: b.active,
    }
}

fn bullet(b: &engine::BulletInfo) -> v1::Bullet {
    v1::Bullet {
        bullet_id: b.id,
        owner: b.owner.clone(),
        victim: b.victim.clone(),
        heading: Some(b.heading),
        x: Some(b.x),
        y: Some(b.y),
        power: Some(b.power),
        active: b.active,
    }
}

pub fn results(name: &str, r: &engine::Results) -> v1::Results {
    v1::Results {
        name: name.to_string(),
        rank: r.rank,
        score: Some(r.score),
        survival: Some(r.survival),
        last_survivor_bonus: Some(r.last_survivor_bonus),
        bullet_damage: Some(r.bullet_damage),
        bullet_damage_bonus: Some(r.bullet_damage_bonus),
        ram_damage: Some(r.ram_damage),
        ram_damage_bonus: Some(r.ram_damage_bonus),
        firsts: r.firsts,
        seconds: r.seconds,
        thirds: r.thirds,
    }
}

/// Whether classic keeps this event when a robot skipped turns (`isCriticalEvent`).
pub fn is_critical(e: &engine::Event) -> bool {
    matches!(
        e,
        engine::Event::Death
            | engine::Event::Win
            | engine::Event::SkippedTurn { .. }
            | engine::Event::RoundEnded { .. }
            | engine::Event::BattleEnded { .. }
    )
}

pub fn event(e: &engine::Event, robot_name: &str) -> v1::Event {
    use engine::Event as E;
    use v1::event::Kind;
    let kind = match e {
        E::ScannedRobot {
            name,
            energy,
            bearing,
            distance,
            heading,
            velocity,
            sentry,
        } => Kind::ScannedRobot(v1::ScannedRobot {
            name: name.clone(),
            energy: Some(*energy),
            bearing: Some(*bearing),
            distance: Some(*distance),
            heading: Some(*heading),
            velocity: Some(*velocity),
            sentry: *sentry,
        }),
        E::HitWall { bearing } => Kind::HitWall(v1::HitWall {
            bearing: Some(*bearing),
        }),
        E::HitRobot {
            name,
            bearing,
            energy,
            at_fault,
        } => Kind::HitRobot(v1::HitRobot {
            name: name.clone(),
            bearing: Some(*bearing),
            energy: Some(*energy),
            at_fault: *at_fault,
        }),
        E::HitByBullet { bearing, bullet: b } => Kind::HitByBullet(v1::HitByBullet {
            bearing: Some(*bearing),
            bullet: Some(bullet(b)),
        }),
        E::BulletHit {
            name,
            energy,
            bullet: b,
        } => Kind::BulletHit(v1::BulletHit {
            name: name.clone(),
            energy: Some(*energy),
            bullet: Some(bullet(b)),
        }),
        E::BulletHitBullet {
            bullet: b,
            hit_bullet,
        } => Kind::BulletHitBullet(v1::BulletHitBullet {
            bullet: Some(bullet(b)),
            hit_bullet: Some(bullet(hit_bullet)),
        }),
        E::BulletMissed { bullet: b } => Kind::BulletMissed(v1::BulletMissed {
            bullet: Some(bullet(b)),
        }),
        E::RobotDeath { name } => Kind::RobotDeath(v1::RobotDeath { name: name.clone() }),
        E::Death => Kind::Death(v1::Death {}),
        E::Win => Kind::Win(v1::Win {}),
        E::RoundEnded {
            round,
            turns,
            total_turns,
        } => Kind::RoundEnded(v1::RoundEnded {
            round: *round,
            turns: *turns,
            total_turns: *total_turns,
        }),
        E::BattleEnded {
            aborted,
            results: r,
        } => Kind::BattleEnded(v1::BattleEnded {
            aborted: *aborted,
            results: Some(results(robot_name, r)),
        }),
        E::SkippedTurn { turn } => Kind::SkippedTurn(v1::SkippedTurn { turn: *turn }),
    };
    v1::Event { kind: Some(kind) }
}

pub fn robot_snapshot(
    r: &engine::Robot,
    passthrough: &Passthrough,
    output: String,
) -> v1::RobotSnapshot {
    let state = match r.state {
        engine::RobotState::Active => v1::RobotState::Active,
        engine::RobotState::HitWall => v1::RobotState::HitWall,
        engine::RobotState::HitRobot => v1::RobotState::HitRobot,
        engine::RobotState::Dead => v1::RobotState::Dead,
    };
    v1::RobotSnapshot {
        state: state as i32,
        x: Some(r.x),
        y: Some(r.y),
        energy: Some(r.energy),
        velocity: Some(r.velocity),
        body_heading: Some(r.body_heading),
        gun_heading: Some(r.gun_heading),
        radar_heading: Some(r.radar_heading),
        gun_heat: Some(r.gun_heat),
        score: Some(r.current_score()),
        total_score: Some(r.total_score()),
        colors: passthrough.colors,
        output,
        paint: Vec::new(),
        cpu_nanos: 0,
        budget_nanos: 0,
    }
}

pub fn bullet_snapshot(b: &engine::Bullet) -> v1::BulletSnapshot {
    let state = match b.state {
        engine::BulletState::Fired => v1::BulletState::Fired,
        engine::BulletState::Moving => v1::BulletState::Moving,
        engine::BulletState::HitVictim => v1::BulletState::HitVictim,
        engine::BulletState::HitBullet => v1::BulletState::HitBullet,
        engine::BulletState::HitWall => v1::BulletState::HitWall,
        engine::BulletState::Exploded => v1::BulletState::Exploded,
        engine::BulletState::Inactive => v1::BulletState::Inactive,
    };
    v1::BulletSnapshot {
        owner: b.owner as i32,
        bullet_id: b.id,
        state: state as i32,
        x: Some(b.x),
        y: Some(b.y),
        heading: Some(b.heading),
        power: Some(b.power),
        victim: b.victim.map_or(-1, |v| v as i32),
        explosion: b.kind == engine::BulletKind::Explosion,
        frame: b.frame,
        color: 0,
    }
}
