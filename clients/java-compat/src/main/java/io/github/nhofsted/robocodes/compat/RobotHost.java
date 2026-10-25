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
import io.github.nhofsted.robocodes.protocol.v1.RobotDescription;
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
import java.lang.reflect.Field;
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
 * <p>{@code robot-host --ide} is how an IDE's debugger starts a robot (#17, design §5.2). The
 * robots are those in the class folders of this JVM's classpath (the IDE's output; jars on
 * it are libraries), each offered on a connection of its own: classic's repository looks
 * them up before the {@code Hello}, which describes the robot, and the host then waits for a
 * battle to be made from them, as long as it takes, and ends when the robots taken for one
 * are done. Waiting for a robot's thread at a round's end has no limit either, so a robot
 * held at a breakpoint is not stopped by force. Everything else has a default: {@code
 * --server} is {@code 127.0.0.1:7878}, {@code --data} robocode-s's data folder (as the
 * program finds it), whose {@code ide-token} is the token (made here if robocode-s never
 * listened yet; {@code ROBOCODES_IDE_TOKEN} or {@code --token} override it), and {@code
 * --home} is {@code data/ide-home}. {@code --robot} and {@code --jar} choose robots and
 * folders instead. No module openings are needed for class folders: classic only needs
 * them for jars.
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
        boolean ide = Arrays.asList(args).contains("--ide");
        if (ide) {
            args = Arrays.stream(args).filter(a -> !a.equals("--ide")).toArray(String[]::new);
        }
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
        String token = System.getenv().getOrDefault(ide ? "ROBOCODES_IDE_TOKEN" : "ROBOCODES_ROBOT_TOKEN", "");
        List<String> robots = new ArrayList<>();
        Path home = null;
        Path data = null;
        long seed = 0;
        List<Path> jars = new ArrayList<>();
        if (args.length % 2 != 0) {
            throw new IllegalArgumentException("argument " + args[args.length - 1] + " needs a value");
        }
        for (int i = 0; i + 1 < args.length; i += 2) {
            switch (args[i]) {
                case "--server" -> server = args[i + 1];
                case "--token" -> token = args[i + 1];
                case "--robot" -> robots.add(args[i + 1]);
                case "--jar" -> jars.add(Path.of(args[i + 1]));
                case "--home" -> home = Path.of(args[i + 1]);
                case "--data" -> data = Path.of(args[i + 1]);
                case "--seed" -> seed = Long.parseLong(args[i + 1]);
                default -> throw new IllegalArgumentException("unknown argument " + args[i]);
            }
        }
        if (ide) {
            if (stdio || describe || warm) {
                throw new IllegalArgumentException("usage: robot-host --ide [--server host:port] "
                        + "[--data dir] [--robot name ...] [--jar folder ...] [--home dir] [--seed N]");
            }
            System.exit(runIde(server == null ? IDE_SERVER : server, token, robots, jars, home,
                    data == null ? dataFolder() : data, seed));
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

    /** Where robots from an IDE connect unless told otherwise (the server's default port). */
    static final String IDE_SERVER = "127.0.0.1:7878";

    /** At most this many robots are offered at once: as many as the server keeps waiting. */
    static final int MAX_IDE_ROBOTS = 16;

    /**
     * robocode-s's data folder, as the program finds it: {@code ROBOCODES_DATA}, else the
     * user's data folder of the platform.
     */
    static Path dataFolder() {
        String set = System.getenv("ROBOCODES_DATA");
        if (set != null && !set.isEmpty()) {
            return Path.of(set);
        }
        String os = System.getProperty("os.name", "").toLowerCase(java.util.Locale.ROOT);
        String home = System.getProperty("user.home");
        Path base;
        if (os.startsWith("windows")) {
            String local = System.getenv("LOCALAPPDATA");
            base = local != null && !local.isEmpty() ? Path.of(local) : Path.of(home, "AppData", "Local");
        } else if (os.startsWith("mac")) {
            base = Path.of(home, "Library", "Application Support");
        } else {
            String xdg = System.getenv("XDG_DATA_HOME");
            base = xdg != null && !xdg.isEmpty() && Path.of(xdg).isAbsolute()
                    ? Path.of(xdg) : Path.of(home, ".local", "share");
        }
        return base.resolve("robocode-s");
    }

    /**
     * The IDE token in the data folder (the server's {@code ide::token}): made here, readable
     * by the user only, if robocode-s hasn't listened for an IDE yet.
     */
    static String ideToken(Path data) throws IOException {
        Path file = data.resolve("ide-token");
        for (int attempt = 0; ; attempt++) {
            if (Files.isRegularFile(file)) {
                String token = Files.readString(file).trim();
                if (!token.isEmpty() || attempt > 0) {
                    return token;
                }
            }
            Files.createDirectories(data);
            byte[] bits = new byte[16];
            new java.security.SecureRandom().nextBytes(bits);
            StringBuilder token = new StringBuilder();
            for (byte b : bits) {
                token.append(String.format("%02x", b));
            }
            try {
                if (file.getFileSystem().supportedFileAttributeViews().contains("posix")) {
                    Files.createFile(file, java.nio.file.attribute.PosixFilePermissions.asFileAttribute(
                            java.nio.file.attribute.PosixFilePermissions.fromString("rw-------")));
                } else {
                    Files.createFile(file);
                }
                Files.writeString(file, token);
                return token.toString();
            } catch (java.nio.file.FileAlreadyExistsException e) {
                // robocode-s made it meanwhile: read it.
            }
        }
    }

    /** The class folders of this JVM's classpath: the IDE's output. */
    static List<Path> classFolders() {
        List<Path> folders = new ArrayList<>();
        for (String entry : System.getProperty("java.class.path", "").split(java.io.File.pathSeparator)) {
            if (!entry.isEmpty() && Files.isDirectory(Path.of(entry))) {
                folders.add(Path.of(entry).toAbsolutePath().normalize());
            }
        }
        return folders;
    }

    /**
     * Robots started by an IDE (design §5.2): looks them up in classic's repository first,
     * then connects once for each, tells the server who it is in its {@code Hello} and
     * waits for the battle, as long as it takes. Ends when the robots taken for battles are
     * done, or when none can wait anymore.
     */
    static int runIde(String server, String token, List<String> names, List<Path> jars, Path home,
            Path data, long seed) throws Exception {
        if (token.isEmpty()) {
            token = ideToken(data);
        }
        if (home == null) {
            home = data.resolve("ide-home");
        }
        if (jars.isEmpty()) {
            jars = classFolders();
            if (jars.isEmpty()) {
                System.err.println("No class folders on the classpath, so no robots to offer: run "
                        + "this from your project, with its output folder on the classpath.");
                return 1;
            }
        }
        for (Path jar : jars) {
            // Classic's jar handling reaches into the JDK; a class folder doesn't need that.
            if (!Files.isDirectory(jar) && !Object.class.getModule().isOpen("sun.net.www.protocol.jar",
                    RobotHost.class.getModule())) {
                System.err.println(jar + " is a jar: robots in jars need the VM options "
                        + "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED "
                        + "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED (or give the class folder)");
                return 1;
            }
        }
        System.setProperty("NOSECURITY", "true");
        copyJars(jars, home);
        RobocodeEngine engine = new RobocodeEngine(home.toFile());
        List<RobotSpecification> specs = new ArrayList<>();
        if (names.isEmpty()) {
            // Teams and their robots need several JVMs: the server doesn't take them.
            for (RobotSpecification spec : engine.getLocalRepository()) {
                IRobotSpecItem item = (IRobotSpecItem) HiddenAccess.getFileSpecification(spec);
                if (!item.isTeam() && !((IRobotItem) item).isTeamRobot()) {
                    specs.add(spec);
                }
            }
            if (specs.isEmpty()) {
                System.err.println("No robots found in " + jars + ": build your project first.");
                return 1;
            }
        } else {
            for (String name : names) {
                RobotSpecification[] found = engine.getLocalRepository(name);
                if (found.length == 0) {
                    System.err.println("robot " + name + " not found in " + jars);
                    return 1;
                }
                specs.add(found[0]);
            }
        }
        if (specs.size() > MAX_IDE_ROBOTS) {
            System.err.println("Found " + specs.size() + " robots; offering the first " + MAX_IDE_ROBOTS
                    + " (--robot name chooses).");
            specs = specs.subList(0, MAX_IDE_ROBOTS);
        }

        int colon = server.lastIndexOf(':');
        String host = server.substring(0, colon);
        int port = Integer.parseInt(server.substring(colon + 1));
        IdeRobots state = new IdeRobots();
        List<Thread> threads = new ArrayList<>();
        for (RobotSpecification spec : specs) {
            IRobotSpecItem item = (IRobotSpecItem) HiddenAccess.getFileSpecification(spec);
            RobotDescription description = describe(item);
            Socket socket;
            try {
                socket = new Socket(host, port);
            } catch (java.net.ConnectException e) {
                System.err.println("robocode-s isn't listening on " + server + ": start it (the program, or "
                        + "`robocode-s battle sample.Walls,@ide`) and then this again.");
                return 1;
            }
            socket.setTcpNoDelay(true);
            System.err.println("Waiting for a battle with " + description.getName() + " (robocode-s on "
                    + server + ", From your IDE).");
            String key = token;
            Path robotHome = home;
            Thread thread = new Thread(() -> {
                boolean[] taken = {false};
                int code = 1;
                try (socket) {
                    code = play(new BufferedInputStream(socket.getInputStream()),
                            new BufferedOutputStream(socket.getOutputStream()), key,
                            item.getUniqueFullClassNameWithVersion(), List.of(), robotHome, seed, engine,
                            new RobotSpecification[] {spec}, description, () -> {
                                taken[0] = true;
                                state.taken();
                            });
                } catch (Exception e) {
                    if (taken[0]) {
                        e.printStackTrace();
                    }
                } finally {
                    state.ended(taken[0], code);
                }
            }, "robot-host " + description.getName());
            threads.add(thread);
        }
        threads.forEach(Thread::start);
        return state.await(threads.size());
    }

    /** What a robot from an IDE says about itself in its {@code Hello}. */
    private static RobotDescription describe(IRobotSpecItem item) {
        RobotDescription.Builder description = RobotDescription.newBuilder()
                .setName(item.getUniqueFullClassNameWithVersion())
                .setFullClassName(item.getFullClassName())
                .setPlatform("JVM " + field(System.getProperty("java.version")))
                .setDeterministicRules(RobotRandomness.rulesVersion());
        if (item.isTeam()) {
            // The server says why it doesn't take a team.
            description.setTeamId(item.getUniqueFullClassNameWithVersion()).getTypeBuilder().setTeam(true);
        } else {
            IRobotItem robotItem = (IRobotItem) item;
            description.getTypeBuilder()
                    .setJunior(robotItem.isJuniorRobot())
                    .setAdvanced(robotItem.isAdvancedRobot())
                    .setTeam(robotItem.isTeamRobot())
                    .setDroid(robotItem.isDroid())
                    .setSentry(robotItem.isSentryRobot())
                    .setInteractive(robotItem.isInteractiveRobot())
                    .setPaint(robotItem.isPaintRobot());
        }
        return description.build();
    }

    /**
     * The robots of an IDE's host: the host ends when every robot taken for a battle is
     * done, or when all connections ended without a battle (the server went away).
     */
    private static final class IdeRobots {
        private int taken;
        private int done;
        private int ended;
        private int code;

        synchronized void taken() {
            taken++;
        }

        synchronized void ended(boolean wasTaken, int exit) {
            ended++;
            if (wasTaken) {
                done++;
                code = Math.max(code, exit);
            }
            notifyAll();
        }

        synchronized int await(int robots) throws InterruptedException {
            while (!(taken > 0 && done == taken) && ended < robots) {
                wait();
            }
            return taken > 0 ? code : 1;
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
        return play(in, out, token, robot, jars, home, seed, null, null, null, null);
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
        return play(in, out, token, assignment.getRobot(), jars, home, assignment.getSeed(), engine,
                null, null, null);
    }

    /** Loads the protocol's classes, which the robot's connection needs first. */
    private static void warmUp() throws IOException {
        var buffer = new ByteArrayOutputStream();
        ClientMessage.newBuilder().setHello(Hello.newBuilder()
                .setVersion(ProtocolVersion.newBuilder().setMajor(1).setMinor(2))
                .setRole(Role.ROLE_ROBOT)).build().writeDelimitedTo(buffer);
        ServerMessage.parseDelimitedFrom(new ByteArrayInputStream(buffer.toByteArray()));
    }

    /**
     * The robot's side of the protocol, with classic's {@code engine} if it is started
     * already. A robot from an IDE has looked itself up ({@code found}) and describes
     * itself in its {@code Hello}; {@code taken} runs once it is in a battle.
     */
    private static int play(InputStream in, OutputStream out, String token, String robot,
            List<Path> jars, Path home, long seed, RobocodeEngine engine,
            RobotSpecification[] found, RobotDescription description, Runnable taken) throws Exception {
        Hello.Builder hello = Hello.newBuilder()
                .setVersion(ProtocolVersion.newBuilder().setMajor(1).setMinor(2))
                .setRole(Role.ROLE_ROBOT)
                .setToken(token);
        if (description != null) {
            hello.setRobotDescription(description);
        }
        ClientMessage.newBuilder().setHello(hello).build().writeDelimitedTo(out);
        out.flush();
        ServerMessage first = ServerMessage.parseDelimitedFrom(in);
        if (first == null && description != null) {
            System.err.println(robot + ": robocode-s ended the connection without a battle");
            return 1;
        }
        if (first == null || !first.hasHelloAck()) {
            System.err.println("server refused the robot: " + (first == null ? "connection closed" : first));
            return 1;
        }
        if (taken != null) {
            taken.run();
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
        if (found == null) {
            found = engine.getLocalRepository(robot);
        }
        if (found.length == 0) {
            System.err.println("robot " + robot + " not found in " + jars);
            return 1;
        }
        RemotePeer peer = new RemotePeer(out);
        IHostingRobotProxy proxy;
        // A host from an IDE may start several of its robots at once.
        synchronized (RobotHost.class) {
            proxy = createProxy(found[0], statics, peer);
        }
        return serve(in, proxy, peer, description != null);
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
     * listing in the same home are not found again; but for {@code robots/.data}, the
     * robot's data files, which the runner puts there before the robot starts.
     *
     * @return the jars by the file name of their copy
     */
    private static Map<String, Path> copyJars(List<Path> jars, Path home) throws IOException {
        Path robots = home.resolve("robots");
        if (Files.isDirectory(robots)) {
            Path data = robots.resolve(".data");
            try (var paths = Files.walk(robots)) {
                for (Path p : paths.sorted(Comparator.reverseOrder()).toList()) {
                    if (!p.equals(robots) && !p.startsWith(data)) {
                        Files.delete(p);
                    }
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

    /**
     * Classic's wait for the robot's thread to end, which interrupts it and, after a few
     * seconds, stops it by force. A robot from an IDE may be held at a breakpoint (in an
     * event handler, say) while the round is over: it is waited for as long as it takes.
     */
    private static void waitForStop(IHostingRobotProxy proxy, boolean ide) {
        if (ide) {
            try {
                Field manager = proxy.getClass().getDeclaredField("robotThreadManager");
                manager.setAccessible(true);
                Object threads = manager.get(proxy);
                Field run = threads.getClass().getDeclaredField("runThread");
                run.setAccessible(true);
                Thread thread = (Thread) run.get(threads);
                while (thread != null && thread.isAlive()) {
                    thread.join(100);
                }
            } catch (ReflectiveOperationException | RuntimeException | InterruptedException e) {
                // Classic's own wait then.
            }
        }
        proxy.waitForStopThread();
    }

    private static int serve(InputStream in, IHostingRobotProxy proxy, RemotePeer peer, boolean ide)
            throws Exception {
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
                    waitForStop(proxy, ide);
                }
                case BATTLE_OVER -> {
                    if (ide) {
                        waitForStop(proxy, true);
                    }
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
