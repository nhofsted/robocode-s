import { error } from '@sveltejs/kit';
import { loadIndex, loadPairRounds } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, found] = await Promise.all([
		loadIndex(fetch),
		loadPairRounds(fetch, params.a, params.b, [params.a, params.b])
	]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	if (!summary || summary.division !== '1v1') error(404, `${params.rumble} has no 1v1 replays.`);
	if (!found) {
		error(404, `No rounds of ${params.a} against ${params.b} are recorded. Slots fill as battles are run.`);
	}
	const at = found.rounds.findIndex((r) => r.file === params.file);
	if (at < 0) error(404, `${params.a} against ${params.b} has no recorded round "${params.file}".`);
	return { summary, found, at };
};
