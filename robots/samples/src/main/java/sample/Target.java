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
package sample;


import robocode.AdvancedRobot;
import robocode.Condition;
import robocode.CustomEvent;

import java.awt.*;


/**
 * Target - a sample robot that demonstrates custom events.
 * <p>
 * A stationary robot that moves when its energy drops below a certain threshold.
 * This robot demonstrates how to use custom events in Robocode.
 *
 * @author Mathew A. Nelson (original)
 * @author Flemming N. Larsen (contributor)
 */
public class Target extends AdvancedRobot {

	int trigger; // Energy threshold that triggers movement

	/**
	 * Target's run method
	 */
	public void run() {
		// Set colors
		setBodyColor(Color.white);
		setGunColor(Color.white);
		setRadarColor(Color.white);

		// Set the initial energy threshold to 80
		trigger = 80;
		// Add a custom event that fires when energy falls below the threshold
		addCustomEvent(new Condition("triggerhit") {
			public boolean test() {
				return (getEnergy() <= trigger);
			}
		});
	}

	/**
	 * onCustomEvent handler
	 */
	public void onCustomEvent(CustomEvent e) {
		// Check if our custom event "triggerhit" was triggered
		if (e.getCondition().getName().equals("triggerhit")) {
			// Lower the threshold by 20 to prevent continuous triggering
			trigger -= 20;
			out.println("Ouch, down to " + (int) (getEnergy() + .5) + " energy.");
			// Execute evasive movement
			turnLeft(65);
			ahead(100);
		}
	}
}
