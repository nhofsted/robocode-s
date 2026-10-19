// SPDX-License-Identifier: EPL-2.0

//! `ENGINE_VERSION` (design §4.4) changes with every change to `DETERMINISTIC_RULES`.

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_server::DETERMINISTIC_RULES;

#[test]
fn engine_version_follows_the_deterministic_rules() {
    assert_eq!(
        (ENGINE_VERSION, DETERMINISTIC_RULES),
        (1, 1),
        "Bump ENGINE_VERSION (crates/engine/src/lib.rs) whenever DETERMINISTIC_RULES changes, \
         then update this pin."
    );
}
