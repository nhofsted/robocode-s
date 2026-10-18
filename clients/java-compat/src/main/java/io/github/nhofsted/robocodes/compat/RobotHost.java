/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.determinism.RobotRandomness;
import io.github.nhofsted.robocodes.protocol.v1.ClientMessage;
import io.github.nhofsted.robocodes.protocol.v1.Hello;
import io.github.nhofsted.robocodes.protocol.v1.ProtocolVersion;
import io.github.nhofsted.robocodes.protocol.v1.Role;
import io.github.nhofsted.robocodes.protocol.v1.ServerMessage;
import net.sf.robocode.core.ContainerBase;
import net.sf.robocode.host.IHostManager;
import net.sf.robocode.host.RobotStatics;
import net.sf.robocode.host.proxies.IHostingRobotProxy;
import net.sf.robocode.repository.IRobotItem;
import net.sf.robocode.repository.IRobotSpecItem;
import net.sf.robocode.security.HiddenAccess;
import robocode.BattleRules;
import robocode.control.RobocodeEngine;
import robocode.control.RobotSpecification;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.Socket;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.List;
import java.util.Properties;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The robot host: one JVM per robot (design §6.4). It connects to the robocode-s server,
 * then runs the robot with classic Robocode's own robot side and a {@link RemotePeer}.
 *
 * <p>Usage: {@code robot-host --server host:port --robot sample.Fire
 * --jar robot.jar [--jar ...] --home dir [--seed N]}, with the robot's token in the
 * environment variable {@code ROBOCODES_ROBOT_TOKEN} (other users may read a command line;
 * {@code --token T} still works). A {@code --jar} may also name a class
 * folder (an IDE's output, e.g. {@code bin}): classic reads it as a development path, so
 * its robots are named with classic's {@code *} for development versions (except classic's
 * {@code sample.*} and {@code tested.*}). With the determinism agent loaded,
 * the robot's random streams are derived from the seed and its unique name (ADR 0009).
 *
 * <p>{@code robot-host --describe --home dir --jar robot.jar --robot a [--robot b ...]}
 * instead prints what classic's repository knows about each robot, teams expanded into
 * their members, in battle order. One tab-separated line per robot: {@code robot}, the
 * unique class name with version, the full class name, the team id (empty without a
 * team) and the type flags junior, advanced, team, droid, sentry, interactive and paint
 * as {@code 0}/{@code 1}. The battle's unique names are derived from these as in
 * classic's {@code Battle.createPeers}. Without {@code --robot} it lists every robot and
 * team in the jars instead: {@code list}, the name to select it by (with version) and
 * {@code 1} for a team, else {@code 0}.
 */
public final class RobotHost {

    private RobotHost() {
    }

    public static void main(String[] args) throws Exception {
        boolean describe = args.length > 0 && args[0].equals("--describe");
        if (describe) {
            args = Arrays.copyOfRange(args, 1, args.length);
        }
        String server = null;
        String token = System.getenv().getOrDefault("ROBOCODES_ROBOT_TOKEN", "");
        List<String> robots = new ArrayList<>();
        Path home = null;
        long seed = 0;
        List<Path> jars = new ArrayList<>();
        for (int i = 0; i + 1 < args.length; i += 2) {
            switch (args[i]) {
                case "--server" -> server = args[i + 1];
                case "--token" -> token = args[i + 1];
                case "--robot" -> robots.add(args[i + 1]);
                case "--jar" -> jars.add(Path.of(args[i + 1]));
                case "--home" -> home = Path.of(args[i + 1]);
                case "--seed" -> seed = Long.parseLong(args[i + 1]);
                default -> throw new IllegalArgumentException("unknown argument " + args[i]);
            }
        }
        if (describe) {
            if (home == null) {
                throw new IllegalArgumentException("usage: robot-host --describe --home dir "
                        + "--jar robot.jar [--robot name ...]");
            }
            System.exit(describe(robots, jars, home));
        }
        String robot = robots.isEmpty() ? null : robots.get(0);
        if (server == null || robot == null || home == null) {
            throw new IllegalArgumentException("usage: robot-host --server host:port --robot name "
                    + "--jar robot.jar --home dir [--seed N]");
        }
        System.exit(run(server, token, robot, jars, home, seed));
    }

