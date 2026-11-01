// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: long division and remainder. */
public class LongDiv extends Bench {

    

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        long a = 123456789L, s = 0;
        for (int i = 1; i <= n; i++) {
            s += a / i + a % (i + 7);
        }
        return s;
    }
}
