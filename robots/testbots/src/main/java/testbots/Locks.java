// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

import java.util.ArrayDeque;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.locks.Condition;
import java.util.concurrent.locks.ReentrantLock;

/**
 * Three threads fill a table, the main thread waits for them on a {@code CountDownLatch};
 * then a producer hands it offsets through a queue guarded by a {@code ReentrantLock} and two
 * {@code Condition}s (test robot for ADR 0025).
 */
public class Locks extends AdvancedRobot {

    private final ReentrantLock lock = new ReentrantLock();
    private final Condition notFull = lock.newCondition();
    private final Condition notEmpty = lock.newCondition();
    private final ArrayDeque<Double> queue = new ArrayDeque<>();
    private final double[] table = new double[3];
    private volatile double lastBearing;

    public void run() {
        CountDownLatch filled = new CountDownLatch(3);
        for (int n = 0; n < 3; n++) {
            int slot = n;
            new Thread(() -> {
                double x = 0;
                for (int k = 0; k < 100_000 * (3 - slot); k++) {
                    x += Math.cos(k * 0.0003);
                }
                table[slot] = x;
                filled.countDown();
            }).start();
        }
        try {
            filled.await();
        } catch (InterruptedException e) {
            return;
        }
        Thread producer = new Thread(() -> {
            try {
                for (int i = 0; ; i++) {
                    double x = table[i % 3];
                    for (int k = 0; k < 15_000 + (i % 5) * 4_000; k++) {
                        x += Math.sin(k * 0.002 + lastBearing);
                    }
                    lock.lock();
                    try {
                        while (queue.size() == 2) {
                            notFull.await();
                        }
                        queue.addLast((x % 20) - 10);
                        notEmpty.signal();
                    } finally {
                        lock.unlock();
                    }
                }
            } catch (InterruptedException e) {
                // the round is over
            }
        });
        producer.setDaemon(true);
        producer.start();
        while (true) {
            double offset;
            lock.lock();
            try {
                while (queue.isEmpty()) {
                    notEmpty.await();
                }
                offset = queue.removeFirst();
                notFull.signal();
            } catch (InterruptedException e) {
                return;
            } finally {
                lock.unlock();
            }
            setTurnRadarRight(45);
            setTurnGunRight(offset);
            setAhead(offset > 0 ? 50 : -50);
            execute();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        lastBearing = e.getBearingRadians();
        setFire(1.5);
    }
}
