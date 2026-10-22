/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.determinism.RobotRandomness;
import io.github.nhofsted.robocodes.protocol.v1.ClientMessage;
import io.github.nhofsted.robocodes.protocol.v1.Hello;
import io.github.nhofsted.robocodes.protocol.v1.ProtocolVersion;
import io.github.nhofsted.robocodes.protocol.v1.Role;
import io.github.nhofsted.robocodes.protocol.v1.RobotAssignment;
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
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.Socket;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
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
 * {@code --token T} still works). With {@code --stdio} instead of {@code --server} the
 * protocol runs over this process's stdin and stdout, as the runner starts robots (ADR
 * 0019): no network and no token, the pipes are the robot's identity. The host takes the
 * two streams for itself and points {@code System.out} at stderr and {@code System.in} at
 * nothing, so that nothing else can write into or read from the protocol. A {@code --jar}
 * may also name a class folder (an IDE's output, e.g. {@code bin}): classic reads it as a
 * development path, so its robots are named with classic's {@code *} for development
 * versions (except classic's {@code sample.*} and {@code tested.*}). With the determinism
 * agent loaded, the robot's random streams are derived from the seed and its unique name
 * (ADR 0009).
 *
 * <p>{@code robot-host --stdio --warm --home dir} starts without a robot, for a runner's
 * warm pool (design §6.2): the JVM and classic's container are started while the host
 * waits on its stdin for a {@link RobotAssignment}, the robot, its seed and the mode. The
 * runner has put the robot's jars into {@code dir/jars} (sandbox grants are fixed when the
 * process starts, so the home is all it may read and write); the host moves them to where
 * classic reads them. Then it goes on as above. Nothing of the robot is initialized before the
 * assignment: its random streams are configured after {@code HelloAck}, as always.
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
        boolean stdio = args.length > 0 && Arrays.asList(args).contains("--stdio");
        if (stdio) {
            args = Arrays.stream(args).filter(a -> !a.equals("--stdio")).toArray(String[]::new);
        }
        boolean warm = Arrays.asList(args).contains("--warm");
        if (warm) {
            args = Arrays.stream(args).filter(a -> !a.equals("--warm")).toArray(String[]::new);
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
        if (warm) {
            if (!stdio || home == null || !robots.isEmpty() || !jars.isEmpty()) {
                throw new IllegalArgumentException("usage: robot-host --stdio --warm --home dir");
            }
            InputStream in = new BufferedInputStream(new FileInputStream(FileDescriptor.in));
            OutputStream out = new BufferedOutputStream(new FileOutputStream(FileDescriptor.out));
            System.setOut(System.err);
            System.setIn(new ByteArrayInputStream(new byte[0]));
            System.exit(runWarm(in, out, token, home));
        }
        String robot = robots.isEmpty() ? null : robots.get(0);
        if ((server == null) != stdio || robot == null || home == null) {
            throw new IllegalArgumentException("usage: robot-host (--server host:port | --stdio) "
                    + "--robot name --jar robot.jar --home dir [--seed N]");
        }
        if (stdio) {
            // Before anything else can print: the protocol owns the real stdin and stdout.
            InputStream in = new BufferedInputStream(new FileInputStream(FileDescriptor.in));
            OutputStream out = new BufferedOutputStream(new FileOutputStream(FileDescriptor.out));
            System.setOut(System.err);
            System.setIn(new ByteArrayInputStream(new byte[0]));
            System.exit(run(in, out, token, robot, jars, home, seed));
        }
        System.exit(run(server, token, robot, jars, home, seed));
    }

    static int run(String server, String token, String robot, List<Path> jars, Path home, long seed)
            throws Exception {
        int colon = server.lastIndexOf(':');
        try (Socket socket = new Socket(server.substring(0, colon), Integer.parseInt(server.substring(colon + 1)))) {
            socket.setTcpNoDelay(true);
            return run(new BufferedInputStream(socket.getInputStream()),
                    new BufferedOutputStream(socket.getOutputStream()), token, robot, jars, home, seed);
        }
    }

    /** Plays the robot over the connection {@code in} and {@code out}; each message is flushed. */
    static int run(InputStream in, OutputStream out, String token, String robot, List<Path> jars,
            Path home, long seed) throws Exception {
        // One class loader for classic's engine classes, so our peer and classic's proxies
        // share types. Classic's package and thread checks are off; robocode-s isolates
        // robots by process and container instead (design §6).
        System.setProperty("NOSECURITY", "true");
        copyJars(jars, home);
        return play(in, out, token, robot, jars, home, seed, null);
    }

    /**
     * A warm host: loads what it can before the robot is known, then reads the
     * {@link RobotAssignment} from {@code in} and plays the robot. Returns 0 if the
     * runner closes the pipe instead (the pool is discarded).
     */
    static int runWarm(InputStream in, OutputStream out, String token, Path home) throws Exception {
        System.setProperty("NOSECURITY", "true");
        // Classic's settings and engine (container, modules, repository) are independent of
        // the robot. Its repository reads the robots directory when asked for the robot
        // (not before: jars that arrive while it starts up are not found).
        Files.createDirectories(home.resolve("robots"));
        configure(home, List.of());
        RobocodeEngine engine = new RobocodeEngine(home.toFile());
        warmUp();
        RobotAssignment assignment = RobotAssignment.parseDelimitedFrom(in);
        if (assignment == null) {
            return 0;
        }
        if (assignment.getDeterministic() != RobotRandomness.isAgentInstalled()) {
            System.err.println("this host was started "
                    + (RobotRandomness.isAgentInstalled() ? "" : "not ") + "deterministic");
            return 1;
        }
        List<Path> jars = new ArrayList<>();
        for (String jar : assignment.getJarsList()) {
            Path from = home.resolve(jar).normalize();
            if (!from.startsWith(home.normalize())) {
                System.err.println("jar outside the home: " + jar);
                return 1;
            }
            jars.add(Files.move(from, home.resolve("robots").resolve(from.getFileName()),
                    StandardCopyOption.REPLACE_EXISTING));
        }
        return play(in, out, token, assignment.getRobot(), jars, home, assignment.getSeed(), engine);
    }

    /** Loads the protocol's classes, which the robot's connection needs first. */
    private static void warmUp() throws IOException {
        var buffer = new ByteArrayOutputStream();
        ClientMessage.newBuilder().setHello(Hello.newBuilder()
                .setVersion(ProtocolVersion.newBuilder().setMajor(1).setMinor(1))
                .setRole(Role.ROLE_ROBOT)).build().writeDelimitedTo(buffer);
        ServerMessage.parseDelimitedFrom(new ByteArrayInputStream(buffer.toByteArray()));
    }

    /** The robot's side of the protocol, with classic's {@code engine} if it is started already. */
    private static int play(InputStream in, OutputStream out, String token, String robot,
            List<Path> jars, Path home, long seed, RobocodeEngine engine) throws Exception {
        ClientMessage.newBuilder().setHello(Hello.newBuilder()
                .setVersion(ProtocolVersion.newBuilder().setMajor(1).setMinor(1))
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
        if (engine == null) {
            engine = new RobocodeEngine(home.toFile());
        }
        RobotSpecification[] found = engine.getLocalRepository(robot);
        if (found.length == 0) {
            System.err.println("robot " + robot + " not found in " + jars);
            return 1;
        }
        RemotePeer peer = new RemotePeer(out);
        IHostingRobotProxy proxy = createProxy(found[0], statics, peer);
        return serve(in, proxy, peer);
    }

    /** A field of a {@code --describe} line: whitespace runs as one space, "" for none. */
    static String field(Object value) {
        return value == null ? "" : value.toString().replaceAll("\\s+", " ").trim();
    }

    static int describe(List<String> robots, List<Path> jars, Path home) throws Exception {
        System.setProperty("NOSECURITY", "true");
        Map<String, Path> copies = copyJars(jars, home);
        RobocodeEngine engine = new RobocodeEngine(home.toFile());
        if (robots.isEmpty()) {
            StringBuilder out = new StringBuilder();
            // What the robots run on: this JVM, as robots get the same one.
            out.append("platform\tJVM ").append(field(System.getProperty("java.version")))
                    .append('\n');
            // And the deterministic rules it follows: a runner agent reports them.
            out.append("rules\t").append(RobotRandomness.rulesVersion()).append('\n');
            for (RobotSpecification spec : engine.getLocalRepository()) {
                IRobotSpecItem item = (IRobotSpecItem) HiddenAccess.getFileSpecification(spec);
                // The name classic looks it up by: a development version's is
                // `name version*`, where the specification says `name* version`. Then the
                // details from its .properties, for the robot chooser, and where it is.
                out.append("list\t").append(item.getUniqueFullClassNameWithVersion())
                        .append('\t').append(item.isTeam() ? '1' : '0')
                        .append('\t').append(field(spec.getAuthorName()))
                        .append('\t').append(field(spec.getVersion()))
                        .append('\t').append(field(spec.getDescription()))
                        .append('\t').append(field(spec.getWebpage()))
                        .append('\t').append(field(spec.getRobocodeVersion()))
                        .append('\t').append(source(item, copies))
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
     *
     * @return the jars by the file name of their copy
     */
    private static Map<String, Path> copyJars(List<Path> jars, Path home) throws IOException {
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
        Map<String, Path> copies = new HashMap<>();
        for (Path jar : jars) {
            if (Files.isDirectory(jar)) {
                folders.add(jar.toAbsolutePath().toString());
            } else {
                Files.copy(jar, robots.resolve(jar.getFileName()), StandardCopyOption.REPLACE_EXISTING);
                copies.put(jar.getFileName().toString(), jar.toAbsolutePath());
            }
        }
        configure(home, folders);
        return copies;
    }

    /**
     * Writes classic's settings into {@code home}: its development paths (class folders),
     * in its robocode.properties (it splits them at commas and the path separator), and
     * what it would otherwise measure.
     */
    private static void configure(Path home, List<String> folders) throws IOException {
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

    /**
     * The jar or class folder a robot was found in, as given on the command line ("" if
     * unknown): robocode-s caches listings per jar and gives battles only their robots' jars.
     */
    static String source(IRobotSpecItem item, Map<String, Path> copies) {
        String root = item.getRootPath();
        if (root == null) {
            return "";
        }
        try {
            String url = root.startsWith("jar:") ? root.substring(4) : root;
            int inside = url.indexOf("!/");
            Path path = Path.of(new URI(inside < 0 ? url : url.substring(0, inside)));
            Path given = item.isInJAR() ? copies.get(path.getFileName().toString()) : path;
            String s = given == null ? "" : given.toString();
            return s.indexOf('\t') >= 0 || s.indexOf('\n') >= 0 ? "" : s;
        } catch (Exception e) {
            return "";
        }
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
