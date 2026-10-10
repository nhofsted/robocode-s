/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import robocode.BattleEndedEvent;
import robocode.BattleResults;
import robocode.Bullet;
import robocode.BulletHitBulletEvent;
import robocode.BulletHitEvent;
import robocode.BulletMissedEvent;
import robocode.DeathEvent;
import robocode.Event;
import robocode.HitByBulletEvent;
import robocode.HitRobotEvent;
import robocode.HitWallEvent;
import robocode.RobotDeathEvent;
import robocode.RoundEndedEvent;
import robocode.ScannedRobotEvent;
import robocode.SkippedTurnEvent;
import robocode.WinEvent;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Collects what classic's engine does per turn and robot, for the replay test of the
 * robocode-s engine: the command state it loads, the events it adds and the remaining
 * values it reports back. {@link ClassicPatches} calls these hooks from {@code RobotPeer}.
 *
 * <p>{@code RobotPeer} and {@code ExecCommands} belong to classic's engine class loader,
 * so they are accessed through reflection. Events and bullets are {@code robocode.*}
 * API classes from the application class loader and are used directly.
 */
public final class OracleHooks {

    /** Classic's {@code EventManager.MAX_QUEUE_SIZE}. */
    private static final int MAX_QUEUE_SIZE = 256;

    private static volatile boolean enabled;
    private static Turn current;

    private OracleHooks() {
    }

    /** What was collected for one turn; arrays are indexed by robot index. */
    static final class Turn {
        final String[] commands;
        final String[] remaining;
        final List<List<String>> events = new ArrayList<>();

        Turn(int robots) {
            commands = new String[robots];
            remaining = new String[robots];
            for (int i = 0; i < robots; i++) {
                events.add(new ArrayList<>());
            }
        }
    }

    /** Starts collecting for a battle with {@code robots} robots. */
    static synchronized void enable(int robots) {
        current = new Turn(robots);
        enabled = true;
    }

    /** Returns what was collected since the last call and starts a new turn. */
    static synchronized Turn drain() {
        Turn turn = current;
        current = new Turn(turn.commands.length);
        return turn;
    }

    // ------------------------------------------------------------ hooks (called by RobotPeer)

    /** Entry of {@code RobotPeer.performLoadCommands()}. */
    public static void commandsLoaded(Object peer) {
        if (!enabled) {
            return;
        }
        try {
            Object commands = commands(peer);
            Oracle.Json json = new Oracle.Json().begin()
                    .field("bodyTurn", (double) call(commands, "getBodyTurnRemaining"))
                    .field("gunTurn", (double) call(commands, "getGunTurnRemaining"))
                    .field("radarTurn", (double) call(commands, "getRadarTurnRemaining"))
                    .field("distance", (double) call(commands, "getDistanceRemaining"))
                    .field("adjustGunForBody", (boolean) call(commands, "isAdjustGunForBodyTurn"))
                    .field("adjustRadarForGun", (boolean) call(commands, "isAdjustRadarForGunTurn"))
                    .field("adjustRadarForBody", (boolean) call(commands, "isAdjustRadarForBodyTurn"))
                    .field("maxTurnRate", (double) call(commands, "getMaxTurnRate"))
                    .field("maxVelocity", (double) call(commands, "getMaxVelocity"))
                    .field("scan", (boolean) call(commands, "isScan"))
                    .field("io", (boolean) call(commands, "isIORobot"))
                    .name("bullets").beginArray();
            for (Object b : (List<?>) call(commands, "getBullets")) {
                json.begin()
                        .field("id", (int) call(b, "getBulletId"))
                        .field("power", (double) call(b, "getPower"))
                        .field("assistValid", (boolean) call(b, "isFireAssistValid"))
                        .field("assistAngle", (double) call(b, "getFireAssistAngle"))
                        .field("consumed", (boolean) call(b, "isConsumed"))
                        .end();
            }
            json.endArray().name("messages").beginArray();
            for (Object m : (List<?>) call(commands, "getTeamMessages")) {
                json.begin()
                        .field("sender", String.valueOf(field(m, "sender")))
                        .field("recipient", String.valueOf(field(m, "recipient")))
                        .field("bytes", hex((byte[]) field(m, "message")))
                        .end();
            }
            json.endArray().end();
            synchronized (OracleHooks.class) {
                current.commands[index(peer)] = json.toString();
            }
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("cannot read classic's commands", e);
        }
    }

    /** Entry of {@code RobotPeer.publishStatus(long)}. */
    public static void statusPublished(Object peer) {
        if (!enabled) {
            return;
        }
        try {
            Object commands = commands(peer);
            String json = new Oracle.Json().begin()
                    .field("bodyTurn", (double) call(commands, "getBodyTurnRemaining"))
                    .field("gunTurn", (double) call(commands, "getGunTurnRemaining"))
                    .field("radarTurn", (double) call(commands, "getRadarTurnRemaining"))
                    .field("distance", (double) call(commands, "getDistanceRemaining"))
                    .end().toString();
            synchronized (OracleHooks.class) {
                current.remaining[index(peer)] = json;
            }
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("cannot read classic's status", e);
        }
    }

