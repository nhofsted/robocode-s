// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: double add and multiply on locals. */
public class DoubleLoop extends Bench {

    

    protected int iterations() {
        return 300000;
    }

    protected double work(int n) {
        double x = 1.0001, s = 0;
        for (int i = 0; i < n; i++) {
            s = s * 0.999 + x;
            x += 1e-9;
        }
        return s;
    }
}
