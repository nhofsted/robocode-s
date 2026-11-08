import { isHttpError } from '@sveltejs/kit';
import { describe, expect, it } from 'vitest';
import { DATA_BASE } from '#lib/rumble/data.ts';
import { createFixtures } from '#lib/rumble/fixtures.ts';
import type { BotDetails } from '#lib/rumble/types.ts';
import { load } from './+page.ts';

const fixtures = createFixtures({ seed: 42, bots: 120 });

/** `fetch` over the fixtures, as `vite dev` serves them at /data/v1. */
const fetchData = (async (input: RequestInfo | URL) => {
	const path = String(input).slice(DATA_BASE.length + 1);
	const value = fixtures.get(path);
	return value === undefined
		? new Response('not found', { status: 404 })
		: new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } });
}) as typeof fetch;

type Data = Exclude<Awaited<ReturnType<typeof load>>, void>;

function compare(rumble: string, a: string, b: string, query = ''): Promise<Data> {
	return load({
		fetch: fetchData,
		params: { rumble, a, b },
		url: new URL(`https://site/compare/${rumble}/${a}/${b}${query}`)
	} as unknown as Parameters<typeof load>[0]) as Promise<Data>;
}

async function status(promise: Promise<unknown>): Promise<number> {
	try {
		await promise;
	} catch (e) {
		if (isHttpError(e)) return e.status;
		throw e;
	}
	return 200;
}

const rows = fixtures.rankings('1v1')!.bots;
const [first, second] = [rows[0].id, rows[1].id];
/** A ranked 1v1 bot whose previous version has pairings to compare. */
const withEarlier = rows
	.map((r) => fixtures.bot(r.id) as BotDetails)
	.find((bot) => (bot.versions[1]?.shared?.['1v1'] ?? 0) > 0)!;

describe('compare loader', () => {
	it('compares two ranked bots by their rankings', async () => {
		const data = await compare('1v1', first, second);
		expect([data.a.bot.id, data.b.bot.id]).toEqual([first, second]);
		expect([data.a.rank, data.b.rank]).toEqual([1, 2]);
		expect(data.a.aps).toBe(rows[0].aps);
		expect(data.b.retiredAt).toBeNull();
		expect(data.a.pairings.length).toBeGreaterThan(0);
		expect(data.summary.id).toBe('1v1');
	});

	it('compares a bot with an earlier version of its own', async () => {
		expect(withEarlier).toBeDefined();
		const earlier = withEarlier.versions[1];
		const data = await compare('1v1', withEarlier.id, withEarlier.id, `?b=${earlier.version}`);
		expect(data.a.bot.id).toBe(withEarlier.id);
		expect(data.a.version).not.toBe(earlier.version);
		expect(data.b.version).toBe(earlier.version);
		expect(data.b.rank).toBeNull();
		expect(data.b.pairings.length).toBe(earlier.shared?.['1v1']);
	});

	it('refuses what it cannot compare', async () => {
		// The same bot without an earlier version picked.
		expect(await status(compare('1v1', first, first))).toBe(404);
		// A version the bot doesn't have, or one without pairings to compare.
		expect(await status(compare('1v1', first, first, '?b=0.0-none'))).toBe(404);
		const current = (fixtures.bot(first) as BotDetails).versions[0].version;
		expect(await status(compare('1v1', first, first, `?b=${current}`))).toBe(404);
		// An unknown rumble, or a bot that isn't there.
		expect(await status(compare('2v2', first, second))).toBe(404);
		expect(await status(compare('1v1', first, 'no.such.Bot'))).toBe(404);
	});
});
