# robocode-s — Engine Specification

Status: **Draft** · Reference: classic Robocode **1.11.1** (tag `v1.11.1`) · Last updated: 2026-10-10

This document specifies the battle engine precisely enough to reimplement it bit-exactly (ADR 0003). It was written from the classic sources. Source references use the form `File.java:line` and point to 1.11.1:
- `Battle`, `BaseBattle`: `robocode.battle/src/main/java/net/sf/robocode/battle/`
- `RobotPeer`, `BulletPeer`, `ExplosionPeer`, `RobotStatistics`, `TeamStatistics`: the `peer/` subpackage there
- `Rules`, `Utils`, `Event`: `robocode.api/src/main/java/robocode/`

Where classic behavior is odd but observable, this spec keeps it and marks it **(quirk)**. Places where robocode-s deliberately deviates are collected in §15.

---

## 1. Conventions

### 1.1 Coordinates and angles

- The battlefield is `width × height` (integers, default 800×600). The origin is bottom-left, **y points up**.
- Headings are in **radians**, 0 = north (+y), increasing **clockwise** (π/2 = east). A heading `h` moves a robot by `(v·sin h, v·cos h)`.
- Robots are **36×36** squares centered on `(x, y)` and never rotate their bounding box.

### 1.2 Numerics

- All state is IEEE-754 `double` unless stated otherwise. Operations are evaluated **exactly in the order written here**, which is the Java source order (left to right, no fused multiply-add). Where the order is not obvious, it is spelled out with parentheses.
- Transcendental functions are `StrictMath` (fdlibm 5.3; `crates/fdlibm`): `sin`, `cos`, `tan`, `atan2`, `hypot`. `sqrt`, `ceil`, `abs`, `min` and `max` are IEEE-exact.
- `a % b` on doubles is Java's remainder: truncated, with the sign of the dividend. This is the same as Rust's `%` on `f64`.
- **Robot bounding boxes are stored in `float`** (§9.1). This is not a mistake in this spec: classic uses `Rectangle2D.Float`.
- `toRadians(d) = d * 0.017453292519943295` and `toDegrees(r) = r * 57.29577951308232` (JDK 9+ semantics).

### 1.3 Angle helpers (`Utils`)

Let `TWO_PI = 2*PI`, `PI_OVER_TWO = PI/2`, `THREE_PI_OVER_TWO = 3*PI/2`, `NEAR_DELTA = 0.00001`.

| Function | Definition |
|---|---|
| `normalAbsoluteAngle(a)` | `a' = a % TWO_PI`; result `a'` if `a' >= 0`, else `a' + TWO_PI` |
| `normalRelativeAngle(a)` | `a' = a % TWO_PI`; if `a' >= 0`: `a' < PI ? a' : a' - TWO_PI`; else `a' >= -PI ? a' : a' + TWO_PI` |
| `isNear(a, b)` | `abs(a - b) < NEAR_DELTA` |
| `normalNearAbsoluteAngle(a)` | `a' = normalAbsoluteAngle(a)`. Then: near `PI` → `PI`; else if `a' < PI`: near 0 → 0, near `PI_OVER_TWO` → `PI_OVER_TWO`; else near `THREE_PI_OVER_TWO` → that value, near `TWO_PI` → 0. Otherwise `a'`. Checks happen in exactly this order. |

---

## 2. Constants (`Rules`)

| Name | Value |
|---|---|
| `ACCELERATION` | 1 |
| `DECELERATION` | 2 |
| `MAX_VELOCITY` | 8 |
| `RADAR_SCAN_RADIUS` | 1200 |
| `MIN_BULLET_POWER` / `MAX_BULLET_POWER` | 0.1 / 3 |
| `MAX_TURN_RATE_RADIANS` | `toRadians(10)` |
| `GUN_TURN_RATE_RADIANS` | `toRadians(20)` |
| `RADAR_TURN_RATE_RADIANS` | `toRadians(45)` |
| `ROBOT_HIT_DAMAGE` / `ROBOT_HIT_BONUS` | 0.6 / `2 * 0.6` |
| Robot size | 36 × 36 (half: 18) |
| Bullet radius (wall check) | 3 |
| Bullet explosion length | 17 frames; robot explosion 71 frames |
| `MAX_SKIPPED_TURNS` | 30 (240 for robots that did file I/O) |
| Event queue limit | 256 (`EventManager.MAX_QUEUE_SIZE`); `MAX_EVENT_STACK` = 2 |
| Rounds end | `TURNS_DISPLAYED_AFTER_ENDING` = 30 (§4.4) |

Formulas:

| Formula | Definition |
|---|---|
| `getBulletDamage(p)` | `d = 4*p`; if `p > 1`: `d += 2*(p - 1)` |
| `getBulletHitBonus(p)` | `3*p` |
| `getBulletSpeed(p)` | `20 - 3*min(max(p, 0.1), 3)` |
| `getGunHeat(p)` | `1 + (p / 5)` |
| `getWallHitDamage(v)` | `max(abs(v)/2 - 1, 0)` |

Battle rules: `numRounds`, `gunCoolingRate` (default 0.1), `inactivityTime` (default 450), `hideEnemyNames` (default false), `sentryBorderSize` (default 100).

---

## 3. Randomness

All engine randomness comes from **one** `java.util.Random` instance (`RandomFactory.getRandom()`), reset with `RANDOMSEED` when the battle starts. Port: `crates/java-random`.

**The exact sequence of draws is part of the specification.** Most draws come from `Collections.shuffle`. Shuffling a list of `n` elements calls `nextInt(i)` for `i = n, n-1, …, 2` (none for `n ≤ 1`). The size of each shuffled list therefore matters, including dead robots and bullets that are exploding (§8.5).

Draw sites, in the order they occur:

