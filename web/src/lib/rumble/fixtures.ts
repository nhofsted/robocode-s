// Synthetic rumble data in the v1 contract (types.ts), for developing the website before the
// rankings Lambda exists. Seeded, so the same seed always gives the same data. Served at
// /data/v1 by the dev and preview servers (vite.config.ts); never part of a build.
//
// Model: each bot has a skill and a "style" angle; the pairing score of A against B is a
// logistic function of the skill difference plus a non-transitive style term, so rankings
// look plausible and pairing tables have upsets.

import {
	SCHEMA_VERSION,
	type BotDetails,
	type BotId,
	type BotRumble,
	type BotVersion,
	type Division,
	type HistoryPoint,
	type Metrics,
	type Pairing,
	type RankingRow,
	type Rankings,
	type RumbleId,
	type RumbleIndex,
	type RumbleSummary,
	type WeightClass
} from './types.ts';

export interface FixtureOptions {
	seed?: number;
	/** The "current" time of the data set. */
	now?: Date;
	/** Size of the 1v1 division; the other divisions scale with it. */
	bots?: number;
}

/** The fixture data set, addressed by path relative to /data/v1 (e.g. `rankings/1v1.json`). */
export interface Fixtures {
	get(path: string): unknown | undefined;
	index(): RumbleIndex;
	rankings(rumble: RumbleId): Rankings | undefined;
	bot(id: BotId): BotDetails | undefined;
}

interface Bot {
	id: BotId;
	author: string;
	skill: number;
	style: number;
	versions: BotVersion[];
	weightClass: WeightClass;
	divisions: Division[];
	/** Fraction of possible opponents fought; below 1 for bots that are still new. */
	coverage: number;
	addedDay: number;
}

interface RumbleState {
	summary: RumbleSummary;
	members: Bot[];
	rows: RankingRow[];
	details: Map<BotId, BotRumble>;
}

const DIVISIONS: { division: Division; classes: WeightClass[]; rounds: number; field: number }[] = [
	{ division: '1v1', classes: ['mega', 'mini', 'micro', 'nano'], rounds: 35, field: 800 },
	{ division: 'melee', classes: ['mega', 'mini', 'micro', 'nano'], rounds: 35, field: 1000 },
	{ division: 'teams', classes: ['mega'], rounds: 10, field: 1200 },
	{ division: 'twinduel', classes: ['mega'], rounds: 75, field: 800 }
];

const CLASS_ORDER: WeightClass[] = ['nano', 'micro', 'mini', 'mega'];

/** Codesize limits as in classic RoboRumble (exclusive upper bounds). */
const CLASS_LIMIT: Record<WeightClass, number> = {
	nano: 250,
	micro: 750,
	mini: 1500,
	mega: Infinity
};

const HISTORY_DAYS = 120;

const SAMPLES = [
	'sample.Corners',
	'sample.Crazy',
	'sample.Fire',
	'sample.RamFire',
	'sample.SpinBot',
	'sample.Tracker',
	'sample.TrackFire',
	'sample.Walls',
	'sample.VelociRobot',
	'sampleex.Alien'
];

const PREFIXES = ['ax', 'bk', 'cz', 'dv', 'eo', 'fj', 'gm', 'hq', 'ir', 'jt', 'kw', 'lx', 'mn', 'ny', 'op', 'pr', 'qs', 'rt', 'su', 'tv', 'uw', 'vx', 'wy', 'xz', 'yb', 'zc'];
const PACKAGE_WORDS = ['mega', 'mini', 'micro', 'nano', 'bots', 'tank', 'rumble', 'arena', 'lab', 'core', 'zone', 'kit'];
const NAME_FIRST = ['Iron', 'Silent', 'Crimson', 'Rapid', 'Lunar', 'Neon', 'Ghost', 'Thunder', 'Quantum', 'Shadow', 'Pixel', 'Wobbly', 'Static', 'Hollow', 'Brass', 'Frost', 'Cobalt', 'Feral', 'Lucky', 'Glass', 'Sly', 'Red', 'Grey', 'Tiny'];
const NAME_SECOND = ['Viper', 'Comet', 'Tortoise', 'Mirage', 'Hornet', 'Bastion', 'Specter', 'Lancer', 'Gecko', 'Raven', 'Golem', 'Wisp', 'Falcon', 'Badger', 'Kestrel', 'Puma', 'Orbit', 'Sentry', 'Drifter', 'Moth', 'Pike', 'Yeti', 'Beetle', 'Nomad'];
const TEAM_SUFFIX = ['Team', 'Squad', 'Pack', 'Crew'];
const AUTHOR_FIRST = ['quiet', 'rusty', 'pixel', 'bit', 'lazy', 'neon', 'arc', 'orbit', 'tiny', 'grumpy', 'swift', 'night'];
const AUTHOR_SECOND = ['coder', 'gunner', 'wave', 'tank', 'robo', 'owl', 'fox', 'shark', 'surfer', 'smith'];

