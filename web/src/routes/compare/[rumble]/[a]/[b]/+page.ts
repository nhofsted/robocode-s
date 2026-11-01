import { error } from '@sveltejs/kit';
import { loadBot, loadIndex, loadRankings, loadVersion } from '#lib/rumble/data.ts';
import type { BotColors, BotDetails, RumbleId } from '#lib/rumble/types.ts';
import type { PageLoad } from './$types';

/** One side of a comparison: a bot's ranked version, or (`?b=<version>`) an earlier one. */
export interface Side {
	bot: BotDetails;
	version: string;
	colors: BotColors | null | undefined;
	/** Rank in the rumble; null for an earlier version (not ranked). */
	rank: number | null;
	/** The ranked APS, or an earlier version's mean score against today's field. */
	aps: number;
	retiredAt: string | null;
	pairings: { opponent: string; opponentVersion: string; score: number; survival: number }[];
}

function ranked(bot: BotDetails, rumble: RumbleId): Side {
	const entry = bot.rumbles[rumble];
	if (!entry) error(404, `${bot.id} isn't in ${rumble}.`);
	return {
		bot,
		version: entry.history.at(-1)?.version ?? bot.versions[0].version,
		colors: bot.versions[0]?.colors,
		rank: entry.rank,
		aps: entry.aps,
		retiredAt: null,
		pairings: entry.pairings
	};
}

export const load: PageLoad = async ({ fetch, params, url }) => {
	const [index, rankings, a, b] = await Promise.all([
		loadIndex(fetch),
		loadRankings(fetch, params.rumble),
		loadBot(fetch, params.a),
		params.b === params.a ? null : loadBot(fetch, params.b)
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	if (!summary) error(404, `No rumble ${params.rumble}.`);
	const botB = b ?? a;
	const wanted = url.searchParams.get('b');
	let sideB: Side;
	if (wanted === null) {
		if (botB.id === a.id) error(404, 'Pick another bot, or one of its earlier versions.');
		sideB = ranked(botB, params.rumble);
	} else {
		const v = botB.versions.find((v) => v.version === wanted);
		if (!v || !v.shared?.[params.rumble]) error(404, `${botB.id} ${wanted} has no pairings to compare in ${params.rumble}.`);
		const details = await loadVersion(fetch, botB.id, v.sha256);
		const entry = details.rumbles[params.rumble];
		if (!entry) error(404, `${botB.id} ${wanted} has no pairings to compare in ${params.rumble}.`);
		sideB = {
			bot: botB,
			version: v.version,
			colors: v.colors,
			rank: null,
			aps: entry.aps,
			retiredAt: v.retiredAt,
			pairings: entry.pairings
		};
	}
	return { summary, rankings, a: ranked(a, params.rumble), b: sideB };
};
