// SPDX-License-Identifier: EPL-2.0
package testbots;

/** Works a second: 71 turns late, disabled for it in the first round (ADR 0028). */
public class SlowStart1000 extends SlowStart {

    protected long startNanos() {
        return 1000_000_000L;
    }
}