export function createFixtures(options: FixtureOptions = {}): Fixtures {
	const seed = options.seed ?? 1;
	const now = options.now ?? new Date('2026-10-11T06:00:00Z');
	const size = options.bots ?? 600;
	const rand = mulberry32(seed);
	const bots = createBots(rand, size, now);
	const byId = new Map(bots.map((b) => [b.id, b]));

	const rumbles = new Map<RumbleId, RumbleState>();
	for (const d of DIVISIONS) {
		for (const weightClass of d.classes) {
			const id = weightClass === 'mega' ? d.division : `${d.division}-${weightClass}`;
			const members = bots.filter(
				(b) => b.divisions.includes(d.division) && fitsClass(b.weightClass, weightClass)
			);
			rumbles.set(id, buildRumble(id, d.division, weightClass, d, members, seed, now));
		}
	}

	const generatedAt = now.toISOString();

	const index = (): RumbleIndex => ({
		schema: SCHEMA_VERSION,
		generatedAt,
		rumbles: [...rumbles.values()].map((r) => r.summary)
	});

	const rankings = (rumble: RumbleId): Rankings | undefined => {
		const r = rumbles.get(rumble);
		if (!r) return undefined;
		return {
			schema: SCHEMA_VERSION,
			rumble,
			generatedAt,
			fullMetricsAt: isoDay(now, 0) + 'T03:00:00.000Z',
			bots: r.rows
		};
	};

	const bot = (id: BotId): BotDetails | undefined => {
		const b = byId.get(id);
		if (!b) return undefined;
		const entries: Record<RumbleId, BotRumble> = {};
		for (const [rid, r] of rumbles) {
			const d = r.details.get(id);
			if (d) entries[rid] = d;
		}
		return {
			schema: SCHEMA_VERSION,
			id,
			author: { name: b.author, github: hash(id) % 3 === 0 ? b.author : null },
			description: hash(id) % 4 === 0 ? null : `${shortName(id)}: a ${b.weightClass} bot.`,
			homepage: hash(id) % 2 === 0 ? `https://robowiki.net/wiki/${shortName(id)}` : null,
			versions: b.versions,
			rumbles: entries
		};
	};

	const get = (path: string): unknown | undefined => {
		const clean = path.replace(/^\/+/, '');
		if (clean === 'rumbles.json') return index();
		let m = /^rankings\/([^/]+)\.json$/.exec(clean);
		if (m) return rankings(decodeURIComponent(m[1]));
		m = /^bots\/([^/]+)\.json$/.exec(clean);
		if (m) return bot(decodeURIComponent(m[1]));
		return undefined;
	};

	return { get, index, rankings, bot };
}

