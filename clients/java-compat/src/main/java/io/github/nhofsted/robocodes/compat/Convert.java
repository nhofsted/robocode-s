/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.protocol.v1.Colors;
import io.github.nhofsted.robocodes.protocol.v1.RobotCommands;
import io.github.nhofsted.robocodes.protocol.v1.Tick;
import net.sf.robocode.peer.BulletCommand;
import net.sf.robocode.peer.BulletStatus;
import net.sf.robocode.peer.ExecCommands;
import net.sf.robocode.peer.TeamMessage;
import net.sf.robocode.security.HiddenAccess;
import net.sf.robocode.security.SafeComponent;
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
import robocode.KeyPressedEvent;
import robocode.KeyReleasedEvent;
import robocode.KeyTypedEvent;
import robocode.MouseClickedEvent;
import robocode.MouseDraggedEvent;
import robocode.MouseEnteredEvent;
import robocode.MouseExitedEvent;
import robocode.MouseMovedEvent;
import robocode.MousePressedEvent;
import robocode.MouseReleasedEvent;
import robocode.MouseWheelMovedEvent;
import robocode.RobotDeathEvent;
import robocode.RobotStatus;
import robocode.RoundEndedEvent;
import robocode.ScannedRobotEvent;
import robocode.SkippedTurnEvent;
import robocode.WinEvent;

import java.awt.event.KeyEvent;
import java.awt.event.MouseEvent;
import java.awt.event.MouseWheelEvent;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Conversions between protocol messages and classic Robocode's robot-side objects. */
final class Convert {

    private Convert() {
    }

    /** The robot's command state for {@code Execute}. */
    static RobotCommands commands(ExecCommands c) {
        RobotCommands.Builder b = RobotCommands.newBuilder()
                .setBodyTurnRemaining(c.getBodyTurnRemaining())
                .setGunTurnRemaining(c.getGunTurnRemaining())
                .setRadarTurnRemaining(c.getRadarTurnRemaining())
                .setDistanceRemaining(c.getDistanceRemaining())
                .setAdjustGunForBodyTurn(c.isAdjustGunForBodyTurn())
                .setAdjustRadarForGunTurn(c.isAdjustRadarForGunTurn())
                .setAdjustRadarForBodyTurn(c.isAdjustRadarForBodyTurn())
                .setAdjustRadarForBodyTurnSet(c.isAdjustRadarForBodyTurnSet())
                .setMaxTurnRate(c.getMaxTurnRate())
                .setMaxVelocity(c.getMaxVelocity())
                .setColors(Colors.newBuilder()
                        .setBody(c.getBodyColor())
                        .setGun(c.getGunColor())
                        .setRadar(c.getRadarColor())
                        .setScan(c.getScanColor())
                        .setBullet(c.getBulletColor()))
                .setScan(c.isScan())
                .setIoRobot(c.isIORobot());
        for (BulletCommand bullet : c.getBullets()) {
            b.addBullets(io.github.nhofsted.robocodes.protocol.v1.BulletCommand.newBuilder()
                    .setBulletId(bullet.getBulletId())
                    .setPower(bullet.getPower())
                    .setFireAssistValid(bullet.isFireAssistValid())
                    .setFireAssistAngle(bullet.getFireAssistAngle()));
        }
        for (TeamMessage m : c.getTeamMessages()) {
            io.github.nhofsted.robocodes.protocol.v1.TeamMessage.Builder message =
                    io.github.nhofsted.robocodes.protocol.v1.TeamMessage.newBuilder()
                            .setSender(m.sender)
                            .setPayload(com.google.protobuf.ByteString.copyFrom(m.message));
            if (m.recipient != null) {
                message.setRecipient(m.recipient);
            }
            b.addTeamMessages(message);
        }
        return b.build();
    }

    /** The server's copy of the command state (no one-shot parts), as classic returns it. */
    static ExecCommands commands(RobotCommands c) {
        ExecCommands e = new ExecCommands();
        e.setBodyTurnRemaining(c.getBodyTurnRemaining());
        e.setGunTurnRemaining(c.getGunTurnRemaining());
        e.setRadarTurnRemaining(c.getRadarTurnRemaining());
        e.setDistanceRemaining(c.getDistanceRemaining());
        e.setAdjustGunForBodyTurn(c.getAdjustGunForBodyTurn());
        e.setAdjustRadarForGunTurn(c.getAdjustRadarForGunTurn());
        e.setAdjustRadarForBodyTurn(c.getAdjustRadarForBodyTurn());
        e.setAdjustRadarForBodyTurnSet(c.getAdjustRadarForBodyTurnSet());
        if (c.hasMaxTurnRate()) {
            e.setMaxTurnRate(c.getMaxTurnRate());
        }
        if (c.hasMaxVelocity()) {
            e.setMaxVelocity(c.getMaxVelocity());
        }
        if (c.hasColors()) {
            Colors colors = c.getColors();
            e.setBodyColor(colors.getBody());
            e.setGunColor(colors.getGun());
            e.setRadarColor(colors.getRadar());
            e.setScanColor(colors.getScan());
            e.setBulletColor(colors.getBullet());
        }
        return e;
    }

