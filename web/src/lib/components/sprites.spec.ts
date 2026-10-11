import { describe, expect, it } from 'vitest';
import { recolorPixel, type RGB } from './sprites.ts';
// [robot colour, pixel, classic's result], all 0xRRGGBB: every distinct pixel colour of
// body.png, turret.png and radar.png, recoloured by classic 1.11.1's own ColorUtil for eight
// robot colours. Regenerate with web/tools/Recolor.java.
import expected from './recolor.expected.json';

const rgb = (v: number): RGB => [(v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff];

describe('recolorPixel', () => {
	it('matches classic Robocode on every sprite pixel', () => {
		const wrong = (expected as [number, number, number][]).filter(
			([color, pixel, result]) => recolorPixel(rgb(pixel), rgb(color)).join() !== rgb(result).join()
		);
		expect(expected.length).toBeGreaterThan(2000);
		expect(wrong).toEqual([]);
	});
});
