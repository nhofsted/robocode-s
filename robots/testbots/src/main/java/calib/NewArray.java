// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: new double[64] arrays. */
public class NewArray extends Bench {

    

    protected int iterations() {
        return 20000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            double[] a = new double[64];
            a[i & 63] = i;
            s += a[(i + 1) & 63] + a.length;
        }
        return s;
    }
}
