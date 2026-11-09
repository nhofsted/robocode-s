// SPDX-License-Identifier: EPL-2.0
package testbots;

/** Works 800 ms, just over classic's 756 ms: misses turn 1's wait too, 8 turns late (ADR 0028). */
public class SlowStart800 extends SlowStart {

    protected long startNanos() {
        return 800_000_000L;
    }
}
