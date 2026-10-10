/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import net.sf.robocode.battle.BoundingRectangle;

import java.awt.geom.Arc2D;
import java.awt.geom.Point2D;
import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Random;

/**
 * Writes test vectors for the Rust geometry crate ({@code crates/geom}), computed with
 * the JDK's {@code java.awt.geom} and classic Robocode's {@code BoundingRectangle}.
 * Must run with {@code -XX:-InlineMathNatives}, like the oracle (ADR 0003).
 *
 * <p>Lines; doubles are written in Java's shortest round-trip form, so they parse back exactly:
 * <ul>
 *   <li>{@code box <ax> <ay> <bx> <by> <intersects>}: robot boxes at two centers;</li>
 *   <li>{@code contains <cx> <cy> <px> <py> <result>}: point in the robot box at (cx, cy);</li>
 *   <li>{@code line <cx> <cy> <x1> <y1> <x2> <y2> <result>}: segment through that robot box;</li>
 *   <li>{@code arc <x> <y> <w> <h> <start> <extent> <rx> <ry> <rw> <rh> <result>}: {@code Arc2D.intersects} (PIE);</li>
 *   <li>{@code start <x> <y> <w> <h> <start> <extent> <px> <py>}: {@code getStartPoint()};</li>
 *   <li>{@code scan <x> <y> <lastRadar> <radar> <ox> <oy> <result>}: classic's radar test, with the
 *       arc built as in {@code RobotPeer.scan} and the other robot's box at (ox, oy);</li>
 *   <li>{@code bulk <kind> <seed> <count> <hash>}: random cases from {@code new Random(seed)}, hashed;
 *       the Rust test regenerates them.</li>
 * </ul>
 * Generators must stay identical to {@code crates/geom/tests/jdk_geometry.rs}.
 * Regenerate with {@code ./gradlew :tools:difftest:geometryVectors}.
 */
public final class GeometryVectors {

