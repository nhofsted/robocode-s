// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;
import robocode.SkippedTurnEvent;

/**
 * Sleeps on its main thread now and then (80 ms, about 10 of classic's turns) and steers by
 * how long that took on its clock; it skips turns as classic's would (test robot for ADR 0025).
 */
public class Sleeper extends AdvancedRobot {

    private int skipped;

    public void run() {
        double turn = 10;
        while (true) {
            if (getTime() % 60 == 30) {
                long start = System.nanoTime();
                try {
                    Thread.sleep(80);
                } catch (InterruptedException e) {
                    return;
                }
                turn = ((System.nanoTime() - start) / 1_000_000 + skipped) % 40 - 20;
            }
            setTurnRadarRight(45);
            setTurnGunRight(turn);
            setAhead(skipped % 2 == 0 ? 50 : -50);
            execute();
        }
    }

    public void onSkippedTurn(SkippedTurnEvent e) {
        skipped++;
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        setFire(1.5);
    }
}