    static int run(String server, String token, String robot, List<Path> jars, Path home, long seed)
            throws Exception {
        // One class loader for classic's engine classes, so our peer and classic's proxies
        // share types. Classic's package and thread checks are off; robocode-s isolates
        // robots by process and container instead (design §6).
        System.setProperty("NOSECURITY", "true");
        copyJars(jars, home);

        int colon = server.lastIndexOf(':');
        try (Socket socket = new Socket(server.substring(0, colon), Integer.parseInt(server.substring(colon + 1)))) {
            socket.setTcpNoDelay(true);
            InputStream in = new BufferedInputStream(socket.getInputStream());
            OutputStream out = new BufferedOutputStream(socket.getOutputStream());
            ClientMessage.newBuilder().setHello(Hello.newBuilder()
                    .setVersion(ProtocolVersion.newBuilder().setMajor(1).setMinor(0))
                    .setRole(Role.ROLE_ROBOT)
                    .setToken(token)).build().writeDelimitedTo(out);
            out.flush();
            ServerMessage first = ServerMessage.parseDelimitedFrom(in);
            if (first == null || !first.hasHelloAck()) {
                System.err.println("server refused the robot: " + (first == null ? "connection closed" : first));
                return 1;
            }
            var statics = first.getHelloAck().getRobot();
            if (RobotRandomness.isAgentInstalled()) {
                RobotRandomness.configure(seed, List.of(statics.getName()));
            }

            // Initializes classic's container and repository. Not closed: classic's cleanup
            // expects its battle module, and the process ends with the battle anyway.
            RobocodeEngine engine = new RobocodeEngine(home.toFile());
            RobotSpecification[] found = engine.getLocalRepository(robot);
            if (found.length == 0) {
                System.err.println("robot " + robot + " not found in " + jars);
                return 1;
            }
            RemotePeer peer = new RemotePeer(out);
            IHostingRobotProxy proxy = createProxy(found[0], statics, peer);
            return serve(in, proxy, peer);
        }
    }

    /** A field of a {@code --describe} line: whitespace runs as one space, "" for none. */
    static String field(Object value) {
        return value == null ? "" : value.toString().replaceAll("\\s+", " ").trim();
    }

    static int describe(List<String> robots, List<Path> jars, Path home) throws Exception {
        System.setProperty("NOSECURITY", "true");
        copyJars(jars, home);
        RobocodeEngine engine = new RobocodeEngine(home.toFile());
        if (robots.isEmpty()) {
            StringBuilder out = new StringBuilder();
            // What the robots run on: this JVM, as robots get the same one.
            out.append("platform\tJVM ").append(field(System.getProperty("java.version")))
                    .append('\n');
            for (RobotSpecification spec : engine.getLocalRepository()) {
                IRobotSpecItem item = (IRobotSpecItem) HiddenAccess.getFileSpecification(spec);
                // The name classic looks it up by: a development version's is
                // `name version*`, where the specification says `name* version`. Then the
                // details from its .properties, for the robot chooser.
                out.append("list\t").append(item.getUniqueFullClassNameWithVersion())
                        .append('\t').append(item.isTeam() ? '1' : '0')
                        .append('\t').append(field(spec.getAuthorName()))
                        .append('\t').append(field(spec.getVersion()))
                        .append('\t').append(field(spec.getDescription()))
                        .append('\t').append(field(spec.getWebpage()))
                        .append('\t').append(field(spec.getRobocodeVersion()))
                        .append('\n');
            }
            System.out.print(out);
            System.out.flush();
            return 0;
        }
        // Classic skips robots it can't find, so each one is looked up on its own first.
        for (String robot : robots) {
            if (engine.getLocalRepository(robot).length == 0) {
                System.err.println("robot " + robot + " not found in " + jars);
                return 1;
            }
        }
        // The deterministic rules this host's robots follow, checked against robocode-s's.
        StringBuilder out = new StringBuilder("rules\t").append(RobotRandomness.rulesVersion()).append('\n');
        // Then all in one selection, as classic does: a team's id numbers it by its
        // position in the selection, which tells two copies of the same team apart.
        for (RobotSpecification spec : engine.getLocalRepository(String.join(",", robots))) {
            IRobotItem item = (IRobotItem) HiddenAccess.getFileSpecification(spec);
            String team = HiddenAccess.getRobotTeamName(spec);
            out.append("robot\t").append(item.getUniqueFullClassNameWithVersion())
                    .append('\t').append(item.getFullClassName())
                    .append('\t').append(team == null ? "" : team);
            for (boolean flag : new boolean[] {item.isJuniorRobot(), item.isAdvancedRobot(),
                    item.isTeamRobot(), item.isDroid(), item.isSentryRobot(),
                    item.isInteractiveRobot(), item.isPaintRobot()}) {
                out.append('\t').append(flag ? '1' : '0');
            }
            out.append('\n');
        }
        System.out.print(out);
        System.out.flush();
        return 0;
    }

