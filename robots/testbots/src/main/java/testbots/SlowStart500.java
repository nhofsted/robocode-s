// SPDX-License-Identifier: EPL-2.0
package testbots;

/** Works about half a second, within classic's 756 ms: never late (ADR 0028). */
public class SlowStart500 extends SlowStart {

    protected long startNanos() {
        return 500_000_000L;
    }
}
