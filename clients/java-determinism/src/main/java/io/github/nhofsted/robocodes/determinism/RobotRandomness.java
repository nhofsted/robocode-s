/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.function.DoubleSupplier;
import java.util.function.Supplier;

/**
 * Per-robot random number streams (design §7.2).
 *
 * <p>Each robot gets its own {@code Math.random()} stream and its own sequence of
 * seeds for {@code new Random()}, derived only from the battle seed and the
 * robot's unique name. The streams are therefore the same whether all robots
 * share one JVM (classic) or each robot has its own JVM (robocode-s).
 *
 * <p>Derivation (keep in sync with every runner that implements it):
 * <pre>
 *   base       = mix64(battleSeed) ^ mix64(fnv1a64(utf8(robotName)))
 *   Math.random: new java.util.Random(mix64(base ^ 0x4D41544852414E44L)).nextDouble()
 *   k-th new Random() (k = 1, 2, ...): seed = mix64(base + k * 0x9E3779B97F4A7C15L)
 *   Utils.getRandom(): one new java.util.Random(mix64(base ^ 0x5554494C53524E44L)) per robot
 * </pre>
 * where {@code mix64} is the SplitMix64 finalizer. Calls from threads that do
 * not belong to a robot keep the JDK's normal behavior.
 *
 * <p>A robot is identified by the name of its thread group or the nearest
 * ancestor group with a registered robot name; classic names each robot's
 * thread group after the robot, and threads a robot starts inherit it.
 */
public final class RobotRandomness {

    /** System property key of the {@code Math.random()} hook (a {@link DoubleSupplier}). */
    static final String MATH_RANDOM_KEY = "robocodes.determinism.mathRandom";
    /** System property key of the {@code new Random()} seed hook (a {@code Supplier<Long>}). */
    static final String RANDOM_SEED_KEY = "robocodes.determinism.randomSeed";
    /**
     * System property key of the {@code robocode.util.Utils.getRandom()} hook (a
     * {@code Supplier<Random>}). Classic returns the engine's generator there; robocode-s
     * gives each robot its own instance (engine-spec §15.1). The classic oracle installs a
     * patch that reads this hook.
     */
    public static final String UTILS_RANDOM_KEY = "robocodes.determinism.utilsRandom";

    private static final long MATH_SALT = 0x4D41544852414E44L; // "MATHRAND"
    private static final long UTILS_SALT = 0x5554494C53524E44L; // "UTILSRND"
    private static final long GOLDEN_GAMMA = 0x9E3779B97F4A7C15L;

    private static volatile boolean agentInstalled;

    private final long battleSeed;
    private final List<String> robotNames;
    private final Map<String, Stream> streams = new HashMap<>();

    private RobotRandomness(long battleSeed, Collection<String> robotNames) {
        this.battleSeed = battleSeed;
        this.robotNames = new ArrayList<>(robotNames);
    }

    /**
     * Activates per-robot streams for a battle. Robots are recognized by thread
     * group names equal to one of {@code robotNames}, or starting with
     * {@code name + " ("} (classic's naming of duplicate robots).
     *
     * @throws IllegalStateException if the agent is not installed
     */
    public static void configure(long battleSeed, Collection<String> robotNames) {
        if (!agentInstalled) {
            throw new IllegalStateException("robocode-s determinism agent is not installed (-javaagent)");
        }
        RobotRandomness randomness = new RobotRandomness(battleSeed, robotNames);
        System.getProperties().put(MATH_RANDOM_KEY, (DoubleSupplier) randomness::mathRandom);
        System.getProperties().put(RANDOM_SEED_KEY, (Supplier<Long>) randomness::nextSeed);
        System.getProperties().put(UTILS_RANDOM_KEY, (Supplier<Random>) randomness::utilsRandom);
    }

    /** Whether the agent rewrote {@code Math.random()} and {@code new Random()}. */
    public static boolean isAgentInstalled() {
        return agentInstalled;
    }

    static void markAgentInstalled() {
        agentInstalled = true;
    }

    /** Next value for a robot's {@code Math.random()}, or NaN outside robot threads. */
    private double mathRandom() {
        Stream stream = currentStream();
        return stream == null ? Double.NaN : stream.mathRandom();
    }

    /** Seed for a robot's next {@code new Random()}, or null outside robot threads. */
    private Long nextSeed() {
        Stream stream = currentStream();
        return stream == null ? null : stream.nextSeed();
    }

    /** The robot's {@code Utils.getRandom()} instance, or null outside robot threads. */
    private Random utilsRandom() {
        Stream stream = currentStream();
        return stream == null ? null : stream.utils;
    }

    private Stream currentStream() {
        for (ThreadGroup group = Thread.currentThread().getThreadGroup(); group != null; group = group.getParent()) {
            String name = group.getName();
            if (isRobot(name)) {
                synchronized (streams) {
                    return streams.computeIfAbsent(name, n -> new Stream(baseSeed(battleSeed, n)));
                }
            }
        }
        return null;
    }

    private boolean isRobot(String groupName) {
        for (String robot : robotNames) {
            if (groupName.equals(robot) || groupName.startsWith(robot + " (")) {
                return true;
            }
        }
        return false;
    }

    /** The per-robot base seed. */
    public static long baseSeed(long battleSeed, String robotName) {
        return mix64(battleSeed) ^ mix64(fnv1a64(robotName.getBytes(StandardCharsets.UTF_8)));
    }

    /** Seed of the k-th (1-based) {@code new Random()} a robot creates. */
    public static long randomSeed(long baseSeed, long k) {
        return mix64(baseSeed + k * GOLDEN_GAMMA);
    }

    /** Seed of the {@link Random} behind a robot's {@code Math.random()}. */
    public static long mathRandomSeed(long baseSeed) {
        return mix64(baseSeed ^ MATH_SALT);
    }

    /** Seed of the {@link Random} a robot gets from {@code Utils.getRandom()}. */
    public static long utilsRandomSeed(long baseSeed) {
        return mix64(baseSeed ^ UTILS_SALT);
    }

    /** SplitMix64 finalizer. */
    static long mix64(long z) {
        z = (z ^ (z >>> 30)) * 0xBF58476D1CE4E5B9L;
        z = (z ^ (z >>> 27)) * 0x94D049BB133111EBL;
        return z ^ (z >>> 31);
    }

    static long fnv1a64(byte[] bytes) {
        long hash = 0xCBF29CE484222325L;
        for (byte b : bytes) {
            hash ^= b & 0xFF;
            hash *= 0x100000001B3L;
        }
        return hash;
    }

    /** One robot's streams. */
    private static final class Stream {
        private final long base;
        private final Random math;
        final Random utils;
        private long created;

        Stream(long base) {
            this.base = base;
            this.math = new Random(mathRandomSeed(base));
            this.utils = new Random(utilsRandomSeed(base));
        }

        synchronized double mathRandom() {
            return math.nextDouble();
        }

        synchronized long nextSeed() {
            return randomSeed(base, ++created);
        }
    }
}
