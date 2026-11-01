// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: Vector.get and size (synchronized). */
public class VectorGet extends Bench {

    private final java.util.Vector<Double> list = new java.util.Vector<>();

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        if (list.isEmpty()) {
            for (int i = 0; i < 1000; i++) {
                list.add((double) i);
            }
        }
        double s = 0;
        for (int i = 0; i < n; i++) {
            s += list.get(i % list.size());
        }
        return s;
    }
}
