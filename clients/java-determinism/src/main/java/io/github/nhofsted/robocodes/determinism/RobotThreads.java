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
 * <p>A robot's threads that robot code started run one at a time, and the CPU changes
 * hands only at points that depend on what the robot executes, never on the machine:
 * <ul>
 *   <li>a thread blocks: {@code Thread.sleep}, {@code join}, or it ends;</li>
 *   <li>{@code Thread.yield}, a new thread's start (it waits in line);</li>
 *   <li>a thread used up its quantum of counted work ({@link #QUANTUM_INSTRUCTIONS}),
 *       noticed where robot code checks its work limit (loops and handlers, ADR 0018), so
 *       busy-waiting on another thread ends;</li>
 *   <li>the end of the robot's turn: before its main thread executes, the other threads
 *       run, for at most what is left of the turn's budget and at most the virtual clock's
 *       time between turns ({@link #gap}).</li>
 * </ul>
 * The next thread is the first waiting one of the highest priority (round-robin among
 * equals). {@code Thread.sleep} and timed joins wait on the robot's virtual clock (ADR
 * 0014): a sleeper wakes at the first switch point after its deadline, or as soon as
 * nothing else can run. At a round's end the robot's other threads are interrupted as in
 * classic, given a bounded amount of work to end, and then stopped.
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
    private static volatile boolean any;

    enum State { READY, RUNNING, SLEEPING, JOINING, GAP, DONE }

    /** One of a robot's threads. */
    static final class Task {
        final Thread thread;
        final RobotThreads cpu;
        final long id;
        State state = State.READY;
        /** Virtual time (nanos since the robot's clock started) a sleep or timed join ends. */
        long deadline = Long.MAX_VALUE;
        Task joining;
        boolean entered;
        boolean killed;

        Task(Thread thread, RobotThreads cpu, long id) {
            this.thread = thread;
            this.cpu = cpu;
            this.id = id;
        }
    }

    private final RobotRandomness.Stream stream;
    /** Live tasks, in the order they were created. */
    private final List<Task> tasks = new ArrayList<>();
    private final ArrayDeque<Task> ready = new ArrayDeque<>();
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

    static Task task(Thread t) {
        if (!any) {
            return null;
        }
        synchronized (TASKS) {
            return TASKS.get(t);
        }
    }

    // ---- Hooks ----

    /**
     * {@code Thread.start()} (any thread): robot code on one of a robot's threads starts a
     * thread. The new thread joins the robot's CPU and waits in line; the starting thread,
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
        Class<?> caller = stream == null ? null : robotCaller();
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
     * A robot class that uses what the scheduler doesn't cover (monitors, {@code wait} and
     * {@code notify}, blocking {@code java.util.concurrent} classes) was loaded on one of the
     * robot's threads: its threads may block outside the scheduler, so a robot with threads is
     * reported as not reproducible (ADR 0024, 0025), now or when it starts one.
     */
    static void uncovered() {
        RobotRandomness.Stream stream = RobotRandomness.streamOfCurrentThread();
        if (stream == null) {
            return;
        }
        stream.uncovered = true;
        if (stream.threads != null) {
            RobotRandomness.reportThreads();
        }
    }

    /**
     * Whether the code that starts a thread is robot code: the first caller outside
     * {@code java.lang.Thread} (and Kotlin's {@code thread {}} helpers). Threads the JDK starts
     * for the robot (a {@code Timer}'s, an executor's) are left alone: their JDK code blocks
     * in ways the scheduler doesn't see.
     */
    private static boolean calledFromRobotCode() {
        return robotCaller() != null;
    }

    /** That caller, if it is robot code; null otherwise. */
    private static Class<?> robotCaller() {
        return STACK.walk(frames -> frames
                .map(StackWalker.StackFrame::getDeclaringClass)
                .filter(c -> c != Thread.class && c != RobotThreads.class && c != DeterminismAgent.class
                        && !c.getName().startsWith("kotlin."))
                .findFirst()
                .filter(c -> IdentityHashes.isRobotClassLoader(c.getClassLoader()))
                .orElse(null));
    }

    private static final StackWalker STACK = StackWalker.getInstance(StackWalker.Option.RETAIN_CLASS_REFERENCE);

    /**
     * {@code Thread.run()} and the {@code run()} of robot classes that extend {@code Thread}:
     * a new robot thread waits for the CPU before its code runs.
     */
    static void entering() {
        Task me = task(Thread.currentThread());
        if (me == null || me.entered) {
            return;
        }
        me.entered = true;
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
        Task me = task(Thread.currentThread());
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
        Task me = task(Thread.currentThread());
        if (me == null || !calledFromRobotCode()) {
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
                me.deadline = saturatedAdd(cpu.now(), duration);
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
        Task me = task(Thread.currentThread());
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
     * come here too): robot code on one of a robot's scheduled threads joins another of them.
     * Otherwise false: the JDK joins.
     */
    static boolean join(Thread t, long millis) throws InterruptedException {
        Task me = task(Thread.currentThread());
        Task target = task(t);
        if (me == null || target == null || target.cpu != me.cpu || target == me || !calledFromRobotCode()) {
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
                        : saturatedAdd(cpu.now(), millis > Long.MAX_VALUE / 1_000_000 ? Long.MAX_VALUE
                                : millis * 1_000_000);
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
     * another that sleeps or joins, which then gets the CPU in line (and throws when it runs).
     */
    static void interrupting(Object o) {
        if (!any) {
            return;
        }
        Task target = task((Thread) o);
        Task me = task(Thread.currentThread());
        if (target == null || me == null || target.cpu != me.cpu) {
            return;
        }
        RobotThreads cpu = me.cpu;
        synchronized (cpu) {
            if (cpu.running == me && (target.state == State.SLEEPING || target.state == State.JOINING)) {
                cpu.makeReady(target);
            }
        }
    }

    /**
     * The robot's turn ends (classic's robot side executes, on the main thread): the turn's
     * work cutoff starts again (ADR 0018), and the robot's other threads that wait for the CPU
     * run in what is left of the turn's budget, less a quantum, and at most
     * {@link #GAP_INSTRUCTIONS}.
     */
    static void turn(RobotRandomness.Stream stream, long[] fuel) {
        stream.cutoffAt = RobotRandomness.count(fuel) + RobotRandomness.CUTOFF_INSTRUCTIONS;
        RobotThreads cpu = stream.threads;
        if (cpu == null) {
            RobotRandomness.limit(fuel, stream.cutoffAt);
            return;
        }
        Task me = task(Thread.currentThread());
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
                    if (t.state == State.SLEEPING || t.state == State.JOINING) {
                        cpu.makeReady(t);
                    }
                }
                cpu.drive();
            }
            if (cpu.hasLive()) {
                for (Task t : List.copyOf(cpu.tasks)) {
                    t.killed = true;
                    if (t.state == State.SLEEPING || t.state == State.JOINING) {
                        cpu.makeReady(t);
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

    /** Sleepers and timed joins whose deadline passed get in line, earliest first. */
    private void wakeDue() {
        long now = -1;
        List<Task> due = null;
        for (Task t : tasks) {
            if ((t.state == State.SLEEPING || t.state == State.JOINING) && t.deadline != Long.MAX_VALUE) {
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
            due.forEach(this::makeReady);
        }
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
        running = next;
        next.state = State.RUNNING;
        setLimit(next);
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
            next = wakeEarly();
        }
        if (next == null) {
            if (!driving && gapOwner == null && main != null && main.state != State.DONE) {
                System.err.println("robocode-s: the robot's threads all wait for each other");
            }
            notifyAll();
            return;
        }
        grant(next);
    }

    /**
     * Nothing can run: the virtual clock only moves with turns and work, so a sleeper wakes
     * early: the main thread if it sleeps, else the earliest deadline.
     */
    private Task wakeEarly() {
        Task best = null;
        for (Task t : tasks) {
            if ((t.state == State.SLEEPING || t.state == State.JOINING) && t.deadline != Long.MAX_VALUE) {
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
