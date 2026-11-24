import { describe, expect, it } from 'vitest';
import {
	compareBy,
	formatIn,
	formatNumber,
	nextPublish,
	packageName,
	rumbleName,
	shortName,
	updateTimes
} from './format.ts';

describe('update times', () => {
	it('finds the next scheduled publish', () => {
		expect(nextPublish(new Date('2026-10-24T17:58:00Z')).toISOString()).toBe('2026-10-24T21:30:00.000Z');
		expect(nextPublish(new Date('2026-10-24T21:30:00Z')).toISOString()).toBe('2026-10-25T03:30:00.000Z');
		expect(nextPublish(new Date('2026-12-31T22:00:00Z')).toISOString()).toBe('2027-01-01T03:30:00.000Z');
	});

	it('formats the wait', () => {
		expect(formatIn(30_000)).toBe('in under a minute');
		expect(formatIn(40 * 60_000)).toBe('in 40 min');
		expect(formatIn(120 * 60_000)).toBe('in 2 h');
		expect(formatIn(125 * 60_000 + 59_000)).toBe('in 2 h 5 min');
	});

	it('says when the last and next updates are', () => {
		const at = '2026-10-24T15:31:12Z';
		expect(updateTimes(at, new Date('2026-10-24T19:25:00Z'))).toBe(
			'last 24 Oct 2026, 15:31 UTC; next at 21:30 UTC (in 2 h 5 min)'
		);
		// The 21:30 run hasn't published yet: due, not tomorrow's 03:30.
		expect(updateTimes(at, new Date('2026-10-24T21:40:00Z'))).toBe(
			'last 24 Oct 2026, 15:31 UTC; next one due now'
		);
	});
});

describe('compareBy', () => {
	const rows = [{ v: 2 }, { v: null }, { v: 5 }, { v: 1 }];

	it('sorts nulls last in both directions', () => {
		const get = (r: { v: number | null }) => r.v;
		expect([...rows].sort(compareBy(get, 'asc')).map((r) => r.v)).toEqual([1, 2, 5, null]);
		expect([...rows].sort(compareBy(get, 'desc')).map((r) => r.v)).toEqual([5, 2, 1, null]);
	});

	it('compares strings', () => {
		const names = [{ n: 'walls' }, { n: 'corners' }];
		expect(names.sort(compareBy((r) => r.n, 'asc')).map((r) => r.n)).toEqual(['corners', 'walls']);
	});
});

describe('formatting', () => {
	it('formats numbers with fixed digits and a dash for missing values', () => {
		expect(formatNumber(71.234, 2)).toBe('71.23');
		expect(formatNumber(1734.6, 0)).toBe('1,735');
		expect(formatNumber(null, 2)).toBe('–');
	});

	it('names rumbles', () => {
		expect(rumbleName({ division: '1v1', weightClass: 'mega' })).toBe('1v1');
		expect(rumbleName({ division: 'melee', weightClass: 'micro' })).toBe('Micro Melee');
	});

	it('splits bot ids', () => {
		expect(shortName('jk.mega.DrussGT')).toBe('DrussGT');
		expect(packageName('jk.mega.DrussGT')).toBe('jk.mega');
		expect(packageName('Lonely')).toBe('');
	});
});
