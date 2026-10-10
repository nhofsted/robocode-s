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
import java.util.List;
import java.util.Random;
import java.util.function.DoubleBinaryOperator;
import java.util.function.DoubleUnaryOperator;

/**
 * Writes test vectors for the Rust fdlibm port ({@code crates/fdlibm}), computed
 * with {@code java.lang.StrictMath} of the running JDK.
 *
 * <p>Two kinds of lines, all doubles as raw bits in hex (NaN canonicalized):
 * <ul>
 *   <li>{@code v <fn> <x> [<y>] <result>}: explicit vectors for special values and
 *       the first inputs of every generator;</li>
 *   <li>{@code bulk <fn> <generator> <seed> <count> <hash>}: {@code count} inputs from
 *       a generator driven by {@code new java.util.Random(seed)}, with a 64-bit
 *       FNV-1a hash over all results. The Rust test regenerates the same inputs
 *       with its {@code java.util.Random} port.</li>
 * </ul>
 * Generators must stay identical to those in {@code crates/fdlibm/tests/strictmath.rs}.
 * Regenerate with {@code ./gradlew :tools:difftest:strictMathVectors}.
 */
public final class StrictMathVectors {

    private static final int BULK_COUNT = 100_000;
    private static final int EXPLICIT_PER_GENERATOR = 40;

    record Unary(String name, DoubleUnaryOperator f, String... generators) {
    }

    record Binary(String name, DoubleBinaryOperator f, String... generatorPairs) {
    }

    private static final String[] TRIG = {"unit", "ten", "million", "huge", "bits", "npio2", "small"};

    private static final List<Unary> UNARY = List.of(
            new Unary("sin", StrictMath::sin, TRIG),
            new Unary("cos", StrictMath::cos, TRIG),
            new Unary("tan", StrictMath::tan, TRIG),
            new Unary("atan", StrictMath::atan, "unit", "ten", "million", "huge", "bits", "small"),
            new Unary("log", StrictMath::log, "positive", "near1", "unit", "bits", "small"),
            new Unary("toRadians", StrictMath::toRadians, "unit", "huge", "bits"),
            new Unary("toDegrees", StrictMath::toDegrees, "unit", "huge", "bits"));

