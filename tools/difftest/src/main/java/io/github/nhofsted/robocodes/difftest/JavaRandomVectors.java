/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Random;

/**
 * Writes test vectors for the Rust port of {@code java.util.Random}
 * ({@code crates/engine/src/java_random.rs}), produced by the running JDK.
 *
 * <p>Format: one operation per line, {@code seed <long>} starts a new generator.
 * Doubles and floats are written as raw bits in hex, so comparisons are exact.
 * Regenerate with {@code ./gradlew :tools:difftest:javaRandomVectors}.
 */
public final class JavaRandomVectors {

    private static final long[] SEEDS = {
        0L, 1L, -1L, 42L, 0x5DEECE66DL, Long.MIN_VALUE, Long.MAX_VALUE, 0xFFFF_FFFF_FFFFL, 1L << 48,
        -8682522807148012L, 123456789L, 7L,
    };

    /** Bounds for nextInt(bound): powers of two, small odd values, and values that force rejections. */
    private static final int[] BOUNDS = {
        1, 2, 3, 5, 7, 10, 16, 100, 255, 256, 1000, 65536, 1 << 30, (1 << 30) + 1, 0x5555_5555,
        Integer.MAX_VALUE, Integer.MAX_VALUE - 1,
    };

    private static final int OPS_PER_SEED = 300;

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: JavaRandomVectors <output file>");
        }
        // Chooses which operations to exercise; unrelated to the generators under test.
        Random chooser = new Random(20261010L);
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(Path.of(args[0]), StandardCharsets.UTF_8))) {
            out.println("# java.util.Random test vectors from " + System.getProperty("java.vm.vendor") + " "
                    + System.getProperty("java.vm.version") + "; regenerate with ./gradlew :tools:difftest:javaRandomVectors");
            for (long seed : SEEDS) {
                Random random = new Random(seed);
                out.println("seed " + seed);
                for (int i = 0; i < OPS_PER_SEED; i++) {
                    switch (chooser.nextInt(9)) {
                        case 0 -> out.println("nextInt " + random.nextInt());
                        case 1, 2 -> {
                            int bound = BOUNDS[chooser.nextInt(BOUNDS.length)];
                            out.println("nextIntBounded " + bound + " " + random.nextInt(bound));
                        }
                        case 3 -> out.println("nextLong " + random.nextLong());
                        case 4 -> out.println("nextBoolean " + random.nextBoolean());
                        case 5 -> out.println("nextFloat " + Integer.toHexString(Float.floatToRawIntBits(random.nextFloat())));
                        case 6 -> out.println("nextDouble " + Long.toHexString(Double.doubleToRawLongBits(random.nextDouble())));
                        case 7 -> {
                            byte[] bytes = new byte[chooser.nextInt(10)];
                            random.nextBytes(bytes);
                            StringBuilder hex = new StringBuilder();
                            for (byte b : bytes) {
                                hex.append(String.format("%02x", b));
                            }
                            out.println("nextBytes " + bytes.length + " " + (hex.isEmpty() ? "-" : hex));
                        }
                        case 8 -> {
                            int size = chooser.nextInt(12);
                            List<Integer> list = new ArrayList<>();
                            for (int j = 0; j < size; j++) {
                                list.add(j);
                            }
                            Collections.shuffle(list, random);
                            StringBuilder order = new StringBuilder();
                            for (int j : list) {
                                order.append(order.isEmpty() ? "" : ",").append(j);
                            }
                            out.println("shuffle " + size + " " + (order.isEmpty() ? "-" : order));
                        }
                        default -> throw new AssertionError();
                    }
                }
                // setSeed mid-stream must reset the generator.
                random.setSeed(seed ^ 0x1234_5678L);
                out.println("setSeed " + (seed ^ 0x1234_5678L));
                out.println("nextLong " + random.nextLong());
            }
        }
    }
}
