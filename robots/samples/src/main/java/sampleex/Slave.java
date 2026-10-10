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
package sampleex;


import robocode.AdvancedRobot;
import robocode.HitByBulletEvent;
import robocode.ScannedRobotEvent;


/**
 * This is robot derived from AdvancedRobot.
 * Only reason to use this inheritance and this class is that external robots are unable to call RobotPeer directly.
 *
 * @author Pavel Savara (original)
 */
class Slave extends AdvancedRobot {
	final MasterBase parent;

	public Slave(MasterBase parent) {
		this.parent = parent;
	}

	public void run() {
		parent.run();
	}

	public void onScannedRobot(ScannedRobotEvent e) {
		parent.onScannedRobot(e);
	}

	public void onHitByBullet(HitByBulletEvent e) {
		parent.onHitByBullet(e);
	}
}