    private static final int BULK = 1_000_000;
    private static PrintWriter out;

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: GeometryVectors <output file>");
        }
        if (Math.sin(1e5) != StrictMath.sin(1e5) || Math.cos(0.7853981633974483) != StrictMath.cos(0.7853981633974483)) {
            throw new IllegalStateException("run with -XX:+UnlockDiagnosticVMOptions -XX:-InlineMathNatives");
        }
        try (PrintWriter w = new PrintWriter(Files.newBufferedWriter(Path.of(args[0]), StandardCharsets.UTF_8))) {
            out = w;
            out.println("# java.awt.geom test vectors from " + System.getProperty("java.vm.vendor") + " "
                    + System.getProperty("java.vm.version") + "; regenerate with ./gradlew :tools:difftest:geometryVectors");
            boxes();
            contains();
            lines();
            arcs();
            starts();
            scans();
            bulk();
        }
    }

    // ---------------------------------------------------------------- boxes

    static BoundingRectangle robotBox(double x, double y) {
        BoundingRectangle box = new BoundingRectangle();
        box.setRect(x - 18, y - 18, 36, 36);
        return box;
    }

    static boolean boxIntersects(double ax, double ay, double bx, double by) {
        return robotBox(ax, ay).intersects(robotBox(bx, by));
    }

    private static void boxes() {
        double[] bases = {100, 100.1, 0.3, 399.99999, 18, 782, 1e-9, 5000.123, 110.30001, 238.70001};
        for (double base : bases) {
            for (double gap : new double[] {36, 35.999999, 36.000001, 0, 18, 72, -36, -35.999999, -36.000001, -18}) {
                for (int ulps = -3; ulps <= 3; ulps++) {
                    double other = step(base + gap, ulps);
                    line("box", base, 50.0, other, 50.0, boxIntersects(base, 50, other, 50));
                    line("box", 50.0, base, 50.0, other, boxIntersects(50, base, 50, other));
                    line("box", base, base, other, other, boxIntersects(base, base, other, other));
                    line("box", base, base, other, step(base - gap, -ulps), boxIntersects(base, base, other, step(base - gap, -ulps)));
                }
            }
        }
        // Float rounding: centers whose box edges round to the same float.
        Random r = new Random(11);
        for (int i = 0; i < 2000; i++) {
            double ax = r.nextDouble() * 800, ay = r.nextDouble() * 600;
            float edge = (float) (ax + 18);
            double bx = (double) Math.nextUp(edge) + 18 + (r.nextInt(5) - 2) * Math.ulp(ax), by = ay + (r.nextDouble() - 0.5) * 70;
            line("box", ax, ay, bx, by, boxIntersects(ax, ay, bx, by));
        }
    }

    // ---------------------------------------------------------------- contains

    private static void contains() {
        // Boxes from 92.3 to 128.3 and 220.7 to 256.7 cross a power of two, where float and
        // double edge sums differ.
        double[][] centers = {{100, 100}, {100.1, 333.3}, {18, 18}, {0.1, 0.2}, {110.30001, 238.70001}};
        for (double[] c : centers) {
            BoundingRectangle box = robotBox(c[0], c[1]);
            double x0 = box.getX(), y0 = box.getY(), x1 = x0 + box.getWidth(), y1 = y0 + box.getHeight();
            for (double px : new double[] {x0, x1, (x0 + x1) / 2}) {
                for (double py : new double[] {y0, y1, (y0 + y1) / 2}) {
                    for (int ux = -1; ux <= 1; ux++) {
                        for (int uy = -1; uy <= 1; uy++) {
                            double qx = step(px, ux), qy = step(py, uy);
                            line("contains", c[0], c[1], qx, qy, box.contains(qx, qy));
                        }
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- lines

    static boolean lineIntersects(double cx, double cy, double x1, double y1, double x2, double y2) {
        return robotBox(cx, cy).intersectsLine(x1, y1, x2, y2);
    }

    private static void lines() {
        double cx = 400.3, cy = 300.7;
        BoundingRectangle box = robotBox(cx, cy);
        double x0 = box.getX(), y0 = box.getY(), x1 = x0 + box.getWidth(), y1 = y0 + box.getHeight();
        double[] xs = {x0 - 20, x0 - 1e-9, x0, step(x0, 1), cx, step(x1, -1), x1, x1 + 1e-9, x1 + 20};
        double[] ys = {y0 - 20, y0 - 1e-9, y0, step(y0, 1), cy, step(y1, -1), y1, y1 + 1e-9, y1 + 20};
        // All segments between grid points around the box: corners, edges, collinear, zero-length.
        for (double ax : xs) {
            for (double ay : ys) {
                for (double bx : xs) {
                    for (double by : ys) {
                        line("line", cx, cy, ax, ay, bx, by, lineIntersects(cx, cy, ax, ay, bx, by));
                    }
                }
            }
        }
        // A box crossing a power of two (float and double edge sums differ): segments
        // between points on and right next to its edges.
        double qx = 110.30001, qy = 238.70001;
        BoundingRectangle q = robotBox(qx, qy);
        double qx0 = q.getX(), qy0 = q.getY(), qx1 = qx0 + q.getWidth(), qy1 = qy0 + q.getHeight();
        double[] qxs = {qx0 - 5, step(qx1, -1), qx1, step(qx1, 1), qx1 + 5};
        double[] qys = {qy0 - 5, step(qy1, -1), qy1, step(qy1, 1), qy1 + 5};
        for (double ax : qxs) {
            for (double ay : qys) {
                for (double bx : qxs) {
                    for (double by : qys) {
                        line("line", qx, qy, ax, ay, bx, by, lineIntersects(qx, qy, ax, ay, bx, by));
                    }
                }
            }
        }
        // Lines exactly through a corner, from all directions.
        double[][] corners = {{x0, y0}, {x1, y0}, {x0, y1}, {x1, y1}};
        Random r = new Random(12);
        for (double[] c : corners) {
            for (int i = 0; i < 200; i++) {
                double a = r.nextDouble() * 2 * Math.PI, len = 1 + r.nextDouble() * 19;
                double sx = c[0] - Math.sin(a) * len, sy = c[1] - Math.cos(a) * len;
                double ex = c[0] + Math.sin(a) * len, ey = c[1] + Math.cos(a) * len;
                line("line", cx, cy, sx, sy, ex, ey, lineIntersects(cx, cy, sx, sy, ex, ey));
                line("line", cx, cy, sx, sy, c[0], c[1], lineIntersects(cx, cy, sx, sy, c[0], c[1]));
            }
        }
    }

    // ---------------------------------------------------------------- arcs

    static boolean arcIntersects(double x, double y, double w, double h, double start, double extent,
            double rx, double ry, double rw, double rh) {
        return new Arc2D.Double(x, y, w, h, start, extent, Arc2D.PIE).intersects(rx, ry, rw, rh);
    }

    private static void arc(double x, double y, double w, double h, double start, double extent,
            double rx, double ry, double rw, double rh) {
        line("arc", x, y, w, h, start, extent, rx, ry, rw, rh, arcIntersects(x, y, w, h, start, extent, rx, ry, rw, rh));
    }

    private static void arcs() {
        double[] starts = {0, 30, 45, 89.99999, 90, 135, 180, 270, 359.99999, -45, 720.5};
        double[] extents = {0, 1e-12, -1e-12, 0.001, 10, 45, -45, 89.9, 90, 179.999, 180, -180, 270, 359.9, 360, -360, 400};
        Random r = new Random(13);
        // A unit-ish circle at the origin, with rectangles in every relation to it.
        for (double s : starts) {
            for (double e : extents) {
                for (int i = 0; i < 4; i++) {
                    double rx = r.nextDouble() * 260 - 130, ry = r.nextDouble() * 260 - 130;
                    double rw = r.nextDouble() * 80, rh = r.nextDouble() * 80;
                    arc(-100, -100, 200, 200, s, e, rx, ry, rw, rh);
                }
                // Rectangles containing the center, touching it, or with a corner on it.
                arc(-100, -100, 200, 200, s, e, -1, -1, 2, 2);
                arc(-100, -100, 200, 200, s, e, 0, 0, 10, 10);
                arc(-100, -100, 200, 200, s, e, -10, -10, 10, 10);
                arc(-100, -100, 200, 200, s, e, 0, -5, 10, 10);
                // Rectangles touching the circle or the frame from outside and inside.
                arc(-100, -100, 200, 200, s, e, 100, -5, 10, 10);
                arc(-100, -100, 200, 200, s, e, step(100, -1), -5, 10, 10);
                arc(-100, -100, 200, 200, s, e, -5, -110, 10, 10);
                arc(-100, -100, 200, 200, s, e, -5, step(-100, 1) - 10, 10, 10);
                arc(-100, -100, 200, 200, s, e, 70.71067811865476, -80.71067811865476, 10, 10);
                // Degenerate rectangles.
                arc(-100, -100, 200, 200, s, e, 10, 10, 0, 10);
                arc(-100, -100, 200, 200, s, e, 10, 10, 10, 0);
                arc(-100, -100, 200, 200, s, e, 10, 10, -5, 10);
            }
        }
        // Rectangles with a corner or a side exactly on a radial edge.
        for (double s : starts) {
            for (double e : new double[] {30, -30, 90, 180}) {
                for (double edgeAngle : new double[] {s, s + e}) {
                    double rad = Math.toRadians(-edgeAngle);
                    for (double t : new double[] {0.1, 0.5, 0.99}) {
                        double px = Math.cos(rad) * 100 * t, py = Math.sin(rad) * 100 * t;
                        arc(-100, -100, 200, 200, s, e, px, py, 5, 5);
                        arc(-100, -100, 200, 200, s, e, px - 5, py, 5, 5);
                        arc(-100, -100, 200, 200, s, e, px, py - 5, 5, 5);
                        arc(-100, -100, 200, 200, s, e, px - 5, py - 5, 5, 5);
                        arc(-100, -100, 200, 200, s, e, px - 2.5, py - 2.5, 5, 5);
                    }
                }
            }
        }
        // Rectangles that only meet the curved edge: all corners outside the circle, which
        // bulges into a side. Varied around the exact tangent by single ulps.
        for (double s : new double[] {0, 45, 60, 90, 180, 300, -30}) {
            for (double e : new double[] {30, -30, 120, 200, -200, 360}) {
                double mid = Math.toRadians(-(s + e / 2));
                double px = Math.cos(mid) * 100, py = Math.sin(mid) * 100;
                for (double depth : new double[] {-1, 0, 1e-9, 0.5}) {
                    for (int ulps = -2; ulps <= 2; ulps++) {
                        // A thin rectangle across the circle, perpendicular to the radius.
                        if (Math.abs(Math.cos(mid)) > Math.abs(Math.sin(mid))) {
                            double edge = step(px - Math.signum(px) * depth, ulps);
                            double rx = px > 0 ? edge : edge - 5;
                            arc(-100, -100, 200, 200, s, e, rx, py - 20, 5, 40);
                        } else {
                            double edge = step(py - Math.signum(py) * depth, ulps);
                            double ry = py > 0 ? edge : edge - 5;
                            arc(-100, -100, 200, 200, s, e, px - 20, ry, 40, 5);
                        }
                    }
                }
                // Exactly at the top, bottom, left and right of the circle.
                for (int ulps = -2; ulps <= 2; ulps++) {
                    arc(-100, -100, 200, 200, s, e, -20, step(-100.0, ulps) - 5, 40, 5);
                    arc(-100, -100, 200, 200, s, e, -20, step(100.0, ulps), 40, 5);
                    arc(-100, -100, 200, 200, s, e, step(-100.0, ulps) - 5, -20, 5, 40);
                    arc(-100, -100, 200, 200, s, e, step(100.0, ulps), -20, 5, 40);
                }
            }
        }
        // Ellipses (non-square frames) and other frame positions; many more in the bulk check.
        for (int i = 0; i < 300; i++) {
            double x = r.nextDouble() * 200 - 100, y = r.nextDouble() * 200 - 100;
            double w = r.nextDouble() * 300, h = r.nextDouble() * 300;
            double s = r.nextDouble() * 720 - 360, e = r.nextDouble() * 760 - 380;
            arc(x, y, w, h, s, e, r.nextDouble() * 400 - 200, r.nextDouble() * 400 - 200,
                    r.nextDouble() * 100, r.nextDouble() * 100);
        }
    }

    private static void starts() {
        Random r = new Random(14);
        for (int i = 0; i < 2000; i++) {
            double x = r.nextDouble() * 2000 - 1000, y = r.nextDouble() * 2000 - 1000;
            double w = r.nextDouble() * 3000, h = r.nextDouble() * 3000;
            double s = i < 40 ? (i - 20) * 45 : r.nextDouble() * 1440 - 720, e = r.nextDouble() * 360 - 180;
            Point2D p = new Arc2D.Double(x, y, w, h, s, e, Arc2D.PIE).getStartPoint();
            line("start", x, y, w, h, s, e, p.getX(), p.getY());
        }
    }

    // ---------------------------------------------------------------- classic's radar test

    /** The arc of {@code RobotPeer.scan}. */
    static Arc2D.Double scanArc(double x, double y, double lastRadarHeading, double radarHeading) {
        double startAngle = lastRadarHeading;
        double scanRadians = radarHeading - startAngle;
        if (scanRadians < -Math.PI) {
            scanRadians = 2 * Math.PI + scanRadians;
        } else if (scanRadians > Math.PI) {
            scanRadians = scanRadians - 2 * Math.PI;
        }
        startAngle -= Math.PI / 2;
        startAngle = robocode.util.Utils.normalAbsoluteAngle(startAngle);
        Arc2D.Double arc = new Arc2D.Double();
        arc.setArc(x - 1200, y - 1200, 2400, 2400, 180.0 * startAngle / Math.PI, 180.0 * scanRadians / Math.PI,
                Arc2D.PIE);
        return arc;
    }

    /** {@code RobotPeer.intersects(scanArc, boundingBox)}. */
    static boolean scanIntersects(double x, double y, double lastRadar, double radar, double ox, double oy) {
        Arc2D.Double arc = scanArc(x, y, lastRadar, radar);
        BoundingRectangle box = robotBox(ox, oy);
        return box.intersectsLine(arc.getCenterX(), arc.getCenterY(), arc.getStartPoint().getX(),
                arc.getStartPoint().getY()) || arc.intersects(box);
    }

    private static void scan(double x, double y, double lastRadar, double radar, double ox, double oy) {
        line("scan", x, y, lastRadar, radar, ox, oy, scanIntersects(x, y, lastRadar, radar, ox, oy));
    }

    private static void scans() {
        Random r = new Random(15);
        double[] headings = {0, Math.PI / 4, Math.PI / 2, Math.PI, 3 * Math.PI / 2, 2 * Math.PI - 1e-12, 1e-12};
        double[] deltas = {0, 1e-12, -1e-12, Math.PI / 4, -Math.PI / 4, Math.PI, -Math.PI, 3.5, -3.5};
        for (double h : headings) {
            for (double d : deltas) {
                // The other robot on the radar line, beside it, at the radius, and overlapping.
                for (double dist : new double[] {0, 10, 36, 600, 1181, 1182, 1199.99, 1218, 1219}) {
                    for (double side : new double[] {0, 18, -18, 17.999999, 18.000001, 30}) {
                        double dirX = Math.sin(h), dirY = Math.cos(h);
                        double ox = 400 + dirX * dist + dirY * side, oy = 300 + dirY * dist - dirX * side;
                        scan(400, 300, h, h + d, ox, oy);
                    }
                }
            }
        }
        for (int i = 0; i < 300; i++) {
            double h = r.nextDouble() * 2 * Math.PI, d = (r.nextDouble() * 2 - 1) * Math.PI / 4;
            double a = r.nextDouble() * 2 * Math.PI, dist = r.nextDouble() * 1300;
            scan(400, 300, h, h + d, 400 + Math.sin(a) * dist, 300 + Math.cos(a) * dist);
        }
    }

    // ---------------------------------------------------------------- bulk

    /** Random cases for the bulk checks; keep identical to the Rust test. */
    static boolean bulkCase(String kind, Random r) {
        switch (kind) {
            case "box" -> {
                double ax = r.nextDouble() * 800, ay = r.nextDouble() * 600;
                return boxIntersects(ax, ay, ax + (r.nextDouble() * 2 - 1) * 40, ay + (r.nextDouble() * 2 - 1) * 40);
            }
            case "line" -> {
                double cx = r.nextDouble() * 800, cy = r.nextDouble() * 600;
                double x1 = cx + (r.nextDouble() * 2 - 1) * 40, y1 = cy + (r.nextDouble() * 2 - 1) * 40;
                double a = r.nextDouble() * 2 * Math.PI, v = 11 + r.nextDouble() * 8.7;
                return lineIntersects(cx, cy, x1, y1, x1 + v * Math.sin(a), y1 + v * Math.cos(a));
            }
            case "scan" -> {
                double x = r.nextDouble() * 800, y = r.nextDouble() * 600;
                double h = r.nextDouble() * 2 * Math.PI, d = (r.nextDouble() * 2 - 1) * Math.PI / 4;
                double ox = r.nextDouble() * 800, oy = r.nextDouble() * 600;
                return scanIntersects(x, y, h, h + d, ox, oy);
            }
            case "arc" -> {
                double s = r.nextDouble() * 720 - 360, e = r.nextDouble() * 760 - 380;
                double rx = r.nextDouble() * 300 - 150, ry = r.nextDouble() * 300 - 150;
                return arcIntersects(-100, -100, 200, 200, s, e, rx, ry, r.nextDouble() * 60, r.nextDouble() * 60);
            }
            case "ellipse" -> {
                double x = r.nextDouble() * 200 - 100, y = r.nextDouble() * 200 - 100;
                double w = r.nextDouble() * 300, h = r.nextDouble() * 300;
                double s = r.nextDouble() * 720 - 360, e = r.nextDouble() * 760 - 380;
                double rx = r.nextDouble() * 400 - 200, ry = r.nextDouble() * 400 - 200;
                return arcIntersects(x, y, w, h, s, e, rx, ry, r.nextDouble() * 100, r.nextDouble() * 100);
            }
            default -> throw new IllegalArgumentException(kind);
        }
    }

    private static void bulk() {
        int seed = 100;
        for (String kind : new String[] {"box", "line", "scan", "arc", "ellipse"}) {
            Random r = new Random(seed);
            long hash = 0xcbf29ce484222325L;
            for (int i = 0; i < BULK; i++) {
                hash ^= bulkCase(kind, r) ? 1 : 0;
                hash *= 0x100000001b3L;
            }
            out.println("bulk " + kind + " " + seed + " " + BULK + " " + Long.toHexString(hash));
            seed++;
        }
    }

    // ---------------------------------------------------------------- output

    private static double step(double d, int ulps) {
        for (int i = 0; i < ulps; i++) {
            d = Math.nextUp(d);
        }
        for (int i = 0; i > ulps; i--) {
            d = Math.nextDown(d);
        }
        return d;
    }

    private static void line(String kind, Object... values) {
        StringBuilder sb = new StringBuilder(kind);
        for (Object v : values) {
            sb.append(' ');
            if (v instanceof Double d) {
                sb.append(d.doubleValue() == 0 && 1 / d.doubleValue() < 0 ? "-0.0" : d.toString());
            } else {
                sb.append(v);
            }
        }
        out.println(sb);
    }
}
