/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import robocode.BattleResults;
import robocode.control.BattleSpecification;
import robocode.control.BattlefieldSpecification;
import robocode.control.RobocodeEngine;
import robocode.control.RobotSpecification;
import robocode.control.events.BattleAdaptor;
import robocode.control.events.BattleCompletedEvent;
import robocode.control.events.BattleErrorEvent;
import robocode.control.events.BattleMessageEvent;
import robocode.control.events.RoundStartedEvent;
import robocode.control.events.TurnEndedEvent;
import robocode.control.snapshot.IBulletSnapshot;
import robocode.control.snapshot.IRobotSnapshot;
import robocode.control.snapshot.IScoreSnapshot;
import robocode.control.snapshot.ITurnSnapshot;
import io.github.nhofsted.robocodes.determinism.RobotRandomness;

import java.io.BufferedWriter;
import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.UncheckedIOException;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Stream;
import java.util.zip.GZIPOutputStream;

/**
 * Runs a battle in classic Robocode and records every turn as JSON Lines.
 *
 * <p>The recording is the reference ("oracle") for differential tests against
 * the robocode-s engine (design §13). Classic must run deterministically and
 * with StrictMath semantics (ADR 0003), which requires these JVM flags:
 * {@code -XX:+UnlockDiagnosticVMOptions -XX:-InlineMathNatives -Ddebug=true}.
 * They are verified at startup. Classic's security mode stays on, as in the RoboRumble.
 *
 * <p>Robots' own randomness ({@code Math.random()}, {@code new Random()}) is not
 * seeded by classic on modern JVMs. The robocode-s determinism agent
 * ({@code -javaagent}) gives each robot its own stream derived from the battle
 * seed and the robot's name, the same way robocode-s runners do. The difftest jar
 * itself is a second agent with a timing-only fix for classic ({@link ClassicPatches}).
 *
 * <p>Usage: {@code Oracle --robots sample.Fire,sample.Walls --seed 42 --rounds 10
 * --out battle.jsonl.gz [--width 800] [--height 600] [--jars a.jar:b.jar] [--with-output]}.
 * Robot jars come from {@code --jars} and the {@code robocodes.difftest.robotJars}
 * system property (both path lists).
 *
 * <p>Robot console output is only recorded with {@code --with-output}: it is
 * buffered by the robot threads, so the turn it appears in is not deterministic.
 */
public final class Oracle {

    /** Version of the recording format; bump on incompatible changes. */
    public static final int FORMAT_VERSION = 2;

    public static void main(String[] args) throws Exception {
        Options options = Options.parse(args);
        checkJvmSetup();
        int errors = record(options);
        System.exit(errors == 0 ? 0 : 1);
    }

    /** Fails unless classic will run deterministically with StrictMath semantics. */
    static void checkJvmSetup() {
        if (!ClassicPatches.isInstalled()) {
            throw new IllegalStateException("the difftest jar must also be loaded as an agent "
                    + "(-javaagent:<difftest jar>) for classic's startRound timing fix, see ClassicPatches");
        }
        if (!RobotRandomness.isAgentInstalled()) {
            throw new IllegalStateException("the robocode-s determinism agent is required: run with "
                    + "-javaagent:<java-determinism jar> (design §7.2)");
        }
        if (!"true".equals(System.getProperty("debug"))) {
            throw new IllegalStateException("-Ddebug=true is required: without it, classic skips turns based on timing");
        }
        // With math intrinsics enabled, Math.sin differs from StrictMath.sin for a few
        // percent of inputs; 10,000 samples will find a difference with certainty.
        for (int i = 0; i < 10_000; i++) {
            double a = i * 0.0012345 - 6.0;
            if (Math.sin(a) != StrictMath.sin(a) || Math.cos(a) != StrictMath.cos(a) || Math.tan(a) != StrictMath.tan(a)) {
                throw new IllegalStateException("java.lang.Math differs from StrictMath: run with "
                        + "-XX:+UnlockDiagnosticVMOptions -XX:-InlineMathNatives (ADR 0003)");
            }
        }
    }

