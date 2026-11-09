// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

/**
 * Works for {@link #startNanos()} before its first execute in every round, by its own clock
 * (counted work in deterministic mode, ADR 0014), then moves about. Classic waits 756 ms for
 * a round's start on the calibration machine (ADR 0028). The subclasses are test robots for
 * that allowance: on time, a few turns late, and late enough to be disabled.
 */
public abstract class SlowStart extends AdvancedRobot {

    /** Counted work (as virtual time) before the round's first execute. */
    protected abstract long startNanos();

    private long sum;

    public void run() {
        long start = System.nanoTime();
        while (System.nanoTime() - start < startNanos()) {
            for (int i = 0; i < 1000; i++) {
                sum += i % 7;
            }
        }
        while (true) {
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
