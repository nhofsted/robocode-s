// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: Math.sqrt, Math.abs and Math.min. */
public class Sqrt extends Bench {

    

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            s += Math.sqrt(i) + Math.abs(s - i) * 1e-9 + Math.min(i, 3);
        }
        return s;
    }
}
