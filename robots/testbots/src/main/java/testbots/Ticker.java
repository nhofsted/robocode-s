// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

/**
 * A {@code Timer} ticks every 50 ms and changes the robot's direction and gun offset: the
 * battle depends on when its task runs (test robot for ADR 0025).
 */
public class Ticker extends AdvancedRobot {

    private volatile int ticks;
    private volatile double offset;

    public void run() {
        java.util.Timer timer = new java.util.Timer(true);
        timer.scheduleAtFixedRate(new java.util.TimerTask() {
            public void run() {
                ticks++;
                double x = 0;
                for (int k = 0; k < 5_000 + ticks % 3 * 2_000; k++) {
                    x += Math.cos(k * 0.01 + ticks);
                }
                offset = (x % 20) - 10;
            }
        }, 0, 50);
        while (true) {
            setTurnRadarRight(45);
            setTurnGunRight(offset);
            setAhead(ticks % 2 == 0 ? 40 : -40);
            execute();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        setFire(1 + (ticks % 3));
    }
}