    static RobotStatus status(io.github.nhofsted.robocodes.protocol.v1.RobotStatus s) {
        return HiddenAccess.createStatus(s.getEnergy(), s.getX(), s.getY(), s.getBodyHeading(), s.getGunHeading(),
                s.getRadarHeading(), s.getVelocity(), s.getBodyTurnRemaining(), s.getRadarTurnRemaining(),
                s.getGunTurnRemaining(), s.getDistanceRemaining(), s.getGunHeat(), s.getOthers(), s.getNumSentries(),
                s.getRound(), s.getNumRounds(), s.getTime());
    }

    static List<BulletStatus> bulletUpdates(Tick tick) {
        List<BulletStatus> updates = new ArrayList<>();
        for (io.github.nhofsted.robocodes.protocol.v1.BulletStatus b : tick.getBulletUpdatesList()) {
            updates.add(new BulletStatus(b.getBulletId(), b.getX(), b.getY(), b.hasVictim() ? name(b.getVictim()) : null,
                    b.getActive()));
        }
        return updates;
    }

    /**
     * One {@code String} instance per robot name. In classic, every event carries the
     * very same name instance (the robot's own name too), and robots compare names
     * with {@code ==}. Not {@link String#intern()}: classic's names aren't interned,
     * so {@code == "literal"} must stay false.
     */
    private static final Map<String, String> NAMES = new HashMap<>();

    /** The canonical instance of a name; the first one registered wins. */
    static synchronized String name(String name) {
        String canonical = NAMES.putIfAbsent(name, name);
        return canonical == null ? name : canonical;
    }

    static List<TeamMessage> teamMessages(Tick tick) {
        List<TeamMessage> messages = new ArrayList<>();
        for (io.github.nhofsted.robocodes.protocol.v1.TeamMessage m : tick.getTeamMessagesList()) {
            messages.add(new TeamMessage(name(m.getSender()), m.hasRecipient() ? name(m.getRecipient()) : null,
                    m.getPayload().toByteArray()));
        }
        return messages;
    }

    static List<Event> events(Tick tick) {
        List<Event> events = new ArrayList<>();
        for (io.github.nhofsted.robocodes.protocol.v1.Event e : tick.getEventsList()) {
            Event event = event(e);
            if (event != null) {
                events.add(event);
            }
        }
        return events;
    }

    private static Bullet bullet(io.github.nhofsted.robocodes.protocol.v1.Bullet b) {
        return new Bullet(b.getHeading(), b.getX(), b.getY(), b.getPower(), name(b.getOwner()),
                b.hasVictim() ? name(b.getVictim()) : null, b.getActive(), b.getBulletId());
    }