| Where | What is drawn |
|---|---|
| Battle setup, only with explicit initial positions | 3 `nextDouble` per position entry (§4.2) |
| Round preload, per robot in **index order**, unless a valid explicit position is given | up to 1000 placement attempts, each 2 `nextDouble` (+1 for sentries) + 1 `nextDouble` for the heading (§4.2) |
| Round start | 1 shuffle of all robots (start order) |
| Each turn, `updateBullets` | 1 shuffle of the bullets list; then **per bullet**: 1 shuffle of all robots + 1 shuffle of the current bullets list (§5) |
| Each turn, `updateRobots` | 1 shuffle of robots for the move order; **per robot**: 1 shuffle of robots (passed to `performMove`, also for dead robots, which return immediately); then 1 shuffle for the scan order; **per robot**: 1 shuffle of robots (passed to `performScan`) |
| Each turn, `handleDeadRobots` | 1 shuffle of the robots that died this turn; **per dead robot**: 1 shuffle of all robots |
| Each turn, `shutdownTurn` (only when it applies and `endTimer == 0`) | 1 shuffle of robots |
| Each turn, `wakeupRobots` | 1 shuffle of robots (robot execution order) |

The argument expressions are evaluated **in Java order**: for `bullet.update(getRobotsAtRandom(), getBulletsAtRandom())`, the robots are shuffled before the bullets. In `for (x : getRobotsAtRandom()) x.performMove(getRobotsAtRandom(), …)`, the outer shuffle happens once, before the loop; each inner shuffle happens when that iteration starts.

`robocode.util.Utils.getRandom()` returns this same instance to robots in classic; see §15.1.

---

## 4. Battle lifecycle

### 4.1 Setup (`Battle.setup`, `createPeers`)

- Robots get **indices** `0..n-1` in the order of the battle specification.
- **Names:** `name` = the robot's unique full class name with version (e.g. `jk.mega.DrussGT 3.1.16`). If a name occurs more than once, every instance gets the suffix ` (k)`, k = 1, 2, … in index order. This applies to short and very short names as well.
- **Anonymous name** (used with `hideEnemyNames`): `#<robotIndex>`.
- **Teams:** a selected team expands into its members at its position in the selection. Internally a team is named `<team class name with version>[<selection number, %4d>]`, so members of two copies of the same team stay apart. Each team becomes a contestant at the position of its **first** member, and that member is the team leader. The display name drops the 6-character `[nnnn]` suffix and appends ` (<teamIndex+1>)`, where teamIndex counts teams in order of first appearance.
- **Contestants** list: teams and robots without a team, in order of first appearance.
- Each robot starts in state `ACTIVE`, with an **empty bounding box** (all zeros).

### 4.2 Round preload (`Battle.preloadRound`, `RobotPeer.initializeRound`)

First, active participants and sentries are counted (§4.5), using the alive state from the **previous** round. Then, for each robot in **index order**:

1. If explicit initial positions were given and contain an entry for this index: use that `x`, `y` and heading (degrees, converted with `toRadians`). Missing or `?` fields are replaced by random values that were drawn at setup: `x = 36 + r·(w - 72)`, `y = 36 + r·(h - 72)`, `heading = 2·PI·r`. Update the bounding box; the spot is valid if it doesn't intersect any other robot's box (§9.1).
2. Otherwise, or if the spot is invalid, up to **1000 attempts**: draw `rndX`, `rndY`, then:
   - **Sentry robots:** draw a third value. `placeOnHorizontalBar = r <= w / (w + h)` (as doubles). Horizontal: `x = 18 + rndX·(w - 36)`, `y = 18 + (moveH·(rndY·2 - 1) + (h - 36)) % (h - 36)`. Vertical: `y = 18 + rndY·(h - 36)`, `x = 18 + (moveW·(rndX·2 - 1) + (w - 36)) % (w - 36)`. Here `moveW = sentryBorderSize - 36` and `moveH = sentryBorderSize - 36` (integers).
   - **Others, when active sentries > 0:** `safeW = w - 2·border`, `safeH = h - 2·border` (integers); `x = border + 36 + rndX·(safeW - 72)`, `y = border + 36 + rndY·(safeH - 72)`.
   - **Others, no sentries:** `x = 36 + rndX·(w - 72)`, `y = 36 + rndY·(h - 72)`.
   - Then `bodyHeading = 2·PI·nextDouble()`; gun and radar headings are set to the body heading. Update the bounding box and stop at the first **valid** spot. After 1000 failures, the last attempt is kept.
   - **(quirk)** "Valid" means not intersecting the box of **any** other robot, including robots not yet placed this round. In round 1 their boxes are empty and never intersect. From round 2 on, they still hold their end-of-previous-round positions, dead robots included.
3. Reset the robot: state `ACTIVE`; not a winner; `velocity = 0`; `energy = 100`, plus 400 for sentries, 100 for team leaders and 20 for droids; `gunHeat = 3`; not halted, not disabled, not drained; `scan = false`; `inCollision = false`; the scan arc is reset; skipped-turn tracking is reset; event, team message and bullet-status queues are emptied. The command state is reset to defaults (§11.1), keeping the colors.

After all robots are placed, active participants and sentries are counted again.

### 4.3 Round start (`Battle.initializeRound`, `RobotPeer.startRound`)

`currentTime = 0`, `endTimer = 0`, `inactiveTurnCount = 0`. Then, in **one shuffled order**, each robot's statistics are reset (§10) and it receives its initial status at time 0. Its thread starts and runs the robot's code up to its first `execute` (§11). Commands from that first `execute` take effect in turn 1.

### 4.4 Round loop and round end (`BaseBattle.runRound`)

Turns run until `isRoundOver()`, i.e. until `endTimer > 150` (5 × 30). `endTimer` only grows inside `shutdownTurn` (§5, step 6), which runs every turn once the battle is aborted or **one team remains**. So a round continues for **151 more turns** after its outcome is decided:
- Scores are finalized at the first of these turns (`endTimer == 0`).
- From `endTimer > 120` on, all robots are halted.
- Robots and bullets keep moving in between, so these turns still consume randomness and produce events.

### 4.5 Counting active robots

`activeParticipants` = alive robots that are not sentries; `activeSentries` = alive sentries. Recomputed at the times stated in §4.2 and at step 7 of every turn (§5).

`oneTeamRemaining()` is true if `activeParticipants <= 1`. Otherwise it looks at all alive robots in index order: false as soon as two of them have different teams, or both have no team. **(quirk)** Alive sentries take part in this check.

