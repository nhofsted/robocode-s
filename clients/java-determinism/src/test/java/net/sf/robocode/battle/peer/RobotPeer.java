/*
 * SPDX-License-Identifier: EPL-2.0
 */
package net.sf.robocode.battle.peer;

import net.sf.robocode.peer.ExecCommands;
import net.sf.robocode.peer.ExecResults;

import java.util.concurrent.atomic.AtomicReference;

/**
 * Stands in for classic's robot peer in tests: the agent patches the class of this name,
 * with the members it patches. The counters tell which method bodies ran.
 */
public final class RobotPeer {
    public int currentExecutionTime;
    public int rounds;
    public int wakeups;
    public int executions;
    private final AtomicReference<ExecCommands> commands = new AtomicReference<>();
    public ExecCommands currentCommands;

    public void startRound(long time, int turn) {
        rounds++;
    }

    public ExecResults executeImpl(ExecCommands commands) {
        executions++;
        // As classic: published for the battle's next load, then the robot sleeps.
        this.commands.set(commands);
        return new ExecResults();
    }

    public void waitWakeup() {
        // A branch, as classic's: the patch's frame must match the method's own.
        if (wakeups >= 0) {
            wakeups++;
        }
    }

    public void performLoadCommands() {
        currentCommands = commands.get();
    }
}