function createBots(rand: () => number, size: number, now: Date): Bot[] {
	const used = new Set<string>();
	const bots: Bot[] = [];
	const authors = Array.from({ length: Math.ceil(size / 3) }, (_, i) => {
		const a = pick(rand, AUTHOR_FIRST) + pick(rand, AUTHOR_SECOND);
		return i % 5 === 0 ? a : a + Math.floor(rand() * 100);
	});

	const add = (id: string, author: string, divisions: Division[], codesize: number, strength: number) => {
		if (used.has(id)) return;
		used.add(id);
		const versions = createVersions(rand, id, codesize, now);
		const addedDay = Math.round(daysBetween(new Date(versions[0].addedAt), now));
		bots.push({
			id,
			author,
			skill: strength,
			style: rand() * Math.PI * 2,
			versions,
			weightClass: classOf(codesize),
			divisions,
			coverage: addedDay < 3 ? 0.2 + rand() * 0.6 : 1,
			addedDay
		});
	};

	SAMPLES.forEach((id, i) =>
		add(id, 'Mathew A. Nelson', ['1v1', 'melee'], 300 + i * 90, -2.2 + rand() * 0.6)
	);

	while (bots.length < size) {
		const pkg = `${pick(rand, PREFIXES)}${rand() < 0.4 ? '.' + pick(rand, PACKAGE_WORDS) : ''}`;
		const id = `${pkg}.${pick(rand, NAME_FIRST)}${pick(rand, NAME_SECOND)}`;
		const codesize = randomCodesize(rand);
		const divisions: Division[] = ['1v1'];
		if (rand() < 0.35) divisions.push('melee');
		add(id, pick(rand, authors), divisions, codesize, gaussian(rand) + (codesize > 1500 ? 0.4 : 0));
	}

	// Melee-only bots, and team bots (teams are separate robots in classic: a .team file).
	const extra = Math.round(size / 8);
	for (let i = 0; i < extra; i++) {
		const id = `${pick(rand, PREFIXES)}.melee.${pick(rand, NAME_FIRST)}${pick(rand, NAME_SECOND)}`;
		add(id, pick(rand, authors), ['melee'], randomCodesize(rand), gaussian(rand));
	}
	const teams = Math.max(12, Math.round(size / 15));
	for (let i = 0; i < teams; i++) {
		const id = `${pick(rand, PREFIXES)}.${pick(rand, NAME_SECOND)}${pick(rand, TEAM_SUFFIX)}`;
		add(id, pick(rand, authors), rand() < 0.5 ? ['teams', 'twinduel'] : ['teams'], 4000, gaussian(rand));
	}
	return bots;
}

function createVersions(rand: () => number, id: string, codesize: number, now: Date): BotVersion[] {
	const count = 1 + Math.floor(rand() * rand() * 5);
	let major = 1 + Math.floor(rand() * 3);
	let minor = 0;
	const versions: BotVersion[] = [];
	// Oldest first, then reversed. Each version comes a while after the previous one; a few
	// bots have a version that is only days old, so "new" bots and partial pairings show up.
	const fresh = rand() < 0.04;
	let day = count === 1 && fresh ? Math.floor(rand() * 7) : Math.floor(400 + rand() * 2500);
	for (let i = 0; i < count; i++) {
		if (rand() < 0.3) {
			major++;
			minor = 0;
		} else minor++;
		const added = new Date(now.getTime() - day * 86_400_000);
		versions.push({
			version: `${major}.${minor}`,
			codesize: Math.max(60, Math.round(codesize * (0.85 + 0.03 * i))),
			weightClass: classOf(codesize),
			sha256: sha256Like(`${id}@${major}.${minor}`),
			addedAt: added.toISOString(),
			retiredAt: null
		});
		const last = i === count - 2;
		day = last && fresh ? Math.floor(rand() * 7) : Math.floor(day * (0.25 + 0.5 * rand()));
	}
	for (let i = 0; i < versions.length - 1; i++) versions[i].retiredAt = versions[i + 1].addedAt;
	return versions.reverse();
}

