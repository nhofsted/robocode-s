import { readFileSync } from 'node:fs';
import { beforeAll, expect, test } from 'vitest';
import { BULLET_END, BULLET_STRIDE, ROBOT_STATE, ROBOT_STRIDE, initTrackWasm, loadTrack } from './track.ts';

// fixtures/small.track: crates/replay-wasm/examples/fixture.rs
const fixture = () => new Uint8Array(readFileSync(new URL('./fixtures/small.track', import.meta.url)));

beforeAll(() => {
	const wasm = new URL('./wasm/robocode_s_replay_wasm_bg.wasm', import.meta.url);
	return initTrackWasm(readFileSync(wasm));
});

test('decodes the metadata and the robots', async () => {
	const t = await loadTrack(fixture());
	expect([t.engineVersion, t.width, t.height, t.gunCoolingMilli]).toEqual([1, 800, 600, 100]);
	expect([t.robotCount, t.frameCount, t.bulletCount]).toEqual([3, 60, 3]);
	expect(t.robots.length).toBe(60 * 3 * ROBOT_STRIDE);
	// Frame 5, robot 1: x = (500 + 4 * 5) / 2 px, y = 400 / 2, body 40 / 1024 of a turn, energy 99.
	const r = t.robots.subarray((5 * 3 + 1) * ROBOT_STRIDE, (5 * 3 + 2) * ROBOT_STRIDE);
	expect(r[0]).toBe(ROBOT_STATE.active);
	expect([r[1], r[2]]).toEqual([260, 200]);
	expect(r[3]).toBeCloseTo((40 * 2 * Math.PI) / 1024, 5);
	expect(r[4]).toBeCloseTo(Math.PI / 2, 5);
	expect(r[6]).toBe(99);
	expect(Array.from(t.deathFrames)).toEqual([-1, -1, 40]);
	expect(t.robots[(45 * 3 + 2) * ROBOT_STRIDE]).toBe(ROBOT_STATE.dead);
});

test('decodes the bullets and where they fly', async () => {
	const t = await loadTrack(fixture());
	const b = (i: number) => Array.from(t.bullets.subarray(i * BULLET_STRIDE, (i + 1) * BULLET_STRIDE));
	// owner, born, x, y, heading, power, end frame, kind, victim, end x, end y
	expect(b(0).map((v, i) => (i === 4 ? Math.round(v * 1000) : v))).toEqual([
		0, 10, 120, 150, 1571, 2, 30, BULLET_END.robot, 1, 350, 200
	]);
	expect(b(1).slice(6, 9)).toEqual([50, BULLET_END.wall, -1]);
	expect(b(2).slice(6, 9)).toEqual([-1, BULLET_END.none, -1]);
	expect(t.bulletPositions(9).length).toBe(0);
	// Bullet 0 flies east at 14 px per turn from (120, 150) in frame 10.
	expect(Array.from(t.bulletPositions(12))).toEqual([0, 148, 150]);
	// Frame 26: bullet 0, bullet 1 (north at 17 px per turn from (140, 150) in frame 20), bullet 2.
	const q = t.bulletPositions(26);
	expect(q.length).toBe(9);
	expect(q[0]).toBe(0);
	expect([q[3], q[4], q[5]]).toEqual([1, 140, 150 + 6 * 17]);
	expect(q[6]).toBe(2);
	expect(t.bulletPositions(55).length).toBe(3);
});

test('rejects garbage', async () => {
	await expect(loadTrack(new Uint8Array([9, 9, 9]))).rejects.toThrow(/not a recording/);
});
