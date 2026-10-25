import { describe, expect, it } from 'vitest';
import {
	displayNames,
	meleeRounds,
	opponentTallies,
	pairRounds,
	trackUrl1v1,
	trackUrlMelee
} from './recordings.ts';
import type { Recordings1v1, RecordingsMelee } from './types.ts';

const walls = { bot: 'sample.Walls', version: '1.0', sha16: '0123456789abcdef' };
const crazy = { bot: 'sample.Crazy', version: '1.2', sha16: 'fedcba9876543210' };

describe('track URLs', () => {
	it('puts the bot whose id sorts first first, whichever way they are given', () => {
		const want = '/rec/e7/1v1/sample.Crazy_fedcba9876543210/sample.Walls_0123456789abcdef/a-0.track';
		expect(trackUrl1v1(7, walls, crazy, 'a-0')).toBe(want);
		expect(trackUrl1v1(7, crazy, walls, 'a-0')).toBe(want);
	});

	it('sorts by bytes: upper case before lower case', () => {
		const x = { bot: 'a.zed', version: '1', sha16: '1111111111111111' };
		const y = { bot: 'B.alpha', version: '1', sha16: '2222222222222222' };
		expect(trackUrl1v1(1, x, y, 'b-3')).toBe(
			'/rec/e1/1v1/B.alpha_2222222222222222/a.zed_1111111111111111/b-3.track'
		);
	});

	it('names a melee track by the bot, its hash prefix and the engine', () => {
		expect(trackUrlMelee(12, walls, 'w-4')).toBe(
			'/rec/e12/melee/sample.Walls_0123456789abcdef/w-4.track'
		);
	});
});

const rec: Recordings1v1 = {
	schema: 1,
	engine: 7,
	...walls,
	pairs: [
		{
			opp: crazy.bot,
			oppVersion: crazy.version,
			oppSha16: crazy.sha16,
			// Crazy sorts first: `a` is a Crazy win. Walls is robot 0 in the first, 1 in the others.
			rounds: [
				['a-0', 3, '5', 100, 0],
				['b-1', 1, '6', 200, 1],
				['b-2', 9, '7', 300, 1]
			]
		}
	]
};

describe('pairRounds', () => {
	it('reads the file of either bot and orders by round', () => {
		const fromWalls = pairRounds(rec, walls.bot, crazy.bot)!;
		expect(fromWalls.a.bot).toBe(walls.bot);
		expect(fromWalls.rounds.map((r) => r.file)).toEqual(['b-1', 'a-0', 'b-2']);
		expect(fromWalls.rounds.map((r) => r.winner)).toEqual([walls.bot, crazy.bot, walls.bot]);
		expect(fromWalls.rounds.map((r) => r.aIndex)).toEqual([1, 0, 1]);

		const swapped = pairRounds(rec, crazy.bot, walls.bot)!;
		expect(swapped.a).toEqual(fromWalls.b);
		expect(swapped.rounds.map((r) => r.aIndex)).toEqual([0, 1, 0]);
		expect(swapped.rounds.map((r) => r.winner)).toEqual([walls.bot, crazy.bot, walls.bot]);
	});

	it('is null for a stranger or a pair without rounds', () => {
		expect(pairRounds(rec, 'x.Y', crazy.bot)).toBeNull();
		expect(pairRounds(rec, walls.bot, 'x.Y')).toBeNull();
	});

	it('carries the colours the file knows', () => {
		const colored: Recordings1v1 = {
			...rec,
			colors: { body: '#000000' },
			pairs: [{ ...rec.pairs[0], oppColors: { gun: '#ff0000' } }]
		};
		const found = pairRounds(colored, crazy.bot, walls.bot)!;
		expect(found.a.colors).toEqual({ gun: '#ff0000' });
		expect(found.b.colors).toEqual({ body: '#000000' });
		expect(opponentTallies(colored)[0].colors).toEqual({ gun: '#ff0000' });
		expect(pairRounds(rec, crazy.bot, walls.bot)!.a.colors).toBeUndefined();
	});

	it('tallies wins and losses per opponent', () => {
		expect(opponentTallies(rec)).toEqual([
			{ opponent: crazy.bot, version: '1.2', won: 2, lost: 1 }
		]);
	});
});

describe('melee rounds', () => {
	const melee: RecordingsMelee = {
		schema: 1,
		engine: 7,
		...walls,
		bots: [walls, crazy],
		rounds: [
			['l-3', 9, '17', 3599, [0, 1]],
			['w-0', 2, '6', 3600, [1, 0]]
		]
	};

	it('maps the robot order to bots and sorts by round', () => {
		const rounds = meleeRounds(melee);
		expect(rounds.map((r) => [r.file, r.won])).toEqual([
			['w-0', true],
			['l-3', false]
		]);
		expect(rounds[0].robots.map((b) => b.bot)).toEqual([crazy.bot, walls.bot]);
	});
});

describe('displayNames', () => {
	it('uses the short name unless two would be alike', () => {
		expect(displayNames(['a.Fire', 'b.Walls'])).toEqual(['Fire', 'Walls']);
		expect(displayNames(['a.Fire', 'b.Fire', 'c.Walls'])).toEqual(['a.Fire', 'b.Fire', 'Walls']);
	});
});
