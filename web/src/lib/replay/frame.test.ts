import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, test } from 'vitest';
import {
	BULLET_EXPLOSION_LENGTH,
	ROBOT_EXPLOSION_LENGTH,
	bulletExplosionScale,
	frameView
} from './frame.ts';
import { BULLET_END, BULLET_STRIDE, ROBOT_STATE, ROBOT_STRIDE, initTrackWasm, loadTrack } from './track.ts';

const fixture = (name: string) =>
	new Uint8Array(readFileSync(new URL(`./fixtures/${name}`, import.meta.url)));

beforeAll(() => {
	const wasm = new URL('./wasm/robocode_s_replay_wasm_bg.wasm', import.meta.url);
	return initTrackWasm(readFileSync(wasm));
});

describe('small.track (3 robots, 60 frames)', () => {
	// Bullet 0: robot 0 -> robot 1, born 10, hits in 30 at (350, 200), power 2. Bullet 1 hits
	// a wall in 50. Robot 2 dies in 40.
	test('tanks, headings and energy', async () => {
		const v = frameView(await loadTrack(fixture('small.track')), 5);
		expect(v.tanks.map((t) => t.robot)).toEqual([0, 1, 2]);
		expect([v.tanks[1].x, v.tanks[1].y, v.tanks[1].energy]).toEqual([260, 200, 99]);
		expect(v.tanks[1].gun).toBeCloseTo(Math.PI / 2, 5);
		expect(v.survivors).toBe(3);
		expect(v.over).toBe(false);
		expect(v.winner).toBe(-1);
	});

	test('bullets in flight, sized by power', async () => {
		const t = await loadTrack(fixture('small.track'));
		expect(frameView(t, 9).bullets).toEqual([]);
		const b = frameView(t, 12).bullets;
		expect(b.length).toBe(1);
		expect([b[0].owner, b[0].x, b[0].y, b[0].power]).toEqual([0, 148, 150, 2]);
		expect(b[0].diameter).toBeCloseTo(2 * Math.sqrt(5), 9);
	});

	test('a bullet hit explodes where it ended, for 17 turns, and no longer flies', async () => {
		const t = await loadTrack(fixture('small.track'));
		expect(frameView(t, 29).explosions).toEqual([]);
		const hit = frameView(t, 30);
		expect(hit.bullets.some((b) => b.bullet === 0)).toBe(false);
		expect(hit.explosions).toEqual([
			{ kind: 'bullet', x: 350, y: 200, age: 0, sprite: 0, scale: bulletExplosionScale(2) }
		]);
		const later = frameView(t, 30 + 16).explosions.find((e) => e.kind === 'bullet');
		expect(later?.age).toBe(16);
		expect(later?.sprite).toBe(16);
		expect(frameView(t, 30 + BULLET_EXPLOSION_LENGTH).explosions.some((e) => e.kind === 'bullet')).toBe(false);
		expect(bulletExplosionScale(2)).toBeCloseTo(Math.sqrt(2000) / 128, 12);
	});

	test('a death explosion at the robot, slowed 4x, and the robot is gone', async () => {
		const t = await loadTrack(fixture('small.track'));
		expect(frameView(t, 39).tanks.length).toBe(3);
		const v = frameView(t, 40);
		expect(v.tanks.map((x) => x.robot)).toEqual([0, 1]);
		expect(v.alive).toEqual([true, true, false]);
		const boom = v.explosions.find((e) => e.kind === 'robot')!;
		expect([boom.age, boom.sprite, boom.scale]).toEqual([0, 0, 1]);
		const o = (40 * 3 + 2) * ROBOT_STRIDE;
		expect([boom.x, boom.y]).toEqual([t.robots[o + 1], t.robots[o + 2]]);
		expect(frameView(t, 49).explosions.find((e) => e.kind === 'robot')?.sprite).toBe(2);
		// 60 frames: dies at 40, so the explosion is still going at the last one.
		expect(frameView(t, 59).explosions.some((e) => e.kind === 'robot')).toBe(true);
	});

	test('scan arcs run from the previous radar heading, the shorter way', async () => {
		const t = await loadTrack(fixture('small.track'));
		expect(frameView(t, 0).scans).toEqual([]);
		for (const frame of [1, 20, 39]) {
			for (const s of frameView(t, frame).scans) {
				const prev = t.robots[((frame - 1) * 3 + s.robot) * ROBOT_STRIDE + 5];
				const now = t.robots[(frame * 3 + s.robot) * ROBOT_STRIDE + 5];
				expect(s.start).toBe(prev);
				expect(Math.abs(s.extent)).toBeLessThanOrEqual(Math.PI);
				const d = Math.atan2(Math.sin(prev + s.extent - now), Math.cos(prev + s.extent - now));
				expect(Math.abs(d)).toBeLessThan(1e-5);
			}
		}
		// A robot dead in this or the previous frame has no arc.
		expect(frameView(t, 40).scans.map((s) => s.robot)).toEqual([0, 1]);
		expect(frameView(t, 41).scans.map((s) => s.robot)).toEqual([0, 1]);
	});

	test('the last frame says who won', async () => {
		const v = frameView(await loadTrack(fixture('small.track')), 1000);
		expect(v.frame).toBe(59);
		expect(v.over).toBe(true);
		expect(v.survivors).toBe(2);
		expect(v.winner).toBe(-1); // two alive: no single winner
	});
});

