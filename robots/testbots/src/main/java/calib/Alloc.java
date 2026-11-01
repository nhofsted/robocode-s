// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: small object allocation. */
public class Alloc extends Bench {

    static final class P {
        final double x, y;
        P(double x, double y) { this.x = x; this.y = y; }
    }

    protected int iterations() {
        return 50000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            P p = new P(i, s);
            s += p.x * 1e-9 + p.y * 1e-9;
        }
        return s;
    }
}
