// A replay track (ADR 0017, `.track`) decoded by crates/replay-wasm into plain typed arrays.
// The wasm module is built by tools/build-wasm.sh into ./wasm/ (gitignored).
import init, { decode, type InitInput } from './wasm/robocode_s_replay_wasm.js';
import wasmUrl from './wasm/robocode_s_replay_wasm_bg.wasm?url';

/** Floats per robot in `robots` and per bullet in `bullets`. */
export const ROBOT_STRIDE = 7;
export const BULLET_STRIDE = 11;

export const ROBOT_STATE = { active: 0, hitWall: 1, hitRobot: 2, dead: 3 } as const;
export const BULLET_END = { none: -1, wall: 0, bullet: 1, robot: 2 } as const;

export interface ReplayTrack {
	engineVersion: number;
	/** Field size in pixels. */
	width: number;
	height: number;
	gunCoolingMilli: number;
	robotCount: number;
	frameCount: number;
	bulletCount: number;
	/**
	 * [frame][robot][ROBOT_STRIDE]: state (ROBOT_STATE), x, y (px, y up), body, gun and radar
	 * heading (radians; 0 = north, clockwise), energy.
	 */
	robots: Float32Array;
	/**
	 * [bullet][BULLET_STRIDE], in order of birth: owner, born frame, x, y (px, at birth),
	 * heading (radians), power, end frame (-1 none), end kind (BULLET_END), victim (robot index
	 * or -1), end x, end y (px).
	 */
	bullets: Float32Array;
	/** Per robot, the first frame it is dead in, -1 if never. */
	deathFrames: Int32Array;
	/** The bullets in flight in `frame` as [bullet index, x, y] triples (px); not those ending in it. */
	bulletPositions(frame: number): Float32Array;
}

let ready: Promise<unknown> | undefined;

/**
 * Initialises the wasm module once. Called by `loadTrack`; call it first to supply the module
 * yourself (bytes in tests, where there is no fetch for files).
 */
export function initTrackWasm(module?: InitInput | Promise<InitInput>): Promise<unknown> {
	return (ready ??= init({ module_or_path: module ?? wasmUrl }));
}

/** Decodes a `.track` file; throws an Error if it is not one. */
export async function loadTrack(bytes: Uint8Array): Promise<ReplayTrack> {
	await initTrackWasm();
	const t = decode(bytes);
	try {
		// The flight maths stays in Rust: every frame's bullet positions, once, as slices of one array.
		const frames = t.frameCount;
		const starts = new Uint32Array(frames + 1);
		const chunks: Float32Array[] = [];
		for (let f = 0; f < frames; f++) {
			const p = t.bulletPositions(f);
			chunks.push(p);
			starts[f + 1] = starts[f] + p.length;
		}
		const flights = new Float32Array(starts[frames]);
		chunks.forEach((p, f) => flights.set(p, starts[f]));
		return {
			engineVersion: t.engineVersion,
			width: t.width,
			height: t.height,
			gunCoolingMilli: t.gunCoolingMilli,
			robotCount: t.robotCount,
			frameCount: frames,
			bulletCount: t.bulletCount,
			robots: t.robotFrames(),
			bullets: t.bullets(),
			deathFrames: t.deathFrames(),
			bulletPositions: (frame) => flights.subarray(starts[frame] ?? 0, starts[frame + 1] ?? 0)
		};
	} finally {
		t.free();
	}
}
