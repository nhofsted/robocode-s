/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Function;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** A robot's own threads run deterministically on one virtual CPU (ADR 0025). Agent installed. */
class RobotThreadsTest {

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    /** Work that takes some counted instructions. */
    public static final class Busy {
        static int spin(int n) {
            int x = 0;
            for (int i = 0; i < n; i++) {
                x = x * 31 + i;
            }
            return x;
        }
    }

    /** Logs its name with a counter, with work in between. */
    public static final class Worker implements Runnable {
        final List<String> log;
        final String name;

        public Worker(List<String> log, String name) {
            this.log = log;
            this.name = name;
        }

        @Override
        public void run() {
            for (int i = 0; i < 300; i++) {
                Busy.spin(20_000);
                log.add(name + i);
            }
        }
    }

    /** Robot code: starts a helper that works alongside the main thread, then joins it. */
    public static final class Interleave implements Function<Object, Object> {
        @Override
        @SuppressWarnings("unchecked")
        public Object apply(Object log) {
            List<String> l = (List<String>) log;
            Thread helper = new Thread(new Worker(l, "h"));
            helper.start();
            new Worker(l, "m").run();
            try {
                helper.join();
            } catch (InterruptedException e) {
                l.add("interrupted");
            }
            return null;
        }
    }

    /** A thread of the robot's own class, which sets a flag the main thread busy-waits on. */
    public static final class Setter extends Thread {
        final Spinner spinner;

        public Setter(Spinner spinner) {
            this.spinner = spinner;
        }

        @Override
        public void run() {
            spinner.work = Busy.spin(3_000_000);
            spinner.done = true;
        }
    }

    /** Robot code: busy-waits for its helper, which only ends if the CPU changes hands. */
    public static final class Spinner implements Function<Object, Object> {
        volatile boolean done;
        int work;
        long spins;

        @Override
        public Object apply(Object ignored) {
            new Setter(this).start();
            while (!done) {
                spins++;
            }
            return spins;
        }
    }

    /** Sleeps on the virtual clock; records how long that was there. */
    public static final class Sleeper implements Runnable {
        public volatile long slept = -1;
        public volatile boolean interrupted;
        final long millis;

        public Sleeper(long millis) {
            this.millis = millis;
        }

        @Override
        public void run() {
            long start = System.nanoTime();
            try {
                Thread.sleep(millis);
            } catch (InterruptedException e) {
                interrupted = true;
            }
            slept = System.nanoTime() - start;
        }
    }

    /** Robot code: starts a thread (the start must come from robot code). */
    public static final class Starter implements Function<Object, Object> {
        @Override
        public Object apply(Object runnable) {
            Thread t = new Thread((Runnable) runnable);
            t.start();
            return t;
        }
    }

    /** Robot code: interrupts and joins a thread. */
    public static final class Interrupter implements Function<Object, Object> {
        @Override
        public Object apply(Object thread) {
            Thread t = (Thread) thread;
            t.interrupt();
            try {
                t.join();
            } catch (InterruptedException e) {
                return "interrupted";
            }
            return "joined";
        }
    }

    /** Ignores interrupts: sleeps again and again. */
    public static final class Stubborn implements Runnable {
        public volatile int interrupts;

        @Override
        public void run() {
            while (true) {
                try {
                    Thread.sleep(1000);
                } catch (InterruptedException e) {
                    interrupts++;
                }
            }
        }
    }

