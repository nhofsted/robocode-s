// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: Math.exp, Math.log and Math.pow. */
public class ExpLog extends Bench {

    

    protected int iterations() {
        return 20000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 1; i <= n; i++) {
            s += Math.exp(-i * 1e-4) + Math.log(i) + Math.pow(1.0001, i);
        }
        return s;
    }
}
