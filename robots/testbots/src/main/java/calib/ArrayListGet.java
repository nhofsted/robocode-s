// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: ArrayList.get and size. */
public class ArrayListGet extends Bench {

    private final java.util.ArrayList<Double> list = new java.util.ArrayList<>();

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
