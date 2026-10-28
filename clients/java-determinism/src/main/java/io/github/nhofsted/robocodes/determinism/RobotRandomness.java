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
import java.util.IdentityHashMap;
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
    /** {@code Thread.start()} of every thread: a hook that notes robot code starting threads (a {@code Consumer<Thread>}). */
    static final String THREAD_START_KEY = "robocodes.determinism.threadStart";
    /** Classic's {@code RobotPeer.startRound()}: a new round for the peer (a {@code Consumer<Object>}). */
    static final String PEER_ROUND_KEY = "robocodes.determinism.peerRound";
    /**
     * Classic's {@code RobotPeer.executeImpl()}, on the robot's thread: its turn's work is done
     * (a {@code Consumer<Object[]>}: the peer, the turn it was woken for and its
     * {@code AtomicReference} of published commands).
     */
    static final String PEER_EXECUTE_KEY = "robocodes.determinism.peerExecute";
    /** Classic's {@code RobotPeer.waitWakeup()}: whether the robot stays asleep (a {@code Predicate<Object>}). */
    static final String PEER_ASLEEP_KEY = "robocodes.determinism.peerAsleep";
    /**
     * Classic's {@code RobotPeer.performLoadCommands()}: which commands the battle loads (a
     * {@code Consumer<Object[]>}: the peer and its {@code AtomicReference} of published commands).
     */
    static final String PEER_LOAD_KEY = "robocodes.determinism.peerLoad";

    /*
     * The virtual clock (ADR 0014). Part of the deterministic rules: changing a constant
     * changes what time-reading robots do.
     */
    /**
     * Virtual time per counted instruction, in picoseconds. Calibrated on 2026-10-17 against
     * classic 1.11.1 on OpenJDK 21 (x86-64, WSL2) with only the inexact math intrinsics
     * disabled: robots doing over 1 000 000 counted instructions per turn take 45-209 ps
     * each, median 159 (#15, ADR 0014).
     */
    public static final long PICOS_PER_INSTRUCTION = 160;
    /** Virtual time per turn besides the robot's own work: waiting for the others. */
    public static final long TURN_NANOS = 1_000_000;
    /** Value of the virtual {@code nanoTime()} before the robot did anything. */
    public static final long NANO_ORIGIN = 1_000_000_000_000L;
    /** Value of the virtual {@code currentTimeMillis()} before the robot did anything: 2026-01-01T00:00Z. */
    public static final long EPOCH_MILLIS = 1_767_225_600_000L;
    /**
     * Classic's CPU constant on the machine the clock was calibrated on, which
     * {@link #TURN_BUDGET_NANOS} follows from (ADR 0018). The robot host stores it in classic's
     * settings, so classic needn't measure one, and a robot reading it there finds the rules'.
     */
    public static final long CPU_CONSTANT_NANOS = 2_520_000;
    /**
     * Counted work (as virtual time) a robot may do in a turn before it skips turns: how long
     * classic waits for a robot on the machine the clock was calibrated on (ADR 0018). Its CPU
     * constant there is 2.52 ms, but classic waits in whole milliseconds: 3 waits of 1 ms,
     * 3.28 ms on Linux. Less the 64 µs of classic's own robot-side work per turn, which it
     * times too but isn't counted. The same in robocode-s's server ({@code DETERMINISTIC_BUDGET_NANOS}).
     */
    public static final long TURN_BUDGET_NANOS = 3_200_000;
    /**
     * Budgets a robot may use in turn 1: classic waits 21 milliseconds then instead of 3
     * (ten times its CPU constant's whole milliseconds, and one more).
     */
    public static final long FIRST_TURN_BUDGETS = 7;
    /**
     * Counted work (as virtual time) after which a robot's code throws (ADR 0018): it has run
     * this long since its last turn started, so it is late for more turns than any robot may
     * skip (240 for I/O robots, after turn 1's seven budgets). An endless loop then crashes at
     * the same instruction everywhere instead of keeping the battle waiting.
     */
    public static final long WORK_CUTOFF_NANOS = 256 * TURN_BUDGET_NANOS;
    /** {@link #WORK_CUTOFF_NANOS} in counted instructions. */
    static final long CUTOFF_INSTRUCTIONS = WORK_CUTOFF_NANOS * 1000 / PICOS_PER_INSTRUCTION;

    /**
     * Version of the deterministic rules: the constants above and how they decide skipped
     * turns. Bump it with any change to them. robocode-s refuses deterministic battles with
     * a robot host that follows other rules ({@code DETERMINISTIC_RULES} in its server): an
     * outdated build would otherwise count robots' work differently, without an error. A
     * method, so callers get this jar's value, not one javac copied into them.
     */
    public static int rulesVersion() {
        return 1;
    }

    /** Work counters, one per robot class loader (in classic: one per robot). */
    private static final Map<ClassLoader, long[]> FUEL = new WeakHashMap<>();
    /** Classic's robot peers and their turns (#15). */
    private static final Map<Object, PeerTurns> PEERS = new IdentityHashMap<>();

    /** A classic robot peer's turns. */
    private static final class PeerTurns {
        /** Counted work (as virtual time) when its last turn ended; -1: none yet this round. */
        long work = -1;
        /** Turns it stays asleep, as if still busy. */
        long late;
        /** While late: the commands the battle keeps loading, and those the robot published. */
        Object oldCommands;
        Object newCommands;
    }

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

    /** Whether robot classes trace their Robocode API calls: with the trace, unless {@code -Drobocodes.traceApi=false}. */
    static boolean tracing() {
        return TRACE != null && !"false".equals(System.getProperty("robocodes.traceApi"));
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

    /** With the trace on, {@code -Drobocodes.traceRobots=<name prefixes, comma-separated>}: only these robots. */
    private static final List<String> TRACE_ROBOTS = System.getProperty("robocodes.traceRobots") == null ? null
            : List.of(System.getProperty("robocodes.traceRobots").split(","));

    private static void trace(String what) {
        // Writing the trace hashes JDK objects itself; don't trace that.
        if (TRACE == null || TRACING.get()) {
            return;
        }
        if (TRACE_ROBOTS != null) {
            String group = Thread.currentThread().getThreadGroup().getName();
            if (TRACE_ROBOTS.stream().noneMatch(group::startsWith)) {
                return;
            }
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
        THREAD_STARTERS.clear();
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
                instructions += count(fuel);
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

    /** The stream of the robot whose thread this is, or null (no battle, not a robot thread). */
    static Stream streamOfCurrentThread() {
        RobotRandomness randomness = current;
        return randomness == null ? null : randomness.currentStream();
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

    /**
     * A robot's work counter, {@code {limit, count - limit}} (instructions): robot code adds
     * to the second, and throws once it reaches 0 ({@link WorkMeter}).
     */
    static long count(long[] fuel) {
        return fuel[0] + fuel[1];
    }

    /** Sets the count at which the robot's code throws. */
    static void limit(long[] fuel, long limit) {
        long count = count(fuel);
        fuel[0] = limit;
        fuel[1] = count - limit;
    }

    /** The work counter of the robot whose classes {@code loader} defines. */
    static Object fuel(Object loader) {
        long[] fuel;
        synchronized (FUEL) {
            // The count, and the limit for the robot's work until its first turn.
            fuel = FUEL.computeIfAbsent((ClassLoader) loader, l -> new long[] {CUTOFF_INSTRUCTIONS, -CUTOFF_INSTRUCTIONS});
        }
        // Robot classes initialize on the robot's thread: remember the counter for skipped
        // turns, the per-turn trace and the calibration.
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream != null) {
            stream.fuel = fuel;
            if (stream.cutoffAt == 0) {
                stream.cutoffAt = fuel[0];
            }
        }
        return fuel;
    }

    /** Robots (by thread group name) whose code started a thread of its own. */
    private static final java.util.Set<String> THREAD_STARTERS = java.util.concurrent.ConcurrentHashMap.newKeySet();
    private static volatile Runnable threadListener;

    /**
     * Robot code started a thread of its own (also through {@code Timer} and executors): the
     * thread runs in real time, outside the work meter and the virtual clock, so the robot's
     * battles are not reproducible (ADR 0014, ADR 0024). Called from {@code Thread.start()}
     * on the starting thread; classic's own threads are started from outside the robot's group.
     */
    static void threadStart(Object thread) {
        try {
            RobotRandomness randomness = current;
            if (randomness == null) {
                return;
            }
            for (ThreadGroup group = Thread.currentThread().getThreadGroup(); group != null;
                    group = group.getParent()) {
                String name = group.getName();
                if (randomness.isRobot(name)) {
                    if (startedByRobotCode() && THREAD_STARTERS.add(name)) {
                        Runnable listener = threadListener;
                        if (listener != null) {
                            listener.run();
                        }
                    }
                    return;
                }
            }
        } catch (Throwable t) {
            // Never break Thread.start().
        }
    }

    /**
     * Whether the robot's own code asked for the thread being started: the innermost frame
     * that is neither {@code Thread} nor one of the JDK's thread-starting facilities
     * ({@code Timer}, executors) is a class of a robot class loader. The JDK starts threads
     * as a side effect of other calls (Java2D's disposer when a robot paints, say), and the
     * robot host and classic do on robot threads: those aren't the robot's.
     */
    @SuppressWarnings("removal")
    private static boolean startedByRobotCode() {
        // Privileged: the walker needs a permission that the robot's frames below don't have.
        return java.security.AccessController.doPrivileged((java.security.PrivilegedAction<Boolean>) () ->
                StackWalker.getInstance(StackWalker.Option.RETAIN_CLASS_REFERENCE).walk(frames -> frames
                        .map(StackWalker.StackFrame::getDeclaringClass)
                        .filter(c -> c != RobotRandomness.class && c != DeterminismAgent.class
                                && !isThreadStarter(c.getName()))
                        .findFirst()
                        .map(c -> IdentityHashes.isRobotClassLoader(c.getClassLoader()))
                        .orElse(false)));
    }

    private static boolean isThreadStarter(String className) {
        return className.equals("java.lang.Thread") || className.startsWith("java.lang.System$")
                || className.equals("java.security.AccessController") || className.equals("java.util.Timer")
                || className.startsWith("java.util.Timer$") || className.startsWith("java.util.concurrent.")
                || className.startsWith("jdk.internal.vm.");
    }

    /** Whether robot code started a thread of its own in this JVM since the battle was configured. */
    public static boolean startedThreads() {
        return !THREAD_STARTERS.isEmpty();
    }

    /** Names (thread group names) of the robots whose code started threads. */
    public static java.util.Set<String> robotsWithThreads() {
        return new java.util.TreeSet<>(THREAD_STARTERS);
    }

    /** Called (on the starting thread) the first time a robot starts a thread. */
    public static void onThreadStarted(Runnable listener) {
        threadListener = listener;
    }

    /** One more turn for the robot of the current thread. */
    static void turn() {
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        if (stream != null) {
            stream.turn();
            long[] fuel = stream.fuel;
            if (fuel != null) {
                // Classic's robot side, in robocode-s too: its work until the next turn. A
                // robot with threads of its own runs them now (ADR 0025).
                RobotThreads.turn(stream, fuel);
            }
            if (CALIBRATE != null) {
                stream.measureCpu();
            }
            if (TRACE != null && stream.fuel != null) {
                trace("turn " + stream.turns() + " fuel " + count(stream.fuel));
            }
        }
    }

    /**
     * How many turns late a robot answers that woke for {@code turn} and did {@code workNanos}
     * of counted work: classic waits a budget for each robot each turn (seven on turn 1) and
     * otherwise runs the turn without it. As robocode-s's server ({@code late_turns}).
     */
    public static long lateTurns(long workNanos, long turn) {
        long allowance = turn == 1 ? FIRST_TURN_BUDGETS * TURN_BUDGET_NANOS : TURN_BUDGET_NANOS;
        return turn < 1 || workNanos < allowance ? 0 : 1 + (workNanos - allowance) / TURN_BUDGET_NANOS;
    }

    /** Classic: a new round for this robot peer; its first turn starts its work count. */
    static void peerRound(Object peer) {
        synchronized (PEERS) {
            PEERS.put(peer, new PeerTurns());
        }
    }

    /**
     * Classic, on the robot's thread: the robot ended its turn (it executes). Its work since
     * the last one, counted the way robocode-s's robot host reports it, decides how many
     * turns it stays asleep, as if it were still busy.
     */
    @SuppressWarnings("unchecked")
    static void peerExecute(Object[] args) {
        Object peer = args[0];
        int turn = (Integer) args[1];
        var published = (java.util.concurrent.atomic.AtomicReference<Object>) args[2];
        RobotRandomness randomness = current;
        Stream stream = randomness == null ? null : randomness.currentStream();
        long now = stream == null || stream.fuel == null ? 0 : elapsedNanos(0, count(stream.fuel));
        synchronized (PEERS) {
            PeerTurns turns = PEERS.computeIfAbsent(peer, p -> new PeerTurns());
            long work = turns.work < 0 ? 0 : Math.max(0, now - turns.work);
            turns.work = now;
            turns.late = lateTurns(work, turn);
            if (turns.late > 0) {
                // Not published yet: these stay in effect while it is late.
                turns.oldCommands = published.get();
                turns.newCommands = null;
            }
        }
    }

    /** Classic's battle thread would wake this robot: whether it is still busy instead. */
    static boolean peerAsleep(Object peer) {
        synchronized (PEERS) {
            PeerTurns turns = PEERS.get(peer);
            if (turns == null || turns.late == 0) {
                return false;
            }
            turns.late--;
            return true;
        }
    }

    /**
     * Classic's battle thread loads a robot's commands. A late robot already published its
     * new ones before it went to sleep; in classic it would still be busy, so the battle
     * keeps loading its old ones until the turn after its late turns.
     */
    @SuppressWarnings("unchecked")
    static void peerLoad(Object[] args) {
        Object peer = args[0];
        var published = (java.util.concurrent.atomic.AtomicReference<Object>) args[1];
        synchronized (PEERS) {
            PeerTurns turns = PEERS.get(peer);
            if (turns == null || turns.oldCommands == null) {
                return;
            }
            if (turns.late > 0) {
                Object commands = published.get();
                if (commands != turns.oldCommands) {
                    turns.newCommands = commands;
                    published.set(turns.oldCommands);
                }
            } else {
                if (turns.newCommands != null) {
                    published.set(turns.newCommands);
                }
                turns.oldCommands = null;
                turns.newCommands = null;
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
        return elapsedNanos(stream == null ? 0 : stream.turns(), count((long[]) fuel));
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
                            .append(st.fuel == null ? 0 : count(st.fuel)).append('\t').append(st.clockReads)
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
    static final class Stream {
        private final long base;
        private final Random math;
        final Random utils;
        private long created;
        private long hashed;
        private long turns;
        private long tabled;
        /** The robot's work counter: for its threads (ADR 0025), the trace and the calibration. */
        volatile long[] fuel;
        /** Its threads' scheduler, once robot code started a thread (ADR 0025); reset each round. */
        RobotThreads threads;
        /** Count at which the robot's code throws (ADR 0018); its limit may be lower, for its threads. */
        volatile long cutoffAt;
        /** Count when its main thread's last turn ended, for the turn's spare budget (ADR 0025). */
        long turnStart;
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
