// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: Math.sin and Math.cos. */
public class Trig extends Bench {

    

    protected int iterations() {
        return 20000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            s += Math.sin(i * 0.001) + Math.cos(i * 0.002);
        }
        return s;
    }
}