    /** Robot code: sleeps with nanoseconds out of range (as mn.nano.SkippedTurns does). */
    public static final class BadSleep implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            try {
                Thread.sleep(0, 2_000_000);
            } catch (IllegalArgumentException e) {
                return e;
            } catch (InterruptedException e) {
                return null;
            }
            return null;
        }
    }

    /** Robot code with a monitor, which the scheduler doesn't cover, that starts a thread. */
    public static final class Locking implements Function<Object, Object> {
        @Override
        public synchronized Object apply(Object ignored) {
            Thread t = new Thread(new Worker(new ArrayList<>(), "l"));
            t.start();
            return t;
        }
    }

    private static final Set<String> ROBOT_CLASSES = Set.of(BadSleep.class.getName(), Locking.class.getName(), Busy.class.getName(), Worker.class.getName(),
            Interleave.class.getName(), Setter.class.getName(), Spinner.class.getName(), Sleeper.class.getName(),
            Starter.class.getName(), Interrupter.class.getName(), Stubborn.class.getName());

    /** A robot: its classes in their own loader. */
    private static final class Robot {
        final ClassLoader loader = new RobotClassLoader(RobotThreadsTest.class.getClassLoader(), ROBOT_CLASSES);

        @SuppressWarnings("unchecked")
        Function<Object, Object> code(Class<?> c) throws Exception {
            return (Function<Object, Object>) loader.loadClass(c.getName()).getDeclaredConstructor().newInstance();
        }

        Object make(Class<?> c, Class<?>[] types, Object... args) throws Exception {
            return loader.loadClass(c.getName()).getDeclaredConstructor(types).newInstance(args);
        }

        long[] fuel(Class<?> c) throws Exception {
            Field f = loader.loadClass(c.getName()).getDeclaredField("robocodes$fuel");
            f.setAccessible(true);
            return (long[]) f.get(null);
        }
    }

    private interface Body {
        void run() throws Exception;
    }

    /** Runs the body as a robot's main thread; afterwards ends its round. */
    private static void onRobotThread(String group, Body body) throws Exception {
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
        thread.join(60_000);
        assertFalse(thread.isAlive(), "the robot's code ended");
        RobotThreads.endRound(thread);
        if (failure.get() != null) {
            throw new AssertionError("robot thread failed", failure.get());
        }
    }

    private static List<String> interleaving(int robot) throws Exception {
        List<String> log = new ArrayList<>();
        onRobotThread("sample.Fire (" + robot + ")", () -> new Robot().code(Interleave.class).apply(log));
        return log;
    }

    @Test
    void threadsInterleaveTheSameWayEveryRun() throws Exception {
        List<String> first = interleaving(60);
        assertEquals(600, first.size(), "both threads did all their work: " + first);
        int switches = 0;
        for (int i = 1; i < first.size(); i++) {
            if (first.get(i).charAt(0) != first.get(i - 1).charAt(0)) {
                switches++;
            }
        }
        // 300 blocks of about 120 000 instructions each, quanta of 625 000: a switch every ~5.
        assertTrue(switches > 50, "the CPU changed hands at quanta: " + switches);
        for (int robot = 61; robot < 65; robot++) {
            assertEquals(first, interleaving(robot), "the same interleaving");
        }
    }

    private static long[] spin(int robot) throws Exception {
        long[][] seen = new long[1][];
        onRobotThread("sample.Fire (" + robot + ")", () -> {
            Robot r = new Robot();
            long spins = (Long) r.code(Spinner.class).apply(null);
            seen[0] = new long[] {spins, RobotRandomness.count(r.fuel(Spinner.class))};
        });
        return seen[0];
    }

    @Test
    void aBusyWaitEndsAtTheSameInstructionEveryRun() throws Exception {
        long[] first = spin(70);
        assertTrue(first[0] > 0, "it spun");
        for (int robot = 71; robot < 74; robot++) {
            long[] again = spin(robot);
            assertEquals(first[0], again[0], "same number of spins");
            assertEquals(first[1], again[1], "same count");
        }
    }

    private static long[] sleep(int robot) throws Exception {
        long[] seen = new long[2];
        onRobotThread("sample.Fire (" + robot + ")", () -> {
            Robot r = new Robot();
            Object sleeper = r.make(Sleeper.class, new Class<?>[] {long.class}, 40L);
            r.code(Starter.class).apply(sleeper);
            Field slept = sleeper.getClass().getDeclaredField("slept");
            int turns = 0;
            while ((Long) slept.get(sleeper) < 0 && turns < 100) {
                RobotRandomness.turn(); // its main thread's turn ends: the others run
                turns++;
            }
            seen[0] = turns;
            seen[1] = (Long) slept.get(sleeper);
        });
        return seen;
    }

    @Test
    void sleepWaitsOnTheVirtualClock() throws Exception {
        long[] first = sleep(80);
        // 40 ms last as many turns as in classic, 5 (CLASSIC_TURN_NANOS); a turn is 1 ms of
        // virtual time: it wakes at the first turn's end past 5 ms.
        assertEquals(5, 40_000_000 / RobotRandomness.CLASSIC_TURN_NANOS);
        assertTrue(first[0] >= 5 && first[0] <= 6, "woke after " + first[0] + " turns");
        assertTrue(first[1] >= 5_000_000, "slept " + first[1] + " virtual ns");
        assertEquals(first[0], sleep(81)[0]);
        assertEquals(first[1], sleep(82)[1]);
    }

    @Test
    void anInterruptWakesASleeper() throws Exception {
        Object[] seen = new Object[2];
        onRobotThread("sample.Fire (90)", () -> {
            Robot r = new Robot();
            Object sleeper = r.make(Sleeper.class, new Class<?>[] {long.class}, 1_000_000L);
            Object thread = r.code(Starter.class).apply(sleeper);
            RobotRandomness.turn(); // it starts sleeping
            seen[0] = r.code(Interrupter.class).apply(thread);
            seen[1] = sleeper.getClass().getDeclaredField("interrupted").get(sleeper);
        });
        assertEquals("joined", seen[0]);
        assertEquals(true, seen[1]);
    }

    @Test
    void theRoundsEndStopsThreadsThatDontEnd() throws Exception {
        Object[] stubborn = new Object[2];
        AtomicReference<Thread> main = new AtomicReference<>();
        Thread robot = new Thread(new ThreadGroup("sample.Fire (100)"), () -> {
            try {
                Robot r = new Robot();
                stubborn[0] = r.make(Stubborn.class, new Class<?>[0]);
                stubborn[1] = r.code(Starter.class).apply(stubborn[0]);
                RobotRandomness.turn();
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        });
        main.set(robot);
        robot.setDaemon(true);
        robot.start();
        robot.join(10_000);
        Thread helper = (Thread) stubborn[1];
        assertTrue(helper.isAlive(), "the helper sleeps on");
        assertTrue(RobotThreads.endRound(robot), "the robot had threads of its own");
        helper.join(10_000);
        assertFalse(helper.isAlive(), "stopped");
        assertEquals(RobotThreads.END_PULSES, stubborn[0].getClass().getDeclaredField("interrupts").get(stubborn[0]));
    }

    @Test
    void robotsWithoutThreadsSleepAsBeforeWithTheSameStackTrace() throws Exception {
        AtomicReference<Object> thrown = new AtomicReference<>();
        onRobotThread("sample.Fire (120)", () -> thrown.set(new Robot().code(BadSleep.class).apply(null)));
        StackTraceElement[] stack = ((Throwable) thrown.get()).getStackTrace();
        assertEquals("java.lang.Thread", stack[0].getClassName());
        assertEquals("sleep", stack[0].getMethodName());
        assertEquals(BadSleep.class.getName(), stack[1].getClassName(), "no frames of the agent");
    }

    @Test
    void scheduledThreadsArentReportedButUncoveredOnesAre() throws Exception {
        onRobotThread("sample.Fire (130)", () -> {
            Robot r = new Robot();
            Object thread = r.code(Starter.class).apply(r.make(Sleeper.class, new Class<?>[] {long.class}, 0L));
            r.code(Interrupter.class).apply(thread); // joins it, from robot code
        });
        assertFalse(RobotRandomness.robotsWithThreads().contains("sample.Fire (130)"), "scheduled: reproducible");
        onRobotThread("sample.Fire (131)", () -> {
            Robot r = new Robot();
            r.code(Interrupter.class).apply(r.code(Locking.class).apply(null));
        });
        assertTrue(RobotRandomness.robotsWithThreads().contains("sample.Fire (131)"), "it uses a monitor too");
    }

    @Test
    void robotsWithoutThreadsHaveNoScheduler() throws Exception {
        AtomicReference<Object> threads = new AtomicReference<>("unset");
        onRobotThread("sample.Fire (110)", () -> {
            Robot r = new Robot();
            Busy.class.getName();
            r.code(Starter.class); // loads robot code, starts nothing
            RobotRandomness.turn();
            threads.set(RobotRandomness.streamOfCurrentThread().threads);
            long[] fuel = r.fuel(Starter.class);
            assertEquals(RobotRandomness.count(fuel) + RobotRandomness.CUTOFF_INSTRUCTIONS, fuel[0],
                    "the limit is the cutoff");
        });
        assertNull(threads.get());
    }
}
