import { error } from '@sveltejs/kit';
import { loadIndex, loadRecordings } from '#lib/rumble/data.ts';
import { meleeRounds } from '#lib/rumble/recordings.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, params }) => {
	const [index, rec] = await Promise.all([loadIndex(fetch), loadRecordings(fetch, 'melee', params.bot)]);
	const summary = index.rumbles.find((r) => r.id === params.rumble);
	if (!summary || summary.division !== 'melee') error(404, `${params.rumble} has no melee replays.`);
	const rounds = meleeRounds(rec);
	const at = rounds.findIndex((r) => r.file === params.file);
	if (at < 0) error(404, `${params.bot} has no recorded melee round "${params.file}".`);
	return { summary, rec, rounds, at };
};
