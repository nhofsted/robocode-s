import { error, isHttpError } from '@sveltejs/kit';
import { loadIndex, loadMap, loadRankings } from '#lib/rumble/data.ts';
import type { StyleMap } from '#lib/rumble/types.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, rankings, map] = await Promise.all([
		loadIndex(fetch),
		loadRankings(fetch, params.rumble),
		// A rumble with too few bots has no map; the page says so.
		loadMap(fetch, params.rumble).catch((e: unknown): StyleMap | null => {
			if (isHttpError(e) && e.status === 404) return null;
			throw e;
		})
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	if (!summary) error(404, `No rumble ${params.rumble}.`);
	const siblings = index.rumbles.filter((r) => r.division === summary.division);
	return { summary, siblings, rankings, map };
};
