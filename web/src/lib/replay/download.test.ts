import { readFileSync } from 'node:fs';
import { beforeAll, expect, test } from 'vitest';
import { withBattle, type TrackBattle } from './download.ts';
import { initTrackWasm, loadTrack } from './track.ts';

const fixture = () => new Uint8Array(readFileSync(new URL('./fixtures/small.track', import.meta.url)));

beforeAll(() => {
	const wasm = new URL('./wasm/robocode_s_replay_wasm_bg.wasm', import.meta.url);
	return initTrackWasm(readFileSync(wasm));
});

const battle: TrackBattle = {
	schema: 1,
	engine: 1,
	seed: '-1234567890123456789',
	round: 3,
	rounds: 35,
	width: 800,
	height: 600,
	robots: [
		{ bot: 'a.A', version: '1.0', sha16: '0123456789abcdef' },
		{ bot: 'b.B', version: '2', sha16: 'fedcba9876543210' },
		{ bot: 'c.C', version: '3', sha16: '00112233445566aa' }
	]
};

test('appends the battle after eight zeros, with its length and the magic', () => {
	const track = fixture();
	const out = withBattle(track, battle);
	expect(Array.from(out.subarray(0, track.length))).toEqual(Array.from(track));
	expect(Array.from(out.subarray(track.length, track.length + 8))).toEqual(new Array(8).fill(0));
	expect(new TextDecoder().decode(out.subarray(out.length - 8))).toBe('RSBATTLE');
	const length = new DataView(out.buffer).getUint32(out.length - 12, true);
	const json = new TextDecoder().decode(out.subarray(track.length + 8, track.length + 8 + length));
	expect(track.length + 8 + length + 12).toBe(out.length);
	expect(JSON.parse(json)).toEqual(battle);
});

test('a track with its battle decodes as the track', async () => {
	const plain = await loadTrack(fixture());
	const both = await loadTrack(withBattle(fixture(), battle));
	expect([both.robotCount, both.frameCount, both.bulletCount]).toEqual([
		plain.robotCount,
		plain.frameCount,
		plain.bulletCount
	]);
	expect(Array.from(both.robots)).toEqual(Array.from(plain.robots));
	expect(Array.from(both.bullets)).toEqual(Array.from(plain.bullets));
});
