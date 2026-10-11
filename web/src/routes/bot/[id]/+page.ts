import { error } from '@sveltejs/kit';
import { loadBot, loadIndex, loadRankings } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params, url }) => {
	const [bot, index] = await Promise.all([loadBot(fetch, params.id), loadIndex(fetch)]);
	const joined = index.rumbles.filter((r) => r.id in bot.rumbles);
	if (joined.length === 0) error(404, `${params.id} is not ranked in any rumble.`);
	const wanted = url.searchParams.get('rumble');
	const summary = joined.find((r) => r.id === wanted) ?? joined[0];
	// The rankings give opponents' ranks; the browser caches them across bot pages.
	const rankings = await loadRankings(fetch, summary.id);
	return { bot, joined, summary, rankings };
};
