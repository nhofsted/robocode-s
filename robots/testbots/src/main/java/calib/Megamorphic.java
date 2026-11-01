// SPDX-License-Identifier: EPL-2.0
package calib;

/** Calibration benchmark: virtual calls on three classes. */
public class Megamorphic extends Bench {

    abstract static class Op {
        abstract double apply(double x);
    }
    static final class A extends Op {
        double apply(double x) { return x + 1; }
    }
    static final class B extends Op {
        double apply(double x) { return x * 0.5; }
    }
    static final class C extends Op {
        double apply(double x) { return x - 0.25; }
    }
    private final Op[] ops = {new A(), new B(), new C()};

    protected int iterations() {
        return 100000;
    }

    protected double work(int n) {
        double s = 1;
        Op[] ops = this.ops;
        for (int i = 0; i < n; i++) {
            s = ops[i % 3].apply(s);
        }
        return s;
    }
}
