import { describe, expect, it } from 'vitest';
import { classOf, createFixtures } from './fixtures.ts';
import type { BotDetails, Rankings, RumbleIndex } from './types.ts';

const fixtures = createFixtures({ seed: 42, bots: 120 });
const index = fixtures.get('rumbles.json') as RumbleIndex;

describe('fixtures', () => {
	it('are deterministic for a seed', () => {
		const again = createFixtures({ seed: 42, bots: 120 });
		expect(JSON.stringify(again.get('rankings/1v1.json'))).toBe(
			JSON.stringify(fixtures.get('rankings/1v1.json'))
		);
	});

	it('list every division and weight class', () => {
		expect(index.rumbles.map((r) => r.id)).toEqual([
			'1v1',
			'1v1-mini',
			'1v1-micro',
			'1v1-nano',
			'melee',
			'melee-mini',
			'melee-micro',
			'melee-nano',
			'teams',
			'twinduel'
		]);
	});

	it('rank by APS, best first, with ranks 1..n', () => {
		for (const r of index.rumbles) {
			const rankings = fixtures.get(`rankings/${r.id}.json`) as Rankings;
			expect(rankings.bots).toHaveLength(r.bots);
			rankings.bots.forEach((row, i) => {
				expect(row.rank).toBe(i + 1);
				if (i > 0) expect(row.aps).toBeLessThanOrEqual(rankings.bots[i - 1].aps);
			});
		}
	});

	it('have pairing scores that add up to 100 from both sides', () => {
		const rankings = fixtures.get('rankings/1v1.json') as Rankings;
		const a = fixtures.get(`bots/${rankings.bots[0].id}.json`) as BotDetails;
		for (const p of a.rumbles['1v1'].pairings.slice(0, 20)) {
			const b = fixtures.get(`bots/${p.opponent}.json`) as BotDetails;
			const back = b.rumbles['1v1'].pairings.find((q) => q.opponent === a.id);
			expect(back?.battles).toBe(p.battles);
			expect(back!.score + p.score).toBeCloseTo(100, 1);
		}
	});

	it('keep weight-class rumbles within their codesize limit', () => {
		const nano = fixtures.get('rankings/1v1-nano.json') as Rankings;
		expect(nano.bots.length).toBeGreaterThan(0);
		for (const row of nano.bots) {
			const bot = fixtures.get(`bots/${row.id}.json`) as BotDetails;
			expect(bot.versions[0].weightClass).toBe('nano');
			expect(bot.rumbles['1v1-nano'].rank).toBe(row.rank);
		}
	});

	it('give the current version first and retire older ones', () => {
		const rankings = fixtures.get('rankings/1v1.json') as Rankings;
		for (const row of rankings.bots.slice(0, 30)) {
			const bot = fixtures.get(`bots/${row.id}.json`) as BotDetails;
			expect(bot.versions[0].version).toBe(row.version);
			expect(bot.versions[0].retiredAt).toBeNull();
			for (const v of bot.versions.slice(1)) expect(v.retiredAt).not.toBeNull();
		}
	});

	it('keep history across versions, switching on release days', () => {
		const rankings = fixtures.get('rankings/1v1.json') as Rankings;
		let switches = 0;
		for (const row of rankings.bots) {
			const bot = fixtures.get(`bots/${row.id}.json`) as BotDetails;
			const history = bot.rumbles['1v1'].history;
			const order = bot.versions.map((v) => v.version).reverse();
			expect(history.at(-1)!.version).toBe(row.version);
			for (let i = 1; i < history.length; i++) {
				const [prev, cur] = [history[i - 1], history[i]];
				if (prev.version === cur.version) continue;
				switches++;
				expect(order.indexOf(cur.version)).toBeGreaterThan(order.indexOf(prev.version));
				const added = bot.versions.find((v) => v.version === cur.version)!.addedAt.slice(0, 10);
				expect(cur.date >= added && prev.date < added).toBe(true);
			}
		}
		expect(switches).toBeGreaterThan(0);
	});

	it('return undefined for unknown files', () => {
		expect(fixtures.get('rankings/nope.json')).toBeUndefined();
		expect(fixtures.get('bots/no.such.Bot.json')).toBeUndefined();
		expect(fixtures.get('other.json')).toBeUndefined();
	});
});

describe('classOf', () => {
	it('uses the classic codesize limits', () => {
		expect(classOf(249)).toBe('nano');
		expect(classOf(250)).toBe('micro');
		expect(classOf(749)).toBe('micro');
		expect(classOf(750)).toBe('mini');
		expect(classOf(1499)).toBe('mini');
		expect(classOf(1500)).toBe('mega');
	});
});