---

## 5. Turn sequence (`Battle.runTurn`)

Each turn does the following, in this order:

1. **Time:** `currentTime++`, `totalTurns++`.
2. **Load commands** (`loadCommands`), robots in **index order**: `currentCommands` = the robot's latest command state (§11.1). Fire bullets (§8.1). If the commands ask for a scan, set `scan = true`. Also note I/O robots and clear the `moved` flag.
3. **Update bullets** (`updateBullets`): for each bullet in a shuffled copy of the bullets list, call `bullet.update(shuffledRobots, shuffledBullets)` (§8). Bullets that became `INACTIVE` are removed from the list right away, so later bullets in this loop shuffle a shorter list.
4. **Update robots** (`updateRobots`):
   - `zap = inactiveTurnCount > inactivityTime`; `zapEnergy` = 5 if aborted, else 0.1 if `zap`, else 0.
   - For each robot in shuffled order: `performMove(shuffledRobots, zapEnergy)` (§6).
   - For each robot in **index order**: `updateAfterCollision` (if its state is `HIT_ROBOT`, update its bounding box).
   - For each robot in shuffled order: `performScan(shuffledRobots)` (§7).
5. **Dead robots** (`handleDeadRobots`): for each robot that died this turn, in shuffled order:
   - **Placement score:** if it has no team, or no teammate is alive anymore: `scoreRobotDeath(activeContestantCount)` (§10).
   - Then, for each robot in a fresh shuffle that is still alive: add `RobotDeathEvent(nameForEvent(dead))`; and if it is not on the dead robot's team, `scoreSurvival()`.
   - Clear the list of robots that died.
6. **Shutdown** (`shutdownTurn`), if aborted or `oneTeamRemaining()`:
   - If `endTimer == 0` and **not** aborted (one team remains): create one `RoundEndedEvent(round, currentTime, totalTurns)`. Then, for each robot in a shuffled order:
     - add that event;
     - if it is alive, not yet a winner and not a sentry: `scoreLastSurvivor()`, mark it as winner, add a `WinEvent`, and note whether it is a team leader or belongs to a winning team;
     - call `generateTotals()`.
     After the loop: if a team won but its leader wasn't among the survivors, the leader gets `scoreFirsts()`.
   - If `endTimer == 0` and it's the last round (or aborted): rank all robots by `compareTo` (§10), in descending order (sort, then reverse; the sort is stable, so ties keep **reversed index order**). Set each robot's rank and add a `BattleEndedEvent` with its final results.
   - If `endTimer > 120`: halt all robots.
   - `endTimer++`.
7. `inactiveTurnCount++`; recount active robots (§4.5); publish each robot's status (index order, §11.2).
8. **Wake up robots** (shuffled order): each robot runs until its next `execute`. Its new commands take effect in the next turn's step 2.

The observer snapshot (and the oracle recording) is taken after step 8.

---

## 6. Robot movement (`RobotPeer.performMove`)

A dead robot does nothing. Otherwise:

1. `state = ACTIVE`.
2. **Gun heat:** `gunHeat -= gunCoolingRate`; if negative, set it to 0.
3. Remember `lastHeading`, `lastGunHeading`, `lastRadarHeading`, `lastX`, `lastY`.
4. **Body turn**, only if `inCollision` is false (that is, the robot didn't ram in the previous turn's move) (§6.1).
5. **Gun turn** (§6.2), then **radar turn** (§6.3).
6. **Movement** (§6.4).
7. **Wall collision** (§6.5); for sentries also the **sentry border** (§6.6).
8. **Robot collisions** (§6.7).
9. If `scan` is false: `scan = ` (any of the five values from step 3 changed).
10. If the robot died in this move, stop. Otherwise, if `zapEnergy != 0`: zap (§9.3).

### 6.1 Body turn (`updateHeading`)

`turnRate = min(maxTurnRate, (0.4 + 0.6 * (1 - (abs(velocity) / 8))) * MAX_TURN_RATE_RADIANS)`, using the velocity **before** this turn's movement.

**(quirk)** This is mathematically equal to `Rules.getTurnRateRadians`, but it is computed differently, so the last bits can differ. Use this formula.

Let `r = bodyTurnRemaining`.
- If `r > 0`:
  - If `r < turnRate`: add `r` to the body, gun and radar headings. If adjust-gun-for-body: `gunTurnRemaining -= r`. If adjust-radar-for-body: `radarTurnRemaining -= r`. Then `bodyTurnRemaining = 0`.
  - Otherwise: add `turnRate` to all three headings; `bodyTurnRemaining -= turnRate`; and subtract `turnRate` from the gun and radar turn remaining for the corresponding adjust flags.
- `r < 0`: mirrored. Use `r > -turnRate` for the "small" case; otherwise subtract `turnRate` from the headings and add it to the remaining values.
- `r == 0` (or NaN): no change and no normalization.

If the robot turned: `bodyHeading = normalNearAbsoluteAngle(bodyHeading)` if `bodyTurnRemaining == 0`, else `normalAbsoluteAngle(bodyHeading)`. The gun and radar headings are not normalized here.

### 6.2 Gun turn (`updateGunHeading`)

The same pattern as §6.1 with `GUN_TURN_RATE_RADIANS`: the gun **and radar** headings move. If adjust-radar-for-gun is set, `radarTurnRemaining` is reduced by the same amount. Afterwards, always: `gunHeading = normalAbsoluteAngle(gunHeading)`.

### 6.3 Radar turn (`updateRadarHeading`)

The same pattern with `RADAR_TURN_RATE_RADIANS`, only the radar moves. Afterwards, always: `radarHeading = normalAbsoluteAngle(radarHeading)`.

### 6.4 Movement (`updateMovement`)

