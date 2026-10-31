/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.Condition;
import java.util.concurrent.locks.ReentrantLock;
import java.util.function.Function;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Synchronization between a robot's own threads under its scheduler (ADR 0025): monitors,
 * wait and notify, {@code java.util.concurrent}, executors and {@code Timer}. Every scenario
 * runs as two robots and must give the same log, and the right result. Agent installed.
 */
class RobotSyncTest {

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    /** Work: about six counted instructions a pass, more than a quantum for 200 000 passes. */
    public static final class Spin {
        static int spin(int n) {
            int x = 0;
            for (int i = 0; i < n; i++) {
                x = x * 31 + i;
            }
            return x;
        }
    }

    // ---- synchronized ----

    /** Shared state, incremented under a monitor with a quantum's work inside: lost updates show. */
    public static final class Shared {
        final Object lock = new Object();
        int value;
        int methodValue;
        final List<String> log = new ArrayList<>();

        void increment(String who) {
            synchronized (lock) {
                int v = value;
                Spin.spin(200_000); // preempted in here, holding the monitor
                value = v + 1;
                log.add(who);
            }
        }

        synchronized void incrementMethod(String who) {
            int v = methodValue;
            Spin.spin(200_000);
            methodValue = v + 1;
            log.add(who.toUpperCase());
        }
    }

    public static final class Incrementer implements Runnable {
        final Shared shared;
        final String name;

        public Incrementer(Shared shared, String name) {
            this.shared = shared;
            this.name = name;
        }

        @Override
        public void run() {
            for (int i = 0; i < 20; i++) {
                shared.increment(name);
                shared.incrementMethod(name);
            }
        }
    }