    /**
     * Entry of {@code RobotPeer.addEvent(Event)}. Records the event if classic will queue
     * it: the robot must be running, and a full queue only takes critical events.
     */
    public static void eventAdded(Object peer, Object event) {
        if (!enabled) {
            return;
        }
        try {
            if (!(boolean) call(peer, "isRunning")) {
                return;
            }
            Object queue = ((AtomicReference<?>) field(peer, "events")).get();
            int size = ((List<?>) queue).size();
            if (size > MAX_QUEUE_SIZE
                    && !(event instanceof DeathEvent || event instanceof WinEvent || event instanceof SkippedTurnEvent)) {
                return;
            }
            String json = describe((Event) event);
            synchronized (OracleHooks.class) {
                current.events.get(index(peer)).add(json);
            }
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("cannot read classic's event queue", e);
        }
    }

    // ------------------------------------------------------------ events

    static String describe(Event event) {
        Oracle.Json json = new Oracle.Json().begin().field("type", event.getClass().getSimpleName());
        if (event instanceof ScannedRobotEvent e) {
            json.field("name", e.getName()).field("energy", e.getEnergy()).field("bearing", e.getBearingRadians())
                    .field("distance", e.getDistance()).field("heading", e.getHeadingRadians())
                    .field("velocity", e.getVelocity()).field("sentry", e.isSentryRobot());
        } else if (event instanceof HitWallEvent e) {
            json.field("bearing", e.getBearingRadians());
        } else if (event instanceof HitRobotEvent e) {
            json.field("name", e.getName()).field("bearing", e.getBearingRadians()).field("energy", e.getEnergy())
                    .field("atFault", e.isMyFault());
        } else if (event instanceof HitByBulletEvent e) {
            json.field("bearing", e.getBearingRadians());
            bullet(json, "bullet", e.getBullet());
        } else if (event instanceof BulletHitEvent e) {
            json.field("name", e.getName()).field("energy", e.getEnergy());
            bullet(json, "bullet", e.getBullet());
        } else if (event instanceof BulletHitBulletEvent e) {
            bullet(json, "bullet", e.getBullet());
            bullet(json, "hitBullet", e.getHitBullet());
        } else if (event instanceof BulletMissedEvent e) {
            bullet(json, "bullet", e.getBullet());
        } else if (event instanceof RobotDeathEvent e) {
            json.field("name", e.getName());
        } else if (event instanceof RoundEndedEvent e) {
            json.field("round", e.getRound()).field("turns", e.getTurns()).field("totalTurns", e.getTotalTurns());
        } else if (event instanceof SkippedTurnEvent e) {
            json.field("skippedTurn", e.getSkippedTurn());
        } else if (event instanceof BattleEndedEvent e) {
            BattleResults r = e.getResults();
            json.field("aborted", e.isAborted()).field("rank", r.getRank()).field("score", r.getScore())
                    .field("survival", r.getSurvival()).field("firsts", r.getFirsts());
        }
        return json.end().toString();
    }

    private static void bullet(Oracle.Json json, String name, Bullet b) {
        if (b == null) {
            json.name(name).nullValue();
            return;
        }
        int id;
        try {
            id = (int) field(b, "bulletId");
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("cannot read the bullet id", e);
        }
        json.name(name).begin()
                .field("id", id)
                .field("owner", String.valueOf(b.getName()))
                .field("victim", String.valueOf(b.getVictim()))
                .field("heading", b.getHeadingRadians())
                .field("x", b.getX())
                .field("y", b.getY())
                .field("power", b.getPower())
                .field("active", b.isActive())
                .end();
    }

    // ------------------------------------------------------------ reflection

    private static Object commands(Object peer) throws ReflectiveOperationException {
        return ((AtomicReference<?>) field(peer, "commands")).get();
    }

    private static int index(Object peer) throws ReflectiveOperationException {
        return (int) call(peer, "getRobotIndex");
    }

    /** Methods and fields by name, looked up once per class. */
    private static final ClassValue<Map<String, Object>> MEMBERS = new ClassValue<>() {
        @Override
        protected Map<String, Object> computeValue(Class<?> type) {
            return new ConcurrentHashMap<>();
        }
    };

    private static Object call(Object target, String method) throws ReflectiveOperationException {
        Map<String, Object> members = MEMBERS.get(target.getClass());
        Method m = (Method) members.get("m:" + method);
        if (m == null) {
            m = target.getClass().getMethod(method);
            m.setAccessible(true);
            members.put("m:" + method, m);
        }
        return m.invoke(target);
    }

    private static Object field(Object target, String name) throws ReflectiveOperationException {
        Map<String, Object> members = MEMBERS.get(target.getClass());
        Field f = (Field) members.get("f:" + name);
        if (f == null) {
            f = findField(target.getClass(), name);
            f.setAccessible(true);
            members.put("f:" + name, f);
        }
        return f.get(target);
    }

    private static Field findField(Class<?> type, String name) throws NoSuchFieldException {
        for (Class<?> c = type; c != null; c = c.getSuperclass()) {
            try {
                return c.getDeclaredField(name);
            } catch (NoSuchFieldException ignored) {
                // look in the superclass
            }
        }
        throw new NoSuchFieldException(name);
    }

    private static String hex(byte[] bytes) {
        StringBuilder sb = new StringBuilder();
        for (byte b : bytes) {
            sb.append(String.format("%02x", b));
        }
        return sb.toString();
    }
}