function buildRumble(
	id: RumbleId,
	division: Division,
	weightClass: WeightClass,
	d: (typeof DIVISIONS)[number],
	members: Bot[],
	seed: number,
	now: Date
): RumbleState {
	const n = members.length;
	const rand = mulberry32(seed ^ hash(id));
	// score[i][j]: bot i's percent score against bot j (NaN if they never fought).
	const score: Float64Array[] = members.map(() => new Float64Array(n).fill(NaN));
	const survival: Float64Array[] = members.map(() => new Float64Array(n).fill(NaN));
	const battles: Int32Array[] = members.map(() => new Int32Array(n));
	const spread = division === '1v1' ? 0.75 : 0.5;

	for (let i = 0; i < n; i++) {
		for (let j = i + 1; j < n; j++) {
			const a = members[i];
			const b = members[j];
			if (rand() > Math.min(a.coverage, b.coverage)) continue;
			const x =
				spread * (a.skill - b.skill) + 0.45 * Math.sin(a.style - b.style) + 0.15 * gaussian(rand);
			const s = 100 / (1 + Math.exp(-x));
			const surv = clamp(50 + (s - 50) * 1.3 + 4 * gaussian(rand), 0, 100);
			const count = 1 + Math.floor(rand() * 6);
			score[i][j] = s;
			score[j][i] = 100 - s;
			survival[i][j] = surv;
			survival[j][i] = 100 - surv;
			battles[i][j] = battles[j][i] = count;
		}
	}

	// Per-opponent min and max, for the normalised score (ANPP), and the best scorer (vote).
	const minVs = new Float64Array(n).fill(Infinity);
	const maxVs = new Float64Array(n).fill(-Infinity);
	const votes = new Float64Array(n);
	for (let j = 0; j < n; j++) {
		let best = -1;
		for (let i = 0; i < n; i++) {
			const s = score[i][j];
			if (Number.isNaN(s)) continue;
			if (s < minVs[j]) minVs[j] = s;
			if (s > maxVs[j]) {
				maxVs[j] = s;
				best = i;
			}
		}
		if (best >= 0) votes[best]++;
	}

	const metrics: (Metrics & { battles: number; pairings: number })[] = members.map((bot, i) => {
		let sum = 0;
		let wins = 0;
		let surv = 0;
		let norm = 0;
		let pairings = 0;
		let total = 0;
		for (let j = 0; j < n; j++) {
			const s = score[i][j];
			if (Number.isNaN(s)) continue;
			pairings++;
			total += battles[i][j];
			sum += s;
			surv += survival[i][j];
			wins += s > 50 ? 1 : s === 50 ? 0.5 : 0;
			norm += maxVs[j] > minVs[j] ? (100 * (s - minVs[j])) / (maxVs[j] - minVs[j]) : 50;
		}
		const aps = pairings ? sum / pairings : 50;
		const settled = bot.addedDay >= 1;
		return {
			aps,
			pwin: pairings ? (100 * wins) / pairings : 0,
			survival: pairings ? surv / pairings : 0,
			glicko: settled ? 1500 + (aps - 50) * 14 + 6 * gaussian(rand) : null,
			glickoRd: settled ? 18 + 60 / Math.sqrt(1 + total / 40) : null,
			anpp: settled && pairings ? norm / pairings : null,
			vote: settled ? (100 * votes[i]) / n : null,
			battles: total,
			pairings
		};
	});

	const order = members.map((_, i) => i).sort((x, y) => metrics[y].aps - metrics[x].aps);
	const rankOf = new Int32Array(n);
	order.forEach((i, r) => (rankOf[i] = r + 1));

	const details = new Map<BotId, BotRumble>();
	const rows: RankingRow[] = order.map((i) => {
		const bot = members[i];
		const m = metrics[i];
		const history = createHistory(rand, bot, rankOf[i], m, n, now);
		const weekAgo = history.length > 7 ? history[history.length - 8].rank : null;
		const pairings: Pairing[] = [];
		for (let j = 0; j < n; j++) {
			const s = score[i][j];
			if (Number.isNaN(s)) continue;
			pairings.push({
				opponent: members[j].id,
				opponentVersion: members[j].versions[0].version,
				battles: battles[i][j],
				score: round(s, 2),
				survival: round(survival[i][j], 2),
				normalized:
					m.anpp === null
						? null
						: round(maxVs[j] > minVs[j] ? (100 * (s - minVs[j])) / (maxVs[j] - minVs[j]) : 50, 2)
			});
		}
		const rounded = roundMetrics(m);
		details.set(bot.id, { ...rounded, rank: rankOf[i], battles: m.battles, pairings, history });
		return {
			id: bot.id,
			version: bot.versions[0].version,
			author: bot.author,
			rank: rankOf[i],
			...rounded,
			pairings: m.pairings,
			battles: m.battles,
			rankWeekAgo: weekAgo
		};
	});

	return {
		summary: {
			id,
			division,
			weightClass,
			bots: n,
			battles: Math.round(metrics.reduce((t, m) => t + m.battles, 0) / 2),
			rounds: d.rounds,
			battlefield: { width: d.field, height: d.field === 800 ? 600 : d.field }
		},
		members,
		rows,
		details
	};
}