    private static final List<Binary> BINARY = List.of(
            new Binary("atan2", StrictMath::atan2, "unit:unit", "million:unit", "unit:million", "bits:bits",
                    "small:unit", "unit:small", "huge:huge"),
            new Binary("hypot", StrictMath::hypot, "unit:unit", "million:unit", "unit:small", "bits:bits",
                    "huge:huge", "small:small"));

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: StrictMathVectors <output file>");
        }
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(Path.of(args[0]), StandardCharsets.UTF_8))) {
            out.println("# java.lang.StrictMath test vectors from " + System.getProperty("java.vm.vendor") + " "
                    + System.getProperty("java.vm.version") + "; regenerate with ./gradlew :tools:difftest:strictMathVectors");
            List<Double> specials = specials();
            List<Double> binarySpecials = binarySpecials();
            int seed = 1;
            for (Unary u : UNARY) {
                for (double x : specials) {
                    out.println("v " + u.name + " " + hex(x) + " " + hex(u.f.applyAsDouble(x)));
                }
                for (String generator : u.generators) {
                    Random random = new Random(seed);
                    for (int i = 0; i < EXPLICIT_PER_GENERATOR; i++) {
                        double x = generate(generator, random);
                        out.println("v " + u.name + " " + hex(x) + " " + hex(u.f.applyAsDouble(x)));
                    }
                    random = new Random(seed);
                    long hash = FNV_OFFSET;
                    for (int i = 0; i < BULK_COUNT; i++) {
                        hash = fnv(hash, u.f.applyAsDouble(generate(generator, random)));
                    }
                    out.println("bulk " + u.name + " " + generator + " " + seed + " " + BULK_COUNT + " " + Long.toHexString(hash));
                    seed++;
                }
            }
            for (Binary b : BINARY) {
                for (double y : binarySpecials) {
                    for (double x : binarySpecials) {
                        out.println("v " + b.name + " " + hex(y) + " " + hex(x) + " " + hex(b.f.applyAsDouble(y, x)));
                    }
                }
                for (String pair : b.generatorPairs) {
                    String[] gens = pair.split(":");
                    Random random = new Random(seed);
                    for (int i = 0; i < EXPLICIT_PER_GENERATOR; i++) {
                        double y = generate(gens[0], random);
                        double x = generate(gens[1], random);
                        out.println("v " + b.name + " " + hex(y) + " " + hex(x) + " " + hex(b.f.applyAsDouble(y, x)));
                    }
                    random = new Random(seed);
                    long hash = FNV_OFFSET;
                    for (int i = 0; i < BULK_COUNT; i++) {
                        double y = generate(gens[0], random);
                        double x = generate(gens[1], random);
                        hash = fnv(hash, b.f.applyAsDouble(y, x));
                    }
                    out.println("bulk " + b.name + " " + pair + " " + seed + " " + BULK_COUNT + " " + Long.toHexString(hash));
                    seed++;
                }
            }
        }
    }

    /** Input generators; keep identical to the Rust test. */
    static double generate(String generator, Random r) {
        return switch (generator) {
            case "unit" -> (r.nextDouble() * 2 - 1);
            case "ten" -> (r.nextDouble() * 2 - 1) * 10;
            case "million" -> (r.nextDouble() * 2 - 1) * 1e6;
            case "huge" -> (r.nextDouble() * 2 - 1) * 1e300;
            case "bits" -> Double.longBitsToDouble(r.nextLong());
            case "positive" -> Double.longBitsToDouble(r.nextLong() & Long.MAX_VALUE);
            case "near1" -> 1 + (r.nextDouble() * 2 - 1) * Math.scalb(1.0, -r.nextInt(40));
            case "small" -> (r.nextDouble() * 2 - 1) * Math.scalb(1.0, -r.nextInt(60));
            case "npio2" -> {
                // Close to a multiple of pi/2, where argument reduction cancels.
                double x = (r.nextInt(2_000_000) - 1_000_000) * (Math.PI / 2);
                yield Double.longBitsToDouble(Double.doubleToRawLongBits(x) + (r.nextInt(41) - 20));
            }
            default -> throw new IllegalArgumentException(generator);
        };
    }

    private static List<Double> specials() {
        List<Double> s = new ArrayList<>();
        double[] base = {
            0.0, Double.MIN_VALUE, Double.MIN_NORMAL, Math.nextDown(Double.MIN_NORMAL), Double.MAX_VALUE,
            Double.POSITIVE_INFINITY, Double.NaN, 1.0, 0.5, 2.0, 0x1p-27, 0x1p-28, 0x1p-29, 0x1p-20, 0x1p66,
            0x1p19 * (Math.PI / 2), 0.3, 0.78125, 0.6744, 0.4375, 0.6875, 1.1875, 2.4375, Math.PI / 4, Math.PI / 2,
            3 * Math.PI / 4, Math.PI, 2 * Math.PI, 1e22, 1e300, 1e-300, 0.1, 10.0, 180.0, 1e6, 823549.6,
        };
        for (double b : base) {
            for (double x : new double[] {b, Math.nextUp(b), Math.nextDown(b)}) {
                s.add(x);
                s.add(-x);
            }
        }
        return s;
    }

    private static List<Double> binarySpecials() {
        List<Double> s = new ArrayList<>();
        double[] base = {0.0, Double.MIN_VALUE, Double.MIN_NORMAL, 1.0, 0.5, 0x1p60, 0x1p500, 0x1p-500, 0x1p-600,
            Double.MAX_VALUE, Double.POSITIVE_INFINITY, Double.NaN, 3.0, 1e300};
        for (double b : base) {
            s.add(b);
            s.add(-b);
        }
        return s;
    }

    private static final long FNV_OFFSET = 0xcbf29ce484222325L;

    /** FNV-1a over the 8 little-endian bytes of the result (NaN canonicalized). */
    static long fnv(long hash, double value) {
        long bits = Double.doubleToLongBits(value);
        for (int i = 0; i < 8; i++) {
            hash ^= (bits >>> (8 * i)) & 0xFF;
            hash *= 0x100000001b3L;
        }
        return hash;
    }

    private static String hex(double d) {
        return Long.toHexString(Double.doubleToLongBits(d));
    }
}
