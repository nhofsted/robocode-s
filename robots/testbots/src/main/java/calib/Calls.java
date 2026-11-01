// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: small static calls. */
public class Calls extends Bench {

    private static int f(int x) {
        return x * 3 + 1;
    }

    protected int iterations() {
        return 200000;
    }

    protected double work(int n) {
        int s = 0;
        for (int i = 0; i < n; i++) {
            s += f(i);
        }
        return s;
    }
}
