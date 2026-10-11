// The rumble data contract, version 1: the JSON files the rankings Lambda writes to S3 once
// a day, and the website reads (docs/rumble-data.md). This file is the source of truth for the
// field semantics; the Rust side mirrors it.
//
// Conventions: percentages are numbers in 0–100, timestamps are ISO 8601 UTC strings,
// dates are YYYY-MM-DD. Fields are only ever added within a version; anything else bumps
// `schema` and the URL prefix (/data/v1 → /data/v2).

export const SCHEMA_VERSION = 1;

export type Division = '1v1' | 'melee' | 'teams' | 'twinduel';

/** Codesize classes as in classic RoboRumble; `mega` has no limit. */
export type WeightClass = 'mega' | 'mini' | 'micro' | 'nano';

/** Rumble id: the division, plus `-<class>` for the weight-class rumbles (e.g. `1v1-micro`). */
export type RumbleId = string;

/** Bot id: the robot's fully qualified class name without version (e.g. `sample.Walls`). */
export type BotId = string;

/** `/data/v1/rumbles.json` */
export interface RumbleIndex {
	schema: typeof SCHEMA_VERSION;
	generatedAt: string;
	rumbles: RumbleSummary[];
}

export interface RumbleSummary {
	id: RumbleId;
	division: Division;
	weightClass: WeightClass;
	/** Bots currently ranked. */
	bots: number;
	/** Battles that count towards the current rankings. */
	battles: number;
	rounds: number;
	battlefield: { width: number; height: number };
}

/** The ranking metrics, all derived from the pairing statistics (design §12.6). */
export interface Metrics {
	/** Average percent score: the mean, over all pairings, of this bot's share of the total score. */
	aps: number;
	/** Percentage of pairings won (pairing score above 50%; exactly 50% counts half). */
	pwin: number;
	/** Average percentage of rounds survived. */
	survival: number;
	/** Glicko-2 rating. */
	glicko: number;
	/** Glicko-2 rating deviation. */
	glickoRd: number;
	/**
	 * Average normalised percentage pairs: per opponent, this bot's pairing score scaled
	 * between the lowest (0) and highest (100) score any bot got against that opponent.
	 */
	anpp: number;
	/**
	 * Every bot votes for the opponent that scores best against it; this is the share of all
	 * votes.
	 */
	vote: number;
}

/** `/data/v1/rankings/<rumble>.json` */
export interface Rankings {
	schema: typeof SCHEMA_VERSION;
	rumble: RumbleId;
	generatedAt: string;
	/** Sorted by APS, best first. */
	bots: RankingRow[];
}

export interface RankingRow extends Metrics {
	id: BotId;
	version: string;
	author: string;
	/** Rank by APS, 1-based. */
	rank: number;
	/** Opponents this bot has fought at least once. */
	pairings: number;
	battles: number;
	/** Rank by APS one week ago; null for bots that were not ranked then. */
	rankWeekAgo: number | null;
}

/** `/data/v1/bots/<id>.json` */
export interface BotDetails {
	schema: typeof SCHEMA_VERSION;
	id: BotId;
	author: Author;
	/** Robot description from the jar's .properties, if any. */
	description: string | null;
	/** Author-provided link, e.g. a RoboWiki page. */
	homepage: string | null;
	/** Newest first; the first entry is the current version. */
	versions: BotVersion[];
	/** Only the rumbles this bot takes part in. */
	rumbles: Record<RumbleId, BotRumble>;
}

export interface Author {
	name: string;
	/** GitHub login, once the author has claimed the bot. */
	github: string | null;
}

export interface BotVersion {
	version: string;
	/** Codesize in bytes (Java robots), null where not defined. */
	codesize: number | null;
	weightClass: WeightClass;
	/** SHA-256 of the robot package, hex. */
	sha256: string;
	addedAt: string;
	/** Set when the version was replaced or retired. */
	retiredAt: string | null;
}

export interface BotRumble extends Metrics {
	rank: number;
	battles: number;
	pairings: Pairing[];
	/**
	 * Snapshots since the bot's first version, oldest first, across all its versions (design
	 * §12.6): daily for the last 365 days, weekly (every 7th day) before that. `version` tells
	 * which version was ranked on that day.
	 */
	history: HistoryPoint[];
}

export interface Pairing {
	opponent: BotId;
	opponentVersion: string;
	battles: number;
	/** This bot's average percent score against the opponent. */
	score: number;
	/** Average percentage of rounds this bot survived. */
	survival: number;
	/** The pairing score normalised against all bots' scores vs this opponent (see `anpp`). */
	normalized: number;
}

export interface HistoryPoint {
	date: string;
	/** The version ranked on that day. */
	version: string;
	rank: number;
	aps: number;
	glicko: number;
}
