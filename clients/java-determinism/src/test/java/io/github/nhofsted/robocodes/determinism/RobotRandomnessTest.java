/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Random;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** Runs with the agent installed (see build.gradle.kts). */
class RobotRandomnessTest {

    private static final long SEED = 42;

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(SEED, List.of("sample.Fire", "jk.mega.DrussGT 3.1.16"));
    }

    @Test
    void agentIsInstalled() {
        assertTrue(RobotRandomness.isAgentInstalled());
    }

    @Test
    void mathRandomFollowsTheRobotsOwnStream() throws Exception {
        double[] actual = inRobot("sample.Fire", () -> new double[] {Math.random(), Math.random(), StrictMath.random()});
        Random expected = new Random(RobotRandomness.mathRandomSeed(RobotRandomness.baseSeed(SEED, "sample.Fire")));
        assertArrayEquals(new double[] {expected.nextDouble(), expected.nextDouble(), expected.nextDouble()}, actual);
    }

    @Test
    void newRandomGetsTheRobotsNextSeed() throws Exception {
        String robot = "jk.mega.DrussGT 3.1.16";
        long[] actual = inRobot(robot, () -> new long[] {new Random().nextLong(), new Random().nextLong()});
        long base = RobotRandomness.baseSeed(SEED, robot);
        assertArrayEquals(new long[] {
                new Random(RobotRandomness.randomSeed(base, 1)).nextLong(),
                new Random(RobotRandomness.randomSeed(base, 2)).nextLong()}, actual);
    }

    @Test
    void threadsStartedByARobotShareItsStream() throws Exception {
        double[] fromChild = inRobot("sample.Fire (2)", () -> {
            AtomicReference<Double> value = new AtomicReference<>();
            Thread child = new Thread(() -> value.set(Math.random()));
            child.start();
            join(child);
            return new double[] {value.get()};
        });
        Random expected = new Random(RobotRandomness.mathRandomSeed(RobotRandomness.baseSeed(SEED, "sample.Fire (2)")));
        assertEquals(expected.nextDouble(), fromChild[0]);
    }

    @Test
    void duplicateRobotsGetDifferentStreams() {
        assertNotEquals(RobotRandomness.baseSeed(SEED, "sample.Fire (1)"), RobotRandomness.baseSeed(SEED, "sample.Fire (2)"));
    }

    @Test
    void nonRobotThreadsKeepJdkBehavior() throws Exception {
        double[] values = inGroup("Battle", () -> new double[] {Math.random(), new Random().nextDouble()});
        Random robotStream = new Random(RobotRandomness.mathRandomSeed(RobotRandomness.baseSeed(SEED, "Battle")));
        assertNotEquals(robotStream.nextDouble(), values[0]);
        assertFalse(Double.isNaN(values[0]));
    }

    /** Golden values, cross-checked with an independent Python implementation. Any other
     * implementation of the derivation (e.g. in Rust) must match these. */
    @Test
    void derivationGoldenValues() {
        long base = RobotRandomness.baseSeed(42, "sample.Fire");
        assertEquals(GOLDEN_BASE, base);
        assertEquals(GOLDEN_MATH_SEED, RobotRandomness.mathRandomSeed(base));
        assertEquals(GOLDEN_FIRST_SEED, RobotRandomness.randomSeed(base, 1));
    }

    private static final long GOLDEN_BASE = 4017542787009561894L;
    private static final long GOLDEN_MATH_SEED = -7329177522749448640L;
    private static final long GOLDEN_FIRST_SEED = 5799085141444697617L;

    private static <T> T inRobot(String robot, Supplier<T> body) throws Exception {
        return inGroup(robot, body);
    }

    private static <T> T inGroup(String group, Supplier<T> body) throws Exception {
        AtomicReference<T> result = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup(group), () -> result.set(body.get()));
        thread.start();
        thread.join();
        return result.get();
    }

    private static void join(Thread thread) {
        try {
            thread.join();
        } catch (InterruptedException e) {
            throw new IllegalStateException(e);
        }
    }
}
