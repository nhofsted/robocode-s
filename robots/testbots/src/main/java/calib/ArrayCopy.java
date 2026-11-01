// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: System.arraycopy of 1000 doubles. */
public class ArrayCopy extends Bench {

    private final double[] a = new double[1000], b = new double[1000];

    protected int iterations() {
        return 2000;
    }

    protected double work(int n) {
        for (int i = 0; i < n; i++) {
            System.arraycopy(a, 0, b, 0, 1000);
            a[i % 1000] = i;
        }
        return b[n % 1000];
    }
}
