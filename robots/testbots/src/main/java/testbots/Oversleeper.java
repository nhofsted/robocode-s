// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

/**
 * Sleeps on its main thread once a round, at turn 20: 200 ms in odd rounds (about 25 of
 * classic's turns, within the limit of 30) and 300 ms in even ones (about 38: it is disabled).
 * A test robot for skipped turns and the limit (ADR 0018, 0025).
 */
public class Oversleeper extends AdvancedRobot {

    public void run() {
        boolean slept = false;
        while (true) {
            if (!slept && getTime() >= 20) {
                slept = true;
                try {
                    Thread.sleep(getRoundNum() % 2 == 1 ? 200 : 300);
                } catch (InterruptedException e) {
                    return;
                }
            }
            setTurnRadarRight(45);
            setAhead(getTime() % 80 < 40 ? 60 : -60);
            setTurnRight(15);
            execute();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        setFire(1);
    }
}
