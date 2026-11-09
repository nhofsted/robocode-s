// SPDX-License-Identifier: EPL-2.0

//! `ENGINE_VERSION` (design §4.4) changes with every change to `DETERMINISTIC_RULES`, but
//! for the owner's exceptions: rules 2 (ADR 0025) kept engine version 1, as robots without
//! threads of their own give byte-identical battles and those with threads are no-fault in
//! the rumble (ADR 0023); rules 3 (ADR 0026, new work prices) kept it too: the rankings stay,
//! and the battles the new prices change are purged and fought again (rumble-backend §12.4).
//! Rules 4 (ADR 0028, the round start's allowance) keeps it for now: it changes only battles
//! of robots that work for more than 756 ms before a round's first execute. To be confirmed
//! by the owner.

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_server::DETERMINISTIC_RULES;

#[test]
fn engine_version_follows_the_deterministic_rules() {
    assert_eq!(
        (ENGINE_VERSION, DETERMINISTIC_RULES),
        (1, 4),
        "Bump ENGINE_VERSION (crates/engine/src/lib.rs) whenever DETERMINISTIC_RULES changes \
         (unless the owner decides otherwise, as for rules 2 to 4), then update this pin."
    );
}
