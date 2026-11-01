// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: double division. */
public class DoubleDiv extends Bench {

    

    protected int iterations() {
        return 200000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 1; i <= n; i++) {
            s += 1.0 / (i + s);
        }
        return s;
    }
}
