/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

import net.sf.robocode.host.security.RobotClassLoader;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.parallel.Isolated;

import java.util.List;
import java.util.Set;
import java.util.function.IntUnaryOperator;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** A robot's {@code OutOfMemoryError} is noticed wherever it is caught (#178). Agent installed. */
@Isolated
class OutOfMemoryTest {

    /** Robot code that catches what it throws: an {@code OutOfMemoryError} for 1, else an exception. */
    public static class Catcher implements IntUnaryOperator {
        @Override
        public int applyAsInt(int n) {
            try {
                if (n == 1) {
                    throw new OutOfMemoryError("test");
                }
                throw new IllegalStateException("test");
            } catch (Throwable t) {
                return t instanceof OutOfMemoryError ? 2 : 3;
            }
        }
    }

    @BeforeEach
    void reset() {
        OutOfMemory.reset();
    }

    private static IntUnaryOperator robot() throws Exception {
        ClassLoader loader = new RobotClassLoader(OutOfMemoryTest.class.getClassLoader(),
                Set.copyOf(List.of(Catcher.class.getName())));
        return (IntUnaryOperator) loader.loadClass(Catcher.class.getName()).getDeclaredConstructor().newInstance();
    }

    @Test
    void robotCodeThatCatchesOneIsNoticed() throws Exception {
        IntUnaryOperator code = robot();
        assertEquals(3, code.applyAsInt(0), "the handler runs as before");
        assertFalse(OutOfMemory.happened(), "another exception isn't one");
        assertEquals(2, code.applyAsInt(1));
        assertTrue(OutOfMemory.happened());
    }

    @Test
    void aThreadThatDiesOfOneIsNoticed() throws Exception {
        Thread dying = new Thread(() -> {
            throw new OutOfMemoryError("test");
        });
        dying.setUncaughtExceptionHandler((t, e) -> { });
        dying.start();
        dying.join();
        assertTrue(OutOfMemory.happened());
    }

    @Test
    void everyHandlerIsPatchedOnce() throws Exception {
        byte[] original;
        try (var in = Catcher.class.getResourceAsStream("OutOfMemoryTest$Catcher.class")) {
            assertNotNull(in);
            original = in.readAllBytes();
        }
        byte[] patched = OutOfMemory.patchHandlers(original);
        assertNotNull(patched, "a handler patched");
        assertEquals(null, OutOfMemory.patchHandlers(classWithoutHandlers()), "nothing to patch");
    }

    private static byte[] classWithoutHandlers() throws Exception {
        try (var in = OutOfMemory.class.getResourceAsStream("OutOfMemory.class")) {
            // OutOfMemory itself has no try/catch.
            return in.readAllBytes();
        }
    }
}
