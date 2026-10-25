/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.protocol.v1.Colors;
import net.sf.robocode.peer.ExecCommands;

/**
 * Which colours a robot set itself ({@code Colors.set_parts}), so that observers can draw a
 * robot that set none in its own colours rather than classic's blue. Classic's proxies write
 * {@code setBodyColor} and the like straight into the robot's {@link ExecCommands}, and robots
 * can't read their colours back: so the commands a robot gets hold {@link #MARKER} in place of
 * each colour, and a part that holds something else at the next execute was set, whatever the
 * value (also classic's default). The colours sent are exactly classic's, as before: the server
 * hashes them, and the marker never leaves the host. One per robot, for the whole battle
 * (classic keeps colours across rounds).
 */
final class ColorTracker {

    static final int BODY = 1;
    static final int GUN = 2;
    static final int RADAR = 4;
    static final int SCAN = 8;
    static final int BULLET = 16;

    /**
     * Stands for "not set since the robot got its commands": a fully transparent colour,
     * which a robot would have to make on purpose ({@code new Color(0x00C0FFEE, true)}).
     */
    static final int MARKER = 0x00C0FFEE;

    /** The robot's colours, as classic would send them: body, gun, radar, scan, bullet. */
    private final int[] colors = {
        ExecCommands.defaultBodyColor,
        ExecCommands.defaultGunColor,
        ExecCommands.defaultRadarColor,
        ExecCommands.defaultScanColor,
        ExecCommands.defaultBulletColor,
    };
    private int set;

    /** The server's copy of the command state: its colours stand until the robot sets others. */
    synchronized void fromServer(Colors c) {
        colors[0] = c.getBody();
        colors[1] = c.getGun();
        colors[2] = c.getRadar();
        colors[3] = c.getScan();
        colors[4] = c.getBullet();
        // Servers that drop the field return 0: what the robot set stays set.
        set |= c.getSetParts();
    }

    /** Puts the markers in commands the robot is about to get. */
    static void mark(ExecCommands e) {
        e.setBodyColor(MARKER);
        e.setGunColor(MARKER);
        e.setRadarColor(MARKER);
        e.setScanColor(MARKER);
        e.setBulletColor(MARKER);
    }

    /**
     * The colours to send for the robot's commands: what it set since it got them, the earlier
     * ones elsewhere. Reading the same commands again gives the same colours.
     */
    synchronized Colors fromRobot(ExecCommands e) {
        int[] now = {e.getBodyColor(), e.getGunColor(), e.getRadarColor(), e.getScanColor(), e.getBulletColor()};
        for (int i = 0; i < now.length; i++) {
            if (now[i] != MARKER) {
                colors[i] = now[i];
                set |= 1 << i;
            }
        }
        return Colors.newBuilder()
                .setBody(colors[0])
                .setGun(colors[1])
                .setRadar(colors[2])
                .setScan(colors[3])
                .setBullet(colors[4])
                .setSetParts(set)
                .build();
    }
}
