/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedList;
import java.util.List;
import java.util.Random;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** Size-dependent JDK calls in robot classes charge per unit through synthetic helpers. Agent installed. */
class JdkCostsTest {

    private static final AtomicInteger GROUP = new AtomicInteger(100);

    @BeforeAll
    static void configure() {
        RobotRandomness.configure(42, List.of("sample.Fire"));
    }

    /** Robot code takes its operation and arguments as an array. */
    public interface Op {
        Object run(Object[] a);
    }

    static final int ARRAYCOPY = 0, FILL_DOUBLE = 1, COPY_OF = 2, CLONE = 3, SORT_DOUBLE = 4, SORT_OBJECTS = 5,
            COLLECTIONS_SORT = 6, CONTAINS_MISS = 7, FILL_LONG = 8, FILL_RANGE = 9, CONTAINS_LIST = 10,
            OWN_LIST = 11, COPY_OF_RANGE = 12, INDEX_OF = 13;

    /** A robot-side list whose contains must still be called. */
    public static class CountingList extends ArrayList<Integer> {
        static int calls;

        @Override
        public boolean contains(Object o) {
            calls++;
            return true;
        }
    }

    /** Robot code without loops: the same operation at two sizes differs only in the charge. */
    public static class Ops implements Op {
        @SuppressWarnings("unchecked")
        @Override
        public Object run(Object[] a) {
            switch ((Integer) a[0]) {
                case ARRAYCOPY -> {
                    int[] d = (int[]) a[2];
                    System.arraycopy((int[]) a[1], 0, d, 0, (Integer) a[3]);
                    return d;
                }
                case FILL_DOUBLE -> {
                    double[] x = (double[]) a[1];
                    Arrays.fill(x, 1.5);
                    return x;
                }
                case COPY_OF -> {
                    return Arrays.copyOf((int[]) a[1], (Integer) a[2]);
                }
                case COPY_OF_RANGE -> {
                    return Arrays.copyOfRange((int[]) a[1], (Integer) a[2], (Integer) a[3]);
                }
                case CLONE -> {
                    return ((int[]) a[1]).clone();
                }
                case SORT_DOUBLE -> {
                    Arrays.sort((double[]) a[1]);
                    return a[1];
                }
                case SORT_OBJECTS -> {
                    Arrays.sort((Integer[]) a[1]);
                    return a[1];
                }
                case COLLECTIONS_SORT -> {
                    Collections.sort((ArrayList<Integer>) a[1]);
                    return a[1];
                }
                case CONTAINS_MISS -> {
                    return ((ArrayList<Integer>) a[1]).contains(-1);
                }
                case INDEX_OF -> {
                    return ((List<Integer>) a[1]).indexOf(-1);
                }
                case FILL_LONG -> {
                    long[] x = (long[]) a[1];
                    Arrays.fill(x, 123456789012L);
                    return x;
                }
                case FILL_RANGE -> {
                    double[] x = (double[]) a[1];
                    Arrays.fill(x, (Integer) a[2], (Integer) a[3], 2.5);
                    return x;
                }
                case CONTAINS_LIST -> {
                    return ((List<Integer>) a[1]).contains(7);
                }
                case OWN_LIST -> {
                    List<Integer> l = new CountingList();
                    boolean r = l.contains(5);
                    return new Object[] {r, CountingList.calls};
                }
                default -> throw new IllegalArgumentException();
            }
        }
    }