    /** Runs the battle and writes the recording. Returns the number of battle errors. */
    static int record(Options options) throws IOException {
        Path home = Files.createTempDirectory("robocodes-oracle");
        try {
            Path robotsDir = Files.createDirectories(home.resolve("robots"));
            for (String jar : options.robotJars) {
                Path source = Path.of(jar);
                Files.copy(source, robotsDir.resolve(source.getFileName()), StandardCopyOption.REPLACE_EXISTING);
            }
            // Read by classic's BattleManager when the battle starts.
            System.setProperty("RANDOMSEED", Long.toString(options.seed));

            try (Writer out = open(options.out)) {
                Recorder recorder = new Recorder(out, options);
                RobocodeEngine engine = new RobocodeEngine(home.toFile());
                try {
                    recorder.classicVersion = engine.getVersion();
                    engine.addBattleListener(recorder);
                    RobotSpecification[] robots = engine.getLocalRepository(String.join(",", options.robots));
                    if (robots.length != options.robots.size()) {
                        throw new IllegalArgumentException("Found " + robots.length + " of " + options.robots.size()
                                + " robots " + options.robots + " in " + options.robotJars);
                    }
                    List<String> names = new ArrayList<>();
                    for (RobotSpecification robot : robots) {
                        names.add(robot.getNameAndVersion());
                    }
                    RobotRandomness.configure(options.seed, names);
                    OracleHooks.enable(robots.length);
                    BattleSpecification battle = new BattleSpecification(
                            new BattlefieldSpecification(options.width, options.height),
                            options.rounds, 450, 0.1, 100, false, robots);
                    engine.runBattle(battle, true);
                } finally {
                    // close() calls setVisible(false), which reports a battle error when the UI
                    // module is absent; stop listening first.
                    engine.removeBattleListener(recorder);
                    engine.close();
                }
                recorder.rethrow();
                return recorder.errors;
            }
        } finally {
            deleteRecursively(home);
        }
    }

    private static Writer open(Path path) throws IOException {
        OutputStream stream = Files.newOutputStream(path);
        if (path.getFileName().toString().endsWith(".gz")) {
            stream = new GZIPOutputStream(stream, 1 << 16);
        }
        return new BufferedWriter(new OutputStreamWriter(stream, StandardCharsets.UTF_8), 1 << 16);
    }

    /** Best effort: on Windows, classic may still hold the robot jars open. */
    private static void deleteRecursively(Path dir) {
        try (Stream<Path> paths = Files.walk(dir)) {
            for (Path p : paths.sorted(Comparator.reverseOrder()).toList()) {
                Files.deleteIfExists(p);
            }
        } catch (IOException | UncheckedIOException e) {
            System.err.println("warning: could not delete " + dir + ": " + e);
        }
    }

    /** Writes one JSON line per event of interest. */
    private static final class Recorder extends BattleAdaptor {
        private final Writer out;
        private final Options options;
        private IOException failure;
        String classicVersion;
        int errors;

        Recorder(Writer out, Options options) {
            this.out = out;
            this.options = options;
        }

        @Override
        public void onRoundStarted(RoundStartedEvent event) {
            if (event.getRound() == 0) {
                writeHeader(event.getStartSnapshot());
            }
            writeSnapshot("roundStart", event.getStartSnapshot());
        }

        @Override
        public void onTurnEnded(TurnEndedEvent event) {
            writeSnapshot("turn", event.getTurnSnapshot());
        }

        @Override
        public void onBattleCompleted(BattleCompletedEvent event) {
            Json json = new Json().begin().field("type", "results").name("results").beginArray();
            for (BattleResults r : event.getIndexedResults()) {
                json.begin()
                        .field("name", r.getTeamLeaderName())
                        .field("rank", r.getRank())
                        .field("score", r.getScore())
                        .field("survival", r.getSurvival())
                        .field("lastSurvivorBonus", r.getLastSurvivorBonus())
                        .field("bulletDamage", r.getBulletDamage())
                        .field("bulletDamageBonus", r.getBulletDamageBonus())
                        .field("ramDamage", r.getRamDamage())
                        .field("ramDamageBonus", r.getRamDamageBonus())
                        .field("firsts", r.getFirsts())
                        .field("seconds", r.getSeconds())
                        .field("thirds", r.getThirds())
                        .end();
            }
            write(json.endArray().end());
        }

