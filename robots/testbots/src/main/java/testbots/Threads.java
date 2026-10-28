// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.Robot;
import robocode.ScannedRobotEvent;

/**
 * Starts a thread of its own, like Toad, Fermat (a writer thread) and TNT (an analyzer
 * thread) do; otherwise it spins its gun and fires, like {@code sample.Fire}.
 */
public class Threads extends Robot {

    public void run() {
        Thread helper = new Thread(() -> {
            try {
                Thread.sleep(50);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        });
        helper.setDaemon(true);
        helper.start();
        while (true) {
            turnGunRight(10);
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        fire(1);
    }
}
