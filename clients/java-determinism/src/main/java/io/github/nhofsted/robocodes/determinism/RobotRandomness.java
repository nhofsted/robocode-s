/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import java.lang.ref.Reference;
import java.lang.ref.ReferenceQueue;
import java.lang.ref.WeakReference;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.WeakHashMap;
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
 *   k-th identity hash code (k = 1, 2, ...):
 *     h = (int) (mix64((base ^ 0x4944454E54495459L) + k * 0x9E3779B97F4A7C15L) >>> 33), 1 if 0
 *   k-th table identity hash code (arrays, super.hashCode(), System.identityHashCode):
 *     the k-th identity hash code above, with base ^ 0x5441424C45484153L as base
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
    /**
     * System property key of the identity hash hook (a {@code ToIntFunction<Object>}), used
     * by robot classes and {@code Enum.hashCode()} as rewritten by the agent (ADR 0013).
     */
    static final String IDENTITY_HASH_KEY = "robocodes.determinism.identityHash";
    /**
     * System property key of the hash code hook for calls that may reach
     * {@code Object.hashCode()} of an array (a {@code ToIntFunction<Object>}): the JDK's hash
     * collections and robot code, as rewritten by the agent (ADR 0013).
     */
    static final String HASH_CODE_KEY = "robocodes.determinism.hashCode";
    /**
     * System property key of the hook for robot code's {@code System.identityHashCode(o)}
     * and {@code super.hashCode()} on {@code Object} (a {@code ToIntFunction<Object>}).
     */
    static final String TABLE_HASH_KEY = "robocodes.determinism.tableHash";
    /** Hook giving robot classes their robot's work counter (a {@code Function<ClassLoader, long[]>}). */
    static final String FUEL_KEY = "robocodes.determinism.fuel";
    /** Virtual {@code System.nanoTime()} for robot code (a {@code ToLongFunction<long[]>}). */
    static final String CLOCK_NANOS_KEY = "robocodes.determinism.clockNanos";
    /** Virtual {@code System.currentTimeMillis()} for robot code (a {@code ToLongFunction<long[]>}). */
    static final String CLOCK_MILLIS_KEY = "robocodes.determinism.clockMillis";
    /** Called by classic's robot side on every turn of a robot thread (a {@code Runnable}). */
    static final String TURN_KEY = "robocodes.determinism.turn";

    /*
     * The virtual clock (ADR 0014). Part of the deterministic rules: changing a constant
     * changes what time-reading robots do.
     */
    /**
     * Virtual time per counted instruction, in picoseconds. Calibrated on 2026-10-13 against
     * classic 1.11.1 on OpenJDK 21 (x86-64, WSL2): robots doing over 100 000 counted
     * instructions per turn take 820-1010 ps each (#15, ADR 0014).
     */
    public static final long PICOS_PER_INSTRUCTION = 900;
    /** Virtual time per turn besides the robot's own work: waiting for the others. */
    public static final long TURN_NANOS = 1_000_000;
    /** Value of the virtual {@code nanoTime()} before the robot did anything. */
    public static final long NANO_ORIGIN = 1_000_000_000_000L;
    /** Value of the virtual {@code currentTimeMillis()} before the robot did anything: 2026-01-01T00:00Z. */
    public static final long EPOCH_MILLIS = 1_767_225_600_000L;

    /** Work counters, one per robot class loader (in classic: one per robot). */
    private static final Map<ClassLoader, long[]> FUEL = new WeakHashMap<>();

    private static final long MATH_SALT = 0x4D41544852414E44L; // "MATHRAND"
    private static final long UTILS_SALT = 0x5554494C53524E44L; // "UTILSRND"
    private static final long HASH_SALT = 0x4944454E54495459L; // "IDENTITY"
    private static final long TABLE_SALT = 0x5441424C45484153L; // "TABLEHAS"
    private static final long GOLDEN_GAMMA = 0x9E3779B97F4A7C15L;

    private static volatile boolean agentInstalled;
    /**
     * Diagnostics: with {@code -Drobocodes.traceDeterminism=<file>}, every identity hash,
     * {@code new Random()} and virtual clock read of a robot is appended to the file with a
     * short stack, to compare runs (e.g. classic and robocode-s) where they diverge.
     */
    private static final String TRACE = System.getProperty("robocodes.traceDeterminism");

    /**
     * Diagnostics: with the trace on, robot code's calls into the Robocode API that return
     * a primitive also trace the value (a {@code BiConsumer<Object, Object>}: value, method).
     */
    static final String API_TRACE_KEY = "robocodes.determinism.apiTrace";

    static boolean tracing() {
        return TRACE != null;
    }

    /** A robot's Robocode API call returned {@code value}. */
    static void traceApi(Object value, Object method) {
        if (TRACE != null) {
            trace("api " + method + " " + (value instanceof Double d ? Long.toHexString(Double.doubleToRawLongBits(d))
                    + " " + d : value));
        }
    }

    /**
     * Calibration of the virtual clock (#15): with {@code -Drobocodes.calibrate=<file>},
     * each robot's turns, counted work, clock reads and real CPU time are written to the
     * file when the JVM exits. With {@code -Drobocodes.meterWork=false} robot code isn't
     * counted (and reads the real clock), to measure its CPU time without that overhead.
     */
    private static final String CALIBRATE = System.getProperty("robocodes.calibrate");
    static final boolean METER_WORK = !"false".equals(System.getProperty("robocodes.meterWork"));

    /** Stack frames per trace line ({@code -Drobocodes.traceDepth}, default 3). */
    private static final int TRACE_DEPTH = Integer.getInteger("robocodes.traceDepth", 3);

    private static final ThreadLocal<Boolean> TRACING = ThreadLocal.withInitial(() -> false);

    private static void trace(String what) {
        // Writing the trace hashes JDK objects itself; don't trace that.
        if (TRACE == null || TRACING.get()) {
            return;
        }
        TRACING.set(true);
        try {
            traceLine(what);
        } finally {
            TRACING.set(false);
        }
    }

    private static void traceLine(String what) {
        StringBuilder b = new StringBuilder(Thread.currentThread().getThreadGroup().getName()).append(' ').append(what);
        int shown = 0;
        for (StackTraceElement e : new Throwable().getStackTrace()) {
            String c = e.getClassName();
            if (c.startsWith("io.github.nhofsted.robocodes.determinism") || c.startsWith("java.")) {
                continue;
            }
            b.append(" < ").append(c).append('.').append(e.getMethodName());
            if (++shown == TRACE_DEPTH) {
                break;
            }
        }
        b.append('\n');
        synchronized (RobotRandomness.class) {
            try {
                java.nio.file.Files.writeString(java.nio.file.Path.of(TRACE), b,
                        java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
            } catch (java.io.IOException e) {
                throw new java.io.UncheckedIOException(e);
            }
        }
    }

    /** The configured battle, for the identity hash hook (installed before configuration). */
    private static volatile RobotRandomness current;

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
        current = randomness;
        if (CALIBRATE != null) {
            Runtime.getRuntime().addShutdownHook(new Thread(randomness::writeCalibration));
        }
    }

    /**
     * The identity hash code for an object first hashed now: for the robot's own objects
     * (classes its class loader defined) on a robot thread, the next value of the robot's
     * identity hash stream; otherwise the JVM's own. Other enum constants, e.g. the JDK's,
     * keep the JVM's: they are also hashed by code that isn't the robot's (in robocode-s,
     * the robot host's protocol code runs on the robot thread), which would shift the
     * robot's sequence.
     */
    static int identityHash(Object o) {
        if (!IdentityHashes.isRobotClassLoader(o.getClass().getClassLoader())) {
            return System.identityHashCode(o);
        }
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream == null) {
            return System.identityHashCode(o);
        }
        int hash = stream.nextHash();
        if (TRACE != null) {
            trace("hash " + o.getClass().getName() + " " + hash);
        }
        return hash;
    }

    /**
     * {@code o.hashCode()}, but for an array (which inherits the JVM's identity hash, e.g. a
     * {@code double[]} used as a {@code HashMap} key) on a robot thread: its
     * {@linkplain #tableHash table identity hash}.
     */
    static int hashCode(Object o) {
        return o.getClass().isArray() ? tableHash(o) : o.hashCode();
    }

    /**
     * The identity hash code of an object that has no field for it (an array, or any object
     * asked through {@code System.identityHashCode} or {@code super.hashCode()}): on a robot
     * thread, the next value of the robot's table hash stream when it is first asked, kept
     * in a weak identity table; otherwise the JVM's own. A separate stream, so these can't
     * shift the robot's other identity hashes.
     */
    static int tableHash(Object o) {
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream == null) {
            return System.identityHashCode(o);
        }
        return stream.tableHash(o);
    }

    /** Whether the agent rewrote {@code Math.random()} and {@code new Random()}. */
    public static boolean isAgentInstalled() {
        return agentInstalled;
    }

    static void markAgentInstalled() {
        agentInstalled = true;
    }

    /** Whether the agent counts the work of robot code (ADR 0014). */
    public static boolean isMeteringWork() {
        return agentInstalled && METER_WORK;
    }

    /**
     * The work counted for the robots of this JVM so far (in robocode-s: one robot), as
     * virtual time in nanoseconds: what the virtual clock adds for it, without the turns.
     */
    public static long countedWorkNanos() {
        long instructions = 0;
        synchronized (FUEL) {
            for (long[] fuel : FUEL.values()) {
                instructions += fuel[0];
            }
        }
        return elapsedNanos(0, instructions);
    }

    /** Next value for a robot's {@code Math.random()}, or NaN outside robot threads. */
    private double mathRandom() {
        Stream stream = currentStream();
        return stream == null ? Double.NaN : stream.mathRandom();
    }

    /** Seed for a robot's next {@code new Random()}, or null outside robot threads. */
    private Long nextSeed() {
        Stream stream = currentStream();
        if (stream == null) {
            return null;
        }
        long seed = stream.nextSeed();
        if (TRACE != null) {
            trace("newRandom " + seed);
        }
        return seed;
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

    /** The work counter of the robot whose classes {@code loader} defines. */
    static Object fuel(Object loader) {
        long[] fuel;
        synchronized (FUEL) {
            fuel = FUEL.computeIfAbsent((ClassLoader) loader, l -> new long[1]);
        }
        if (TRACE != null || CALIBRATE != null) {
            // Robot classes initialize on the robot's thread: remember the counter for the
            // per-turn trace and the calibration.
            RobotRandomness randomness = current;
            Stream stream = randomness == null ? null : randomness.currentStream();
            if (stream != null) {
                stream.fuel = fuel;
            }
        }
        return fuel;
    }

    /** One more turn for the robot of the current thread. */
    static void turn() {
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream != null) {
            stream.turn();
            if (CALIBRATE != null) {
                stream.measureCpu();
            }
            if (TRACE != null && stream.fuel != null) {
                trace("turn " + stream.turns() + " fuel " + stream.fuel[0]);
            }
        }
    }

    /** Virtual time a robot has spent: its turns and its counted work. */
    public static long elapsedNanos(long turns, long instructions) {
        return turns * TURN_NANOS + instructions * PICOS_PER_INSTRUCTION / 1000;
    }

    private static long elapsedNanos(Object fuel) {
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        return elapsedNanos(stream == null ? 0 : stream.turns(), ((long[]) fuel)[0]);
    }

    /** Counts a robot's clock reads, for the calibration (robots that read it are left out). */
    private static void countClockRead() {
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream != null) {
            stream.clockReads++;
        }
    }

    /** One line per robot: name, turns, counted work, clock reads, CPU nanos. */
    private void writeCalibration() {
        StringBuilder out = new StringBuilder();
        synchronized (streams) {
            for (Map.Entry<String, Stream> e : streams.entrySet()) {
                Stream st = e.getValue();
                synchronized (st) {
                    out.append(e.getKey()).append('\t').append(st.turns).append('\t')
                            .append(st.fuel == null ? 0 : st.fuel[0]).append('\t').append(st.clockReads)
                            .append('\t').append(st.cpuNanos).append('\n');
                }
            }
        }
        try {
            java.nio.file.Files.writeString(java.nio.file.Path.of(CALIBRATE), out,
                    java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
        } catch (java.io.IOException e) {
            System.err.println("robocode-s: could not write calibration: " + e);
        }
    }

    static long virtualNanos(Object fuel) {
        if (CALIBRATE != null) {
            countClockRead();
        }
        long nanos = NANO_ORIGIN + elapsedNanos(fuel);
        if (TRACE != null) {
            trace("nanoTime " + nanos);
        }
        return nanos;
    }

    static long virtualMillis(Object fuel) {
        if (CALIBRATE != null) {
            countClockRead();
        }
        long millis = EPOCH_MILLIS + elapsedNanos(fuel) / 1_000_000;
        if (TRACE != null) {
            trace("currentTimeMillis " + millis);
        }
        return millis;
    }

    /**
     * The k-th (1-based) identity hash code a robot hands out. Like HotSpot's: 31 bits,
     * never 0.
     */
    public static int identityHashCode(long baseSeed, long k) {
        int hash = (int) (mix64((baseSeed ^ HASH_SALT) + k * GOLDEN_GAMMA) >>> 33);
        return hash == 0 ? 1 : hash;
    }

    /** The k-th (1-based) table identity hash code a robot hands out (see {@link #tableHash}). */
    public static int tableHashCode(long baseSeed, long k) {
        return identityHashCode(baseSeed ^ TABLE_SALT, k);
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
        private long hashed;
        private long turns;
        private long tabled;
        /** The robot's work counter, for the trace and the calibration only. */
        volatile long[] fuel;
        /** Calibration: clock reads, and the CPU time of the robot's threads at its turns. */
        long clockReads;
        long cpuNanos;
        private Thread cpuThread;
        private long cpuSeen;
        /** Table identity hashes, by object identity; entries go when the object does. */
        private final Map<IdentityKey, Integer> table = new HashMap<>();
        private final ReferenceQueue<Object> collected = new ReferenceQueue<>();

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

        synchronized int nextHash() {
            return identityHashCode(base, ++hashed);
        }

        synchronized int tableHash(Object o) {
            for (Reference<?> gone; (gone = collected.poll()) != null; ) {
                table.remove(gone);
            }
            Integer hash = table.get(new IdentityKey(o, null));
            if (hash == null) {
                hash = tableHashCode(base, ++tabled);
                table.put(new IdentityKey(o, collected), hash);
                if (TRACE != null) {
                    trace("tableHash " + o.getClass().getName() + " " + hash);
                }
            }
            return hash;
        }

        /** At a turn of the robot's thread: adds the CPU time it used since its last turn. */
        synchronized void measureCpu() {
            long now = java.lang.management.ManagementFactory.getThreadMXBean().getCurrentThreadCpuTime();
            Thread thread = Thread.currentThread();
            // A new thread (classic starts one per round) counts from its start.
            cpuNanos += thread == cpuThread ? now - cpuSeen : now;
            cpuThread = thread;
            cpuSeen = now;
        }

        synchronized void turn() {
            turns++;
        }

        synchronized long turns() {
            return turns;
        }
    }

    /** A weak reference that is equal to another one to the same object. */
    private static final class IdentityKey extends WeakReference<Object> {
        private final int hash;

        IdentityKey(Object o, ReferenceQueue<Object> queue) {
            super(o, queue);
            hash = System.identityHashCode(o);
        }

        @Override
        public int hashCode() {
            return hash;
        }

        @Override
        public boolean equals(Object other) {
            if (this == other) {
                return true;
            }
            Object o = get();
            return o != null && other instanceof IdentityKey key && key.get() == o;
        }
    }
}
