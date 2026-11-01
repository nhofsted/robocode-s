// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: HashMap.get and put with boxed keys. */
public class HashMapGet extends Bench {

    private final java.util.HashMap<Integer, Double> map = new java.util.HashMap<>();

    protected int iterations() {
        return 50000;
    }

    protected double work(int n) {
        double s = 0;
        for (int i = 0; i < n; i++) {
            Integer k = i & 1023;
            Double v = map.get(k);
            s += v == null ? 0 : v;
            map.put(k, s * 1e-9);
        }
        return s;
    }
}
