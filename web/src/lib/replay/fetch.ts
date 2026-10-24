// Fetches a rumble track from /rec (same origin, immutable) and decodes it.
import { loadTrack, type ReplayTrack } from './track.ts';

/** Why a track could not be had: it is not (or no longer) there, it is not a track, or the network failed. */
export class TrackError extends Error {
	constructor(
		readonly kind: 'missing' | 'invalid' | 'network',
		message: string
	) {
		super(message);
	}
}

export async function fetchTrack(url: string): Promise<ReplayTrack> {
	let response: Response;
	try {
		response = await fetch(url);
	} catch {
		throw new TrackError('network', 'Could not load the replay. Check your connection and try again.');
	}
	// The CDN answers unknown paths with the site's fallback page, so HTML means missing too.
	const html = response.headers.get('content-type')?.includes('text/html') ?? false;
	if (response.status === 404 || response.status === 403 || (response.ok && html)) {
		throw new TrackError('missing', 'This recording is gone.');
	}
	if (!response.ok) throw new TrackError('network', `Loading the replay failed (HTTP ${response.status}).`);
	let track: ReplayTrack;
	try {
		track = await loadTrack(new Uint8Array(await response.arrayBuffer()));
	} catch {
		throw new TrackError('invalid', 'This file is not a replay track.');
	}
	if (track.frameCount === 0 || track.robotCount === 0) {
		throw new TrackError('invalid', 'This track has no turns.');
	}
	return track;
}
