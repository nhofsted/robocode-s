/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.battle.peer.RobotPeer;
import net.sf.robocode.host.security.RobotClassLoader;
import net.sf.robocode.peer.ExecCommands;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;

import static io.github.nhofsted.robocodes.determinism.RobotRandomness.TURN_BUDGET_NANOS;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The turn budget: {@code lateTurns}, and the agent's patch of classic's RobotPeer. Runs with the agent installed. */
class PeerPatchTest {

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    @Test
    void noLateTurnsBelowTheBudget() {
        assertEquals(0, RobotRandomness.lateTurns(0, 2));
        assertEquals(0, RobotRandomness.lateTurns(TURN_BUDGET_NANOS - 1, 2));
    }

    @Test
    void oneLateTurnFromTheBudgetOn() {
        assertEquals(1, RobotRandomness.lateTurns(TURN_BUDGET_NANOS, 2));
        assertEquals(1, RobotRandomness.lateTurns(2 * TURN_BUDGET_NANOS - 1, 2));
    }

    @Test
    void anotherLateTurnPerBudget() {
        assertEquals(2, RobotRandomness.lateTurns(2 * TURN_BUDGET_NANOS, 5));
        assertEquals(4, RobotRandomness.lateTurns(4 * TURN_BUDGET_NANOS + 1, 5));
    }

    @Test
    void theFirstTurnAllowsSevenBudgets() {
        assertEquals(0, RobotRandomness.lateTurns(7 * TURN_BUDGET_NANOS - 1, 1));
        assertEquals(1, RobotRandomness.lateTurns(7 * TURN_BUDGET_NANOS, 1));
        assertEquals(3, RobotRandomness.lateTurns(9 * TURN_BUDGET_NANOS, 1));
    }

    @Test
    void turnZeroIsNeverLate() {
        assertEquals(0, RobotRandomness.lateTurns(0, 0));
        assertEquals(0, RobotRandomness.lateTurns(100 * TURN_BUDGET_NANOS, 0));
    }

    /** Robot code doing counted work. */
    public static class Busy implements java.util.function.IntConsumer {
        @Override
        public void accept(int iterations) {
            long sum = 0;
            for (int i = 0; i < iterations; i++) {
                sum += i % 7;
            }
            if (sum < 0) {
                throw new IllegalStateException();
            }
        }
    }

    @Test
    void patchedPeerLoadsAndRunsItsMethods() {
        // Loaded by the application loader, so the agent patched it (and the verifier passed it).
        RobotPeer peer = new RobotPeer();
        peer.startRound(0, 0);
        peer.currentExecutionTime = 2;
        peer.executeImpl(new ExecCommands());
        peer.waitWakeup();
        assertEquals(1, peer.rounds);
        assertEquals(1, peer.executions);
        assertEquals(1, peer.wakeups, "a peer that did no work is not asleep");
    }

    @Test
    void aPeerThatDidTooMuchWorkSleepsThroughItsLateTurns() throws Exception {
        RobotPeer peer = new RobotPeer();
        peer.startRound(0, 0);
        ClassLoader loader = new RobotClassLoader(PeerPatchTest.class.getClassLoader(), Set.of(Busy.class.getName()));
        AtomicReference<Throwable> failure = new AtomicReference<>();
        ExecCommands before = new ExecCommands();
        ExecCommands late = new ExecCommands();
        Thread thread = new Thread(new ThreadGroup("sample.Fire (30)"), () -> {
            try {
                var busy = (java.util.function.IntConsumer) loader.loadClass(Busy.class.getName())
                        .getDeclaredConstructor().newInstance();
                peer.currentExecutionTime = 2;
                peer.executeImpl(before); // the turn's work starts here
                busy.accept(20_000_000);
                peer.executeImpl(late);
            } catch (Throwable t) {
                failure.set(t);
            }
        });
        thread.start();
        thread.join();
        assertNull(failure.get());
        assertEquals(2, peer.executions);

        // Classic's turns: load the commands, then wake the robots.
        int asleep = 0;
        while (asleep < 1000) {
            peer.performLoadCommands();
            int woken = peer.wakeups;
            peer.waitWakeup();
            if (peer.wakeups > woken) {
                break;
            }
            assertSame(before, peer.currentCommands, "late: the battle keeps its old commands");
            asleep++;
        }
        assertTrue(asleep >= 1, "the late robot skipped turns");
        assertTrue(asleep < 1000, "and then woke up");
        // The turn after its late turns loads its new commands and wakes it.
        assertSame(late, peer.currentCommands, "its new commands load when it answers");
        peer.performLoadCommands();
        assertSame(late, peer.currentCommands);
        peer.waitWakeup();
        assertEquals(2, peer.wakeups, "awake again afterwards");
    }
}
