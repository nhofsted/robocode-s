// SPDX-License-Identifier: EPL-2.0
package calib;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

/**
 * Calibration benchmarks (#15): each turn, {@link #work} does a fixed amount of one kind
 * of work, so the work meter's price for that kind can be compared with its real CPU time
 * (tools/difftest/corpus/calibrate.py). The result steers the robot a little, so the JIT
 * can't drop the work. Fights sample.SittingDuck: it fires whenever it can, so rounds end.
 */
public abstract class Bench extends AdvancedRobot {

    /** Keeps results alive. */
    protected static volatile double sink;

    /** Iterations per turn. */
    protected abstract int iterations();

    /** One turn's work; its result is used. */
    protected abstract double work(int n);

    public void run() {
        setAdjustRadarForGunTurn(true);
        while (true) {
            double r = work(iterations());
            sink = r;
            setTurnRadarRight(360);
            setAhead(Double.isNaN(r) ? 1 : 0);
            execute();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        double bearing = getHeading() + e.getBearing() - getGunHeading();
        while (bearing > 180) {
            bearing -= 360;
        }
        while (bearing < -180) {
            bearing += 360;
        }
        setTurnGunRight(bearing);
        if (getGunHeat() == 0) {
            setFire(3);
        }
    }
}
