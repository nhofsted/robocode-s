/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.protocol.v1.Colors;
import io.github.nhofsted.robocodes.protocol.v1.RobotCommands;
import net.sf.robocode.peer.ExecCommands;
import org.junit.jupiter.api.Test;

import java.awt.Color;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Which colours a robot set ({@code Colors.set_parts}), through the commands it gets from
 * the server and sends back, as {@link RemotePeer} converts them. A robot's colour setters
 * write {@code color.getRGB()} (classic's default for null) into its commands, as classic's
 * {@code BasicRobotProxy} does.
 */
class ColorTrackerTest {

    private static final int BLUE = ExecCommands.defaultBodyColor;

    /** The server's copy as it returns the colours sent. */
    private static RobotCommands echo(RobotCommands sent) {
        return RobotCommands.newBuilder().setColors(sent.getColors()).build();
    }

    @Test
    void aRobotThatSetsNoColoursSendsClassicsDefaultsAndNoParts() {
        ColorTracker tracker = new ColorTracker();
        // The first round starts without colours.
        ExecCommands commands = Convert.commands(RobotCommands.getDefaultInstance(), tracker);
        for (int turn = 0; turn < 3; turn++) {
            RobotCommands sent = Convert.commands(commands, tracker);
            assertEquals(Colors.newBuilder()
                    .setBody(BLUE)
                    .setGun(ExecCommands.defaultGunColor)
                    .setRadar(ExecCommands.defaultRadarColor)
                    .setScan(ExecCommands.defaultScanColor)
                    .setBullet(ExecCommands.defaultBulletColor)
                    .build(), sent.getColors());
            commands = Convert.commands(echo(sent), tracker);
        }
    }

    @Test
    void setPartsStaySetWithTheirValues() {
        ColorTracker tracker = new ColorTracker();
        ExecCommands commands = Convert.commands(RobotCommands.getDefaultInstance(), tracker);
        // setColors(Color.BLACK, Color.RED, null): body, gun and radar, the radar to the default.
        commands.setBodyColor(Color.BLACK.getRGB());
        commands.setGunColor(Color.RED.getRGB());
        commands.setRadarColor(ExecCommands.defaultRadarColor);
        RobotCommands sent = Convert.commands(commands, tracker);
        Colors colors = sent.getColors();
        assertEquals(Color.BLACK.getRGB(), colors.getBody());
        assertEquals(Color.RED.getRGB(), colors.getGun());
        assertEquals(ExecCommands.defaultRadarColor, colors.getRadar());
        assertEquals(ExecCommands.defaultScanColor, colors.getScan());
        assertEquals(ColorTracker.BODY | ColorTracker.GUN | ColorTracker.RADAR, colors.getSetParts());
        // Reading the same commands again (a final execute) gives the same.
        assertEquals(sent, Convert.commands(commands, tracker));

        // Next turn: nothing set, the colours and parts stay; then the bullet.
        commands = Convert.commands(echo(sent), tracker);
        assertEquals(colors, Convert.commands(commands, tracker).getColors());
        commands.setBulletColor(Color.WHITE.getRGB());
        Colors more = Convert.commands(commands, tracker).getColors();
        assertEquals(colors.getBody(), more.getBody());
        assertEquals(Color.WHITE.getRGB(), more.getBullet());
        assertEquals(ColorTracker.BODY | ColorTracker.GUN | ColorTracker.RADAR | ColorTracker.BULLET,
                more.getSetParts());
    }

    @Test
    void coloursAndPartsLastAcrossRoundsAndServersThatDropTheParts() {
        ColorTracker tracker = new ColorTracker();
        ExecCommands commands = Convert.commands(RobotCommands.getDefaultInstance(), tracker);
        commands.setScanColor(Color.GREEN.getRGB());
        RobotCommands sent = Convert.commands(commands, tracker);
        // An older server returns the colours without set_parts, at the next round's start.
        RobotCommands copy = RobotCommands.newBuilder()
                .setColors(sent.getColors().toBuilder().clearSetParts())
                .build();
        commands = Convert.commands(copy, tracker);
        Colors colors = Convert.commands(commands, tracker).getColors();
        assertEquals(Color.GREEN.getRGB(), colors.getScan());
        assertEquals(BLUE, colors.getBody());
        assertEquals(ColorTracker.SCAN, colors.getSetParts());
    }

    @Test
    void theRobotGetsMarkersNotColours() {
        ColorTracker tracker = new ColorTracker();
        RobotCommands copy = RobotCommands.newBuilder()
                .setColors(Colors.newBuilder().setBody(Color.BLACK.getRGB()))
                .build();
        ExecCommands commands = Convert.commands(copy, tracker);
        assertEquals(ColorTracker.MARKER, commands.getBodyColor());
        assertEquals(ColorTracker.MARKER, commands.getBulletColor());
        // The server's colours stand, but the robot didn't set them.
        Colors colors = Convert.commands(commands, tracker).getColors();
        assertEquals(Color.BLACK.getRGB(), colors.getBody());
        assertEquals(0, colors.getSetParts());
    }
}
