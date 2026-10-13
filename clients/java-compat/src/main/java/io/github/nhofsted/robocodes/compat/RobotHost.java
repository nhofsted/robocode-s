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
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The robot host: one JVM per robot (design §6.4). It connects to the robocode-s server,
 * then runs the robot with classic Robocode's own robot side and a {@link RemotePeer}.
 *
 * <p>Usage: {@code robot-host --server host:port --token T --robot sample.Fire
 * --jar robot.jar [--jar ...] --home dir [--seed N]}. With the determinism agent loaded,
 * the robot's random streams are derived from the seed and its unique name (ADR 0009).
 *
 * <p>{@code robot-host --describe --home dir --jar robot.jar --robot a [--robot b ...]}
 * instead prints what classic's repository knows about each robot, teams expanded into
 * their members, in battle order. One tab-separated line per robot: {@code robot}, the
 * unique class name with version, the full class name, the team id (empty without a
 * team) and the type flags junior, advanced, team, droid, sentry, interactive and paint
 * as {@code 0}/{@code 1}. The battle's unique names are derived from these as in
 * classic's {@code Battle.createPeers}.
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
        String token = "";
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
            if (robots.isEmpty() || home == null) {
                throw new IllegalArgumentException("usage: robot-host --describe --home dir "
                        + "--jar robot.jar --robot name [--robot name ...]");
            }
            System.exit(describe(robots, jars, home));
        }
        String robot = robots.isEmpty() ? null : robots.get(0);
        if (server == null || robot == null || home == null) {
            throw new IllegalArgumentException("usage: robot-host --server host:port --token T --robot name "
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

    static int describe(List<String> robots, List<Path> jars, Path home) throws Exception {
        System.setProperty("NOSECURITY", "true");
        copyJars(jars, home);
        RobocodeEngine engine = new RobocodeEngine(home.toFile());
        // Classic skips robots it can't find, so each one is looked up on its own first.
        for (String robot : robots) {
            if (engine.getLocalRepository(robot).length == 0) {
                System.err.println("robot " + robot + " not found in " + jars);
                return 1;
            }
        }
        // Then all in one selection, as classic does: a team's id numbers it by its
        // position in the selection, which tells two copies of the same team apart.
        StringBuilder out = new StringBuilder();
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

    private static void copyJars(List<Path> jars, Path home) throws IOException {
        Path robots = Files.createDirectories(home.resolve("robots"));
        for (Path jar : jars) {
            Files.copy(jar, robots.resolve(jar.getFileName()), StandardCopyOption.REPLACE_EXISTING);
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
