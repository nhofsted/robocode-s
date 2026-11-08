// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;

import java.util.ArrayList;
import java.util.List;

/**
 * From turn 20 of the second round, holds on to 64 MB more every turn until its JVM runs out
 * of memory. It doesn't catch the error: classic's robot thread does, and the robot stops
 * (test robot for #178).
 */
public class Hog extends AdvancedRobot {

    private final List<long[]> held = new ArrayList<>();

    public void run() {
        while (true) {
            if (getRoundNum() == 1 && getTime() >= 20) {
                for (int i = 0; i < 64; i++) {
                    held.add(new long[1 << 17]);
                }
            }
            setTurnRadarRight(45);
            setAhead(getTime() % 40 < 20 ? 50 : -50);
            execute();
        }
    }
}
