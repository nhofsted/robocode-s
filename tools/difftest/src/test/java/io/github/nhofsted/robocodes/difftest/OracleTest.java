/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Runs the oracle in a separate JVM per battle, like real use: classic Robocode
 * keeps global state, and the math flags must be set at JVM start.
 */
class OracleTest {

    private static final String MELEE = "sample.Corners,sample.Crazy,sample.SpinBot,sample.Tracker,sample.Walls";

    @TempDir
    Path dir;

    @Test
    void sameSeedGivesByteIdenticalRecordings() throws Exception {
        Path a = dir.resolve("a.jsonl");
        Path b = dir.resolve("b.jsonl");
        assertEquals(0, runOracle(oracleJvmArgs(), MELEE, 1, 3, a).exitCode);
        assertEquals(0, runOracle(oracleJvmArgs(), MELEE, 1, 3, b).exitCode);
        assertEquals(-1, Files.mismatch(a, b), "recordings with the same seed differ");
    }

    @Test
    void differentSeedGivesDifferentRecording() throws Exception {
        Path a = dir.resolve("a.jsonl");
        Path b = dir.resolve("b.jsonl");
        assertEquals(0, runOracle(oracleJvmArgs(), MELEE, 1, 1, a).exitCode);
        assertEquals(0, runOracle(oracleJvmArgs(), MELEE, 2, 1, b).exitCode);
        assertNotEquals(-1, Files.mismatch(a, b));
    }

    @Test
    void recordingHasHeaderRoundsTurnsAndResults() throws Exception {
        Path out = dir.resolve("battle.jsonl");
        assertEquals(0, runOracle(oracleJvmArgs(), "sample.Fire,sample.Walls", 3, 2, out).exitCode);
        List<String> lines = Files.readAllLines(out);
        assertTrue(lines.get(0).startsWith("{\"type\":\"header\",\"format\":1,\"robotRandomness\":\"robocode-s/1\","
                + "\"classic\":\"1.11.1\""), lines.get(0));
        assertTrue(lines.get(0).contains("\"robots\":[\"sample.Fire\",\"sample.Walls\"]"), lines.get(0));
        assertTrue(lines.get(1).startsWith("{\"type\":\"roundStart\",\"round\":0,\"turn\":0,"), lines.get(1));
        assertTrue(lines.get(2).startsWith("{\"type\":\"turn\",\"round\":0,\"turn\":1,"), lines.get(2));
        assertEquals(2, lines.stream().filter(l -> l.startsWith("{\"type\":\"roundStart\"")).count());
        assertTrue(lines.get(lines.size() - 1).startsWith("{\"type\":\"results\""));
        assertTrue(lines.stream().noneMatch(l -> l.contains("\"output\":")), "console output is opt-in");
    }

    @Test
    void refusesToRunWithMathIntrinsics() throws Exception {
        List<String> jvmArgs = new ArrayList<>(oracleJvmArgs());
        jvmArgs.remove("-XX:-InlineMathNatives");
        Result result = runOracle(jvmArgs, "sample.Fire,sample.Walls", 1, 1, dir.resolve("x.jsonl"));
        assertNotEquals(0, result.exitCode);
        assertTrue(result.stderr.contains("ADR 0003"), result.stderr);
    }

    @Test
    void refusesToRunWithoutDeterminismAgent() throws Exception {
        Result result = runOracle(oracleJvmArgs(), false, "sample.Fire,sample.Walls", 1, 1, dir.resolve("x.jsonl"));
        assertNotEquals(0, result.exitCode);
        assertTrue(result.stderr.contains("determinism agent"), result.stderr);
    }

    @Test
    void jsonEscapesStringsAndNonFiniteDoubles() {
        String json = new Oracle.Json().begin()
                .field("s", "a\"b\\c\nd\u0001")
                .field("nan", Double.NaN)
                .field("d", 0.1)
                .field("n", 42L)
                .end()
                .toString();
        assertEquals("{\"s\":\"a\\\"b\\\\c\\nd\\u0001\",\"nan\":\"NaN\",\"d\":0.1,\"n\":42}", json);
    }

    private static List<String> oracleJvmArgs() {
        return Arrays.asList(System.getProperty("robocodes.difftest.oracleJvmArgs").split(" "));
    }

    private record Result(int exitCode, String stderr) {
    }

    private Result runOracle(List<String> jvmArgs, String robots, long seed, int rounds, Path out)
            throws IOException, InterruptedException {
        return runOracle(jvmArgs, true, robots, seed, rounds, out);
    }

    private Result runOracle(List<String> jvmArgs, boolean withAgent, String robots, long seed, int rounds, Path out)
            throws IOException, InterruptedException {
        List<String> command = new ArrayList<>();
        command.add(Path.of(System.getProperty("java.home"), "bin", "java").toString());
        command.addAll(jvmArgs);
        if (withAgent) {
            command.add("-javaagent:" + System.getProperty("robocodes.difftest.agentJar"));
        }
        command.add("-Drobocodes.difftest.robotJars=" + System.getProperty("robocodes.difftest.robotJars"));
        command.add("-cp");
        command.add(System.getProperty("java.class.path"));
        command.add(Oracle.class.getName());
        command.addAll(List.of("--robots", robots, "--seed", Long.toString(seed), "--rounds",
                Integer.toString(rounds), "--out", out.toString()));
        Path stderr = Files.createTempFile(dir, "stderr", ".txt");
        Process process = new ProcessBuilder(command)
                .redirectOutput(ProcessBuilder.Redirect.DISCARD)
                .redirectError(stderr.toFile())
                .start();
        if (!process.waitFor(5, TimeUnit.MINUTES)) {
            process.destroyForcibly();
            throw new AssertionError("oracle timed out");
        }
        return new Result(process.exitValue(), Files.readString(stderr, StandardCharsets.UTF_8));
    }
}
