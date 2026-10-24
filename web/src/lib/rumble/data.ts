// Loads the rumble data files (types.ts) from /data/v1, or from VITE_RUMBLE_DATA if set.

import { error, isHttpError } from '@sveltejs/kit';
import type {
	Archive,
	BotDetails,
	BotId,
	Contributors,
	Rankings,
	Recordings1v1,
	RecordingsMelee,
	RumbleId,
	RumbleIndex
} from './types.ts';
import { pairRounds, type PairRounds } from './recordings.ts';

type Fetch = typeof fetch;

export const DATA_BASE: string = import.meta.env.VITE_RUMBLE_DATA ?? '/data/v1';

async function load<T>(fetch: Fetch, path: string, what: string): Promise<T> {
	let response: Response;
	try {
		response = await fetch(`${DATA_BASE}/${path}`);
	} catch {
		error(503, `Could not load ${what}. Check your connection and try again.`);
	}
	if (response.status === 404 || response.status === 403) error(404, `No ${what} found.`);
	if (!response.ok) error(502, `Loading ${what} failed (HTTP ${response.status}).`);
	return (await response.json()) as T;
}

export const loadIndex = (fetch: Fetch) => load<RumbleIndex>(fetch, 'rumbles.json', 'rumbles');

export const loadContributors = (fetch: Fetch) =>
	load<Contributors>(fetch, 'contributors.json', 'contributors');

export const loadRankings = (fetch: Fetch, rumble: RumbleId) =>
	load<Rankings>(fetch, `rankings/${encodeURIComponent(rumble)}.json`, `rumble "${rumble}"`);

export const loadBot = (fetch: Fetch, id: BotId) =>
	load<BotDetails>(fetch, `bots/${encodeURIComponent(id)}.json`, `bot "${id}"`);

export const loadArchive = (fetch: Fetch) => load<Archive>(fetch, 'archive.json', 'the archive');

/** The recordings index of a bot's current version (types.ts); a 404 error where it has none. */
export function loadRecordings(fetch: Fetch, division: '1v1', bot: BotId): Promise<Recordings1v1>;
export function loadRecordings(fetch: Fetch, division: 'melee', bot: BotId): Promise<RecordingsMelee>;
export function loadRecordings(fetch: Fetch, division: string, bot: BotId) {
	return load<Recordings1v1 | RecordingsMelee>(
		fetch,
		`rec/${division}/${encodeURIComponent(bot)}.json`,
		`replays of "${bot}"`
	);
}

/**
 * The recorded rounds of a 1v1 pairing, from the first of `from` (a or b) whose index has them,
 * or null. The tracks are in both bots' indexes.
 */
export async function loadPairRounds(
	fetch: Fetch,
	a: BotId,
	b: BotId,
	from: readonly BotId[]
): Promise<PairRounds | null> {
	for (const id of from) {
		try {
			const found = pairRounds(await loadRecordings(fetch, '1v1', id), a, b);
			if (found) return found;
		} catch (e) {
			if (!isHttpError(e) || e.status !== 404) throw e;
		}
	}
	return null;
}
