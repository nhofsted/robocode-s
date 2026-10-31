/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.determinism;

/**
 * What robot code calls around its monitors and notifies ({@link ThreadCalls}, ADR 0025):
 * public, so robot classes link to it directly (their loader delegates to the agent's), and
 * cheap until a robot of this JVM has threads of its own.
 */
public final class RobotSync {

    private RobotSync() {
    }

    /** Before robot code's {@code monitorenter} on {@code o}. */
    public static void enter(Object o) {
        if (RobotThreads.active()) {
            RobotThreads.monitorEnter(o);
        }
    }

    /** After robot code's {@code monitorexit} on {@code o}. */
    public static void exit(Object o) {
        if (RobotThreads.active()) {
            RobotThreads.monitorExit(o);
        }
    }

    /** Before robot code's {@code o.notify()}. */
    public static void notify(Object o) {
        if (RobotThreads.active()) {
            RobotThreads.notifying(o, false);
        }
    }

    /** Before robot code's {@code o.notifyAll()}. */
    public static void notifyAll(Object o) {
        if (RobotThreads.active()) {
            RobotThreads.notifying(o, true);
        }
    }
}
