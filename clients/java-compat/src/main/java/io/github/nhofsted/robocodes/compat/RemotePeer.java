/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.determinism.RobotRandomness;
import io.github.nhofsted.robocodes.protocol.v1.ClientMessage;
import io.github.nhofsted.robocodes.protocol.v1.DebugProperty;
import io.github.nhofsted.robocodes.protocol.v1.Execute;
import io.github.nhofsted.robocodes.protocol.v1.RobotCommands;
import io.github.nhofsted.robocodes.protocol.v1.Tick;
import net.sf.robocode.peer.BadBehavior;
import net.sf.robocode.peer.ExecCommands;
import net.sf.robocode.peer.ExecResults;
import net.sf.robocode.peer.IRobotPeer;
import robocode.Event;
import robocode.RobotStatus;
import robocode.exception.AbortedException;
import robocode.exception.DeathException;
import robocode.exception.WinException;

import java.io.IOException;
import java.io.OutputStream;
import java.lang.management.ManagementFactory;
import java.lang.management.ThreadMXBean;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;

/**
 * Classic's robot side talks to the engine through {@link IRobotPeer}. This peer forwards
 * each {@code execute} to the robocode-s server and returns the next {@code Tick} as
 * classic's {@link ExecResults}, following classic's {@code RobotPeer.executeImpl}:
 * the commands count first; a robot that died or was halted then gets
 * {@link DeathException}, {@link WinException} or {@link AbortedException}, and its final
 * commands are sent from {@link #waitForBattleEndImpl}.
 */
final class RemotePeer implements IRobotPeer {

    /** Placed in the inbox when the round ends while the robot waits. */
    private static final Object ROUND_OVER = new Object();

    private final OutputStream out;
    /** Which colours the robot set, over the whole battle. */
    final ColorTracker colors = new ColorTracker();
    private final BlockingQueue<Object> inbox = new LinkedBlockingQueue<>();
    private final ThreadMXBean threads = ManagementFactory.getThreadMXBean();
    /**
     * Deterministic mode: a robot's CPU time is its counted work as virtual time (ADR 0014),
     * the same on every machine, rather than this JVM's. The server skips turns by it (ADR 0018).
     */
    private final boolean countedWork = RobotRandomness.isMeteringWork();

    private volatile boolean running;
    // Robot thread state, reset each round.
    private long lastTurn;
    private boolean stopping;
    private boolean dead;
    private boolean winner;
    private boolean finished;
    private boolean drained;
    private long cpuAtReturn = -1;
    /**
     * The final {@code Execute} of a robot that died or was halted, sent from
     * {@link #waitForBattleEndImpl} (see there), and the commands it was made from.
     */
    private Execute.Builder finalExecute;
    private ExecCommands finalCommands;

    RemotePeer(OutputStream out) {
        this.out = out;
    }

    /** Main thread: a new round starts. */
    void newRound() {
        inbox.clear();
        lastTurn = 0;
        // A round's start is free: no skipped turns for the work before the first execute,
        // as in the classic oracle (RobotRandomness.peerRound).
        cpuAtReturn = -1;
        stopping = false;
        dead = false;
        winner = false;
        finished = false;
        drained = false;
        cpuAtReturn = -1;
        finalExecute = null;
        finalCommands = null;
    }

    /** Main thread: results of a turn arrived. */
    void deliver(Tick tick) {
        inbox.add(tick);
    }

    /** Main thread: the round is over; a robot still waiting stops. */
    void roundOver() {
        inbox.add(ROUND_OVER);
    }

    @Override
    public ExecResults executeImpl(ExecCommands commands) {
        Execute.Builder execute = finished ? null : Execute.newBuilder()
                .setTurn(lastTurn)
                .setCommands(Convert.commands(commands, colors))
                .setOutput(nullToEmpty(commands.getOutputText()))
                .addAllDebugProperties(debugProperties(commands))
                .addAllPaint(PaintRecorder.ops(commands.getGraphicsCalls()))
                .setCpuNanos(cpuSinceReturn());
        if (finished) {
            sleep();
        } else if (!stopping) {
            send(execute);
        }
        if (stopping) {
            if (execute != null) {
                finalExecute = execute;
                finalCommands = commands;
            }
            finished = true;
            if (dead) {
                throw new DeathException();
            }
            throw winner ? new WinException() : new AbortedException();
        }
        Object next = take();
        if (next == ROUND_OVER) {
            finished = true;
            throw new AbortedException();
        }
        Tick tick = (Tick) next;
        lastTurn = tick.getTurn();
        List<Event> events = Convert.events(tick);
        for (Event e : events) {
            dead |= e instanceof robocode.DeathEvent;
            winner |= e instanceof robocode.WinEvent;
        }
        stopping = tick.getHalt();
        ExecResults results = new ExecResults(Convert.commands(tick.getCommands(), colors), status(tick), events,
                Convert.teamMessages(tick), Convert.bulletUpdates(tick), tick.getHalt(), tick.getShouldWait(),
                tick.getPaintEnabled());
        cpuAtReturn = cpu();
        return results;
    }