        @Override
        public void onBattleError(BattleErrorEvent event) {
            errors++;
            System.err.println("battle error: " + event.getError());
            if (event.getErrorInstance() != null) {
                event.getErrorInstance().printStackTrace();
            }
        }

        @Override
        public void onBattleMessage(BattleMessageEvent event) {
            System.err.println(event.getMessage());
        }

        private void writeHeader(ITurnSnapshot start) {
            Json json = new Json().begin()
                    .field("type", "header")
                    .field("format", FORMAT_VERSION)
                    .field("robotRandomness", "robocode-s/1")
                    .field("classic", classicVersion == null ? "" : classicVersion.trim())
                    .field("java", System.getProperty("java.vm.vendor") + " " + System.getProperty("java.vm.version"))
                    .field("arch", System.getProperty("os.arch"))
                    .field("seed", options.seed)
                    .field("rounds", options.rounds)
                    .field("width", options.width)
                    .field("height", options.height)
                    .field("inactivityTime", 450)
                    .field("gunCoolingRate", 0.1)
                    .name("robots").beginArray();
            for (IRobotSnapshot robot : start.getRobots()) {
                json.value(robot.getName());
            }
            write(json.endArray().end());
        }

        private void writeSnapshot(String type, ITurnSnapshot snapshot) {
            Json json = new Json().begin()
                    .field("type", type)
                    .field("round", snapshot.getRound())
                    .field("turn", snapshot.getTurn())
                    .name("robots").beginArray();
            for (IRobotSnapshot r : snapshot.getRobots()) {
                IScoreSnapshot score = r.getScoreSnapshot();
                json.begin()
                        .field("state", r.getState().name())
                        .field("x", r.getX())
                        .field("y", r.getY())
                        .field("energy", r.getEnergy())
                        .field("velocity", r.getVelocity())
                        .field("bodyHeading", r.getBodyHeading())
                        .field("gunHeading", r.getGunHeading())
                        .field("radarHeading", r.getRadarHeading())
                        .field("gunHeat", r.getGunHeat())
                        .field("score", score.getCurrentScore())
                        .field("totalScore", score.getTotalScore());
                String output = options.withOutput ? r.getOutputStreamSnapshot() : null;
                if (output != null && !output.isEmpty()) {
                    json.field("output", output);
                }
                json.end();
            }
            OracleHooks.Turn hooks = OracleHooks.drain();
            json.endArray().name("commands").beginArray();
            for (String c : hooks.commands) {
                raw(json, c);
            }
            json.endArray().name("remaining").beginArray();
            for (String r : hooks.remaining) {
                raw(json, r);
            }
            json.endArray().name("events").beginArray();
            for (List<String> robotEvents : hooks.events) {
                json.beginArray();
                for (String e : robotEvents) {
                    raw(json, e);
                }
                json.endArray();
            }
            json.endArray().name("bullets").beginArray();
            for (IBulletSnapshot b : snapshot.getBullets()) {
                json.begin()
                        .field("owner", b.getOwnerIndex())
                        .field("id", b.getBulletId())
                        .field("state", b.getState().name())
                        .field("x", b.getX())
                        .field("y", b.getY())
                        .field("heading", b.getHeading())
                        .field("power", b.getPower())
                        .field("victim", b.getVictimIndex())
                        .end();
            }
            write(json.endArray().end());
        }

        private static void raw(Json json, String value) {
            if (value == null) {
                json.nullValue();
            } else {
                json.rawValue(value);
            }
        }

        private void write(Json json) {
            if (failure != null) {
                return;
            }
            try {
                out.write(json.toString());
                out.write('\n');
            } catch (IOException e) {
                failure = e;
            }
        }

        void rethrow() throws IOException {
            if (failure != null) {
                throw failure;
            }
        }
    }