1. `distance = distanceRemaining` (NaN → 0).
2. `velocity = getNewVelocity(velocity, distance)`.
3. If `isNear(velocity, 0)` and `isOverDriving`: `distanceRemaining = 0`, `distance = 0`, `isOverDriving = false`.
4. If `signum(distance * velocity) != -1`: `isOverDriving = getDistanceTraveledUntilStop(velocity) > abs(distance)`.
5. `distanceRemaining = distance - velocity`.
6. If `velocity != 0`: `x += velocity * sin(bodyHeading)`, `y += velocity * cos(bodyHeading)`, and update the bounding box.

`isOverDriving` is per-robot state that is **not** reset between rounds. **(quirk)**

**`getNewVelocity(v, d)`:**
- If `d < 0`: return `-getNewVelocity(-v, -d)`.
- `goal = maxVelocity` if `d == +Infinity`, else `min(getMaxVelocity(d), maxVelocity)` (`maxVelocity` from the commands).
- If `v >= 0`: return `max(v - 2, min(goal, v + 1))`.
- Else: return `max(v - 1, min(goal, v + maxDecel(-v)))`.

**`getMaxVelocity(d)`:**
- `decelTime = max(1, ceil((sqrt((4 * 2 / 2) * d + 1) - 1) / 2))`. The constant expression `4 * 2 / DECELERATION` evaluates to 4.0.
- If `decelTime == +Infinity`: return 8.
- `decelDist = ((decelTime / 2.0) * (decelTime - 1)) * 2`.
- Return `((decelTime - 1) * 2) + ((d - decelDist) / decelTime)`.

**`maxDecel(speed)`:** `decelTime = speed / 2`; `accelTime = 1 - decelTime`; return `min(1, decelTime) * 2 + max(0, accelTime) * 1`.

**`getDistanceTraveledUntilStop(v)`:** `distance = 0`, `v = abs(v)`; while `v > 0`: `v = getNewVelocity(v, 0)`, `distance += v`. Return `distance`.

### 6.5 Wall collision (`checkWallCollision`)

`minX = 18`, `minY = 18`, `maxX = (int)width - 18`, `maxY = (int)height - 18` (integers).

- `x < minX`: hit, `adjustX = minX - x`, `angle = normalRelativeAngle(3*PI/2 - bodyHeading)`. `x > maxX`: hit, `adjustX = maxX - x`, `angle = normalRelativeAngle(PI/2 - bodyHeading)`.
- Then, overriding `angle`: `y < minY`: hit, `adjustY = minY - y`, `angle = normalRelativeAngle(PI - bodyHeading)`. `y > maxY`: hit, `adjustY = maxY - y`, `angle = normalRelativeAngle(-bodyHeading)`.

On a hit:
1. Add `HitWallEvent(angle)`.
2. If `bodyHeading % (PI/2) != 0`, with `t = tan(bodyHeading)`:
   - if `adjustX == 0`: `adjustX = adjustY * t`;
   - else if `adjustY == 0`: `adjustY = adjustX / t`;
   - else if `abs(adjustX / t) > abs(adjustY)`: `adjustY = adjustX / t`;
   - else if `abs(adjustY * t) > abs(adjustX)`: `adjustX = adjustY * t`.
3. `x += adjustX`; `y += adjustY`; then clamp `x` to `[minX, maxX]` and `y` to `[minY, maxY]` (one bound each, checked in the order low, high).
4. For **advanced robots** only: `setEnergy(energy - getWallHitDamage(velocity))`, without resetting the inactivity counter (§9.2). `velocity` here is the velocity from §6.4, before it is zeroed.
5. Update the bounding box; `distanceRemaining = 0`; `velocity = 0`; `state = HIT_WALL`.

### 6.6 Sentry border (`checkSentryOutsideBorder`)

Only for sentry robots. `range = sentryBorderSize`; `minX = range - 18`, `minY = range - 18`, `maxX = (int)width - range + 18`, `maxY = (int)height - range + 18`.

The robot is "inside" the battlefield's inner area if `x > minX && x < maxX && y > minY && y < maxY`. In that case, the **first** matching condition applies:

| Condition | `adjust` | `angle` |
|---|---|---|
| `x - minX <= 8` | `adjustX = minX - x` | `normalRelativeAngle(PI/2 - bodyHeading)` |
| `maxX - x <= 8` | `adjustX = maxX - x` | `normalRelativeAngle(3*PI/2 - bodyHeading)` |
| `y - minY <= 8` | `adjustY = minY - y` | `normalRelativeAngle(-bodyHeading)` |
| `maxY - y <= 8` | `adjustY = maxY - y` | `normalRelativeAngle(PI - bodyHeading)` |

On a hit: the same steps as §6.5. The clamp is different: if inside, `x` is set to `minX` if `x - minX <= 8`, else to `maxX` if `maxX - x <= 8`; `y` likewise. Energy loss, box, distance, velocity and state follow §6.5.

### 6.7 Robot collisions (`checkRobotCollision`)

`inCollision = false`. For each other robot in the given shuffled list that is not dead and whose **current** bounding box intersects this robot's box (§9.1):

1. `angle = atan2(other.x - x, other.y - y)`; `movedx = velocity * sin(bodyHeading)`; `movedy = velocity * cos(bodyHeading)`; `bearing = normalRelativeAngle(angle - bodyHeading)`.
2. The robot is **at fault** if (`velocity > 0` and `-PI/2 < bearing < PI/2`) or (`velocity < 0` and (`bearing < -PI/2` or `bearing > PI/2`)). Only then:
   1. `inCollision = true`; `velocity = 0`; `distanceRemaining = 0`; `x -= movedx`; `y -= movedy`. The bounding box is **not** updated here; that happens in step 4 of the turn (§5).
   2. `teamFire` = both robots are on the same team. Unless `teamFire` or the other robot is a sentry: `scoreRammingDamage(other)`.
   3. `updateEnergy(-0.6)` for this robot, then for the other.
   4. If the other robot's energy is now 0 and it was alive: kill it (§9.4). Unless `teamFire` or sentry: `scoreRammingKill(other)`.
   5. Add `HitRobotEvent(nameForEvent(other), normalRelativeAngle(angle - bodyHeading), other.energy, atFault=true)` to this robot.
   6. Add `HitRobotEvent(other.nameForEvent(this), normalRelativeAngle(PI + angle - other.bodyHeading), this.energy, false)` to the other robot.

