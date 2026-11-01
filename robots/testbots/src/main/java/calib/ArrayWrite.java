// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: sequential double[] writes. */
public class ArrayWrite extends Bench {

    private final double[] a = new double[4096];

    protected int iterations() {
        return 200000;
    }

    protected double work(int n) {
        double[] a = this.a;
        for (int i = 0; i < n; i++) {
            a[i & 4095] = i * 0.5;
        }
        return a[n & 4095];
    }
}