    public static final class Contended implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            Shared shared = new Shared();
            Thread a = new Thread(new Incrementer(shared, "a"));
            Thread b = new Thread(new Incrementer(shared, "b"));
            a.start();
            b.start();
            try {
                a.join();
                b.join();
            } catch (InterruptedException e) {
                return "interrupted";
            }
            return shared.value + " " + shared.methodValue + " " + shared.log;
        }
    }

    // ---- wait and notify ----

    /** A bounded buffer with synchronized methods, wait and notifyAll. */
    public static final class Buffer {
        private final ArrayDeque<Integer> items = new ArrayDeque<>();
        final List<String> log = new ArrayList<>();

        synchronized void put(int item) throws InterruptedException {
            while (items.size() == 2) {
                wait();
            }
            items.addLast(item);
            log.add("put" + item);
            notifyAll();
        }

        synchronized int take() throws InterruptedException {
            while (items.isEmpty()) {
                wait();
            }
            int item = items.removeFirst();
            log.add("take" + item);
            notifyAll();
            return item;
        }
    }

    public static final class Producer implements Runnable {
        final Buffer buffer;

        public Producer(Buffer buffer) {
            this.buffer = buffer;
        }

        @Override
        public void run() {
            try {
                for (int i = 0; i < 30; i++) {
                    Spin.spin(i * 5_000);
                    buffer.put(i);
                }
            } catch (InterruptedException e) {
                buffer.log.add("interrupted");
            }
        }
    }

    public static final class WaitNotify implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            Buffer buffer = new Buffer();
            Thread producer = new Thread(new Producer(buffer));
            producer.start();
            int sum = 0;
            try {
                for (int i = 0; i < 30; i++) {
                    sum += buffer.take();
                    Spin.spin(7_000);
                }
                producer.join();
            } catch (InterruptedException e) {
                return "interrupted";
            }
            return sum + " " + buffer.log;
        }
    }

    /** Waits on a monitor nobody notifies, with a timeout: on the virtual clock. */
    public static final class TimedWaiter implements Runnable {
        public volatile long waited = -1;

        @Override
        public void run() {
            Object o = new Object();
            synchronized (o) {
                long start = System.nanoTime();
                try {
                    o.wait(40);
                } catch (InterruptedException e) {
                    return;
                }
                waited = System.nanoTime() - start;
            }
        }
    }

    // ---- java.util.concurrent ----

    /** A producer and consumer with a ReentrantLock and two Conditions. */
    public static final class LockBuffer {
        private final ReentrantLock lock = new ReentrantLock();
        private final Condition notFull = lock.newCondition();
        private final Condition notEmpty = lock.newCondition();
        private final ArrayDeque<Integer> items = new ArrayDeque<>();
        final List<String> log = new ArrayList<>();

        void put(int item) throws InterruptedException {
            lock.lock();
            try {
                while (items.size() == 2) {
                    notFull.await();
                }
                items.addLast(item);
                log.add("put" + item);
                notEmpty.signal();
            } finally {
                lock.unlock();
            }
        }

        int take() throws InterruptedException {
            lock.lock();
            try {
                while (items.isEmpty()) {
                    notEmpty.await();
                }
                int item = items.removeFirst();
                log.add("take" + item);
                notFull.signal();
                return item;
            } finally {
                lock.unlock();
            }
        }
    }

    public static final class LockProducer implements Runnable {
        final LockBuffer buffer;

        public LockProducer(LockBuffer buffer) {
            this.buffer = buffer;
        }

        @Override
        public void run() {
            try {
                for (int i = 0; i < 30; i++) {
                    Spin.spin(i * 5_000);
                    buffer.put(i);
                }
            } catch (InterruptedException e) {
                buffer.log.add("interrupted");
            }
        }
    }

    public static final class LockCondition implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            LockBuffer buffer = new LockBuffer();
            Thread producer = new Thread(new LockProducer(buffer));
            producer.start();
            int sum = 0;
            try {
                for (int i = 0; i < 30; i++) {
                    sum += buffer.take();
                    Spin.spin(7_000);
                }
                producer.join();
            } catch (InterruptedException e) {
                return "interrupted";
            }
            return sum + " " + buffer.log;
        }
    }

    public static final class LatchWorker implements Runnable {
        final CountDownLatch latch;
        final List<String> log;
        final int n;

        public LatchWorker(CountDownLatch latch, List<String> log, int n) {
            this.latch = latch;
            this.log = log;
            this.n = n;
        }

        @Override
        public void run() {
            Spin.spin(n * 300_000);
            synchronized (log) {
                log.add("done" + n);
            }
            latch.countDown();
        }
    }

    public static final class Latch implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            CountDownLatch latch = new CountDownLatch(3);
            List<String> log = new ArrayList<>();
            for (int n = 3; n >= 1; n--) {
                new Thread(new LatchWorker(latch, log, n)).start();
            }
            try {
                latch.await();
            } catch (InterruptedException e) {
                return "interrupted";
            }
            synchronized (log) {
                log.add("awaited");
                return log.toString();
            }
        }
    }

    public static final class Executor implements Function<Object, Object> {
        @Override
        public Object apply(Object ignored) {
            ExecutorService pool = Executors.newFixedThreadPool(2);
            List<String> log = new ArrayList<>();
            try {
                List<Future<Integer>> results = new ArrayList<>();
                for (int n = 0; n < 6; n++) {
                    int task = n;
                    results.add(pool.submit(() -> {
                        int x = Spin.spin((6 - task) * 150_000);
                        synchronized (log) {
                            // The JDK numbers pools JVM-wide: in robocode-s each robot has a JVM of its own.
                            log.add("task" + task + "@" + Thread.currentThread().getName().replaceAll("pool-[0-9]+", "pool"));
                        }
                        return x;
                    }));
                }
                long sum = 0;
                for (Future<Integer> f : results) {
                    sum += f.get();
                }
                pool.shutdown();
                boolean ended = pool.awaitTermination(10, TimeUnit.SECONDS);
                synchronized (log) {
                    return sum + " " + ended + " " + log;
                }
            } catch (Exception e) {
                return e.toString();
            }
        }
    }

    /** Schedules a task on a Timer; the test's turns move the virtual clock. */
    public static final class Timed implements Function<Object, Object> {
        public volatile long ranAt = -1;
        private java.util.Timer timer;

        @Override
        public Object apply(Object command) {
            if ("cancel".equals(command)) {
                timer.cancel();
                return null;
            }
            timer = new java.util.Timer(true);
            long start = System.currentTimeMillis();
            timer.schedule(new java.util.TimerTask() {
                @Override
                public void run() {
                    ranAt = System.currentTimeMillis() - start;
                }
            }, 40);
            return null;
        }
    }

    private static final Set<String> ROBOT_CLASSES = Set.of(Spin.class.getName(), Shared.class.getName(),
            Incrementer.class.getName(), Contended.class.getName(), Buffer.class.getName(),
            Producer.class.getName(), WaitNotify.class.getName(), TimedWaiter.class.getName(),
            LockBuffer.class.getName(), LockProducer.class.getName(), LockCondition.class.getName(),
            LatchWorker.class.getName(), Latch.class.getName(), Executor.class.getName(), Timed.class.getName(),
            Timed.class.getName() + "$1", StarterHolder.class.getName());

    /** A robot: its classes in their own loader. */
    private static final class Robot {
        final ClassLoader loader = new RobotClassLoader(RobotSyncTest.class.getClassLoader(), ROBOT_CLASSES);

        @SuppressWarnings("unchecked")
        Function<Object, Object> code(Class<?> c) throws Exception {
            return (Function<Object, Object>) loader.loadClass(c.getName()).getDeclaredConstructor().newInstance();
        }

        Object make(Class<?> c) throws Exception {
            return loader.loadClass(c.getName()).getDeclaredConstructor().newInstance();
        }
    }

    private interface Body {
        void run() throws Exception;
    }

    private static int robots = 200;

    /** Runs the body as a new robot's main thread; afterwards ends its round. */
    private static void onRobotThread(Body body) throws Exception {
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup("sample.Fire (" + robots++ + ")"), () -> {
            try {
                body.run();
            } catch (Throwable t) {
                failure.set(t);
            }
        });
        thread.setDaemon(true);
        thread.start();
        thread.join(120_000);
        assertFalse(thread.isAlive(), "the robot's code ended");
        RobotThreads.endRound(thread);
        if (failure.get() != null) {
            throw new AssertionError("robot thread failed", failure.get());
        }
    }

    /** The scenario's result, twice: as two robots. */
    private static Object twice(Class<?> scenario) throws Exception {
        Object[] results = new Object[2];
        for (int k = 0; k < 2; k++) {
            int run = k;
            onRobotThread(() -> results[run] = new Robot().code(scenario).apply(null));
        }
        assertEquals(results[0], results[1], "the same every run");
        return results[0];
    }

    @Test
    void contendedMonitorsWithPreemptionInside() throws Exception {
        String result = (String) twice(Contended.class);
        assertTrue(result.startsWith("40 40 "), "no lost updates: " + result);
        assertTrue(result.contains("a") && result.contains("b"), result);
    }

    @Test
    void waitAndNotifyAll() throws Exception {
        String result = (String) twice(WaitNotify.class);
        assertTrue(result.startsWith("435 "), "every item, once: " + result);
        assertFalse(result.contains("interrupted"), result);
    }

    @Test
    void timedWaitOnTheVirtualClock() throws Exception {
        long[] turns = new long[2];
        long[] waited = new long[2];
        for (int k = 0; k < 2; k++) {
            int run = k;
            onRobotThread(() -> {
                Robot r = new Robot();
                Object waiter = r.make(TimedWaiter.class);
                startThread(r, (Runnable) waiter);
                var field = waiter.getClass().getField("waited");
                int n = 0;
                while ((Long) field.get(waiter) < 0 && n < 100) {
                    RobotRandomness.turn();
                    n++;
                }
                turns[run] = n;
                waited[run] = (Long) field.get(waiter);
            });
        }
        // 40 ms last as many turns as in classic: 5.
        assertTrue(turns[0] >= 5 && turns[0] <= 6, "woke after " + turns[0] + " turns");
        assertEquals(turns[0], turns[1]);
        assertEquals(waited[0], waited[1]);
    }

    @Test
    void reentrantLockAndConditions() throws Exception {
        String result = (String) twice(LockCondition.class);
        assertTrue(result.startsWith("435 "), "every item, once: " + result);
        assertFalse(result.contains("interrupted"), result);
    }

    @Test
    void countDownLatch() throws Exception {
        String result = (String) twice(Latch.class);
        assertTrue(result.endsWith("awaited]"), result);
        assertTrue(result.contains("done1") && result.contains("done2") && result.contains("done3"), result);
    }

    @Test
    void executorThreadsAreScheduled() throws Exception {
        String result = (String) twice(Executor.class);
        assertTrue(result.contains(" true "), "the pool ended: " + result);
        for (int n = 0; n < 6; n++) {
            assertTrue(result.contains("task" + n), result);
        }
    }

    @Test
    void timerOnTheVirtualClock() throws Exception {
        long[] turns = new long[2];
        long[] ran = new long[2];
        for (int k = 0; k < 2; k++) {
            int run = k;
            onRobotThread(() -> {
                Robot r = new Robot();
                Function<Object, Object> timed = r.code(Timed.class);
                timed.apply(null);
                var field = timed.getClass().getField("ranAt");
                int n = 0;
                while ((Long) field.get(timed) < 0 && n < 100) {
                    RobotRandomness.turn();
                    n++;
                }
                timed.apply("cancel");
                turns[run] = n;
                ran[run] = (Long) field.get(timed);
            });
        }
        // 40 ms of classic's time: 5 turns. The robot's own clock (ADR 0014) moves 1 ms a turn.
        assertTrue(turns[0] >= 5 && turns[0] <= 6, "after " + turns[0] + " turns");
        assertTrue(ran[0] >= 5 && ran[0] <= 6, "ran " + ran[0] + " ms of the robot's clock after scheduling");
        assertEquals(turns[0], turns[1]);
        assertEquals(ran[0], ran[1]);
    }

    /** Placeholder so the robot loader has a starter class of its own. */
    public static final class StarterHolder implements Function<Object, Object> {
        @Override
        public Object apply(Object runnable) {
            Thread t = new Thread((Runnable) runnable);
            t.start();
            return t;
        }
    }

    private static void startThread(Robot r, Runnable runnable) throws Exception {
        r.code(StarterHolder.class).apply(runnable);
    }
}
