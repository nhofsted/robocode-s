// What to draw for one turn of a replay track, as the desktop GUI draws it (crates/gui/src/
// track.rs and view.rs): pure data, no canvas. A track has no scan arcs or explosions; they
// are derived here the way the GUI derives them (ADR 0017).

import { BULLET_END, BULLET_STRIDE, ROBOT_STATE, ROBOT_STRIDE, type ReplayTrack } from './track.ts';

/** A bullet's explosion lasts this many turns (engine rules.rs BULLET_EXPLOSION_LENGTH). */
export const BULLET_EXPLOSION_LENGTH = 17;
/** A robot's death explosion lasts this many turns (ROBOT_EXPLOSION_LENGTH). */
export const ROBOT_EXPLOSION_LENGTH = 71;
/** A death explosion advances one sprite frame every this many turns. */
export const ROBOT_EXPLOSION_SLOWDOWN = 4;
/** How far a scan arc reaches, in pixels (the radar range). */
export const RADAR_RANGE = 1200;
/** A scan arc narrower than this (radians, 0.5 degrees) is drawn as a line. */
export const SCAN_LINE_BELOW = (0.5 * Math.PI) / 180;
/** Number of sprite frames of classic's explosion (assets/classic/explosion). */
export const EXPLOSION_FRAMES = 17;
/** The size of an explosion sprite in pixels. */
export const EXPLOSION_SIZE = 128;

export interface TankView {
	robot: number;
	x: number;
	y: number;
	/** Radians, 0 = north (+y), clockwise. */
	body: number;
	gun: number;
	radar: number;
	energy: number;
	hitWall: boolean;
	hitRobot: boolean;
}

export interface ScanArc {
	robot: number;
	/** The radar's origin. */
	x: number;
	y: number;
	/** Radar heading in the turn before, and the signed sweep to now (the shorter way, |extent| <= pi). */
	start: number;
	extent: number;
}

export interface BulletView {
	bullet: number;
	owner: number;
	x: number;
	y: number;
	power: number;
	/** Diameter in field pixels: classic's 2 * sqrt(2.5 * power). */
	diameter: number;
}

export interface Explosion {
	/** A bullet hit (wall, robot or another bullet), or a robot's death. */
	kind: 'bullet' | 'robot';
	x: number;
	y: number;
	/** Turns since it began. */
	age: number;
	/** Index into the 17 explosion sprites. */
	sprite: number;
	/** Multiplier on the sprite's 128 px. */
	scale: number;
}

export interface FrameView {
	frame: number;
	/** Robots alive in this frame, in robot order. */
	tanks: TankView[];
	/** Energy of every robot, by robot index (a dead robot keeps what it died with). */
	energy: Float32Array;
	alive: boolean[];
	scans: ScanArc[];
	bullets: BulletView[];
	explosions: Explosion[];
	/** The last frame of the round. */
	over: boolean;
	/** In the last frame: the robot left alive, or -1 (none alive, or several). Otherwise -1. */
	winner: number;
	/** How many robots are alive. */
	survivors: number;
}

/** A bullet hit's explosion scale for a bullet of `power`, as classic. */
export function bulletExplosionScale(power: number): number {
	return Math.sqrt(1000 * power) / EXPLOSION_SIZE;
}

/** Wraps to (-pi, pi]. */
function shorter(extent: number): number {
	if (extent < -Math.PI) return extent + 2 * Math.PI;
	if (extent > Math.PI) return extent - 2 * Math.PI;
	return extent;
}

/** What to draw in `frame` (clamped to the track). */
export function frameView(track: ReplayTrack, frame: number): FrameView {
	const last = track.frameCount - 1;
	const f = Math.max(0, Math.min(last, Math.floor(frame)));
	const n = track.robotCount;
	const at = (fr: number, r: number) => (fr * n + r) * ROBOT_STRIDE;
	const R = track.robots;

	const tanks: TankView[] = [];
	const scans: ScanArc[] = [];
	const energy = new Float32Array(n);
	const alive: boolean[] = [];
	for (let r = 0; r < n; r++) {
		const o = at(f, r);
		energy[r] = R[o + 6];
		const live = R[o] !== ROBOT_STATE.dead;
		alive.push(live);
		if (!live) continue;
		tanks.push({
			robot: r,
			x: R[o + 1],
			y: R[o + 2],
			body: R[o + 3],
			gun: R[o + 4],
			radar: R[o + 5],
			energy: R[o + 6],
			hitWall: R[o] === ROBOT_STATE.hitWall,
			hitRobot: R[o] === ROBOT_STATE.hitRobot
		});
		if (f > 0 && R[at(f - 1, r)] !== ROBOT_STATE.dead) {
			const start = R[at(f - 1, r) + 5];
			scans.push({
				robot: r,
				x: R[o + 1],
				y: R[o + 2],
				start,
				extent: shorter(R[o + 5] - start)
			});
		}
	}

	const B = track.bullets;
	const bullets: BulletView[] = [];
	const flights = track.bulletPositions(f);
	for (let k = 0; k < flights.length; k += 3) {
		const id = flights[k];
		const power = B[id * BULLET_STRIDE + 5];
		bullets.push({
			bullet: id,
			owner: B[id * BULLET_STRIDE],
			x: flights[k + 1],
			y: flights[k + 2],
			power,
			diameter: 2 * Math.sqrt(2.5 * power)
		});
	}

	const explosions: Explosion[] = [];
	for (let id = 0; id < track.bulletCount; id++) {
		const o = id * BULLET_STRIDE;
		if (B[o + 1] > f) break; // in order of birth
		const end = B[o + 6];
		if (B[o + 7] === BULLET_END.none || end < 0 || end > f) continue;
		const age = f - end;
		if (age >= BULLET_EXPLOSION_LENGTH) continue;
		explosions.push({
			kind: 'bullet',
			x: B[o + 9],
			y: B[o + 10],
			age,
			sprite: Math.min(age, EXPLOSION_FRAMES - 1),
			scale: bulletExplosionScale(B[o + 5])
		});
	}
	for (let r = 0; r < n; r++) {
		const died = track.deathFrames[r];
		if (died < 0 || died > f) continue;
		const age = f - died;
		if (age >= ROBOT_EXPLOSION_LENGTH) continue;
		// A dead robot is kept as it died: the explosion is where it was.
		const o = at(f, r);
		explosions.push({
			kind: 'robot',
			x: R[o + 1],
			y: R[o + 2],
			age,
			sprite: Math.min(Math.floor(age / ROBOT_EXPLOSION_SLOWDOWN), EXPLOSION_FRAMES - 1),
			scale: 1
		});
	}

	const over = f === last;
	const survivors = alive.filter(Boolean).length;
	return {
		frame: f,
		tanks,
		energy,
		alive,
		scans,
		bullets,
		explosions,
		over,
		winner: over && survivors === 1 ? alive.indexOf(true) : -1,
		survivors
	};
}
