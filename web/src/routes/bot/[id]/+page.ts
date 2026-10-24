import { error } from '@sveltejs/kit';
import { loadBot, loadIndex, loadRankings, loadRecordings } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params, url }) => {
	const [bot, index] = await Promise.all([loadBot(fetch, params.id), loadIndex(fetch)]);
	const joined = index.rumbles.filter((r) => r.id in bot.rumbles);
	if (joined.length === 0) error(404, `${params.id} is not ranked in any rumble.`);
	const wanted = url.searchParams.get('rumble');
	const summary = joined.find((r) => r.id === wanted) ?? joined[0];
	// The rankings give opponents' ranks; the browser caches them across bot pages.
	const rankings = await loadRankings(fetch, summary.id);
	// The recorded rounds of the rumble's division, where the bot file says there are some; the
	// page shows the rest while they load.
	const division = summary.division;
	const has = (bot.recordings?.[division] ?? 0) > 0;
	const replays =
		has && division === '1v1'
			? loadRecordings(fetch, '1v1', bot.id).catch(() => null)
			: has && division === 'melee'
				? loadRecordings(fetch, 'melee', bot.id).catch(() => null)
				: Promise.resolve(null);
	return { bot, joined, summary, rankings, replays };
};
