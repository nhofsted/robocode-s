// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: int arithmetic on locals. */
public class IntLoop extends Bench {

    

    protected int iterations() {
        return 300000;
    }

    protected double work(int n) {
        int a = 1, b = 7;
        for (int i = 0; i < n; i++) {
            a = a * 31 + i;
            b ^= a >>> 3;
        }
        return a + b;
    }
}