    /**
     * Gives classic's repository the robots: jars are copied into {@code home/robots},
     * class folders (an IDE's output) become classic's development paths, read in place.
     * The robots directory starts empty, so jars and the robot database of an earlier
     * listing in the same home are not found again.
     */
    private static void copyJars(List<Path> jars, Path home) throws IOException {
        Path robots = home.resolve("robots");
        if (Files.isDirectory(robots)) {
            try (var paths = Files.walk(robots)) {
                for (Path p : paths.sorted(Comparator.reverseOrder()).toList()) {
                    Files.delete(p);
                }
            }
        }
        Files.createDirectories(robots);
        List<String> folders = new ArrayList<>();
        for (Path jar : jars) {
            if (Files.isDirectory(jar)) {
                folders.add(jar.toAbsolutePath().toString());
            } else {
                Files.copy(jar, robots.resolve(jar.getFileName()), StandardCopyOption.REPLACE_EXISTING);
            }
        }
        // Classic's development paths, in its robocode.properties (it splits them at commas
        // and the path separator).
        Path settings = Files.createDirectories(home.resolve("config")).resolve("robocode.properties");
        Properties properties = new Properties();
        if (Files.isRegularFile(settings)) {
            try (InputStream in = Files.newInputStream(settings)) {
                properties.load(in);
            }
        }
        properties.setProperty("robocode.options.development.path", String.join(",", folders));
        properties.setProperty("robocode.options.development.path.excluded", "");
        // Without a stored CPU constant, classic's start-up measures one in a 5-second busy
        // loop. Only its engine side uses it, for turn timeouts; the server keeps time here.
        properties.setProperty("robocode.cpu.constant", Long.toString(RobotRandomness.CPU_CONSTANT_NANOS));
        try (OutputStream out = Files.newOutputStream(settings)) {
            properties.store(out, "Robocode Properties");
        }
        // Robots in a class folder write their data files into the battle's home, as robots
        // in jars do, rather than into the folder (classic's default for development robots).
        System.setProperty("ALWAYSUSECACHEFORDATA", "true");
    }

    private static IHostingRobotProxy createProxy(RobotSpecification spec,
            io.github.nhofsted.robocodes.protocol.v1.RobotStatics s, RemotePeer peer) {
        var r = s.getRules();
        BattleRules rules = HiddenAccess.createRules(r.getBattlefieldWidth(), r.getBattlefieldHeight(),
                r.getNumRounds(), r.getGunCoolingRate(), r.getInactivityTime(), r.getHideEnemyNames(),
                r.getSentryBorderSize());
        // The robot's own name is the canonical instance, as in classic (Convert.name).
        String name = Convert.name(s.getName());
        List<String> members = new ArrayList<>();
        for (String member : s.getTeamMembersList()) {
            members.add(Convert.name(member));
        }
        Matcher suffix = Pattern.compile("( \\(\\d+\\))$").matcher(name);
        RobotStatics statics = new RobotStatics(spec, name, suffix.find() ? suffix.group(1) : "",
                s.getTeamLeader(), rules, s.hasTeamName() ? s.getTeamName() : null,
                members.isEmpty() ? null : members, s.getRobotIndex(), s.getTeamIndex());
        IHostManager hostManager = ContainerBase.getComponent(IHostManager.class);
        return (IHostingRobotProxy) hostManager.createRobotProxy(spec, statics, peer);
    }

    private static int serve(InputStream in, IHostingRobotProxy proxy, RemotePeer peer) throws Exception {
        while (true) {
            ServerMessage message = ServerMessage.parseDelimitedFrom(in);
            if (message == null) {
                return 0;
            }
            switch (message.getMsgCase()) {
                case ROUND_START -> {
                    peer.newRound();
                    var start = message.getRoundStart();
                    proxy.startRound(Convert.commands(start.getCommands()), Convert.status(start.getStatus()));
                }
                case TICK -> peer.deliver(message.getTick());
                case ROUND_OVER -> {
                    peer.roundOver();
                    proxy.waitForStopThread();
                }
                case BATTLE_OVER -> {
                    proxy.cleanup();
                    return 0;
                }
                case ERROR -> {
                    System.err.println("server: " + message.getError().getMessage());
                    return 1;
                }
                default -> {
                }
            }
        }
    }
}
