/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.ObjectStreamClass;
import java.io.Serializable;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotSame;

/** Deterministic identity hash codes (ADR 0013). Runs with the agent installed. */
class IdentityHashesTest {

    private static final long SEED = 42;

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(SEED, List.of("sample.Fire"));
    }

    /** Golden values, cross-checked with an independent Python implementation. */
    @Test
    void derivationGoldenValues() {
        long base = RobotRandomness.baseSeed(42, "sample.Fire");
        assertEquals(1056479175, RobotRandomness.identityHashCode(base, 1));
        assertEquals(1831160965, RobotRandomness.identityHashCode(base, 2));
    }

    /** A robot's own enum. */
    public enum Colour { RED, GREEN }

    @Test
    void robotEnumConstantsFirstHashedByTheRobotFollowItsStream() throws Exception {
        String robot = "sample.Fire (7)";
        Class<?> colour = robotClass(Colour.class);
        Object[] constants = colour.getEnumConstants();
        int[] hashes = inGroup(robot, () -> new int[] {
                constants[1].hashCode(), constants[0].hashCode(), constants[1].hashCode()});
        long base = RobotRandomness.baseSeed(SEED, robot);
        assertArrayEquals(new int[] {
                RobotRandomness.identityHashCode(base, 1),
                RobotRandomness.identityHashCode(base, 2),
                RobotRandomness.identityHashCode(base, 1)}, hashes);
    }

    /** JDK enums keep the JVM's identity hash, also on a robot thread. */
    @Test
    void otherEnumConstantsKeepTheJvmsHash() throws Exception {
        String robot = "sample.Fire (10)";
        int hash = inGroup(robot, () -> java.time.DayOfWeek.FRIDAY.hashCode());
        assertEquals(System.identityHashCode(java.time.DayOfWeek.FRIDAY), hash);
        int robotObject = inGroup(robot, () -> newInstance(robotClassUnchecked(Wave.class)).hashCode());
        assertEquals(RobotRandomness.identityHashCode(RobotRandomness.baseSeed(SEED, robot), 1), robotObject,
                "the JDK enum did not use the robot's stream");
    }

    private static Class<?> robotClassUnchecked(Class<?> original) {
        try {
            return robotClass(original);
        } catch (ClassNotFoundException e) {
            throw new IllegalStateException(e);
        }
    }

    /** A robot's own class without hashCode(); Serializable without a serialVersionUID. */
    public static class Wave implements Serializable {
        double distance;
    }

    /** A robot class with its own hashCode(), which must stay. */
    public static class Enemy {
        @Override
        public int hashCode() {
            return 7;
        }
    }

    @Test
    void robotObjectsGetHashesFromTheRobotsStream() throws Exception {
        Class<?> wave = robotClass(Wave.class);
        String robot = "sample.Fire (8)";
        int[] hashes = inGroup(robot, () -> {
            Object first = newInstance(wave);
            Object second = newInstance(wave);
            return new int[] {second.hashCode(), first.hashCode(), second.hashCode()};
        });
        long base = RobotRandomness.baseSeed(SEED, robot);
        assertArrayEquals(new int[] {
                RobotRandomness.identityHashCode(base, 1),
                RobotRandomness.identityHashCode(base, 2),
                RobotRandomness.identityHashCode(base, 1)}, hashes);
    }

    @Test
    void ownHashCodeStays() throws Exception {
        Class<?> enemy = robotClass(Enemy.class);
        assertEquals(7, inGroup("sample.Fire (9)", () -> newInstance(enemy).hashCode()));
    }

    @Test
    void serialVersionUidDoesNotChange() throws Exception {
        Class<?> wave = robotClass(Wave.class);
        assertNotSame(Wave.class, wave);
        assertEquals(ObjectStreamClass.lookup(Wave.class).getSerialVersionUID(),
                ObjectStreamClass.lookup(wave).getSerialVersionUID());
    }

    /** Arrays as keys of the JDK's hash collections, hashed on a robot thread. */
    @Test
    void robotArraysGetTableHashesInHashCollections() throws Exception {
        String robot = "sample.Fire (11)";
        int[] hashes = inGroup(robot, () -> {
            double[] first = {1};
            double[] second = {2};
            java.util.Map<double[], Integer> map = new java.util.HashMap<>();
            map.put(second, 2);
            map.put(first, 1);
            return new int[] {java.util.Objects.hashCode(first), java.util.Objects.hashCode(second),
                    map.get(first), new java.util.concurrent.ConcurrentHashMap<>(map).get(second)};
        });
        long base = RobotRandomness.baseSeed(SEED, robot);
        assertArrayEquals(new int[] {
                RobotRandomness.tableHashCode(base, 2),
                RobotRandomness.tableHashCode(base, 1), 1, 2}, hashes);
        double[] elsewhere = {3};
        assertEquals(System.identityHashCode(elsewhere), java.util.Objects.hashCode(elsewhere),
                "off robot threads, arrays keep the JVM's hash");
    }

    /** Robot code that reaches the JVM's identity hash directly. */
    public static class Hasher implements Supplier<int[]> {
        @Override
        public int[] get() {
            int[] array = {1};
            Object object = new Object();
            return new int[] {array.hashCode(), System.identityHashCode(object), ((Object) array).hashCode(),
                    new Hasher().super_hashCode(), System.identityHashCode(array)};
        }

        int super_hashCode() {
            return super.hashCode();
        }

        @Override
        public int hashCode() {
            return super.hashCode();
        }
    }

    @Test
    void robotCodeAsksTheTableForIdentityHashes() throws Exception {
        String robot = "sample.Fire (12)";
        Class<?> hasher = robotClass(Hasher.class);
        @SuppressWarnings("unchecked")
        int[] hashes = inGroup(robot, () -> ((Supplier<int[]>) newInstance(hasher)).get());
        long base = RobotRandomness.baseSeed(SEED, robot);
        assertArrayEquals(new int[] {
                RobotRandomness.tableHashCode(base, 1),
                RobotRandomness.tableHashCode(base, 2),
                RobotRandomness.tableHashCode(base, 1),
                RobotRandomness.tableHashCode(base, 3),
                RobotRandomness.tableHashCode(base, 1)}, hashes);
    }

    private static Class<?> robotClass(Class<?> original) throws ClassNotFoundException {
        ClassLoader loader = new RobotClassLoader(IdentityHashesTest.class.getClassLoader(), Set.of(original.getName()));
        return loader.loadClass(original.getName());
    }

    private static Object newInstance(Class<?> type) {
        try {
            return type.getDeclaredConstructor().newInstance();
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException(e);
        }
    }

    private static <T> T inGroup(String group, Supplier<T> body) throws Exception {
        AtomicReference<T> result = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup(group), () -> {
            try {
                result.set(body.get());
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
}