Because `velocity` is 0 after the first collision, later overlaps in the same loop can't be "at fault" and produce no events. **(quirk)** Whether the other robot has already moved this turn depends on the shuffle (§3).

If `inCollision`: `state = HIT_ROBOT`.

---

## 7. Scanning (`RobotPeer.performScan`, `scan`)

For a robot that is not dead:

1. `turnedRadarWithGun = false`.
2. If `scan` is set and the robot is **not a droid**:
   1. `scanRadians = radarHeading - lastRadarHeading`. Map it into `[-PI, PI]` with a single correction: if `< -PI`, add `2*PI`; else if `> PI`, subtract `2*PI`.
   2. `startAngle = normalAbsoluteAngle(lastRadarHeading - PI/2)`.
   3. The scan arc is a `java.awt.geom.Arc2D.Double` of type `PIE`: frame `(x - 1200, y - 1200, 2400, 2400)`, start `180.0 * startAngle / PI` degrees, extent `180.0 * scanRadians / PI` degrees. Note: this is **not** `toDegrees`.
   4. For each other robot in the given shuffled list that is not dead and **intersects the arc** (see below), add a `ScannedRobotEvent`:

      | Field | Value |
      |---|---|
      | `name` | `nameForEvent(other)` |
      | `energy` | `other.energy` |
      | `bearing` | `normalRelativeAngle(atan2(dx, dy) - bodyHeading)` |
      | `distance` | `hypot(dx, dy)` |
      | `heading` | `other.bodyHeading` |
      | `velocity` | `other.velocity` |
      | `isSentryRobot` | `other.isSentryRobot` |

      Here `dx = other.x - x` and `dy = other.y - y`. **Intersects** means: the other robot's bounding box intersects the line segment from the arc's center to the arc's start point (`Rectangle2D.intersectsLine` with the robocode outcode, §9.1), **or** `Arc2D.intersects(box)`.
3. Also when `scan` is set: `turnedRadarWithGun = (lastGunHeading == lastRadarHeading) && (gunHeading == radarHeading)`; then `scan = false`. Droids get this too; they only skip the arc test.
4. **Team messages:** for team robots, each message in `currentCommands` goes to every team member (in team order) for which `checkDispatchToMember` is true:
   - the member must be alive;
   - if the recipient is null, the member must not be the sender;
   - otherwise, the member's name or full class name must start with the recipient string.
5. `currentCommands = null`; the "last" headings are set to -1.

Note: because scans happen after **all** robots moved, a robot sees the others at their end-of-turn positions.

---

## 8. Bullets (`BulletPeer`)

### 8.1 Firing (in step 2, `RobotPeer.fireBullets`)

For each bullet command in the robot's command state, in order:
1. If its power is NaN: skip it (with a warning).
2. If it was already consumed: skip it.
3. If `gunHeat > 0` or `energy == 0`: **stop processing** this turn. Remaining commands stay unconsumed.
4. `power = min(energy, min(max(p, 0.1), 3))`; `updateEnergy(-power)`; `gunHeat += getGunHeat(power)`.
5. Create a bullet with the command's bullet id. Its heading is `gunHeading`, except: for robots that are **not** advanced robots, if `turnedRadarWithGun` (from the previous scan) and the command carries a valid fire-assist angle, the heading is that angle.
6. Position `(x, y)`, i.e. the robot's position **before** this turn's move. State `FIRED`, frame 0. Mark the command consumed.

At most one bullet is created per turn, because the gun is hot afterwards. It is appended to the bullets list after the loop.

### 8.2 Bullet update (`update`)

`frame++`. If the bullet is active (`FIRED` or `MOVING`):
1. **Move:** `lastX = x`, `lastY = y`; `x += speed * sin(heading)`, `y += speed * cos(heading)` with `speed = getBulletSpeed(power)`. The bounding line is `(lastX, lastY) → (x, y)`.
2. **Wall:** if `x - 3 <= 0` or `y - 3 <= 0` or `x + 3 >= width` or `y + 3 >= height`: `state = HIT_WALL`, `frame = 0`, and the owner gets a `BulletMissedEvent`.
3. If still active: **robot hit** (§8.3); if still active: **bullet hit** (§8.4).

Then update the state (§8.5), and add a bullet status (id, x, y, victim name, active) to the owner's bullet updates if the owner is alive.

A bullet fired in step 2 is therefore already moved once in step 3 of the same turn.

### 8.3 Bullet hits robot

For each robot in the given shuffled list that is not the owner and not dead, and whose bounding box intersects the bounding line (§9.1), take the first one:

1. `state = HIT_VICTIM`, `frame = 0`, `victim = robot`.
2. **Damage:** `damage = getBulletDamage(power)`. If the owner is a sentry: damage is 0 against sentries, and 0 if the bullet is strictly inside the sentry border (`range < x < width - range`, same for y).
3. `score = min(damage, victim.energy)`, using the energy before the damage. `victim.updateEnergy(-damage)`.
4. `teamFire` = owner and victim are on the same team. Unless `teamFire` or the victim is a sentry: `owner.scoreBulletDamage(victim, score)`.
5. If the victim's energy is `<= 0` and it was alive: kill it (§9.4). Unless `teamFire` or sentry: `owner.scoreBulletKill(victim)`.
6. Unless the victim is a sentry: `owner.updateEnergy(getBulletHitBonus(power))`.
7. Victim gets `HitByBulletEvent(normalRelativeAngle(heading + PI - victim.bodyHeading), bullet)`; owner gets `BulletHitEvent(owner.nameForEvent(victim), victim.energy, bullet)`. Bullet objects: §12.2.
8. If the victim's box contains `(lastX, lastY)`: the bullet's `x, y` (and last x, y) are set back to `lastX, lastY`. The paint offset relative to the victim is recorded (display only).

### 8.4 Bullet hits bullet

For each bullet `b` in the given shuffled list, with `b != this`, `b.owner != owner`, `b` active, and the two bounding lines intersecting:

