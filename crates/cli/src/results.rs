// SPDX-License-Identifier: EPL-2.0

//! The results table, as classic shows it, and as JSON.

pub use robocode_s_server::results::{Contestant, contestants, table};

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
