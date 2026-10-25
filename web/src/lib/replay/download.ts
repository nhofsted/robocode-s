// A track as the Download button saves it: with the battle it is a round of appended
// (crates/replay `with_battle`, docs/rumble-data.md "Downloaded tracks"), so that the
// desktop app can name its robots and run the battle again.
import type { RecordedBot } from '../rumble/types.ts';

/** The battle appended to a downloaded track. */
export interface TrackBattle {
	schema: 1;
	/** The engine version the battle ran with. */
	engine: number;
	/** The battle's seed (a decimal string, 64 bits). */
	seed: string;
	/** 0-based round within the battle. */
	round: number;
	rounds: number;
	width: number;
	height: number;
	/** In battle order: the track's robot i is robots[i]. */
	robots: RecordedBot[];
}

const MAGIC = new TextEncoder().encode('RSBATTLE');
/** Decoders read up to 8 bytes past a track's end as zeros; these keep the track as it was for them. */
const PADDING = 8;

/** `track`, 8 zero bytes, the battle as JSON, its length (u32, little-endian) and `RSBATTLE`. */
export function withBattle(track: Uint8Array, battle: TrackBattle): Uint8Array {
	const robots = battle.robots.map(({ bot, version, sha16 }) => ({ bot, version, sha16 }));
	const json = new TextEncoder().encode(JSON.stringify({ ...battle, robots }));
	const out = new Uint8Array(track.length + PADDING + json.length + 4 + MAGIC.length);
	out.set(track, 0);
	out.set(json, track.length + PADDING);
	new DataView(out.buffer).setUint32(track.length + PADDING + json.length, json.length, true);
	out.set(MAGIC, out.length - MAGIC.length);
	return out;
}

/**
 * Saves the track at `url` as `name` with `battle` appended. Without the bytes (a network
 * error), the plain track is saved instead.
 */
export async function downloadTrack(url: string, name: string, battle: TrackBattle): Promise<void> {
	let href = url;
	try {
		const response = await fetch(url);
		if (response.ok) {
			const track = new Uint8Array(await response.arrayBuffer());
			const blob = new Blob([withBattle(track, battle) as Uint8Array<ArrayBuffer>], {
				type: 'application/octet-stream'
			});
			href = URL.createObjectURL(blob);
		}
	} catch {
		// The plain track, from its link.
	}
	const a = document.createElement('a');
	a.href = href;
	a.download = name;
	document.body.append(a);
	a.click();
	a.remove();
	if (href !== url) setTimeout(() => URL.revokeObjectURL(href), 10_000);
}