**Line intersection** (robocode's own formula): with this bullet's line `(x3,y3)-(x4,y4)` and `b`'s line `(x1,y1)-(x2,y2)`:
- `dx13 = x1-x3`, `dx21 = x2-x1`, `dx43 = x4-x3` (y likewise);
- `dn = dy43*dx21 - dx43*dy21`;
- `ua = (dx43*dy13 - dy43*dx13) / dn`, `ub = (dx21*dy13 - dy21*dx13) / dn`;
- they intersect if `0 <= ua <= 1 && 0 <= ub <= 1`. NaN from parallel lines yields false.

Note: `b`'s line is from its **last** update. If `b` hasn't moved yet this turn, that is its previous segment. **(quirk)**

If either owner is a sentry and this bullet is strictly inside the sentry border: skip `b` and continue. Otherwise:
1. Both bullets: `state = HIT_BULLET`, `frame = 0`, position reset to their last position.
2. This owner gets `BulletHitBulletEvent(this bullet, other bullet)`; `b`'s owner gets `BulletHitBulletEvent(b, this bullet)`.
3. Stop.

### 8.5 Bullet states and lifetime

| State | Transition |
|---|---|
| `FIRED` | becomes `MOVING` at the end of an update with `frame > 0` (i.e. the first update) |
| `HIT_WALL`, `HIT_VICTIM`, `HIT_BULLET`, `EXPLODED` | becomes `INACTIVE` once `frame >= explosion length` (17 for bullets) |
| `INACTIVE` | removed from the list in step 3 (§5) |

**Explosions** (`ExplosionPeer`) are also in the bullets list. One is created per killed robot (§9.4): state `EXPLODED`, power 1, victim = owner, explosion length **71**. Each update only does `frame++`, follows the owner's position and updates the state. Explosions do not report bullet status and never collide.

Because they stay in the list, hit bullets and explosions **affect every shuffle of the bullets list** (§3).

---

## 9. Geometry, energy and death

### 9.1 Bounding boxes and geometric tests

- A robot's box is a `Rectangle2D.Float` (robocode's `BoundingRectangle`): `setRect(x - 18, y - 18, 36, 36)` stores **`(float)(x - 18)`**, `(float)(y - 18)` and 36f.
- It is updated:
  - at placement (§4.2);
  - after movement when `velocity != 0` (§6.4);
  - after a wall or sentry-border hit (§6.5, §6.6);
  - in step 4 of the turn for robots in state `HIT_ROBOT`.
- **Box–box** intersection (`Rectangle2D.intersects`): false if either box is empty (width or height `<= 0`). Otherwise true if the two open intervals overlap on both axes. The floats are widened to double, and edges are compared as `x + w` with doubles.
- **Box–line** intersection (`Rectangle2D.intersectsLine`) uses the JDK's clipping algorithm based on `outcode`. Robocode **overrides** `outcode`: for a box with `width <= 0` the point is both left and right (likewise top and bottom for height). Otherwise:
  - left if `px < box.x`, right if `px > box.x + (double) box.width`;
  - top if `py < box.y`, bottom if `py > box.y + (double) box.height`.
- **Point containment** (`Rectangle2D.contains(px, py)`): inside the half-open box `[x, x+w) × [y, y+h)`, after widening.
- **Arc–box** intersection (`Arc2D.intersects`, `PIE`) and the arc's start point: see §7 and ADR 0011. Observed JDK behavior:
  - an extent of 0 never intersects;
  - the radial edges count as closed segments, also for full ellipses;
  - the curved edge is excluded;
  - the start ray belongs to the arc and the end ray doesn't.

  Implemented and verified in `crates/geom`.

### 9.2 Energy changes

**`updateEnergy(delta)`:** applied only if the robot is not disabled and not drained, **or** if `delta < 0`. Then `setEnergy(energy + delta, resetInactivity = true)`.

**`setEnergy(e, reset)`:**
1. If `reset` and the energy changes: `battle.resetInactiveTurnCount(old - new)`. Negative losses are ignored. Losses are accumulated, and each full 10 subtracts 10 and sets `inactiveTurnCount = 0`.
2. `energy = e`.
3. If `energy < 0.01`: `energy = 0`, and in the robot's **stored** command state `distanceRemaining = 0` and `bodyTurnRemaining = 0`.

### 9.3 Inactivity zap (`zap`)

- If `energy == 0`: kill the robot.
- Otherwise: `energy -= abs(zapAmount)`. This does **not** go through `setEnergy`. If `energy < 0.1`: `energy = 0` and the current command state's distance and body turn are set to 0. The robot dies in a later turn's zap.

### 9.4 Death (`kill`)

1. `battle.resetInactiveTurnCount(10)`.
2. If the robot is alive:
   1. Add a `DeathEvent`.
   2. If it is a team leader: each **alive** teammate (team order) gets `updateEnergy(-30)`. A display-only bullet is added to the bullets list: state `HIT_VICTIM`, power 4, id 0, at the teammate's position.
   3. Register the robot as died this turn (§5, step 5).
   4. Add an explosion to the bullets list (§8.5).
3. `updateEnergy(-energy)`; `state = DEAD`.

Note: the leader's display bullets live in the bullets list for 17 frames and take part in the shuffles.

### 9.5 Disabled robots

A robot can be disabled because it couldn't start, skipped too many turns, or caused a security violation (`punishBadBehavior`). It is then killed, its statistics become inactive with all scores reset, and it scores nothing more. `drainEnergy` sets the energy to 0 and marks the robot drained, so it can't gain energy anymore (§9.2).

---

## 10. Scoring (`RobotStatistics`, `TeamStatistics`)

Per robot, per round (reset at round start):

| Score | Rule |
|---|---|
| `survival` | +50 each time another robot (not a teammate) dies while this robot is alive (§5, step 5) |
| `lastSurvivorBonus` | `+10 * enemyCount` for winners, with `enemyCount = robotCount - 1 - (teamSize - 1)` |
| `bulletDamage` | `+score` per hit (§8.3); damage per victim name is also accumulated |
| `bulletKillBonus` | on a kill: `0.20 *` the damage dealt to that victim this round. For team robots, summed over all teammates' damage. |
| `rammingDamage` | `+ROBOT_HIT_BONUS` (1.2) per at-fault ram; damage per victim accumulates by 0.6 |
| `rammingKillBonus` | on a ram kill: `0.30 *` the accumulated damage to that victim (team sum as above) |

