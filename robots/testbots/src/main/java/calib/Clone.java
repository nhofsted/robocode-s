// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: clones of a double[1000]. */
public class Clone extends Bench {

    private final double[] a = new double[1000];

    protected int iterations() {
        return 1000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            double[] b = a.clone();
            s += b[i % 1000];
        }
        return s;
    }
}
