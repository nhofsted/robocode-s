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


import java.awt.*;


/**
 * RobotColors - A serializable class to send Colors to teammates.
 *
 * @author Mathew A. Nelson (original)
 * @author Flemming N. Larsen (contributor)
 */
public class RobotColors implements java.io.Serializable {

	private static final long serialVersionUID = 1L;

	public Color bodyColor;
	public Color gunColor;
	public Color radarColor;
	public Color scanColor;
	public Color bulletColor;
}