All of these apply only while the statistics are active (not disabled), and never for sentries.

- **Current score:** `bulletDamage + rammingDamage + survival + rammingKillBonus + bulletKillBonus + lastSurvivorBonus`, summed **left to right** (0 for sentries).
- **Totals:** at the end of each decided round (§5, step 6), each total is increased by its round score. Then `totalScore = totalBulletDamage + totalRammingDamage + totalSurvival + totalRammingKillBonus + totalBulletKillBonus + totalLastSurvivorBonus`, left to right (0 for sentries).
- **Placements:**
  - `scoreLastSurvivor` increments firsts for robots without a team and for team leaders.
  - `scoreRobotDeath(remaining)`: firsts if 0 remain (and the robot isn't a winner), seconds if 1 remains, thirds if 2 remain.
  - `remaining` counts alive non-sentry robots without a team, plus teams (other than the dead robot's own team) with at least one alive non-sentry member.
- **Team results:** sums over the members of each total, in team order.
- **Ordering (`compareTo`):** by total score, plus the current score while a round is in progress.
- **(quirk)** In `BattleCompletedEvent`, `rank` is the contestant's **list position** + 1 (§4.1), not its placement. Robots do get correct ranks in `BattleEndedEvent` (§5, step 6).

---

## 11. Robot commands and status

### 11.1 Command state (`ExecCommands`)

The command state is what a robot sends with every `execute`. It replaces the robot's stored state as a whole:

| Field(s) | Notes |
|---|---|
| `bodyTurnRemaining`, `gunTurnRemaining`, `radarTurnRemaining`, `distanceRemaining` | the engine reduces these while executing |
| `adjustGunForBodyTurn`, `adjustRadarForGunTurn`, `adjustRadarForBodyTurn` | flags |
| `maxVelocity`, `maxTurnRate` | default `Double.MAX_VALUE` |
| body, gun, radar, scan and bullet colors | |
| bullet commands | `power`, `bulletId`, fire-assist angle + validity, consumed |
| `scan`, `moved`, `isIORobot`, `isTryingToPaint` | flags |
| team messages | sender, recipient (null = all), serialized payload |
| debug properties, graphics calls, output text | display only |

Each round starts with a new command state that only keeps the colors (`RobotPeer.initializeRound`). That includes the robot side's record of whether `setAdjustRadarForRobotTurn` was called. While it wasn't, `setAdjustRadarForGunTurn(b)` also sets `adjustRadarForBodyTurn = b`. A robot that turns body adjustment off in one round, then only calls `setAdjustRadarForGunTurn(true)` in the next round, therefore has its radar adjusted for the body again (`catcat20.Matcha`).

**(quirk) Late bullets.** When the engine stores a robot's commands (`new ExecCommands(commands, true)`), it shares the bullet and team message lists with the robot side instead of copying them. After a normal `execute` the robot gets a fresh copy back. When `execute` throws instead (the robot died, or was halted), the robot keeps the shared lists. Bullets and team messages its handlers add while it waits for the battle's end (`waitForBattleEnd` processes events once more) still reach the engine with the next turn's `loadCommands`. Example: `lessonz.robocode.Oz` fires from a custom event after winning, when the winner is halted 120 turns after the round's last death. The robocode-s Java host sends that last `Execute` only after this event processing.

**Validation on receipt:**
- `maxTurnRate = min(abs(maxTurnRate), MAX_TURN_RATE_RADIANS)`;
- `maxVelocity = min(abs(maxVelocity), 8)`;
- NaN values only produce a warning and then go through the same `min` (Java `min` with NaN yields NaN).

**Loading:** in step 2 of a turn (§5), the engine uses the robot's stored command state **object itself**. Every change the engine makes (remaining values, consumed bullets, zeroing) is therefore visible in the state returned to the robot (§11.2). If a robot doesn't execute (skipped turn), the same state is used again, so it keeps turning and moving.

### 11.2 Status and results (what the robot receives)

`publishStatus` (step 7, §5) creates a `RobotStatus` with:
- energy, x, y, body, gun and radar headings, velocity;
- body, radar and gun turn remaining and distance remaining (from the stored command state);
- gun heat, `others`, number of sentries, round, number of rounds and time.

`others = activeParticipants - (dead or sentry ? 0 : 1)`.

When a robot executes, it gets back:
- a copy of its stored command state (with the engine's updates, without the one-shot lists);
- its status;
- its event queue, taken out;
- its team messages and bullet status updates;
- the halt flag, a "should wait" flag and whether painting is enabled.

### 11.3 Skipped turns

On each execute, the engine compares the current turn with the turn of the previous execute. If turns were skipped:
- non-critical events in the queue are cleared;
- one `SkippedTurnEvent(turn)` is added per skipped turn (if alive);
- more than 30 consecutive skipped turns (240 for I/O robots) halt and disable the robot.

In robocode-s deterministic mode, a robot's counted work decides when its execute arrives (ADR 0018): the turns it is late for are skipped as above.

---

## 12. Events

### 12.1 Generation, order and priorities

The engine **appends** events to a robot's queue in the order described in §5–§10. That order is observable: the robot side sorts events stably by time (ascending), then by priority (higher first). Events with equal time and priority are dispatched in insertion order. Events get their time on the **robot side**, when they are added to its event manager.

The engine drops new events when a queue holds more than 256, except `DeathEvent`, `WinEvent` and `SkippedTurnEvent`. In that case it also clears non-critical events older than `time - 2`.

| Event | Priority | Generated in |
|---|---|---|
| `DeathEvent` | -1 | §9.4 |
| `ScannedRobotEvent` | 10 | §7 |
| `HitByBulletEvent` | 20 | §8.3 |
| `HitWallEvent` | 30 | §6.5, §6.6 |
| `HitRobotEvent` | 40 | §6.7 |
| `BulletHitEvent` | 50 | §8.3 |
| `BulletHitBulletEvent` | 55 | §8.4 |
| `BulletMissedEvent` | 60 | §8.2 |
| `RobotDeathEvent` | 70 | §5, step 5 |
| `MessageEvent` | 75 | team messages (robot side) |
| `CustomEvent` | 80 | robot side (`Condition.test()`) |
| `StatusEvent` | 99 | robot side, every turn |
| `WinEvent`, `SkippedTurnEvent`, `BattleEndedEvent` | 100 | §5, §11.3 |
| `RoundEndedEvent` | 110 | §5, step 6 |
| `PaintEvent`, keyboard and mouse events | 5, 98 | interactive/display, out of scope (the server delivers keyboard and mouse events, design §5.2) |

### 12.2 Bullet objects in events

`Bullet(heading, x, y, power, ownerName, victimName, isActive, bulletId)`. Names use `nameForEvent`, so with `hideEnemyNames` other robots appear as `#<index>`:
- the **owner's** view shows the victim through the owner's eyes;
- the **victim's** view (`HitByBulletEvent`, the second bullet of `BulletHitBulletEvent`) shows the owner's name through the receiver's eyes.

---

## 13. Robot types

| Type | Differences |
|---|---|
| `Robot` (basic) | no wall-hit damage (only advanced robots take it); bullets may use fire assist (§8.1) |
| `AdvancedRobot` | wall-hit damage; never fire assist |
| `TeamRobot` | team messages (§7) |
| `Droid` | +20 energy; never scans (§7) |
| Team leader | +100 energy; its death costs alive teammates 30 energy each (§9.4) |
| `BorderSentry` | +400 energy; placed in the border zone (§4.2), kept there (§6.6); harmless inside the border (§8.3, §8.4); no score; doesn't count as a participant |
| `JuniorRobot`, `RateControlRobot`, `Interactive` | differences are on the robot side (API), not in the engine |

---

## 14. Observable output used by difftest

The oracle (`tools/difftest`) records each turn after classic publishes it, which is **after** the turn's wakeup (`runTurn` ends with `wakeupRobots`; `finalizeTurn` publishes). It records:
- **per robot:** state, x, y, energy, velocity, body/gun/radar heading, gun heat, and current and total score;
- **per bullet (explosions included):** owner, id, state, x, y, heading, power and victim;
- **per robot, from hooks in `RobotPeer`:**
  - the command state loaded this turn (`performLoadCommands`);
  - the remaining values reported to it (`publishStatus`, before the wakeup);
  - every event generated for it (`addEvent`);
  - `actions`, only present when something happened: what the robot's own thread did to its robot during the wakeup. That is `drainEnergy` (its code crashed or was disabled robot-side, e.g. 10,000 `getXX` calls without `execute`) or `disable:<reason>` (`punishBadBehavior`).

The replay compares the remaining values, applies the actions, then compares the state and the events. A turn's snapshot therefore includes a crash in the robot's handling of that turn. The server does the same: it collects the turn's `Execute`s before it sends the `TurnSnapshot`.

---

## 15. Deviations and open questions

### 15.1 `Utils.getRandom()` (deviation, ADR 0010)

In classic, `robocode.util.Utils.getRandom()` returns the **engine's** generator. A robot that draws from it shifts every later shuffle (§3). Robots in robocode-s run in their own processes and can't share it.

**Decision:** `Utils.getRandom()` returns one `Random` per robot, seeded with `mix64(base ^ 0x5554494C53524E44)` (ADR 0010). The oracle patches classic's `Utils.getRandom()` the same way for robot threads (`ClassicPatches`), so recordings stay comparable. None of the six top bots checked, and none of the samples, use it.

### 15.2 JDK geometry and licensing (ADR 0011)

Scanning and collisions depend on `java.awt.geom`:
- `Rectangle2D.intersects`, `intersectsLine` and `contains` (§9.1);
- `Arc2D.Double.setArc`, `getStartPoint` and `intersects` (§7).

OpenJDK's code is GPL and can't be copied. **Decision:** `crates/geom` is an independent implementation, with its behavior established by black-box experiments on the JDK. It is verified against JDK vectors: ~24,000 explicit edge cases and 5 million random cases, all matching. Details of the observed behavior are in ADR 0011.

### 15.3 Other notes

- **Math semantics:** classic uses `Math`; with intrinsics, results differ on some JVMs. The reference runs with `StrictMath` semantics (ADR 0003).
- **Dead robots' energy and ghost shots:**
  - **Energy:** a dead robot's bullets still hit, and the owner gets `updateEnergy(bulletHitBonus)` (§8.3) until its thread has finished. A robot finishes on its first `execute` after death, which sets `isExecFinishedAndDisabled`. When its own bullet hits in the turn it dies, it therefore has energy again after dying.
  - **Ghost shots:** the dead robot's thread still handles that turn's events. Its last `execute` (stored before `DeathException`) and `waitForBattleEnd` (a dead robot isn't halted) store its commands once more. Classic loads dead robots' commands too (`performLoadCommands`), and `fireBullets` only checks gun heat and energy, so a bullet it fires there **flies on the next turn** and can hit other robots (`robo.PartsBot`).
  - **The race:** `wakeupRobots` doesn't wait for dead robots. Whether the robot gets there before the battle's next turn, and whether more of its bullets hit before its thread finishes, depends on thread timing (e.g. slow work in `onDeath`).
  - **robocode-s** models the robot that finishes quickly: a robot that just died sends one final `Execute`, the server waits for it, and its commands count from the next turn. Energy from later hits is not gained (its thread has finished).
  - **The difftest** doesn't compare a dead robot's energy. It reports a difference as a known race (`RACE`) when classic's recording shows a dead robot with energy earlier in that round.
- **Classic timing bug:** `RobotPeer.startRound` has a lost-wakeup race. It affects timing only, never results; the oracle patches it (`ClassicPatches`).
- **Out of scope for the engine:** painting, debug properties, robot console output, interactive (keyboard and mouse) events, and the robot-side API semantics (blocking calls, custom events, event dispatch and interruption). The robot-side API will be specified separately for `java-compat`.
