import { loadIndex, loadRankings } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, rankings] = await Promise.all([
		loadIndex(fetch),
		loadRankings(fetch, params.rumble)
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	const siblings = summary ? index.rumbles.filter((r) => r.division === summary.division) : [];
	return { rankings, summary, siblings };
};
