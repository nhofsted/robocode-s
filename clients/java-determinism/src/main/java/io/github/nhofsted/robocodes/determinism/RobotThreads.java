/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;

/**
 * Deterministic threads for robots (ADR 0025): one virtual CPU per robot.
 *
 * <p>A robot's threads (those robot code starts, also through a {@code Timer} or an
 * executor, and the thread that starts the first) run one at a time, and the CPU changes
 * hands only at points that depend on what the robot executes, never on the machine:
 * <ul>
 *   <li>a thread blocks: {@code Thread.sleep}, {@code join}, a monitor another of the
 *       robot's threads holds, {@code Object.wait}, {@code LockSupport.park} (so every
 *       {@code java.util.concurrent} lock, condition, latch and queue), or it ends;</li>
 *   <li>{@code Thread.yield}, a new thread's start (it waits in line);</li>
 *   <li>a thread used up its quantum of counted work ({@link #QUANTUM_INSTRUCTIONS}),
 *       noticed where robot code checks its work limit (loops and handlers, ADR 0018), so
 *       busy-waiting on another thread ends;</li>
 *   <li>the end of the robot's turn: before its main thread executes, the other threads
 *       run, for at most what is left of the turn's budget and at most the virtual clock's
 *       time between turns ({@link #gap}).</li>
 * </ul>
 * The next thread is the first waiting one of the highest priority (round-robin among
 * equals). Monitors of robot code are owned in the scheduler too (reentrant counts), so a
 * thread only takes a real monitor when no other of the robot's threads holds it.
 * {@code Thread.sleep}, timed waits and parks wait on the robot's virtual clock (ADR 0014),
 * as many turns as in classic ({@link RobotRandomness#sleepNanos}): a sleeper wakes at the
 * first switch point after its deadline, or as soon as nothing else can run. At a round's
 * end the robot's other threads are interrupted as in classic, given a bounded amount of
 * work to end, and then stopped.
 *
 * <p>Robots without threads never create a scheduler: their code runs as before.
 */
final class RobotThreads {

    /** Counted work a thread may do while others of its robot wait: 100 µs of virtual time. */
    static final long QUANTUM_NANOS = 100_000;
    static final long QUANTUM_INSTRUCTIONS = QUANTUM_NANOS * 1000 / RobotRandomness.PICOS_PER_INSTRUCTION;
    /** The turn budget (ADR 0018) in counted instructions. */
    static final long BUDGET_INSTRUCTIONS =
            RobotRandomness.TURN_BUDGET_NANOS * 1000 / RobotRandomness.PICOS_PER_INSTRUCTION;
    /**
     * Counted work the other threads may do at most at a turn's end: the time the virtual
     * clock gives a turn for waiting on the other robots (ADR 0014), 1 ms.
     */
    static final long GAP_INSTRUCTIONS = RobotRandomness.TURN_NANOS * 1000 / RobotRandomness.PICOS_PER_INSTRUCTION;
    /**
     * Interrupts at a round's end before the robot's other threads are stopped: classic
     * interrupts them every 100 ms for 3 s ({@code RobotThreadManager.waitForStop}).
     */
    static final int END_PULSES = 30;
    /** Counted work the robot's other threads may do after each of those interrupts: 1 ms. */
    static final long END_SLICE_INSTRUCTIONS = 1_000_000 * 1000 / RobotRandomness.PICOS_PER_INSTRUCTION;
    /** Whether a higher priority runs first (Java priorities, at most classic's group maximum). */
    static final boolean PRIORITIES = !"false".equals(System.getProperty("robocodes.threadPriorities"));
    /**
     * Real time the round's end waits for a robot thread that blocks outside the scheduler
     * (in the JDK, say). Then it is left alone: a safety net, not a rule.
     */
    static final long STUCK_MILLIS = 10_000;

    private static final Map<Thread, Task> TASKS = new IdentityHashMap<>();
    private static final Map<Thread, RobotThreads> BY_MAIN = new IdentityHashMap<>();
    /** The current thread's task, once it runs in the scheduler. */
    private static final ThreadLocal<Task> CURRENT = new ThreadLocal<>();
    private static volatile boolean any;
    /** Diagnostics: {@code -Drobocodes.debugThreads=true} (to stderr) or a file to append the scheduler's events to. */
    private static final String DEBUG_TO = System.getProperty("robocodes.debugThreads");
    private static final boolean DEBUG = DEBUG_TO != null;