    /** Command-line options. */
    record Options(List<String> robots, long seed, int rounds, int width, int height, Path out, boolean withOutput,
            List<String> robotJars) {

        static Options parse(String[] args) {
            List<String> robots = null;
            Long seed = null;
            int rounds = 10;
            int width = 800;
            int height = 600;
            Path out = null;
            boolean withOutput = false;
            String jars = System.getProperty("robocodes.difftest.robotJars", "");
            for (int i = 0; i < args.length; i++) {
                if (args[i].equals("--with-output")) {
                    withOutput = true;
                    continue;
                }
                String value = i + 1 < args.length ? args[i + 1] : null;
                switch (args[i]) {
                    case "--robots" -> robots = List.of(require(args[i], value).split(","));
                    case "--seed" -> seed = Long.parseLong(require(args[i], value));
                    case "--rounds" -> rounds = Integer.parseInt(require(args[i], value));
                    case "--width" -> width = Integer.parseInt(require(args[i], value));
                    case "--height" -> height = Integer.parseInt(require(args[i], value));
                    case "--out" -> out = Path.of(require(args[i], value));
                    case "--jars" -> jars = jars + File.pathSeparator + require(args[i], value);
                    default -> throw usage("unknown argument " + args[i]);
                }
                i++;
            }
            if (robots == null || seed == null || out == null) {
                throw usage("--robots, --seed and --out are required");
            }
            List<String> robotJars = new ArrayList<>();
            for (String jar : jars.split(File.pathSeparator)) {
                if (!jar.isBlank()) {
                    robotJars.add(jar);
                }
            }
            return new Options(robots, seed, rounds, width, height, out, withOutput, robotJars);
        }

        private static String require(String flag, String value) {
            if (value == null) {
                throw usage(flag + " needs a value");
            }
            return value;
        }

        private static IllegalArgumentException usage(String problem) {
            return new IllegalArgumentException(problem + "\nusage: Oracle --robots a.B,c.D --seed N --out FILE"
                    + " [--rounds N] [--width W] [--height H] [--jars PATHS] [--with-output]");
        }
    }

    /** Minimal JSON builder; doubles are written in Java's shortest round-trip form. */
    static final class Json {
        private final StringBuilder sb = new StringBuilder(4096);
        private boolean needComma;

        Json begin() {
            separator();
            sb.append('{');
            needComma = false;
            return this;
        }

        Json end() {
            sb.append('}');
            needComma = true;
            return this;
        }

        Json beginArray() {
            separator();
            sb.append('[');
            needComma = false;
            return this;
        }

        Json endArray() {
            sb.append(']');
            needComma = true;
            return this;
        }

        Json name(String name) {
            separator();
            string(name);
            sb.append(':');
            needComma = false;
            return this;
        }

        Json field(String name, String value) {
            return name(name).value(value);
        }

        Json field(String name, long value) {
            name(name);
            sb.append(value);
            needComma = true;
            return this;
        }

        Json field(String name, double value) {
            name(name);
            if (Double.isFinite(value)) {
                sb.append(value);
            } else {
                string(Double.toString(value)); // NaN and infinities are not valid JSON numbers
            }
            needComma = true;
            return this;
        }

        Json field(String name, boolean value) {
            name(name);
            sb.append(value);
            needComma = true;
            return this;
        }

        Json rawValue(String json) {
            separator();
            sb.append(json);
            needComma = true;
            return this;
        }

        Json nullValue() {
            separator();
            sb.append("null");
            needComma = true;
            return this;
        }

        Json value(String value) {
            separator();
            string(value);
            needComma = true;
            return this;
        }

        private void separator() {
            if (needComma) {
                sb.append(',');
            }
        }

        private void string(String s) {
            sb.append('"');
            for (int i = 0; i < s.length(); i++) {
                char c = s.charAt(i);
                switch (c) {
                    case '"' -> sb.append("\\\"");
                    case '\\' -> sb.append("\\\\");
                    case '\n' -> sb.append("\\n");
                    case '\r' -> sb.append("\\r");
                    case '\t' -> sb.append("\\t");
                    default -> {
                        if (c < 0x20) {
                            sb.append(String.format("\\u%04x", (int) c));
                        } else {
                            sb.append(c);
                        }
                    }
                }
            }
            sb.append('"');
        }

        @Override
        public String toString() {
            return sb.toString();
        }
    }
}
