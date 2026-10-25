import { describe, expect, it } from 'vitest';
import { tankColors } from './tank-colors.ts';

describe('tankColors', () => {
	const id = 'nz.jdc.nano.PatternAdept';
	const derived = tankColors(id);

	it('derives colours from the id where the robot set none, the same each time', () => {
		expect(tankColors(id)).toEqual(derived);
		expect(tankColors(id, null)).toEqual(derived);
		expect(tankColors(id, {})).toEqual(derived);
		expect(tankColors('sample.Walls')).not.toEqual(derived);
	});

	it('matches the desktop app (sprites::default_colors, the same values in its tests)', () => {
		expect(tankColors('sample.Fire')).toEqual({
			body: [44, 150, 84],
			gun: [100, 180, 131],
			radar: [199, 60, 221]
		});
	});

	it("uses the robot's own colours, part by part", () => {
		expect(tankColors(id, { body: '#000000', gun: '#FF0000', radar: '#ffffff' })).toEqual({
			body: [0, 0, 0],
			gun: [255, 0, 0],
			radar: [255, 255, 255]
		});
		expect(tankColors(id, { gun: '#102030' })).toEqual({ ...derived, gun: [16, 32, 48] });
		// Something that isn't a colour falls back to the id's.
		expect(tankColors(id, { body: 'red' })).toEqual(derived);
	});
});