    /**
     * An observer's mouse or keyboard event as classic's interactive event, wrapping a
     * java.awt event as classic's own deserializers do (on {@link SafeComponent}, in
     * battlefield coordinates). Null for one that java.awt refuses (an invalid key or
     * button) or of an unknown kind.
     */
    static Event input(io.github.nhofsted.robocodes.protocol.v1.InputEvent e) {
        try {
            if (e.hasMouse()) {
                var m = e.getMouse();
                int id = switch (m.getAction()) {
                    case MOUSE_CLICKED -> MouseEvent.MOUSE_CLICKED;
                    case MOUSE_PRESSED -> MouseEvent.MOUSE_PRESSED;
                    case MOUSE_RELEASED -> MouseEvent.MOUSE_RELEASED;
                    case MOUSE_MOVED -> MouseEvent.MOUSE_MOVED;
                    case MOUSE_ENTERED -> MouseEvent.MOUSE_ENTERED;
                    case MOUSE_EXITED -> MouseEvent.MOUSE_EXITED;
                    case MOUSE_DRAGGED -> MouseEvent.MOUSE_DRAGGED;
                    case MOUSE_WHEEL_MOVED -> MouseEvent.MOUSE_WHEEL;
                    default -> 0;
                };
                if (id == MouseEvent.MOUSE_WHEEL) {
                    return new MouseWheelMovedEvent(new MouseWheelEvent(SafeComponent.getSafeEventComponent(), id,
                            m.getWhen(), m.getModifiers(), m.getX(), m.getY(), m.getClickCount(), false,
                            m.getScrollType(), m.getScrollAmount(), m.getWheelRotation()));
                }
                if (id == 0) {
                    return null;
                }
                MouseEvent source = new MouseEvent(SafeComponent.getSafeEventComponent(), id, m.getWhen(),
                        m.getModifiers(), m.getX(), m.getY(), m.getClickCount(), false, m.getButton());
                return switch (id) {
                    case MouseEvent.MOUSE_CLICKED -> new MouseClickedEvent(source);
                    case MouseEvent.MOUSE_PRESSED -> new MousePressedEvent(source);
                    case MouseEvent.MOUSE_RELEASED -> new MouseReleasedEvent(source);
                    case MouseEvent.MOUSE_ENTERED -> new MouseEnteredEvent(source);
                    case MouseEvent.MOUSE_EXITED -> new MouseExitedEvent(source);
                    case MouseEvent.MOUSE_DRAGGED -> new MouseDraggedEvent(source);
                    default -> new MouseMovedEvent(source);
                };
            }
            if (e.hasKey()) {
                var k = e.getKey();
                int id = switch (k.getAction()) {
                    case KEY_PRESSED -> KeyEvent.KEY_PRESSED;
                    case KEY_RELEASED -> KeyEvent.KEY_RELEASED;
                    case KEY_TYPED -> KeyEvent.KEY_TYPED;
                    default -> 0;
                };
                if (id == 0) {
                    return null;
                }
                KeyEvent source = new KeyEvent(SafeComponent.getSafeEventComponent(), id, k.getWhen(), k.getModifiers(),
                        k.getKeyCode(), (char) k.getKeyChar(), k.getKeyLocation());
                return switch (id) {
                    case KeyEvent.KEY_PRESSED -> new KeyPressedEvent(source);
                    case KeyEvent.KEY_RELEASED -> new KeyReleasedEvent(source);
                    default -> new KeyTypedEvent(source);
                };
            }
        } catch (IllegalArgumentException ignored) {
            // Not a valid java.awt event: the robot doesn't get it.
        }
        return null;
    }

    static Event event(io.github.nhofsted.robocodes.protocol.v1.Event e) {
        return switch (e.getKindCase()) {
            case SCANNED_ROBOT -> {
                var s = e.getScannedRobot();
                yield new ScannedRobotEvent(name(s.getName()), s.getEnergy(), s.getBearing(), s.getDistance(),
                        s.getHeading(), s.getVelocity(), s.getSentry());
            }
            case HIT_WALL -> new HitWallEvent(e.getHitWall().getBearing());
            case HIT_ROBOT -> {
                var h = e.getHitRobot();
                yield new HitRobotEvent(name(h.getName()), h.getBearing(), h.getEnergy(), h.getAtFault());
            }
            case HIT_BY_BULLET -> new HitByBulletEvent(e.getHitByBullet().getBearing(),
                    bullet(e.getHitByBullet().getBullet()));
            case BULLET_HIT -> {
                var h = e.getBulletHit();
                yield new BulletHitEvent(name(h.getName()), h.getEnergy(), bullet(h.getBullet()));
            }
            case BULLET_HIT_BULLET -> new BulletHitBulletEvent(bullet(e.getBulletHitBullet().getBullet()),
                    bullet(e.getBulletHitBullet().getHitBullet()));
            case BULLET_MISSED -> new BulletMissedEvent(bullet(e.getBulletMissed().getBullet()));
            case ROBOT_DEATH -> new RobotDeathEvent(name(e.getRobotDeath().getName()));
            case DEATH -> new DeathEvent();
            case WIN -> new WinEvent();
            case ROUND_ENDED -> {
                var r = e.getRoundEnded();
                yield new RoundEndedEvent(r.getRound(), (int) r.getTurns(), (int) r.getTotalTurns());
            }
            case BATTLE_ENDED -> {
                var r = e.getBattleEnded().getResults();
                yield new BattleEndedEvent(e.getBattleEnded().getAborted(), new BattleResults(name(r.getName()), r.getRank(),
                        r.getScore(), r.getSurvival(), r.getLastSurvivorBonus(), r.getBulletDamage(),
                        r.getBulletDamageBonus(), r.getRamDamage(), r.getRamDamageBonus(), r.getFirsts(),
                        r.getSeconds(), r.getThirds()));
            }
            case SKIPPED_TURN -> new SkippedTurnEvent(e.getSkippedTurn().getTurn());
            case INPUT -> input(e.getInput());
            case KIND_NOT_SET -> throw new IllegalArgumentException("event without kind");
        };
    }
}
