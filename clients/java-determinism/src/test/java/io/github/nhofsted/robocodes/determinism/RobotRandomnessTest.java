/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Random;
import java.util.Set;
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

    /** Robot code (loaded by the robot class loader) that starts threads in the ways robots do. */
    public static final class StartsThread implements Supplier<Object> {
        public Object get() {
            Thread t = new Thread(() -> { });
            t.start();
            join(t);
            return null;
        }
    }

    public static final class ExtendsThread implements Supplier<Object> {
        public static final class Writer extends Thread {
        }

        public Object get() {
            Writer w = new Writer();
            w.start();
            join(w);
            return null;
        }
    }

    public static final class StartsTimer implements Supplier<Object> {
        public Object get() {
            new java.util.Timer(true).cancel();
            return null;
        }
    }

    public static final class StartsExecutor implements Supplier<Object> {
        public Object get() {
            var pool = java.util.concurrent.Executors.newSingleThreadExecutor();
            try {
                return pool.submit(() -> 1).get();
            } catch (Exception e) {
                throw new IllegalStateException(e);
            } finally {
                pool.shutdown();
            }
        }
    }

    /** Runs {@code code} in a robot's thread, as robot code (loaded by a robot class loader) or not. */
    @SuppressWarnings("unchecked")
    private static boolean startedIn(String robot, Class<? extends Supplier<Object>> code, boolean asRobot)
            throws Exception {
        Supplier<Object> body;
        if (asRobot) {
            ClassLoader loader = new RobotClassLoader(RobotRandomnessTest.class.getClassLoader(),
                    Set.of(code.getName(), code.getName() + "$Writer"));
            body = (Supplier<Object>) loader.loadClass(code.getName()).getDeclaredConstructor().newInstance();
        } else {
            body = code.getDeclaredConstructor().newInstance();
        }
        inRobot(robot, body);
        return RobotRandomness.robotsWithThreads().contains(robot);
    }

    @Test
    void threadsStartedByRobotCodeAreReported() throws Exception {
        assertFalse(RobotRandomness.startedThreads());
        // A robot's thread starting a thread for code that isn't the robot's (Java2D's disposer
        // when it paints, classic's helpers) isn't reported; neither is any other thread group.
        assertFalse(startedIn("sample.Fire", StartsThread.class, false));
        inGroup("Battle", () -> new StartsThread().get());
        assertFalse(RobotRandomness.startedThreads());
        // The robot's code, directly or with a subclass of Thread: the scheduler runs those
        // threads deterministically (ADR 0025), so they aren't reported.
        assertFalse(startedIn("sample.Fire", StartsThread.class, true));
        assertFalse(startedIn("jk.mega.DrussGT 3.1.16", ExtendsThread.class, true));
        assertFalse(RobotRandomness.startedThreads());
        // A Timer's thread and an executor's are the robot's too: scheduled, not reported.
        assertFalse(startedIn("sample.Fire (2)", StartsTimer.class, true));
        assertFalse(startedIn("sample.Fire (3)", StartsExecutor.class, true));
        assertFalse(RobotRandomness.startedThreads());
        assertFalse(RobotRandomness.robotsWithThreads().contains("Battle"));
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
        assertEquals(GOLDEN_UTILS_SEED, RobotRandomness.utilsRandomSeed(base));
    }

    @Test
    @SuppressWarnings("unchecked")
    void utilsRandomHookGivesOneInstancePerRobot() throws Exception {
        Supplier<Random> hook = (Supplier<Random>) System.getProperties().get(RobotRandomness.UTILS_RANDOM_KEY);
        Random[] fire = inRobot("sample.Fire", () -> new Random[] {hook.get(), hook.get()});
        Random[] other = inRobot("jk.mega.DrussGT 3.1.16", () -> new Random[] {hook.get()});
        assertTrue(fire[0] == fire[1], "same instance for the same robot");
        assertTrue(fire[0] != other[0], "different robots get different instances");
        assertEquals(null, inGroup("Battle", hook::get), "no instance outside robot threads");
        long expected = new Random(RobotRandomness.utilsRandomSeed(RobotRandomness.baseSeed(SEED, "sample.Fire"))).nextLong();
        assertEquals(expected, inRobot("sample.Fire", () -> hook.get().nextLong()));
    }

    private static final long GOLDEN_BASE = 4017542787009561894L;
    private static final long GOLDEN_MATH_SEED = -7329177522749448640L;
    private static final long GOLDEN_FIRST_SEED = 5799085141444697617L;
    private static final long GOLDEN_UTILS_SEED = -3133940015624230923L;

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
