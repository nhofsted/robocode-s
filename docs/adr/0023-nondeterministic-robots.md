# 0023. Robots that aren't reproducible are flagged, and their battles never penalize anyone

- Status: Proposed
- Date: 2026-10-27

## Context

Rumble verification (docs/rumble-backend.md §11) assumes every battle is deterministic: a verifier re-runs a battle, the hashes are compared, a third run breaks a tie and the minority of two against one is penalized (reputation 0, quarantine, a mismatch counted, a ban at three in 90 days). ADR 0014 and 0018 already say that robots running threads of their own (helper threads, `Thread.sleep` loops) are not deterministic. On 2026-10-27 prod showed what that does: 34 three-way alerts (30 with `florent.test.Toad`; `ak.Fermat`, `TNT.TNT`, `rapture.Rapture` and `techdude.forest.FlamingForest` were also reproduced locally as nondeterministic), and two innocent volunteers penalized in one tie-break batch. Majority outcomes were logged nowhere and resolved items are deleted, so the battles behind the penalties could not even be found.

## Decision

- **A flag per bot version** (`nd` on the version item, with a reason). It is set by an admin, automatically when a results post says the robot started threads (`RobotResult.threads`, filled in by the robot host and client), and automatically for the bot that stands out in repeated three-way disagreements (`rep.ndDisagreements`, default 3; at least that many and more than every other bot of the same item, so a deterministic opponent is not flagged with it). It is never lifted automatically.
- **No fault.** An item with a flagged bot never credits or penalizes anyone: it is not queued, it is settled instead of served, and if a verifier had claimed it before the flag it resolves as `NoFault` (deleted, held record released). A quarantined user's battles with a flagged bot are released at once, as the item can no longer prove them.
- **Every penalty is logged**: a `penalty` alert (or `correction` where nobody is penalized) with the item, division, seed, bots, users, hashes, who was outvoted and what followed, plus a `PENALTY` log line. Three-way alerts carry division, seed and bots, and count towards the flag.
- **A repair command** (`admin repair`, dry run unless `--apply`) over the existing admin user API, extended with `mismatches` and `clearMismatchTimes`.

## Consequences

- A flagged robot's battles still count in the rankings, but only on trust: they are never checked. That is the accepted cost; the alternative, excluding the robot from the rumble, is the owner's call and not made here.
- A nondeterministic robot that reports no threads and is not yet in three disagreements can still cost a volunteer one mismatch before it is flagged (a ban needs three in 90 days). The repair command undoes such cases.
- Penalties from before this change are only recoverable from the alerts that exist (`engine-mismatch`) and from hand analysis; the repair command does not know a user's reputation from before and leaves it unless given.
- The planner writes version items whole and can overwrite a flag it read just before; the next signal sets it again. A conditional per-attribute write in the planner would remove that and was not worth the change.
