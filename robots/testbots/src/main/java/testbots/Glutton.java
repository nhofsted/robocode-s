// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;

import java.util.ArrayList;
import java.util.List;

/**
 * In turn 30 of the first round, takes memory until its JVM runs out, catches the error, lets
 * go of it all and fights on (test robot for #178).
 */
public class Glutton extends AdvancedRobot {

    public void run() {
        while (true) {
            if (getRoundNum() == 0 && getTime() == 30) {
                List<long[]> held = new ArrayList<>();
                try {
                    while (true) {
                        held.add(new long[1 << 17]);
                    }
                } catch (OutOfMemoryError e) {
                    held = null;
                    out.println("let go");
                }
            }
            setTurnRadarRight(45);
            setAhead(getTime() % 40 < 20 ? 50 : -50);
            execute();
        }
    }
}
