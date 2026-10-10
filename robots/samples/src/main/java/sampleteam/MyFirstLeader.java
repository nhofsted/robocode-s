/*
 * Copyright (c) 2001-2025 Mathew A. Nelson and Robocode contributors
 * All rights reserved. This program and the accompanying materials
 * are made available under the terms of the Eclipse Public License v2.0
 * which accompanies this distribution, and is available at
 * https://www.eclipse.org/legal/epl-2.0/
 *
 * Originally distributed under the Eclipse Public License v1.0 as part of
 * Robocode 1.11.1; distributed here under EPL-2.0 as permitted by
 * section 7 of EPL-1.0.
 *
 * SPDX-License-Identifier: EPL-2.0
 */
package sampleteam;


import robocode.HitByBulletEvent;
import robocode.ScannedRobotEvent;
import robocode.TeamRobot;

import java.awt.*;
import java.io.IOException;


/**
 * MyFirstLeader - a sample team robot by Mathew Nelson.
 * <p>
 * Looks around for enemies, and orders teammates to fire
 *
 * @author Mathew A. Nelson (original)
 * @author Flemming N. Larsen (contributor)
 */
public class MyFirstLeader extends TeamRobot {

	/**
	 * run:  Leader's default behavior
	 */
	public void run() {
		// Prepare RobotColors object
		RobotColors c = new RobotColors();

		c.bodyColor = Color.red;
		c.gunColor = Color.red;
		c.radarColor = Color.red;
		c.scanColor = Color.yellow;
		c.bulletColor = Color.yellow;

		// Set the color of this robot containing the RobotColors
		setBodyColor(c.bodyColor);
		setGunColor(c.gunColor);
		setRadarColor(c.radarColor);
		setScanColor(c.scanColor);
		setBulletColor(c.bulletColor);
		try {
			// Send RobotColors object to our entire team
			broadcastMessage(c);
		} catch (IOException ignored) {}
		// Normal behavior
		while (true) {
			setTurnRadarRight(10000);
			ahead(100);
			back(100);
		}
	}

	/**
	 * onScannedRobot:  What to do when you see another robot
	 */
	public void onScannedRobot(ScannedRobotEvent e) {
		// Don't fire on teammates
		if (isTeammate(e.getName())) {
			return;
		}
		// Calculate enemy bearing
		double enemyBearing = this.getHeading() + e.getBearing();
		// Calculate enemy's position
		double enemyX = getX() + e.getDistance() * Math.sin(Math.toRadians(enemyBearing));
		double enemyY = getY() + e.getDistance() * Math.cos(Math.toRadians(enemyBearing));

		try {
			// Send enemy position to teammates
			broadcastMessage(new Point(enemyX, enemyY));
		} catch (IOException ex) {
			out.println("Unable to send order: ");
			ex.printStackTrace(out);
		}
	}

	/**
	 * onHitByBullet:  Turn perpendicular to bullet path
	 */
	public void onHitByBullet(HitByBulletEvent e) {
		turnLeft(90 - e.getBearing());
	}
}