    /** Robot code using many of the calls, to check the class still verifies. */
    public static class Many implements Op {
        @Override
        public Object run(Object[] a) {
            int[] s = {5, 3, 1};
            int[] d = new int[3];
            System.arraycopy(s, 0, d, 0, 3);
            long[] longs = new long[4];
            Arrays.fill(longs, 7L);
            Arrays.fill(longs, 1, 3, 9L);
            double[] ds = new double[4];
            Arrays.fill(ds, 1, 3, 4.5);
            Arrays.fill(ds, 0.5);
            Arrays.sort(d);
            Arrays.sort(d, 0, 2);
            Arrays.sort(new Integer[] {3, 1, 2}, Collections.reverseOrder());
            int[] c = Arrays.copyOf(d, 5);
            int[] r = Arrays.copyOfRange(d, 1, 3);
            int h = Arrays.hashCode(d);
            Object[] cl = new String[] {"x"}.clone();
            long[] lcl = longs.clone();
            List<Integer> list = new ArrayList<>(List.of(3, 1, 2));
            Collections.sort(list);
            Collections.reverse(list);
            Collections.shuffle(list, new Random(1));
            int mn = Collections.min(list);
            int mx = Collections.max(list);
            list.sort(null);
            list.addAll(List.of(9, 8));
            list.addAll(1, List.of(6));
            boolean b = list.contains(1) | list.remove((Object) 9) | list.removeAll(List.of(8))
                    | list.retainAll(list);
            int i1 = list.indexOf(3) + list.lastIndexOf(3);
            Object[] arr = list.toArray();
            Integer[] arr2 = list.toArray(new Integer[0]);
            LinkedList<Integer> ll = new LinkedList<>(list);
            boolean b2 = ll.contains(3);
            return c.length + r.length + h + cl.length + lcl.length + mn + mx + i1 + arr.length + arr2.length
                    + (b ? 1 : 0) + (b2 ? 1 : 0) + (int) longs[1] + (int) ds[1];
        }
    }

    /** Result of a run: counted work, return value, and what was thrown. */
    private record Result(long work, Object value, Throwable thrown) {
    }

    private static Set<String> names() {
        return Set.of(Ops.class.getName(), Many.class.getName(), CountingList.class.getName());
    }

    private static Result run(Class<? extends Op> robotClass, Object... args) throws Exception {
        AtomicReference<Result> out = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread thread = new Thread(new ThreadGroup("sample.Fire (" + GROUP.incrementAndGet() + ")"), () -> {
            try {
                ClassLoader loader = new RobotClassLoader(JdkCostsTest.class.getClassLoader(), names());
                Class<?> c = loader.loadClass(robotClass.getName());
                Op op = (Op) c.getDeclaredConstructor().newInstance();
                Field f = c.getDeclaredField("robocodes$fuel");
                f.setAccessible(true);
                long[] fuel = (long[]) f.get(null);
                RobotRandomness.limit(fuel, RobotRandomness.count(fuel) + 1_000_000_000L);
                long before = RobotRandomness.count(fuel);
                Object value = null;
                Throwable thrown = null;
                try {
                    value = op.run(args);
                } catch (Throwable t) {
                    thrown = t;
                }
                out.set(new Result(RobotRandomness.count(fuel) - before, value, thrown));
            } catch (Throwable t) {
                failure.set(t);
            }
        });
        thread.setDaemon(true);
        thread.start();
        thread.join(20_000);
        assertTrue(!thread.isAlive(), "the robot's code ended");
        assertNull(failure.get(), () -> String.valueOf(failure.get()));
        return out.get();
    }

    private static Result ops(Object... args) throws Exception {
        return run(Ops.class, args);
    }

    private static long units(int n) {
        return (long) n * (32 - Integer.numberOfLeadingZeros(n));
    }

    private static ArrayList<Integer> list(int n) {
        ArrayList<Integer> l = new ArrayList<>();
        Random r = new Random(n);
        for (int i = 0; i < n; i++) {
            l.add(r.nextInt(1000) + 1);
        }
        return l;
    }

    private static int[] ints(int n) {
        int[] a = new int[n];
        for (int i = 0; i < n; i++) {
            a[i] = n - i;
        }
        return a;
    }

    private static double[] doubles(int n) {
        Random r = new Random(n);
        double[] a = new double[n];
        for (int i = 0; i < n; i++) {
            a[i] = r.nextDouble();
        }
        return a;
    }