    private static void debug(String what) {
        if (!DEBUG) {
            return;
        }
        String line = "[sched] " + Thread.currentThread().getName() + ": " + what;
        if (DEBUG_TO.equals("true")) {
            System.err.println(line);
            return;
        }
        synchronized (RobotThreads.class) {
            try {
                java.nio.file.Files.writeString(java.nio.file.Path.of(DEBUG_TO), line + System.lineSeparator(),
                        java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
            } catch (java.io.IOException e) {
                // diagnostics only
            }
        }
    }

    enum State { READY, RUNNING, SLEEPING, JOINING, BLOCKED, WAITING, PARKED, GAP, DONE }

    /** One of a robot's threads. */
    static final class Task {
        final Thread thread;
        final RobotThreads cpu;
        final long id;
        State state = State.READY;
        /** Virtual time (nanos since the robot's clock started) a sleep or timed wait ends. */
        long deadline = Long.MAX_VALUE;
        Task joining;
        /** The monitor it waits to enter, or {@code Object.wait}s on. */
        Object monitor;
        /** Entries of the monitor it gave up to wait, and gets back. */
        int savedCount;
        /** It waits in a real {@code Object.wait} on this object: getting the CPU wakes it there. */
        Object kickOn;
        /** It may go on (set when it gets the CPU while in a real wait). */
        volatile boolean granted;
        /** Inside the scheduler's own real wait: {@code Object.wait} is the JDK's then. */
        boolean inRealWait;
        /** {@code LockSupport}'s permit. */
        boolean permit;
        boolean entered;
        boolean killed;

        Task(Thread thread, RobotThreads cpu, long id) {
            this.thread = thread;
            this.cpu = cpu;
            this.id = id;
        }
    }

    /** A monitor robot code entered, as the scheduler sees it. */
    private static final class Monitor {
        Task owner;
        int count;
        final ArrayDeque<Task> entrants = new ArrayDeque<>();
        final ArrayDeque<Task> waiters = new ArrayDeque<>();
    }

    private final RobotRandomness.Stream stream;
    /** Live tasks, in the order they were created. */
    private final List<Task> tasks = new ArrayList<>();
    private final ArrayDeque<Task> ready = new ArrayDeque<>();
    private final Map<Object, Monitor> monitors = new IdentityHashMap<>();
    private long nextId;
    private Task running;
    private Task main;
    /** The main thread while the others use the turn's spare budget. */
    private Task gapOwner;
    /** A round's end: a host thread drives the robot's threads; idle hands back to it. */
    private boolean driving;
    /** Count at which the gap or the round end's slice ends. */
    private long sliceEnd = Long.MAX_VALUE;

    private RobotThreads(RobotRandomness.Stream stream) {
        this.stream = stream;
    }

    /** Whether any robot thread is scheduled in this JVM (a cheap first test for the hooks). */
    static boolean active() {
        return any;
    }

    static Task task(Thread t) {
        if (!any) {
            return null;
        }
        synchronized (TASKS) {
            return TASKS.get(t);
        }
    }

    /** The current thread's task, if it runs in the scheduler. */
    static Task current() {
        return any ? CURRENT.get() : null;
    }

    // ---- Hooks ----

    /**
     * {@code Thread.start()} and {@code start(ThreadContainer)} (any thread): the robot's code
     * on one of its threads starts a thread, directly or through a {@code Timer} or an
     * executor. The new thread joins the robot's CPU and waits in line; the starting thread,
     * which holds the CPU, goes on.
     *
     * @return whether the scheduler took the thread (then it isn't reported, ADR 0024): a
     *     robot that also uses what the scheduler doesn't cover is reported here instead
     */
    static boolean starting(Object o) {
        Thread t = (Thread) o;
        if (t.getState() != Thread.State.NEW) {
            return false; // start() throws
        }
        RobotRandomness.Stream stream = RobotRandomness.streamOfCurrentThread();
        Class<?> caller = stream == null ? null : robotStarter();
        if (caller == null) {
            return false;
        }
        if (stream.fuel == null) {
            // The robot's classes were initialized on another thread: its counter by its loader.
            RobotRandomness.fuel(caller.getClassLoader());
        }
        if (stream.uncovered) {
            RobotRandomness.reportThreads();
        }
        Thread me = Thread.currentThread();
        RobotThreads cpu;
        synchronized (stream) {
            if (stream.threads == null) {
                stream.threads = new RobotThreads(stream);
            }
            cpu = stream.threads;
        }
        synchronized (cpu) {
            Task self = task(me);
            if (self == null) {
                if (cpu.running != null) {
                    // Another of the robot's threads holds the CPU: this one runs outside the
                    // scheduler (robot code on a thread it didn't start), so does its child.
                    return false;
                }
                self = cpu.register(me);
                self.entered = true;
                self.state = State.RUNNING;
                cpu.running = self;
                CURRENT.set(self);
                if (cpu.main == null) {
                    cpu.main = self;
                    synchronized (BY_MAIN) {
                        BY_MAIN.put(me, cpu);
                    }
                }
            } else if (self.cpu != cpu || cpu.running != self) {
                return false;
            }
            Task task = cpu.register(t);
            cpu.ready.addLast(task);
            cpu.setLimit(self);
        }
        return true;
    }

    /**
     * A robot class that uses what the scheduler doesn't cover (virtual threads, {@code Unsafe}'s
     * parking) was loaded on one of the robot's threads: such threads aren't reproducible, so
     * the robot is reported (ADR 0024, 0025).
     */
    static void uncovered(ClassLoader loader) {
        synchronized (UNCOVERED) {
            UNCOVERED.put(loader, Boolean.TRUE);
            anyUncovered = true;
        }
        RobotRandomness.Stream stream = RobotRandomness.streamOfCurrentThread();
        if (stream != null && !stream.uncovered) {
            stream.uncovered = true;
            RobotRandomness.reportThreads();
        }
    }

    /** At a turn on the robot's thread: its classes, loaded elsewhere, use what isn't covered. */
    static void checkUncovered(RobotRandomness.Stream stream) {
        if (!anyUncovered || stream.uncovered || stream.fuel == null) {
            return;
        }
        synchronized (UNCOVERED) {
            for (ClassLoader loader : UNCOVERED.keySet()) {
                if (RobotRandomness.fuelOf(loader) == stream.fuel) {
                    stream.uncovered = true;
                }
            }
        }
        if (stream.uncovered) {
            RobotRandomness.reportThreads();
        }
    }

    private static final Map<ClassLoader, Boolean> UNCOVERED = new java.util.WeakHashMap<>();
    private static volatile boolean anyUncovered;

    /**
     * The robot class that starts a thread: the innermost caller outside {@code Thread}, the
     * agent, Kotlin's helpers and the JDK's thread-starting facilities ({@code Timer},
     * {@code java.util.concurrent}), if it is a robot class; null otherwise (the JDK starting
     * a thread for itself, the robot host, classic).
     */
    private static Class<?> robotStarter() {
        return STACK.walk(frames -> frames
                .map(StackWalker.StackFrame::getDeclaringClass)
                .filter(c -> !IdentityHashes.isRobotClassLoader(c.getClassLoader()) ? !isStarterInfrastructure(c) : true)
                .findFirst()
                .filter(c -> IdentityHashes.isRobotClassLoader(c.getClassLoader()))
                .orElse(null));
    }

    private static boolean isStarterInfrastructure(Class<?> c) {
        String n = c.getName();
        return c == Thread.class || isAgent(c) || n.startsWith("kotlin.") || n.equals("java.util.Timer")
                || n.startsWith("java.util.Timer$") || n.startsWith("java.util.concurrent.")
                || n.startsWith("jdk.internal.vm.") || n.startsWith("java.lang.System$")
                || n.equals("java.security.AccessController");
    }

    private static boolean isAgent(Class<?> c) {
        return c.getClassLoader() == RobotThreads.class.getClassLoader()
                && c.getName().startsWith("io.github.nhofsted.robocodes.determinism.");
    }

    /**
     * Whether a blocking call on a scheduled thread is the robot's: its innermost caller that
     * isn't the JDK, Kotlin or the agent is robot code, or there is none (a thread of a
     * {@code Timer} or an executor). The robot host's and classic's own waits on a robot's
     * main thread (for the next turn) stay real.
     */
    private static boolean robotSide() {
        return STACK.walk(frames -> frames
                .map(StackWalker.StackFrame::getDeclaringClass)
                .filter(c -> {
                    if (IdentityHashes.isRobotClassLoader(c.getClassLoader())) {
                        return true;
                    }
                    String n = c.getName();
                    return !(isAgent(c) || n.startsWith("java.") || n.startsWith("jdk.") || n.startsWith("sun.")
                            || n.startsWith("com.sun.") || n.startsWith("kotlin."));
                })
                .findFirst()
                .map(c -> IdentityHashes.isRobotClassLoader(c.getClassLoader()))
                .orElse(true));
    }

    private static final StackWalker STACK = StackWalker.getInstance(StackWalker.Option.RETAIN_CLASS_REFERENCE);

    /**
     * {@code Thread.run()} and the {@code run()} of classes that extend {@code Thread}: a new
     * robot thread waits for the CPU before its code runs.
     */
    static void entering() {
        Task me = task(Thread.currentThread());
        if (me == null || me.entered) {
            return;
        }
        me.entered = true;
        CURRENT.set(me);
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            cpu.park(me);
        }
        checkKilled(me);
    }

