// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.Robot;
import robocode.ScannedRobotEvent;

/** Starts a thread through a {@link java.util.Timer}, as a few robots do. */
public class Timers extends Robot {

    public void run() {
        new java.util.Timer(true).schedule(new java.util.TimerTask() {
            public void run() {
            }
        }, 10_000);
        while (true) {
            turnGunRight(10);
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        fire(1);
    }
}
