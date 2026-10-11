// Loads the rumble data files (types.ts) from /data/v1, or from VITE_RUMBLE_DATA if set.

import { error } from '@sveltejs/kit';
import type { BotDetails, BotId, Rankings, RumbleId, RumbleIndex } from './types.ts';

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

export const loadRankings = (fetch: Fetch, rumble: RumbleId) =>
	load<Rankings>(fetch, `rankings/${encodeURIComponent(rumble)}.json`, `rumble "${rumble}"`);

export const loadBot = (fetch: Fetch, id: BotId) =>
	load<BotDetails>(fetch, `bots/${encodeURIComponent(id)}.json`, `bot "${id}"`);