/** A random walk that ends at today's values; starts when the current version was added. */
function createHistory(
	rand: () => number,
	bot: Bot,
	rank: number,
	m: Metrics,
	n: number,
	now: Date
): HistoryPoint[] {
	const days = Math.min(HISTORY_DAYS, bot.addedDay + 1);
	const points: HistoryPoint[] = [];
	let r = rank;
	let aps = m.aps;
	let glicko = m.glicko;
	for (let k = 0; k < days; k++) {
		points.push({
			date: isoDay(now, -k),
			rank: Math.round(r),
			aps: round(aps, 2),
			glicko: glicko === null ? null : round(glicko, 1)
		});
		// Younger versions settle from noisier early estimates.
		const jitter = k > days - 10 ? 1.5 : 0.25;
		aps = clamp(aps + jitter * gaussian(rand) * 0.4, 0, 100);
		r = clamp(r + jitter * gaussian(rand) * Math.max(1, n / 200), 1, n);
		if (glicko !== null) glicko += jitter * gaussian(rand) * 4;
	}
	return points.reverse();
}

function roundMetrics(m: Metrics): Metrics {
	return {
		aps: round(m.aps, 2),
		pwin: round(m.pwin, 2),
		survival: round(m.survival, 2),
		glicko: m.glicko === null ? null : round(m.glicko, 1),
		glickoRd: m.glickoRd === null ? null : round(m.glickoRd, 1),
		anpp: m.anpp === null ? null : round(m.anpp, 2),
		vote: m.vote === null ? null : round(m.vote, 2)
	};
}

export function classOf(codesize: number): WeightClass {
	return CLASS_ORDER.find((c) => codesize < CLASS_LIMIT[c]) ?? 'mega';
}

function fitsClass(bot: WeightClass, rumble: WeightClass): boolean {
	return CLASS_ORDER.indexOf(bot) <= CLASS_ORDER.indexOf(rumble);
}

function randomCodesize(rand: () => number): number {
	const r = rand();
	if (r < 0.12) return 80 + Math.floor(rand() * 169);
	if (r < 0.3) return 250 + Math.floor(rand() * 499);
	if (r < 0.5) return 750 + Math.floor(rand() * 749);
	return 1500 + Math.floor(rand() * 9000);
}

function shortName(id: BotId): string {
	return id.slice(id.lastIndexOf('.') + 1);
}

function isoDay(now: Date, offset: number): string {
	return new Date(now.getTime() + offset * 86_400_000).toISOString().slice(0, 10);
}

function daysBetween(a: Date, b: Date): number {
	return (b.getTime() - a.getTime()) / 86_400_000;
}

function sha256Like(text: string): string {
	let out = '';
	let h = hash(text);
	for (let i = 0; i < 8; i++) {
		h = Math.imul(h ^ (h >>> 15), 0x2c1b3c6d) >>> 0;
		out += h.toString(16).padStart(8, '0');
	}
	return out;
}

function hash(text: string): number {
	let h = 2166136261;
	for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619);
	return h >>> 0;
}

function mulberry32(seed: number): () => number {
	let a = seed >>> 0;
	return () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

function gaussian(rand: () => number): number {
	const u = 1 - rand();
	return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * rand());
}

function pick<T>(rand: () => number, items: readonly T[]): T {
	return items[Math.floor(rand() * items.length)];
}

function clamp(x: number, lo: number, hi: number): number {
	return Math.min(hi, Math.max(lo, x));
}

function round(x: number, digits: number): number {
	const f = 10 ** digits;
	return Math.round(x * f) / f;
}
