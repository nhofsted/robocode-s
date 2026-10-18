/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.IntUnaryOperator;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The work limit of a robot: counting past it throws, and robot code cannot swallow that. Agent installed. */
class FuelLimitTest {

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    /** Robot code with a loop. */
    public static class Looper implements IntUnaryOperator {
        @Override
        public int applyAsInt(int n) {
            int sum = 0;
            for (int i = 0; i < n; i++) {
                sum += i % 7;
            }
            return sum;
        }
    }

    /** Robot code that swallows everything its loop throws, and loops again. */
    public static class Swallower implements IntUnaryOperator {
        @Override
        public int applyAsInt(int n) {
            int sum = 0;
            while (true) {
                try {
                    sum += new Looper().applyAsInt(n);
                } catch (Throwable t) {
                    sum++;
                }
            }
        }
    }

    /** Robot code whose loop runs in a {@code try} with a {@code finally}. */
    public static class Finally implements IntUnaryOperator {
        static int cleanups;

        @Override
        public int applyAsInt(int n) {
            try {
                return new Looper().applyAsInt(n);
            } finally {
                cleanups++;
            }
        }
    }

    /** Robot code whose loop runs in a {@code synchronized} block. */
    public static class Synchronized implements IntUnaryOperator {
        @Override
        public int applyAsInt(int n) {
            synchronized (this) {
                return new Looper().applyAsInt(n);
            }
        }
    }

    /** A robot in its own loader, with its fuel array. */
    private static final class Robot {
        final IntUnaryOperator code;
        final long[] fuel;

        Robot(Class<?> robotClass) throws Exception {
            ClassLoader loader = new RobotClassLoader(FuelLimitTest.class.getClassLoader(),
                    Set.copyOf(List.of(robotClass.getName(), Looper.class.getName())));
            Class<?> c = loader.loadClass(robotClass.getName());
            code = (IntUnaryOperator) c.getDeclaredConstructor().newInstance();
            Field f = c.getDeclaredField("robocodes$fuel");
            if (robotClass != Looper.class) {
                // Robot classes of one robot share its counter: load the loop too.
                loader.loadClass(Looper.class.getName());
            }
            f.setAccessible(true);
            fuel = (long[]) f.get(null);
        }
    }

    private interface Body {
        void run() throws Exception;
    }

    /** Runs the body on a robot thread; returns what it threw, or null. */
    private static Throwable onRobotThread(String group, Body body) throws Exception {
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup(group), () -> {
            try {
                body.run();
            } catch (Throwable t) {
                failure.set(t);
            }
        });
        thread.setDaemon(true);
        thread.start();
        thread.join(10_000);
        assertTrue(!thread.isAlive(), "the robot's code ended");
        return failure.get();
    }

    /** Lowers the limit by {@code allowance}, runs, returns {count, limit} at the throw. */
    private static long[] runToLimit(String group, long allowance) throws Exception {
        return runToLimit(group, Looper.class, allowance);
    }

    private static long[] runToLimit(String group, Class<?> robotClass, long allowance) throws Exception {
        long[][] at = new long[1][];
        Throwable t = onRobotThread(group, () -> {
            Robot robot = new Robot(robotClass);
            RobotRandomness.limit(robot.fuel, RobotRandomness.count(robot.fuel) + allowance);
            try {
                robot.code.applyAsInt(Integer.MAX_VALUE);
            } finally {
                at[0] = new long[] {RobotRandomness.count(robot.fuel), robot.fuel[0]};
            }
        });
        assertInstanceOf(ArrayIndexOutOfBoundsException.class, t);
        return at[0];
    }

    @Test
    void passingTheLimitThrowsAtADeterministicPoint() throws Exception {
        long[] first = runToLimit("sample.Fire (40)", 10_000);
        long[] second = runToLimit("sample.Fire (41)", 10_000);
        assertEquals(first[0], second[0], "same count at the throw");
        // A loop's start checks: the count is within one pass of the loop from the limit.
        assertTrue(Math.abs(first[0] - first[1]) < 100, "at the limit: " + first[0] + " vs " + first[1]);
    }

    @Test
    void robotCodeCannotCatchIt() throws Exception {
        runToLimit("sample.Fire (46)", Swallower.class, 10_000);
    }

    @Test
    void finallyBlocksRunOnceAndPassItOn() throws Exception {
        runToLimit("sample.Fire (47)", Finally.class, 10_000);
    }

    @Test
    void synchronizedBlocksPassItOn() throws Exception {
        runToLimit("sample.Fire (48)", Synchronized.class, 10_000);
    }

    @Test
    void belowTheLimitNothingChanges() throws Exception {
        AtomicReference<Integer> result = new AtomicReference<>();
        Throwable t = onRobotThread("sample.Fire (44)", () -> {
            Robot robot = new Robot(Looper.class);
            RobotRandomness.limit(robot.fuel, RobotRandomness.count(robot.fuel) + 1_000_000_000L);
            result.set(robot.code.applyAsInt(1000));
        });
        assertNull(t);
        int expected = 0;
        for (int i = 0; i < 1000; i++) {
            expected += i % 7;
        }
        assertEquals(expected, result.get());
    }

    @Test
    void aTurnMovesTheLimitToTheCountPlusTheCutoff() throws Exception {
        long[][] seen = new long[1][];
        Throwable t = onRobotThread("sample.Fire (45)", () -> {
            Robot robot = new Robot(Looper.class);
            robot.code.applyAsInt(500);
            RobotRandomness.limit(robot.fuel, RobotRandomness.count(robot.fuel) + 10);
            RobotRandomness.turn();
            seen[0] = new long[] {RobotRandomness.count(robot.fuel), robot.fuel[0]};
        });
        assertNull(t);
        assertTrue(seen[0][0] > 0, "work was counted");
        assertEquals(seen[0][0] + RobotRandomness.CUTOFF_INSTRUCTIONS, seen[0][1]);
    }
}
