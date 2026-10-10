# 0011. Independent implementation of the JDK geometry, verified against the JDK

- Status: Accepted
- Date: 2026-10-10

## Context

Classic Robocode's scanning and collisions use `java.awt.geom`: `Rectangle2D.intersects`, `intersectsLine` and `contains`, and `Arc2D.getStartPoint` and `intersects` (engine-spec §7, §9.1). Bit-exact compatibility (ADR 0003) needs the same results, including on boundaries. OpenJDK's code is GPLv2 with the Classpath Exception, which doesn't allow copying it into this EPL-2.0 project.

## Decision

`crates/geom` implements these tests independently, without using OpenJDK's source. The JDK's behavior was established by **black-box experiments**: candidate formulas were compared with the JDK on millions of inputs right at the boundaries, and the one that matched every case was chosen. Findings:

- **Start point:** `x + (cos(rad(-start)) * 0.5 + 0.5) * w` (likewise y with sin). Matched 1M/1M cases; the center-plus-radius forms matched only 27%.
- **Point containment:**
  - frame offsets `n = (p - x) / w - 0.5`; inside the ellipse if `nx² + ny² < 0.25` (the curved edge is excluded);
  - angle `-toDegrees(atan2(ny, nx))`.
  1M/1M near the boundaries.
- **Angle containment:** normalize the angle and the start separately to (-180, 180], take the difference, negate it for negative extents, add 360 if negative, then `0 <= d < |extent|`. The start ray is included and the end ray excluded, in both directions. 1.94M/1.94M near the boundaries.
- **Arc–rectangle intersection:**
  - an extent of 0 never intersects;
  - both radial edges are tested as **closed** segments with Cohen–Sutherland clipping, also for full ellipses;
  - then rectangle corners inside the pie, then the curved edge through a rectangle side.
- **Rectangles:**
  - open overlap;
  - half-open `contains`;
  - robocode's own closed `outcode` for line clipping (vertical edges first);
  - coordinates stored as `float` (`Rectangle2D.Float`).

Verification: `crates/geom/tests/jdk_geometry.rs` against vectors from the real JDK (`./gradlew :tools:difftest:geometryVectors`):
- **~24,000 explicit edge cases:**
  - box contact and ±ulp, mirrored, and across float rounding and powers of two;
  - points on box edges;
  - segments on edges, through corners and of zero length;
  - arcs with extent 0, ±1e-12, ±180, ±360 and beyond;
  - start angles on the axes and wrapping;
  - corners on radial edges and on the circle;
  - rectangles that only touch the curved edge (±ulp around the tangent);
  - the center inside or on the rectangle;
  - degenerate rectangles;
  - classic's radar test with the other robot on the radar line, at the radius, and overlapping.
- **5 million random cases** (bulk, hashed).

Every one matches. Mutation checks: every deliberate boundary change is detected.

## Consequences

- No GPL code in the project.
- Agreement is established empirically. If a later JDK changed this behavior, regenerating the vectors would show it.
- `Arc2D` with non-circular frames (ellipses) is verified too, although robocode only uses circles.