    private static Integer[] integers(int n) {
        Random r = new Random(n);
        Integer[] a = new Integer[n];
        for (int i = 0; i < n; i++) {
            a[i] = r.nextInt();
        }
        return a;
    }

    private static final int N1 = 10, N2 = 1000;

    private static void assertGrowth(String what, long expected, Result small, Result big) {
        assertNull(small.thrown(), what);
        assertNull(big.thrown(), what);
        assertEquals(expected, big.work() - small.work(), what);
    }

    @Test
    void arraycopyChargesPerElement() throws Exception {
        Result a = ops(ARRAYCOPY, ints(N1), new int[N1], N1);
        Result b = ops(ARRAYCOPY, ints(N2), new int[N2], N2);
        assertGrowth("arraycopy", 1L * (N2 - N1), a, b);
        assertArrayEquals(ints(N2), (int[]) b.value());
    }

    @Test
    void fillChargesPerElement() throws Exception {
        Result a = ops(FILL_DOUBLE, new double[N1]);
        Result b = ops(FILL_DOUBLE, new double[N2]);
        assertGrowth("fill", 1L * (N2 - N1), a, b);
        for (double d : (double[]) b.value()) {
            assertEquals(1.5, d);
        }
    }

    @Test
    void copyOfChargesPerNewLength() throws Exception {
        Result a = ops(COPY_OF, ints(N1), N1);
        Result b = ops(COPY_OF, ints(N2), N2);
        assertGrowth("copyOf", 4L * (N2 - N1), a, b);
        assertArrayEquals(ints(N2), (int[]) b.value());
    }

    @Test
    void copyOfRangeChargesPerElement() throws Exception {
        Result a = ops(COPY_OF_RANGE, ints(N1), 0, N1);
        Result b = ops(COPY_OF_RANGE, ints(N2), 0, N2);
        assertGrowth("copyOfRange", 4L * (N2 - N1), a, b);
        assertArrayEquals(ints(N2), (int[]) b.value());
    }

    @Test
    void cloneChargesPerElement() throws Exception {
        Result a = ops(CLONE, ints(N1));
        Result b = ops(CLONE, ints(N2));
        assertGrowth("clone", 4L * (N2 - N1), a, b);
        assertArrayEquals(ints(N2), (int[]) b.value());
    }

    @Test
    void primitiveSortChargesNLogN() throws Exception {
        Result a = ops(SORT_DOUBLE, doubles(N1));
        Result b = ops(SORT_DOUBLE, doubles(N2));
        assertGrowth("sort(double[])", 20L * (units(N2) - units(N1)), a, b);
        double[] expected = doubles(N2);
        Arrays.sort(expected);
        assertArrayEquals(expected, (double[]) b.value());
    }

    @Test
    void objectSortChargesNLogN() throws Exception {
        Result a = ops(SORT_OBJECTS, (Object) integers(N1));
        Result b = ops(SORT_OBJECTS, (Object) integers(N2));
        assertGrowth("sort(Object[])", 50L * (units(N2) - units(N1)), a, b);
        Integer[] expected = integers(N2);
        Arrays.sort(expected);
        assertArrayEquals(expected, (Integer[]) b.value());
    }

    @Test
    void collectionsSortChargesNLogN() throws Exception {
        Result a = ops(COLLECTIONS_SORT, list(N1));
        Result b = ops(COLLECTIONS_SORT, list(N2));
        assertGrowth("Collections.sort", 50L * (units(N2) - units(N1)), a, b);
        List<Integer> expected = list(N2);
        Collections.sort(expected);
        assertEquals(expected, b.value());
    }