    /**
     * The robot's code ended (it crashed, was disabled, or its {@code run} returned) and it
     * waits for the round's end. Classic stores its current commands once more, including
     * a {@code setFire} that wasn't executed yet, after draining its energy right away.
     * After that it only re-sends the server's copy each turn, so a final {@code Execute}
     * with {@code stopped} is equivalent. A robot whose last {@code execute} threw (it
     * died or was halted) sends that execute's commands now, with the bullets and team
     * messages its handlers added since. Otherwise a halted robot sends nothing, as in
     * classic.
     */
    @Override
    public ExecResults waitForBattleEndImpl(ExecCommands commands) {
        if (finalExecute != null && dead) {
            // A dead robot isn't halted: classic stores its commands once more, as they are
            // now, after its handlers ran (a robot that regained energy in the turn it died
            // fires its ghost shot from them, engine-spec §15.3).
            if (commands == finalCommands) {
                // The same list of debug properties, maybe with more since.
                finalExecute.clearDebugProperties();
            }
            finalExecute.setCommands(Convert.commands(commands, colors))
                    .setOutput(finalExecute.getOutput() + nullToEmpty(commands.getOutputText()))
                    .addAllDebugProperties(debugProperties(commands));
            send(finalExecute);
            finalExecute = null;
            finalCommands = null;
        } else if (finalExecute != null) {
            // Halted: classic's peer keeps the robot's bullet and team message lists, not
            // copies (ExecCommands(origin, fromRobot)), and a robot whose execute threw keeps
            // adding to them: what its handlers fire here still counts (lessonz.robocode.Oz
            // fires in a custom event after winning). The rest is as it was at the execute.
            RobotCommands late = Convert.commands(finalCommands, colors);
            finalExecute.getCommandsBuilder()
                    .clearBullets().addAllBullets(late.getBulletsList())
                    .clearTeamMessages().addAllTeamMessages(late.getTeamMessagesList());
            send(finalExecute);
            finalExecute = null;
            finalCommands = null;
        } else if (!finished && (!stopping || dead)) {
            // The robot's code ended: classic stores its commands once more (also when it just
            // died without executing again; then the server awaits this as its final Execute).
            Execute.Builder execute = Execute.newBuilder()
                    .setTurn(lastTurn)
                    .setCommands(Convert.commands(commands, colors))
                    .setOutput(nullToEmpty(commands.getOutputText()))
                    .addAllDebugProperties(debugProperties(commands))
                    .setCpuNanos(cpuSinceReturn())
                    .setDrainEnergy(drained);
            send(stopping ? execute : execute.setStopped(true));
        } else if (!finished) {
            // Halted without executing again: classic stores nothing, but the server awaits
            // a final Execute. One without commands changes nothing.
            send(Execute.newBuilder().setTurn(lastTurn).setCpuNanos(cpuSinceReturn()));
        }
        Object next;
        do {
            next = take();
        } while (next != ROUND_OVER);
        finished = true;
        return new ExecResults(commands, null, new ArrayList<>(), new ArrayList<>(), new ArrayList<>(), true, false,
                false);
    }

    /** The robot's code crashed (classic drains its energy). Reported with its final commands. */
    @Override
    public void drainEnergy() {
        drained = true;
    }

    @Override
    public void punishBadBehavior(BadBehavior badBehavior) {
        System.err.println("robot disabled: " + badBehavior);
        if (!finished) {
            finished = true;
            send(Execute.newBuilder().setTurn(lastTurn).setDisable(true).setStopped(true));
        }
    }

    @Override
    public void setRunning(boolean value) {
        running = value;
        if (!value && !finished) {
            // The robot's thread ended without being stopped (its code crashed).
            finished = true;
            send(Execute.newBuilder().setTurn(lastTurn).setStopped(true).setDrainEnergy(drained));
        }
    }

    @Override
    public boolean isRunning() {
        return running;
    }

    private RobotStatus status(Tick tick) {
        return Convert.status(tick.getStatus());
    }

    private long cpu() {
        if (countedWork) {
            return RobotRandomness.countedWorkNanos();
        }
        return threads.isCurrentThreadCpuTimeSupported() ? threads.getCurrentThreadCpuTime() : 0;
    }

    private long cpuSinceReturn() {
        return cpuAtReturn < 0 ? 0 : Math.max(0, cpu() - cpuAtReturn);
    }

    private Object take() {
        try {
            return inbox.take();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            finished = true;
            throw new AbortedException();
        }
    }

    private void send(Execute.Builder execute) {
        try {
            synchronized (out) {
                ClientMessage.newBuilder().setExecute(execute).build().writeDelimitedTo(out);
                out.flush();
            }
        } catch (IOException e) {
            finished = true;
            throw new AbortedException();
        }
    }

    private static void sleep() {
        try {
            Thread.sleep(100);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    /** The debug properties set since the last execute; a null value (removed) is empty. */
    private static List<DebugProperty> debugProperties(ExecCommands commands) {
        List<DebugProperty> properties = new ArrayList<>();
        if (commands.getDebugProperties() != null) {
            for (net.sf.robocode.peer.DebugProperty p : commands.getDebugProperties()) {
                if (p.getKey() != null) {
                    properties.add(DebugProperty.newBuilder()
                            .setKey(p.getKey())
                            .setValue(nullToEmpty(p.getValue()))
                            .build());
                }
            }
        }
        return properties;
    }

    private static String nullToEmpty(String s) {
        return s == null ? "" : s;
    }
}
