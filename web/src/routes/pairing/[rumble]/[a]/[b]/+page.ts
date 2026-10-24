import { error } from '@sveltejs/kit';
import { loadBot, loadIndex, loadPairRounds } from '#lib/rumble/data.ts';
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
	// Recorded rounds (1v1 only), fetched only where a bot file says there are some; the page
	// shows the rest while they load.
	const holders = summary.division === '1v1' ? [a, b].filter((x) => (x.recordings?.['1v1'] ?? 0) > 0) : [];
	const rounds =
		holders.length > 0
			? loadPairRounds(fetch, a.id, b.id, holders.map((x) => x.id)).catch(() => null)
			: Promise.resolve(null);
	return {
		summary,
		rounds,
		a,
		b,
		ea,
		eb,
		pa: ea.pairings.find((p) => p.opponent === b.id) ?? null,
		pb: eb.pairings.find((p) => p.opponent === a.id) ?? null
	};
};