describe.each(['acero-mailmanx.track', 'melee8.track'])('real track %s', (name) => {
	test('every frame is consistent', async () => {
		const t = await loadTrack(fixture(name));
		expect(t.width).toBeGreaterThan(0);
		let flights = 0;
		let blasts = 0;
		for (let f = 0; f < t.frameCount; f++) {
			const v = frameView(t, f);
			expect(v.tanks.length).toBe(v.survivors);
			for (const x of v.tanks) {
				expect(x.x).toBeGreaterThanOrEqual(0);
				expect(x.x).toBeLessThanOrEqual(t.width);
				expect(x.y).toBeGreaterThanOrEqual(0);
				expect(x.y).toBeLessThanOrEqual(t.height);
			}
			for (const e of v.explosions) {
				expect(e.sprite).toBeGreaterThanOrEqual(0);
				expect(e.sprite).toBeLessThan(17);
				expect(e.age).toBeLessThan(e.kind === 'bullet' ? BULLET_EXPLOSION_LENGTH : ROBOT_EXPLOSION_LENGTH);
			}
			for (const s of v.scans) expect(Math.abs(s.extent)).toBeLessThanOrEqual(Math.PI + 1e-6);
			flights += v.bullets.length;
			blasts += v.explosions.length;
		}
		expect(flights).toBeGreaterThan(0);
		expect(blasts).toBeGreaterThan(0);
	});

	test('bullets leave from near the gun tip, along the gun heading', async () => {
		const t = await loadTrack(fixture(name));
		let checked = 0;
		for (let id = 0; id < t.bulletCount; id++) {
			const o = id * BULLET_STRIDE;
			const [owner, born, bx, by, heading] = [t.bullets[o], t.bullets[o + 1], t.bullets[o + 2], t.bullets[o + 3], t.bullets[o + 4]];
			const gunAt = (frame: number) => t.robots[(frame * t.robotCount + owner) * ROBOT_STRIDE + 4];
			const r = (born * t.robotCount + owner) * ROBOT_STRIDE;
			const [x, y] = [t.robots[r + 1], t.robots[r + 2]];
			// Within a robot's size of its owner.
			expect(Math.hypot(bx - x, by - y)).toBeLessThan(40);
			// Flying where the gun points, to within a turn of the gun's turning (20 deg + the body's 10).
			const diff = (a: number, b: number) => Math.abs(Math.atan2(Math.sin(a - b), Math.cos(a - b)));
			expect(diff(gunAt(born), heading)).toBeLessThan(0.6);
			checked++;
		}
		expect(checked).toBe(t.bulletCount);
	});

	test('the round ends with a winner when one robot is left', async () => {
		const t = await loadTrack(fixture(name));
		const v = frameView(t, t.frameCount - 1);
		expect(v.over).toBe(true);
		const dead = Array.from(t.deathFrames).filter((d) => d >= 0).length;
		expect(v.survivors).toBe(t.robotCount - dead);
		if (v.survivors === 1) expect(v.winner).toBe(v.alive.indexOf(true));
		else expect(v.winner).toBe(-1);
		// Dead robots stay dead.
		for (let r = 0; r < t.robotCount; r++) {
			const d = t.deathFrames[r];
			if (d >= 0) expect(t.robots[((t.frameCount - 1) * t.robotCount + r) * ROBOT_STRIDE]).toBe(ROBOT_STATE.dead);
		}
		expect(t.bullets.length / BULLET_STRIDE).toBe(t.bulletCount);
		expect(BULLET_END.none).toBe(-1);
	});
});

test('the melee fixture has at least six robots and several deaths', async () => {
	const t = await loadTrack(fixture('melee8.track'));
	expect(t.robotCount).toBeGreaterThanOrEqual(6);
	expect(Array.from(t.deathFrames).filter((d) => d >= 0).length).toBeGreaterThanOrEqual(3);
});
