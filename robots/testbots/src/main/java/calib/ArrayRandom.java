// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: random reads in a 32 MB array. */
public class ArrayRandom extends Bench {

    private final double[] a = new double[4 << 20];

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        double s = 0;
        double[] a = this.a;
        int j = 12345;
        for (int i = 0; i < n; i++) {
            j = j * 1103515245 + 12345;
            s += a[(j >>> 8) & ((4 << 20) - 1)];
        }
        return s;
    }
}