    @Test
    void listContainsMissChargesPerElement() throws Exception {
        Result a = ops(CONTAINS_MISS, list(N1));
        Result b = ops(CONTAINS_MISS, list(N2));
        assertGrowth("ArrayList.contains", 3L * (N2 - N1), a, b);
        assertEquals(false, b.value());
        // Through the List interface, indexOf likewise.
        Result c = ops(INDEX_OF, list(N1));
        Result d = ops(INDEX_OF, list(N2));
        assertGrowth("List.indexOf", 3L * (N2 - N1), c, d);
        assertEquals(-1, d.value());
    }

    @Test
    void rangeFillChargesToMinusFrom() throws Exception {
        Result a = ops(FILL_RANGE, new double[N2], 0, N1);
        Result b = ops(FILL_RANGE, new double[N2], 5, N2);
        assertGrowth("fill range", 1L * ((N2 - 5) - N1), a, b);
        double[] x = (double[]) b.value();
        assertEquals(0.0, x[4]);
        assertEquals(2.5, x[5]);
        assertEquals(2.5, x[N2 - 1]);
    }

    @Test
    void wideParametersKeepTheirSlots() throws Exception {
        Result a = ops(FILL_LONG, new long[N1]);
        Result b = ops(FILL_LONG, new long[N2]);
        assertGrowth("fill(long[], long)", 1L * (N2 - N1), a, b);
        for (long l : (long[]) b.value()) {
            assertEquals(123456789012L, l);
        }
    }

    @Test
    void containsAnswersCorrectly() throws Exception {
        List<Integer> l = new ArrayList<>(List.of(1, 7, 3));
        assertEquals(true, ops(CONTAINS_LIST, l).value());
        assertEquals(false, ops(CONTAINS_LIST, new ArrayList<>(List.of(1, 3))).value());
        assertEquals(true, ops(CONTAINS_LIST, new LinkedList<>(l)).value());
    }

    @Test
    void virtualDispatchIsKept() throws Exception {
        Result r = ops(OWN_LIST);
        assertNull(r.thrown());
        // The robot-side class is loaded by the robot loader: its calls counter is its own.
        Object[] v = (Object[]) r.value();
        assertEquals(true, v[0], "the override answered");
        assertEquals(1, v[1], "the override was called once");
        // An override on an instance from outside the robot loader also answers true.
        assertEquals(true, ops(CONTAINS_LIST, new CountingList()).value());
    }

    @Test
    void exceptionsPropagateAndNegativeLengthDoesNotLowerTheCount() throws Exception {
        Result zero = ops(ARRAYCOPY, new int[4], new int[4], 0);
        Result negative = ops(ARRAYCOPY, new int[4], new int[4], -1000);
        assertNull(zero.thrown());
        assertInstanceOf(IndexOutOfBoundsException.class, negative.thrown());
        assertInstanceOf(ArrayIndexOutOfBoundsException.class, negative.thrown());
        // The throw skips the robot's code after the call, so compare throws with throws.
        Result smallNegative = ops(ARRAYCOPY, new int[4], new int[4], -1);
        assertEquals(smallNegative.work(), negative.work(), "a negative length charges nothing, whatever its size");
        assertTrue(negative.work() >= zero.work() - 10, "the count did not drop: " + negative.work() + " vs " + zero.work());
        // Too long for the arrays still throws at the same point, and charges its length.
        Result tooLong = ops(ARRAYCOPY, new int[4], new int[4], 50);
        assertInstanceOf(IndexOutOfBoundsException.class, tooLong.thrown());
        assertEquals(negative.work() + 50, tooLong.work());
    }

    @Test
    void aClassUsingManyCallsVerifiesAndRuns() throws Exception {
        Result r = run(Many.class, new Object[0]);
        assertNull(r.thrown(), () -> String.valueOf(r.thrown()));
        assertTrue(r.value() instanceof Integer);
        assertTrue(r.work() > 0);
        // The same code, run again, counts the same.
        Result again = run(Many.class, new Object[0]);
        assertEquals(r.work(), again.work());
        assertEquals(r.value(), again.value());
    }
}
