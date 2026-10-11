/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** Work metering and the virtual clock (ADR 0014). Runs with the agent installed. */
class WorkMeterTest {

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    /** Robot code that measures its own work with the clock. */
    public static class Worker implements java.util.function.Function<Integer, long[]> {
        @Override
        public long[] apply(Integer iterations) {
            long start = System.nanoTime();
            long sum = 0;
            for (int i = 0; i < iterations; i++) {
                sum += i % 7;
            }
            long end = System.nanoTime();
            return new long[] {start, end, System.currentTimeMillis(), sum};
        }
    }

    @SuppressWarnings("unchecked")
    private static long[] measure(String robot, int iterations) throws Exception {
        ClassLoader loader = new RobotClassLoader(WorkMeterTest.class.getClassLoader(), Set.of(Worker.class.getName()));
        Class<?> worker = loader.loadClass(Worker.class.getName());
        AtomicReference<long[]> result = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup(robot), () -> {
            try {
                var f = (java.util.function.Function<Integer, long[]>) worker.getDeclaredConstructor().newInstance();
                result.set(f.apply(iterations));
            } catch (Throwable t) {
                failure.set(t);
            }
        });
        thread.start();
        thread.join();
        if (failure.get() != null) {
            throw new AssertionError(failure.get());
        }
        return result.get();
    }

    @Test
    void robotCodeReadsTheVirtualClock() throws Exception {
        long[] r = measure("sample.Fire (20)", 1000);
        assertTrue(r[0] >= RobotRandomness.NANO_ORIGIN && r[0] < RobotRandomness.NANO_ORIGIN + 1_000_000,
                "virtual nanoTime starts at the origin: " + r[0]);
        assertTrue(r[2] >= RobotRandomness.EPOCH_MILLIS && r[2] < RobotRandomness.EPOCH_MILLIS + 1000,
                "virtual currentTimeMillis starts at the epoch: " + r[2]);
    }

    @Test
    void elapsedTimeFollowsTheCountedWork() throws Exception {
        long[] small = measure("sample.Fire (21)", 1000);
        long[] large = measure("sample.Fire (22)", 2000);
        long smallNanos = small[1] - small[0];
        long largeNanos = large[1] - large[0];
        assertTrue(smallNanos > 0, "work takes time");
        // Twice the loop iterations: twice the time, up to the code around the loop.
        assertEquals(2.0, (double) largeNanos / smallNanos, 0.05);
    }

    /** A branch inside constructor arguments: stack map frames with an uninitialized object. */
    public static class Constructing implements java.util.function.Function<Integer, long[]> {
        @Override
        public long[] apply(Integer n) {
            StringBuilder b = new StringBuilder(n > 0 ? "positive" : "other");
            return new long[] {b.length()};
        }
    }

    @Test
    @SuppressWarnings("unchecked")
    void classesWithUninitializedFramesStillLoad() throws Exception {
        ClassLoader loader = new RobotClassLoader(WorkMeterTest.class.getClassLoader(),
                Set.of(Constructing.class.getName()));
        var f = (java.util.function.Function<Integer, long[]>) loader.loadClass(Constructing.class.getName())
                .getDeclaredConstructor().newInstance();
        assertEquals(8, f.apply(1)[0]);
        assertEquals(5, f.apply(0)[0]);
    }

    @Test
    void theSameWorkTakesTheSameVirtualTime() throws Exception {
        long[] first = measure("sample.Fire (23)", 1500);
        long[] second = measure("sample.Fire (24)", 1500);
        assertArrayEquals(first, second, "fresh robots doing the same work see the same clock");
    }
}
