// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;

/**
 * Each turn the main thread hands two pool threads work and uses the first result that is
 * done, and every result some turns later (test robot for ADR 0025).
 */
public class Pool extends AdvancedRobot {

    private volatile double lastBearing;

    public void run() {
        ExecutorService pool = Executors.newFixedThreadPool(2);
        List<Future<Double>> pending = new ArrayList<>();
        try {
            while (true) {
                double bearing = lastBearing;
                for (int n = 0; n < 2; n++) {
                    int size = 10_000 + n * 25_000 + (int) (getTime() % 4) * 5_000;
                    pending.add(pool.submit(() -> {
                        double x = 0;
                        for (int k = 0; k < size; k++) {
                            x += Math.sin(k * 0.001 + bearing);
                        }
                        return x;
                    }));
                }
                double offset = 0;
                for (Future<Double> f : pending) {
                    if (f.isDone()) {
                        offset += f.get();
                    }
                }
                if (pending.size() > 6) {
                    offset += pending.remove(0).get();
                }
                setTurnRadarRight(45);
                setTurnGunRight((offset % 30) - 15);
                setAhead(offset > 0 ? 30 : -30);
                execute();
            }
        } catch (Exception e) {
            pool.shutdownNow();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        lastBearing = e.getBearingRadians();
        setFire(2);
    }
}
