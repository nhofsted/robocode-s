// SPDX-License-Identifier: EPL-2.0

//! Hidden enemy names in bullet events, as classic 1.12.0 has them: a bullet that another
//! robot sees (`HitByBulletEvent`, the hit bullet of `BulletHitBulletEvent`) names its owner
//! as that robot sees the owner. Before 1.12.0 an enemy saw a team robot's real name there.

use robocode_s_engine::{
    Battle, BattleRules, BulletCommand, BulletInfo, Commands, Event, RobotSetup,
};

fn setup(name: &str, team: usize) -> RobotSetup {
    RobotSetup {
        name: name.to_string(),
        full_class_name: name.to_string(),
        advanced: true,
        team_robot: true,
        droid: false,
        sentry: false,
        leader: false,
        team: Some(team),
    }
}

/// Classic's `normalRelativeAngle`, enough for aiming.
fn relative(angle: f64) -> f64 {
    let mut a = angle % std::f64::consts::TAU;
    if a > std::f64::consts::PI {
        a -= std::f64::consts::TAU;
    } else if a < -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    }
    a
}

#[test]
fn enemies_see_a_team_robots_bullets_without_its_name() {
    let names = ["a.One", "a.Two", "b.One", "b.Two"];
    let teams = [0, 0, 1, 1];
    let setups = names.iter().zip(teams).map(|(n, t)| setup(n, t)).collect();
    let rules = BattleRules {
        num_rounds: 3,
        hide_enemy_names: true,
        ..BattleRules::default()
    };
    let mut battle = Battle::new(rules, setups, 20261121);
    // Who sees `owner` how: the real name for a teammate, `#index` for an enemy.
    let seen = |viewer: usize, owner: &str| -> String {
        let o = names.iter().position(|n| *n == owner).expect("a robot");
        if teams[viewer] == teams[o] {
            owner.to_string()
        } else {
            format!("#{o}")
        }
    };
    let mut checked = 0;
    let mut bullet_ids = [0i32; 4];
    while battle.round < 3 {
        battle.start_round();
        let mut turns = 0;
        while !battle.is_round_over() && turns < 2000 {
            turns += 1;
            for (i, bullet_id) in bullet_ids.iter_mut().enumerate() {
                if !battle.robots[i].is_alive() {
                    continue;
                }
                // Each robot aims at the nearest living enemy and fires when it can.
                let me = &battle.robots[i];
                let target = (0..4)
                    .filter(|&j| teams[j] != teams[i] && battle.robots[j].is_alive())
                    .min_by(|&a, &b| {
                        let d =
                            |j: usize| (battle.robots[j].x - me.x).hypot(battle.robots[j].y - me.y);
                        d(a).total_cmp(&d(b))
                    });
                let Some(t) = target else { continue };
                let (dx, dy) = (battle.robots[t].x - me.x, battle.robots[t].y - me.y);
                let mut commands = Commands {
                    gun_turn_remaining: relative(dx.atan2(dy) - me.gun_heading),
                    ..Commands::default()
                };
                if me.gun_heat == 0.0 {
                    *bullet_id += 1;
                    commands.bullets.push(BulletCommand {
                        id: *bullet_id,
                        power: 1.0,
                        assist_valid: false,
                        assist_angle: 0.0,
                        consumed: false,
                    });
                }
                battle.set_commands(i, commands);
            }
            battle.step();
            for viewer in 0..4 {
                for event in battle.take_events(viewer) {
                    let other: Option<&BulletInfo> = match &event {
                        Event::HitByBullet { bullet, .. } => Some(bullet),
                        Event::BulletHitBullet { hit_bullet, .. } => Some(hit_bullet),
                        _ => None,
                    };
                    if let Some(b) = other {
                        assert_eq!(b.owner, seen(viewer, &names_owner(&b.owner, &names)));
                        checked += 1;
                    }
                }
                battle.take_bullet_updates(viewer);
            }
        }
        battle.finish_round();
    }
    assert!(checked > 0, "no robot was hit");
}

/// The real name behind a name in an event: an anonymous `#index` or a real name.
fn names_owner(name: &str, names: &[&str]) -> String {
    match name.strip_prefix('#') {
        Some(i) => names[i.parse::<usize>().expect("an index")].to_string(),
        None => name.to_string(),
    }
}
