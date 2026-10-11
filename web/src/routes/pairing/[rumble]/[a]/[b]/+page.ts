import { error } from '@sveltejs/kit';
import { loadBot, loadIndex } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, a, b] = await Promise.all([
		loadIndex(fetch),
		loadBot(fetch, params.a),
		loadBot(fetch, params.b)
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	const ea = a.rumbles[params.rumble];
	const eb = b.rumbles[params.rumble];
	if (!summary || !ea || !eb) error(404, `${params.a} and ${params.b} don't meet in ${params.rumble}.`);
	return {
		summary,
		a,
		b,
		ea,
		eb,
		pa: ea.pairings.find((p) => p.opponent === b.id) ?? null,
		pb: eb.pairings.find((p) => p.opponent === a.id) ?? null
	};
};
