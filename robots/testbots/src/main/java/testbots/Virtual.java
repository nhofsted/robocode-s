// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.Robot;
import robocode.ScannedRobotEvent;

/** Starts a virtual thread, which the deterministic scheduler doesn't cover (test robot for ADR 0024, 0025). */
public class Virtual extends Robot {

    public void run() {
        Thread.startVirtualThread(() -> {
            double x = 0;
            for (int k = 0; k < 10_000; k++) {
                x += Math.sqrt(k);
            }
        });
        while (true) {
            turnGunRight(10);
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        fire(1);
    }
}
