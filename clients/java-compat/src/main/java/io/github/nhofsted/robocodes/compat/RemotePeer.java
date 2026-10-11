/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.protocol.v1.ClientMessage;
import io.github.nhofsted.robocodes.protocol.v1.Execute;
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
 * the commands are sent first; a robot that died or was halted then gets
 * {@link DeathException}, {@link WinException} or {@link AbortedException}.
 */
final class RemotePeer implements IRobotPeer {

    /** Placed in the inbox when the round ends while the robot waits. */
    private static final Object ROUND_OVER = new Object();

    private final OutputStream out;
    private final BlockingQueue<Object> inbox = new LinkedBlockingQueue<>();
    private final ThreadMXBean threads = ManagementFactory.getThreadMXBean();

    private volatile boolean running;
    // Robot thread state, reset each round.
    private long lastTurn;
    private boolean stopping;
    private boolean dead;
    private boolean winner;
    private boolean finished;
    private boolean drained;
    private long cpuAtReturn = -1;

    RemotePeer(OutputStream out) {
        this.out = out;
    }

    /** Main thread: a new round starts. */
    void newRound() {
        inbox.clear();
        lastTurn = 0;
        stopping = false;
        dead = false;
        winner = false;
        finished = false;
        drained = false;
        cpuAtReturn = -1;
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
        if (finished) {
            sleep();
        } else {
            send(Execute.newBuilder()
                    .setTurn(lastTurn)
                    .setCommands(Convert.commands(commands))
                    .setOutput(nullToEmpty(commands.getOutputText()))
                    .setCpuNanos(cpuSinceReturn()));
        }
        if (stopping) {
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
        ExecResults results = new ExecResults(Convert.commands(tick.getCommands()), status(tick), events,
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
     * with {@code stopped} is equivalent. A halted robot (the last round's winner) sends
     * nothing, as in classic.
     */
    @Override
    public ExecResults waitForBattleEndImpl(ExecCommands commands) {
        if (!stopping && !finished) {
            send(Execute.newBuilder()
                    .setTurn(lastTurn)
                    .setCommands(Convert.commands(commands))
                    .setOutput(nullToEmpty(commands.getOutputText()))
                    .setCpuNanos(cpuSinceReturn())
                    .setDrainEnergy(drained)
                    .setStopped(true));
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

    private static String nullToEmpty(String s) {
        return s == null ? "" : s;
    }
}
