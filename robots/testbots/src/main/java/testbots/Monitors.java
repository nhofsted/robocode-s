// SPDX-License-Identifier: EPL-2.0
package testbots;

import robocode.AdvancedRobot;
import robocode.ScannedRobotEvent;

/**
 * A producer thread hands the main thread gun offsets through a bounded buffer with
 * {@code synchronized}, {@code wait} and {@code notifyAll}: the battle depends on how the
 * two interleave (test robot for ADR 0025).
 */
public class Monitors extends AdvancedRobot {

    private final Buffer buffer = new Buffer();
    private volatile double lastBearing;

    static final class Buffer {
        private final double[] items = new double[3];
        private int head;
        private int size;
        private long produced;

        synchronized void put(double item) throws InterruptedException {
            while (size == items.length) {
                wait();
            }
            items[(head + size) % items.length] = item;
            size++;
            produced++;
            notifyAll();
        }

        synchronized double take() throws InterruptedException {
            while (size == 0) {
                wait();
            }
            double item = items[head];
            head = (head + 1) % items.length;
            size--;
            notifyAll();
            return item;
        }

        long produced() {
            synchronized (this) {
                return produced;
            }
        }
    }

    public void run() {
        Thread producer = new Thread(() -> {
            try {
                for (int i = 0; ; i++) {
                    double x = 0;
                    for (int k = 0; k < 20_000 + (i % 7) * 5_000; k++) {
                        x += Math.sin(k * 0.001 + lastBearing);
                    }
                    buffer.put((x % 30) - 15);
                }
            } catch (InterruptedException e) {
                // the round is over
            }
        });
        producer.setDaemon(true);
        producer.start();
        while (true) {
            double offset;
            try {
                offset = buffer.take();
            } catch (InterruptedException e) {
                return;
            }
            setTurnRadarRight(45);
            setTurnGunRight(offset);
            setAhead(buffer.produced() % 2 == 0 ? 40 : -40);
            setTurnRight(offset / 3);
            execute();
        }
    }

    public void onScannedRobot(ScannedRobotEvent e) {
        lastBearing = e.getBearingRadians();
        setFire(1 + Math.abs(lastBearing) % 2);
    }
}
