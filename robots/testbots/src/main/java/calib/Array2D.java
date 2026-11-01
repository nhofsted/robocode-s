// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: double[][] reads through fields, as neural nets do. */
public class Array2D extends Bench {

    private final double[][] w = new double[64][64]; private final double[] in = new double[64];

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            s += w[i & 63][(i >> 6) & 63] * in[i & 63];
        }
        return s;
    }
}
