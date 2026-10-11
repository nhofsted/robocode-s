import { describe, expect, it } from 'vitest';
import { compareBy, formatNumber, packageName, rumbleName, shortName } from './format.ts';

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