    /** {@code Thread.exit()}: the thread ends; the CPU goes on to the next. */
    static void exiting(Object o) {
        if (!any) {
            return;
        }
        Task me;
        synchronized (TASKS) {
            me = TASKS.remove((Thread) o);
        }
        if (me == null) {
            return;
        }
        CURRENT.remove();
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            cpu.finish(me);
        }
    }

    /**
     * Where robot code checks its work limit and the count reached it: a quantum ended (the
     * CPU may go to another thread), or the robot's cutoff (robot code then throws, ADR 0018).
     */
    static void atLimit(Object f) {
        long[] fuel = (long[]) f;
        Task me = current();
        if (me == null) {
            RobotRandomness.Stream stream = RobotRandomness.streamOfCurrentThread();
            if (stream != null && stream.threads != null) {
                // Robot code on a thread outside the scheduler: not deterministic anyway; it
                // shouldn't throw at another thread's quantum.
                long count = RobotRandomness.count(fuel);
                if (count < stream.cutoffAt) {
                    RobotRandomness.limit(fuel, Math.min(stream.cutoffAt, count + QUANTUM_INSTRUCTIONS));
                }
            }
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (!me.killed && RobotRandomness.count(fuel) < cpu.stream.cutoffAt) {
                cpu.preempt(me);
            } else if (!me.killed) {
                RobotRandomness.limit(fuel, cpu.stream.cutoffAt);
            }
        }
        checkKilled(me);
    }

    /**
     * {@code Thread.sleep(millis, nanos)}: robot code on one of a robot's scheduled threads
     * sleeps on the virtual clock. Otherwise false: the JDK sleeps.
     */
    static boolean sleep(long millis, int nanos) throws InterruptedException {
        Task me = current();
        if (me == null || !robotSide()) {
            return false;
        }
        if (millis < 0) {
            throw new IllegalArgumentException("timeout value is negative");
        }
        if (nanos < 0 || nanos > 999_999) {
            throw new IllegalArgumentException("nanosecond timeout value out of range");
        }
        if (Thread.interrupted()) {
            throw new InterruptedException("sleep interrupted");
        }
        long duration = millis > Long.MAX_VALUE / 1_000_000 ? Long.MAX_VALUE : millis * 1_000_000 + nanos;
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (duration == 0) {
                cpu.yieldFrom(me);
            } else {
                me.deadline = saturatedAdd(cpu.now(), RobotRandomness.sleepNanos(duration));
                me.state = State.SLEEPING;
                cpu.dispatch();
                cpu.park(me);
                me.deadline = Long.MAX_VALUE;
            }
        }
        checkKilled(me);
        if (Thread.interrupted()) {
            throw new InterruptedException("sleep interrupted");
        }
        return true;
    }

    /** {@code Thread.yield()} in robot code. */
    static void yieldNow() {
        Task me = current();
        if (me == null) {
            Thread.yield();
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            cpu.yieldFrom(me);
        }
        checkKilled(me);
    }

    /**
     * {@code t.join(millis)} (0: without end; {@code join()} and {@code join(millis, nanos)}
     * come here too): one of a robot's scheduled threads joins another of them. Otherwise
     * false: the JDK joins.
     */
    static boolean join(Thread t, long millis) throws InterruptedException {
        Task me = current();
        Task target = task(t);
        if (me == null || target == null || target.cpu != me.cpu || target == me || !robotSide()) {
            return false;
        }
        if (millis < 0) {
            throw new IllegalArgumentException("timeout value is negative");
        }
        if (Thread.interrupted()) {
            throw new InterruptedException();
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (target.state != State.DONE) {
                me.deadline = millis == 0 ? Long.MAX_VALUE
                        : saturatedAdd(cpu.now(), RobotRandomness.sleepNanos(
                                millis > Long.MAX_VALUE / 1_000_000 ? Long.MAX_VALUE : millis * 1_000_000));
                me.joining = target;
                me.state = State.JOINING;
                cpu.dispatch();
                cpu.park(me);
                me.joining = null;
                me.deadline = Long.MAX_VALUE;
            }
        }
        checkKilled(me);
        if (Thread.interrupted()) {
            throw new InterruptedException();
        }
        return true;
    }

    /**
     * {@code t.interrupt()}, before it sets the flag: one of a robot's threads interrupts
     * another that sleeps, joins, waits or parks, which then gets the CPU in line (and throws,
     * or returns from parking, when it runs).
     */
    static void interrupting(Object o) {
        if (!any) {
            return;
        }
        Task target = task((Thread) o);
        Task me = current();
        if (target == null || me == null || target.cpu != me.cpu) {
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (cpu.running == me) {
                cpu.wake(target);
            }
        }
    }

    // ---- Monitors, wait and notify, LockSupport ----

    /**
     * Robot code enters {@code o}'s monitor (before its {@code monitorenter}): if another of
     * the robot's threads holds it, this one gives the CPU on and waits in line, so the real
     * monitor is free when it gets there.
     */
    static void monitorEnter(Object o) {
        Task me = current();
        if (me == null || o == null) {
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            Monitor m = cpu.monitors.computeIfAbsent(o, k -> new Monitor());
            if (m.owner == null) {
                m.owner = me;
                m.count = 1;
            } else if (m.owner == me) {
                m.count++;
            } else {
                debug("blocked, owner " + m.owner.thread.getName());
                me.state = State.BLOCKED;
                me.monitor = o;
                m.entrants.addLast(me);
                cpu.dispatch();
                cpu.park(me);
                me.monitor = null;
            }
        }
        checkKilled(me);
    }

    /** Robot code left {@code o}'s monitor (after its {@code monitorexit}). */
    static void monitorExit(Object o) {
        Task me = current();
        if (me == null || o == null) {
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            Monitor m = cpu.monitors.get(o);
            if (m == null || m.owner != me) {
                return; // entered before the scheduler knew the thread
            }
            if (--m.count == 0) {
                cpu.release(o, m);
            }
        }
    }

    /**
     * {@code o.wait(timeout)} on a scheduled thread (robot code, or the JDK's code on a thread
     * of the robot's {@code Timer} or executor): the thread gives up the monitor (also in the
     * scheduler) and the CPU, and waits in line on {@code o}, first come first served, until a
     * {@code notify}, an interrupt or its deadline on the virtual clock; then it waits for the
     * monitor and the CPU. It waits in a real {@code o.wait()} meanwhile, so the JVM releases
     * and restores the real monitor. Otherwise false: the JDK waits.
     */
    static boolean waiting(Object o, long timeout) throws InterruptedException {
        Task me = current();
        if (me == null || me.inRealWait || o instanceof RobotThreads || timeout < 0 || !Thread.holdsLock(o)
                || (o instanceof Thread th && task(th) == null) || !robotSide()) {
            return false;
        }
        if (Thread.interrupted()) {
            throw new InterruptedException();
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            Monitor m = cpu.monitors.computeIfAbsent(o, k -> new Monitor());
            // In line first: releasing an otherwise unused monitor forgets it.
            m.waiters.addLast(me);
            me.savedCount = m.owner == me ? m.count : 0;
            if (m.owner == me) {
                m.count = 0;
                cpu.release(o, m);
            }
            me.monitor = o;
            me.kickOn = o;
            me.granted = false;
            me.deadline = timeout == 0 ? Long.MAX_VALUE
                    : saturatedAdd(cpu.now(), RobotRandomness.sleepNanos(
                            timeout > Long.MAX_VALUE / 1_000_000 ? Long.MAX_VALUE : timeout * 1_000_000));
            me.state = State.WAITING;
            debug("wait saved=" + me.savedCount);
            cpu.dispatch();
        }
        boolean interrupted = false;
        me.inRealWait = true;
        try {
            while (!me.granted) {
                try {
                    o.wait();
                } catch (InterruptedException e) {
                    interrupted = true;
                }
            }
        } finally {
            me.inRealWait = false;
        }
        me.granted = false;
        me.monitor = null;
        me.deadline = Long.MAX_VALUE;
        checkKilled(me);
        if (interrupted || Thread.interrupted()) {
            Thread.interrupted();
            throw new InterruptedException();
        }
        return true;
    }

    /**
     * Before {@code o.notify()} or {@code notifyAll()} (robot code, and the JDK's code of
     * {@code Timer} and {@code java.util.concurrent}): the first (or every) one of the robot's
     * threads that waits on {@code o} waits for the monitor and the CPU instead.
     */
    static void notifying(Object o, boolean all) {
        Task me = current();
        if (me == null || o == null) {
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            Monitor m = cpu.monitors.get(o);
            if (m == null) {
                return;
            }
            debug("notify all=" + all + " waiters=" + m.waiters.size());
            do {
                Task w = m.waiters.pollFirst();
                if (w == null) {
                    break;
                }
                cpu.reacquire(w, o, m);
            } while (all);
        }
    }

    /**
     * {@code LockSupport.park}/{@code parkNanos}/{@code parkUntil} on a scheduled thread:
     * parks in the scheduler, on the virtual clock. {@code nanos} below 0: without end.
     * Otherwise false: the JDK parks.
     */
    static boolean parking(long nanos, boolean absoluteMillis) {
        Task me = current();
        if (me == null || !robotSide()) {
            return false;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (me.permit) {
                me.permit = false;
            } else if (!me.thread.isInterrupted()) {
                long duration = nanos;
                if (absoluteMillis) {
                    long left = nanos - classicMillis(cpu);
                    duration = left <= 0 ? 0 : left > Long.MAX_VALUE / 1_000_000 ? Long.MAX_VALUE : left * 1_000_000;
                }
                if (duration != 0) {
                    me.deadline = duration < 0 ? Long.MAX_VALUE
                            : saturatedAdd(cpu.now(), RobotRandomness.sleepNanos(duration));
                    me.state = State.PARKED;
                    cpu.dispatch();
                    cpu.park(me);
                    me.deadline = Long.MAX_VALUE;
                }
            }
        }
        checkKilled(me);
        return true;
    }

    /** {@code LockSupport.unpark(t)} from a scheduled thread to one of its robot's: true if done here. */
    static boolean unparking(Thread t) {
        Task me = current();
        Task target = t == null ? null : task(t);
        if (me == null || target == null || target.cpu != me.cpu) {
            return false;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (target.state == State.PARKED) {
                cpu.makeReady(target);
            } else {
                target.permit = true;
            }
        }
        return true;
    }

    /** {@code System.nanoTime()} in the JDK's {@code Timer} and {@code java.util.concurrent}: on a scheduled thread the virtual clock, in classic's time (ADR 0025). */
    static long nanoTime() {
        Task me = current();
        if (me == null) {
            return System.nanoTime();
        }
        synchronized (me.cpu) {
            return RobotRandomness.NANO_ORIGIN + classicElapsed(me.cpu);
        }
    }

    /** {@code System.currentTimeMillis()} there, likewise. */
    static long currentTimeMillis() {
        Task me = current();
        if (me == null) {
            return System.currentTimeMillis();
        }
        synchronized (me.cpu) {
            return classicMillis(me.cpu);
        }
    }

    private static long classicMillis(RobotThreads cpu) {
        return RobotRandomness.EPOCH_MILLIS + classicElapsed(cpu) / 1_000_000;
    }

    /** The robot's virtual time scaled to classic's: a virtual turn is a classic turn. */
    private static long classicElapsed(RobotThreads cpu) {
        long now = cpu.now();
        return now / RobotRandomness.TURN_NANOS * RobotRandomness.CLASSIC_TURN_NANOS
                + now % RobotRandomness.TURN_NANOS * RobotRandomness.CLASSIC_TURN_NANOS / RobotRandomness.TURN_NANOS;
    }

    // ---- Turns and rounds ----

    /**
     * The robot's turn ends (classic's robot side executes, on the main thread): the turn's
     * work cutoff starts again (ADR 0018), and the robot's other threads that wait for the CPU
     * run in what is left of the turn's budget, less a quantum, and at most
     * {@link #GAP_INSTRUCTIONS}.
     */
    static void turn(RobotRandomness.Stream stream, long[] fuel) {
        checkUncovered(stream);
        stream.cutoffAt = RobotRandomness.count(fuel) + RobotRandomness.CUTOFF_INSTRUCTIONS;
        RobotThreads cpu = stream.threads;
        if (cpu == null) {
            RobotRandomness.limit(fuel, stream.cutoffAt);
            return;
        }
        Task me = current();
        synchronized (cpu) {
            if (me != null && me == cpu.main && cpu.running == me) {
                cpu.gap(me);
            }
            long count = RobotRandomness.count(fuel);
            stream.cutoffAt = count + RobotRandomness.CUTOFF_INSTRUCTIONS;
            stream.turnStart = count;
            if (me != null && cpu.running == me) {
                cpu.setLimit(me);
            } else {
                RobotRandomness.limit(fuel, stream.cutoffAt);
            }
        }
        checkKilled(me);
    }

    /**
     * Classic's {@code RobotThreadManager.waitForStop()}, before it interrupts the robot's
     * threads, after the main thread ended: the robot's other threads are interrupted
     * ({@link #END_PULSES} times, each followed by up to {@link #END_SLICE_INSTRUCTIONS} of
     * their work until they all wait), then stopped (a {@code ThreadDeath} where they next
     * run). Those that still don't end never get the CPU again.
     *
     * @return whether the robot had threads of its own (then classic's wait is skipped)
     */
    static boolean endRound(Thread mainThread) {
        RobotThreads cpu;
        synchronized (BY_MAIN) {
            cpu = BY_MAIN.remove(mainThread);
        }
        if (cpu == null) {
            return false;
        }
        synchronized (cpu) {
            cpu.driving = true;
            for (int pulse = 0; pulse < END_PULSES && cpu.hasLive(); pulse++) {
                for (Task t : List.copyOf(cpu.tasks)) {
                    t.thread.interrupt();
                    cpu.wake(t);
                }
                cpu.drive();
            }
            if (cpu.hasLive()) {
                for (Task t : List.copyOf(cpu.tasks)) {
                    t.killed = true;
                    if (t.state == State.BLOCKED) {
                        // Waiting for a monitor: it throws before it enters.
                        Monitor m = cpu.monitors.get(t.monitor);
                        if (m != null) {
                            m.entrants.remove(t);
                        }
                        if (t.savedCount > 0) {
                            t.savedCount = 0;
                        }
                        cpu.makeReady(t);
                    } else {
                        cpu.wake(t);
                    }
                }
                cpu.drive();
            }
            // Those left never run again (parked in the scheduler): the next round starts afresh.
            for (Task t : List.copyOf(cpu.tasks)) {
                System.err.println("robocode-s: robot thread " + t.thread.getName() + " did not stop");
                synchronized (TASKS) {
                    TASKS.remove(t.thread);
                }
            }
            cpu.tasks.clear();
            cpu.ready.clear();
            cpu.monitors.clear();
            cpu.driving = false;
            cpu.running = null;
            cpu.main = null;
        }
        synchronized (cpu.stream) {
            if (cpu.stream.threads == cpu) {
                cpu.stream.threads = null;
            }
        }
        if (cpu.stream.fuel != null) {
            synchronized (cpu.stream) {
                RobotRandomness.limit(cpu.stream.fuel, cpu.stream.cutoffAt);
            }
        }
        return true;
    }

    // ---- The scheduler; all under the monitor ----

    private Task register(Thread t) {
        Task task = new Task(t, this, nextId++);
        tasks.add(task);
        synchronized (TASKS) {
            TASKS.put(t, task);
            any = true;
        }
        return task;
    }

    private boolean hasLive() {
        return !tasks.isEmpty();
    }

    /** Virtual time of the robot, without the clock's origin. */
    private long now() {
        return RobotRandomness.elapsedNanos(stream.turns(), RobotRandomness.count(stream.fuel));
    }

    private long count() {
        return RobotRandomness.count(stream.fuel);
    }

    private void makeReady(Task t) {
        t.state = State.READY;
        ready.addLast(t);
    }

    /**
     * A sleeping, joining, waiting or parked task's wait ends (its deadline, an interrupt, the
     * round's end): it gets in line, a waiter first for its monitor.
     */
    private void wake(Task t) {
        switch (t.state) {
            case SLEEPING, JOINING, PARKED -> makeReady(t);
            case WAITING -> {
                Monitor m = monitors.get(t.monitor);
                if (m != null) {
                    m.waiters.remove(t);
                    reacquire(t, t.monitor, m);
                } else {
                    makeReady(t);
                }
            }
            default -> {
            }
        }
    }

    /** A waiter is done waiting: it gets the monitor back (now, or in line) and then the CPU. */
    private void reacquire(Task w, Object o, Monitor m) {
        if (w.savedCount == 0) {
            makeReady(w); // a monitor the scheduler doesn't own (the JDK's code)
        } else if (m.owner == null) {
            m.owner = w;
            m.count = w.savedCount;
            w.savedCount = 0;
            makeReady(w);
        } else {
            w.state = State.BLOCKED;
            m.entrants.addLast(w);
        }
    }

    /** The owner left the monitor: the first in line gets it (and the CPU in line). */
    private void release(Object o, Monitor m) {
        m.owner = null;
        Task next = m.entrants.pollFirst();
        debug("release to " + (next == null ? null : next.thread.getName()));
        if (next != null) {
            m.owner = next;
            m.count = next.savedCount > 0 ? next.savedCount : 1;
            next.savedCount = 0;
            makeReady(next);
        } else if (m.waiters.isEmpty()) {
            monitors.remove(o);
        }
    }

    /** Sleepers and timed waits whose deadline passed get in line, earliest first. */
    private void wakeDue() {
        long now = -1;
        List<Task> due = null;
        for (Task t : tasks) {
            if (timed(t)) {
                if (now < 0) {
                    now = now();
                }
                if (t.deadline <= now) {
                    if (due == null) {
                        due = new ArrayList<>();
                    }
                    due.add(t);
                }
            }
        }
        if (due != null) {
            due.sort(Comparator.<Task>comparingLong(t -> t.deadline).thenComparingLong(t -> t.id));
            due.forEach(this::wake);
        }
    }

    private static boolean timed(Task t) {
        return (t.state == State.SLEEPING || t.state == State.JOINING || t.state == State.WAITING
                || t.state == State.PARKED) && t.deadline != Long.MAX_VALUE;
    }

    private static int priority(Task t) {
        return PRIORITIES ? t.thread.getPriority() : Thread.NORM_PRIORITY;
    }

    /** Takes the first waiting task of the highest priority, at least {@code min}; null if none. */
    private Task takeNext(int min) {
        Task best = null;
        for (Task t : ready) {
            if (priority(t) >= min && (best == null || priority(t) > priority(best))) {
                best = t;
            }
        }
        if (best != null) {
            ready.remove(best);
        }
        return best;
    }

    private void grant(Task next) {
        debug("grant " + next.thread.getName() + " kick=" + (next.kickOn != null));
        running = next;
        next.state = State.RUNNING;
        setLimit(next);
        Object kick = next.kickOn;
        if (kick != null) {
            // It waits in a real wait on the object: wake it there. Nobody else of the robot
            // holds that monitor now, and the waiter doesn't take this one while it holds it.
            // The waiter only looks at granted while it holds the object's monitor: it can't
            // go on before this hand-over is done (a real notify may have woken it already).
            next.kickOn = null;
            synchronized (kick) {
                next.granted = true;
                kick.notifyAll();
            }
        }
        notifyAll();
    }

    /** Count at which the running task's code next asks the scheduler. */
    private void setLimit(Task t) {
        long count = count();
        long limit = stream.cutoffAt;
        if (t.killed) {
            limit = Math.min(limit, count);
        } else if (tasks.size() > 1) {
            limit = Math.min(limit, count + QUANTUM_INSTRUCTIONS);
        }
        limit = Math.min(limit, sliceEnd);
        RobotRandomness.limit(stream.fuel, limit);
    }

    /** Waits until the task holds the CPU. Interrupts meanwhile are kept for the robot. */
    private void park(Task me) {
        boolean interrupted = false;
        while (running != me) {
            try {
                wait();
            } catch (InterruptedException e) {
                interrupted = true;
            }
        }
        if (interrupted) {
            me.thread.interrupt();
        }
    }

    /** The running task gave the CPU up (it blocks or ended): the next one gets it. */
    private void dispatch() {
        running = null;
        wakeDue();
        Task next = takeNext(Integer.MIN_VALUE);
        if (next == null && gapOwner != null) {
            next = gapOwner;
        }
        if (next == null && !driving) {
            Task early = wakeEarly();
            if (early != null) {
                wake(early);
                next = takeNext(Integer.MIN_VALUE);
            }
        }
        if (next == null) {
            if (!driving && gapOwner == null && main != null && main.state != State.DONE) {
                System.err.println("robocode-s: the robot's threads all wait for each other");
                if (DEBUG) {
                    for (Task t : tasks) {
                        debug("  " + t.thread.getName() + " " + t.state + " monitor=" + t.monitor
                                + " saved=" + t.savedCount + " kick=" + t.kickOn + " granted=" + t.granted);
                    }
                    monitors.forEach((k, m) -> debug("  monitor " + k + " owner="
                            + (m.owner == null ? null : m.owner.thread.getName()) + " count=" + m.count
                            + " entrants=" + m.entrants.size() + " waiters=" + m.waiters.size()));
                    debug("  at " + java.util.Arrays.toString(Thread.currentThread().getStackTrace()));
                }
            }
            notifyAll();
            return;
        }
        grant(next);
    }

    /**
     * Nothing can run: the virtual clock only moves with turns and work, so a sleeper (or a
     * timed wait) wakes early: the main thread if it sleeps, else the earliest deadline.
     */
    private Task wakeEarly() {
        Task best = null;
        for (Task t : tasks) {
            if (timed(t)) {
                if (t == main) {
                    best = t;
                    break;
                }
                if (best == null || t.deadline < best.deadline) {
                    best = t;
                }
            }
        }
        if (best == null && main != null && main.state == State.JOINING) {
            // The main thread joins a thread that sleeps without end: wake the sleeper.
            for (Task t : tasks) {
                if (t.state == State.SLEEPING && (best == null || t.deadline < best.deadline)) {
                    best = t;
                }
            }
        }
        return best;
    }

    private void preempt(Task me) {
        wakeDue();
        if (count() >= sliceEnd && (gapOwner != null || driving) && me != gapOwner) {
            // The gap or the round end's slice is over.
            makeReady(me);
            running = null;
            if (gapOwner != null) {
                grant(gapOwner);
            } else {
                notifyAll();
            }
            park(me);
            return;
        }
        Task next = takeNext(priority(me));
        if (next == null) {
            setLimit(me);
            return;
        }
        makeReady(me);
        grant(next);
        park(me);
    }

    private void yieldFrom(Task me) {
        wakeDue();
        Task next = takeNext(priority(me));
        if (next != null) {
            makeReady(me);
            grant(next);
            park(me);
        }
    }

    private void finish(Task me) {
        me.state = State.DONE;
        tasks.remove(me);
        ready.remove(me);
        for (Map.Entry<Object, Monitor> e : List.copyOf(monitors.entrySet())) {
            Monitor m = e.getValue();
            m.entrants.remove(me);
            m.waiters.remove(me);
            if (m.owner == me) {
                release(e.getKey(), m); // left by an exception the scheduler didn't see
            }
        }
        for (Task t : tasks) {
            if (t.state == State.JOINING && t.joining == me) {
                makeReady(t);
            }
        }
        if (running != me) {
            return;
        }
        if (me == main && !driving) {
            // The round is over: the others wait for its end (endRound).
            running = null;
            notifyAll();
            return;
        }
        dispatch();
    }

    /** The turn's end on the main thread: the others run in the turn's spare budget. */
    private void gap(Task me) {
        wakeDue();
        long spare = Math.min(GAP_INSTRUCTIONS,
                BUDGET_INSTRUCTIONS - (count() - stream.turnStart) - QUANTUM_INSTRUCTIONS);
        if (spare <= 0 || ready.isEmpty()) {
            return;
        }
        sliceEnd = count() + spare;
        gapOwner = me;
        me.state = State.GAP;
        grant(takeNext(Integer.MIN_VALUE));
        park(me);
        gapOwner = null;
        sliceEnd = Long.MAX_VALUE;
        me.state = State.RUNNING;
    }

    /** The round's end (a host thread): runs the robot's threads until none can run or the slice ends. */
    private void drive() {
        sliceEnd = count() + END_SLICE_INSTRUCTIONS;
        if (running == null) {
            dispatch();
        }
        long stuckAt = System.currentTimeMillis() + STUCK_MILLIS;
        while (running != null) {
            long left = stuckAt - System.currentTimeMillis();
            if (left <= 0) {
                System.err.println("robocode-s: robot thread " + running.thread.getName() + " blocks outside the "
                        + "scheduler at the round's end");
                tasks.remove(running);
                running = null;
                break;
            }
            try {
                wait(left);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
        }
        sliceEnd = Long.MAX_VALUE;
    }

    private static void checkKilled(Task me) {
        if (me != null && me.killed && Thread.currentThread() == me.thread) {
            throw newThreadDeath();
        }
    }

    @SuppressWarnings({"deprecation", "removal"})
    private static Error newThreadDeath() {
        // What classic's Thread.stop() gave robot threads that didn't end (up to Java 19).
        return new ThreadDeath();
    }

    private static long saturatedAdd(long a, long b) {
        long r = a + b;
        return ((a ^ r) & (b ^ r)) < 0 ? Long.MAX_VALUE : r;
    }
}
