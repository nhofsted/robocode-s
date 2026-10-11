import { error } from '@sveltejs/kit';
import { loadBot, loadIndex, loadRankings } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, rankings, a, b] = await Promise.all([
		loadIndex(fetch),
		loadRankings(fetch, params.rumble),
		loadBot(fetch, params.a),
		loadBot(fetch, params.b)
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	const ea = a.rumbles[params.rumble];
	const eb = b.rumbles[params.rumble];
	if (!summary || !ea || !eb) error(404, `${params.a} and ${params.b} aren't both in ${params.rumble}.`);
	return { summary, rankings, a, b, ea, eb };
};
