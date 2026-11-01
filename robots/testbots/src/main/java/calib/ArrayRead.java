// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: sequential double[] reads. */
public class ArrayRead extends Bench {

    private final double[] a = new double[4096];

    protected int iterations() {
        return 200000;
    }

    protected double work(int n) {
        double s = 0;
        double[] a = this.a;
        for (int i = 0; i < n; i++) {
            s += a[i & 4095];
        }
        return s;
    }
}
