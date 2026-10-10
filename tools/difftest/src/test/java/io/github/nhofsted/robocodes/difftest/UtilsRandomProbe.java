/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.difftest;

import io.github.nhofsted.robocodes.determinism.RobotRandomness;
import robocode.control.RandomFactory;
import robocode.util.Utils;

import java.util.List;
import java.util.Random;
import java.util.concurrent.atomic.AtomicReference;

/** Run with both agents by {@link OracleTest}: prints what Utils.getRandom() returns. */
public final class UtilsRandomProbe {

    public static void main(String[] args) throws Exception {
        RobotRandomness.configure(7, List.of("probe.Robot"));
        AtomicReference<String> fromRobot = new AtomicReference<>();
        Thread robot = new Thread(new ThreadGroup("probe.Robot"), () -> {
            Random first = Utils.getRandom();
            fromRobot.set(first.nextLong() + " " + (first == Utils.getRandom()));
        });
        robot.start();
        robot.join();
        System.out.println("robot " + fromRobot.get());
        System.out.println("engine " + (Utils.getRandom() == RandomFactory.getRandom()));
    }
}
