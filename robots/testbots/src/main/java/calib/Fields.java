// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: instance field reads and writes. */
public class Fields extends Bench {

    private double f1 = 1, f2 = 2; private int c;

    protected int iterations() {
        return 200000;
    }

    protected double work(int n) {
        for (int i = 0; i < n; i++) {
            f1 = f1 * 0.5 + f2;
            c++;
        }
        return f1 + c;
    }
}
