import { describe, expect, it } from 'vitest';
import { classOf, createFixtures } from './fixtures.ts';
import type { Archive, BotDetails, Contributors, Rankings, RumbleIndex } from './types.ts';

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

	it("give some bots their robot's colours, the same in the rankings", () => {
		const rankings = fixtures.get('rankings/1v1.json') as Rankings;
		let colored = 0;
		for (const row of rankings.bots) {
			const bot = fixtures.get(`bots/${row.id}.json`) as BotDetails;
			expect(row.colors).toEqual(bot.versions[0].colors);
			for (const v of bot.versions.slice(1)) expect(v.colors).toBeUndefined();
			if (row.colors) {
				colored++;
				expect(row.colors.body).toMatch(/^#[0-9a-f]{6}$/);
			}
		}
		expect(colored).toBeGreaterThan(0);
		expect(colored).toBeLessThan(rankings.bots.length);
	});

	it('return undefined for unknown files', () => {
		expect(fixtures.get('rankings/nope.json')).toBeUndefined();
		expect(fixtures.get('bots/no.such.Bot.json')).toBeUndefined();
		expect(fixtures.get('other.json')).toBeUndefined();
	});
});

describe('contributors', () => {
	it('are sorted by battles with week counts within the total', () => {
		const file = fixtures.get('contributors.json') as Contributors;
		expect(file.contributors.length).toBeGreaterThan(0);
		file.contributors.forEach((c, i) => {
			if (i > 0) expect(c.battles).toBeLessThanOrEqual(file.contributors[i - 1].battles);
			expect(c.battlesWeek).toBeLessThanOrEqual(c.battles);
			expect(c.battles).toBeGreaterThan(0);
		});
		expect(new Set(file.contributors.map((c) => c.login)).size).toBe(file.contributors.length);
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

describe('archive', () => {
	it('lists every version of every bot, sorted by id, with the bot pages\' jars', () => {
		const file = fixtures.get('archive.json') as Archive;
		expect(file.bots.length).toBeGreaterThan(0);
		file.bots.forEach((b, i) => {
			if (i > 0) expect(b.id > file.bots[i - 1].id).toBe(true);
			const details = fixtures.bot(b.id);
			if (!details) return;
			expect(b.versions.map((v) => [v.version, v.sha256, v.size])).toEqual(
				details.versions.map((v) => [v.version, v.sha256, v.size])
			);
		});
	});
});

describe('recordings', () => {
	const top = (rumble: string) => (fixtures.get(`rankings/${rumble}.json`) as Rankings).bots;

	it('are consistent between the two bots of a 1v1 pairing', () => {
		let checked = 0;
		for (const row of top('1v1').slice(0, 10)) {
			const rec = fixtures.recordings('1v1', row.id);
			for (const pair of rec?.pairs ?? []) {
				const back = fixtures.recordings('1v1', pair.opp)!.pairs.find((p) => p.opp === row.id)!;
				expect(back.rounds.map((r) => r.slice(0, 4))).toEqual(pair.rounds.map((r) => r.slice(0, 4)));
				pair.rounds.forEach((r, i) => expect(back.rounds[i][4]).toBe(1 - r[4]));
				expect(back.oppSha16).toBe(rec!.sha16);
				expect(back.oppColors).toEqual(rec!.colors);
				checked++;
			}
		}
		expect(checked).toBeGreaterThan(0);
	});

	it('say in the bot file where there are some, and only there', () => {
		for (const row of top('1v1').slice(0, 60)) {
			const bot = fixtures.bot(row.id) as BotDetails;
			const rec = fixtures.recordings('1v1', row.id);
			const n = rec?.pairs.reduce((sum, p) => sum + p.rounds.length, 0) ?? 0;
			expect(bot.recordings?.['1v1']).toBe(n > 0 ? n : undefined);
			expect(fixtures.get(`rec/1v1/${row.id}.json`)).toEqual(rec);
		}
	});

	it('have eight distinct robots in every melee round, the bot among them', () => {
		let rounds = 0;
		for (const row of top('melee').slice(0, 50)) {
			const rec = fixtures.recordings('melee', row.id);
			for (const r of rec?.rounds ?? []) {
				const ids = r[4].map((i) => rec!.bots[i].bot);
				expect(new Set(ids).size).toBe(8);
				expect(ids).toContain(row.id);
				rounds++;
			}
		}
		expect(rounds).toBeGreaterThan(0);
		expect(fixtures.recordings('melee', top('melee').at(-1)!.id)).toBeUndefined();
	});
});
