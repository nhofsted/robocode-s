// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: Math.atan2. */
public class Atan2 extends Bench {

    

    protected int iterations() {
        return 20000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            s += Math.atan2(i * 0.5 - 300, 200 - i * 0.25);
        }
        return s;
    }
}
