// Reading the recordings index (types.ts, docs/rumble-data.md): which tracks exist, where
// they are, and who fights in them. Pure functions; the fetching is in data.ts.

import { shortName } from './format.ts';
import type {
	BotId,
	RecordedBot,
	Recordings1v1,
	RecordingsMelee,
	RoundMelee
} from './types.ts';

/** One recorded round of a 1v1 pairing, seen from the pairing's A and B (the URL's order). */
export interface PairRound {
	/** The track's name without `.track`. */
	file: string;
	/** 0-based round within the battle. */
	round: number;
	seed: string;
	/** The track's bytes. */
	size: number;
	/** The bot that won the round. */
	winner: BotId;
	/** A's robot index in the track; B has the other. */
	aIndex: number;
}

export interface PairRounds {
	engine: number;
	a: RecordedBot;
	b: RecordedBot;
	/** By round, then file. */
	rounds: PairRound[];
}

const byRound = (x: { round: number; file: string }, y: { round: number; file: string }) =>
	x.round - y.round || (x.file < y.file ? -1 : x.file > y.file ? 1 : 0);

/**
 * The rounds of a and b out of a recordings file of either of them, or null if the file
 * has none for the pair.
 */
export function pairRounds(rec: Recordings1v1, a: BotId, b: BotId): PairRounds | null {
	const other = rec.bot === a ? b : rec.bot === b ? a : null;
	if (other === null) return null;
	const pair = rec.pairs.find((p) => p.opp === other);
	if (!pair || pair.rounds.length === 0) return null;
	const self: RecordedBot = { bot: rec.bot, version: rec.version, sha16: rec.sha16, colors: rec.colors };
	const opp: RecordedBot = { bot: pair.opp, version: pair.oppVersion, sha16: pair.oppSha16, colors: pair.oppColors };
	const [pa, pb] = rec.bot === a ? [self, opp] : [opp, self];
	const [first, second] = pa.bot < pb.bot ? [pa.bot, pb.bot] : [pb.bot, pa.bot];
	const rounds = pair.rounds.map(
		([file, round, seed, size, me]): PairRound => ({
			file,
			round,
			seed,
			size,
			winner: file.startsWith('a') ? first : second,
			aIndex: rec.bot === a ? me : 1 - me
		})
	);
	rounds.sort(byRound);
	return { engine: rec.engine, a: pa, b: pb, rounds };
}

const part = (x: RecordedBot) => `${x.bot}_${x.sha16}`;

/** `/rec/e<engine>/1v1/<A>/<B>/<file>.track`, A the bot whose id sorts first (byte order). */
export function trackUrl1v1(engine: number, x: RecordedBot, y: RecordedBot, file: string): string {
	const [first, second] = x.bot < y.bot ? [x, y] : [y, x];
	return `/rec/e${engine}/1v1/${part(first)}/${part(second)}/${file}.track`;
}

/** `/rec/e<engine>/melee/<bot>_<sha16>/<file>.track` */
export function trackUrlMelee(engine: number, bot: RecordedBot, file: string): string {
	return `/rec/e${engine}/melee/${part(bot)}/${file}.track`;
}

export interface MeleeRound {
	file: string;
	round: number;
	seed: string;
	size: number;
	won: boolean;
	/** The track's robots, by robot index. */
	robots: RecordedBot[];
}

export function meleeRound(rec: RecordingsMelee, r: RoundMelee): MeleeRound {
	const [file, round, seed, size, order] = r;
	return { file, round, seed, size, won: file.startsWith('w'), robots: order.map((i) => rec.bots[i]) };
}

/** The rounds of a melee index by round, then file. */
export function meleeRounds(rec: RecordingsMelee): MeleeRound[] {
	return rec.rounds.map((r) => meleeRound(rec, r)).sort(byRound);
}

/** Short names for a replay, the full id for those that would collide. */
export function displayNames(ids: readonly BotId[]): string[] {
	const count = new Map<string, number>();
	for (const id of ids) count.set(shortName(id), (count.get(shortName(id)) ?? 0) + 1);
	return ids.map((id) => ((count.get(shortName(id)) ?? 0) > 1 ? id : shortName(id)));
}

/** Rounds a bot won and lost against each opponent, from its 1v1 index. */
export function opponentTallies(rec: Recordings1v1) {
	return rec.pairs.map((p) => {
		const iAmFirst = rec.bot < p.opp;
		const won = p.rounds.filter(([file]) => file.startsWith('a') === iAmFirst).length;
		return { opponent: p.opp, version: p.oppVersion, colors: p.oppColors, won, lost: p.rounds.length - won };
	});
}
