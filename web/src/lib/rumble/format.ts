import type { BotId, Division, Metrics, RumbleSummary, WeightClass } from './types.ts';

export type MetricKey = keyof Omit<Metrics, 'glickoRd'>;

export const METRICS: { key: MetricKey; label: string; title: string; digits: number }[] = [
	{ key: 'aps', label: 'APS', title: 'Average percent score over all pairings', digits: 2 },
	{ key: 'pwin', label: 'PWIN', title: 'Percentage of pairings won', digits: 1 },
	{ key: 'survival', label: 'Surv', title: 'Average percentage of rounds survived', digits: 1 },
	{ key: 'glicko', label: 'Glicko-2', title: 'Glicko-2 rating', digits: 0 },
	{ key: 'anpp', label: 'ANPP', title: 'Average normalised percentage pairs', digits: 1 },
	{ key: 'vote', label: 'Vote', title: 'Share of votes: each bot votes for the opponent that scores best against it', digits: 2 }
];

export const DIVISION_NAMES: Record<Division, string> = {
	'1v1': '1v1',
	melee: 'Melee',
	teams: 'Teams',
	twinduel: 'Twin Duel'
};

export const CLASS_NAMES: Record<WeightClass, string> = {
	mega: 'General',
	mini: 'Mini',
	micro: 'Micro',
	nano: 'Nano'
};

export function rumbleName(r: Pick<RumbleSummary, 'division' | 'weightClass'>): string {
	const d = DIVISION_NAMES[r.division];
	return r.weightClass === 'mega' ? d : `${CLASS_NAMES[r.weightClass]} ${d}`;
}

export function formatNumber(value: number | null | undefined, digits: number): string {
	if (value === null || value === undefined || Number.isNaN(value)) return '–';
	return value.toLocaleString('en-US', {
		minimumFractionDigits: digits,
		maximumFractionDigits: digits
	});
}

export function formatCount(value: number): string {
	return value.toLocaleString('en-US');
}

export function formatDate(iso: string): string {
	return new Date(iso).toLocaleDateString('en-GB', {
		year: 'numeric',
		month: 'short',
		day: 'numeric',
		timeZone: 'UTC'
	});
}

/** `14:30 UTC`. */
export function formatTime(date: Date): string {
	const time = date.toLocaleTimeString('en-GB', {
		hour: '2-digit',
		minute: '2-digit',
		timeZone: 'UTC'
	});
	return `${time} UTC`;
}

/** When the data files are published: these UTC hours at :30 (`schedule('publish', ...)` in infra/lib/rumble.ts). */
export const PUBLISH_HOURS_UTC = [3, 9, 15, 21];
const PUBLISH_MINUTE_UTC = 30;

/** The first scheduled publish strictly after `after`. */
export function nextPublish(after: Date): Date {
	const day = Date.UTC(after.getUTCFullYear(), after.getUTCMonth(), after.getUTCDate());
	for (let d = 0; d < 2; d++) {
		for (const hour of PUBLISH_HOURS_UTC) {
			const at = new Date(day + d * 86_400_000 + (hour * 60 + PUBLISH_MINUTE_UTC) * 60_000);
			if (at > after) return at;
		}
	}
	throw new Error('no publish in two days');
}

/** `in 2 h 5 min`, `in 40 min`, `in under a minute`. */
export function formatIn(ms: number): string {
	const minutes = Math.floor(ms / 60_000);
	if (minutes < 1) return 'in under a minute';
	const h = Math.floor(minutes / 60);
	const m = minutes % 60;
	return h === 0 ? `in ${m} min` : m === 0 ? `in ${h} h` : `in ${h} h ${m} min`;
}

/**
 * When the data was published and when the next update is due, e.g. `last 24 Oct 2026, 15:31 UTC;
 * next at 21:30 UTC (in 2 h 5 min)`. The next update is the first scheduled run after the data's
 * own time, so a late or failed run reads as due rather than skipping ahead.
 */
export function updateTimes(generatedAt: string, now: Date): string {
	const last = new Date(generatedAt);
	const next = nextPublish(last);
	const when =
		next > now
			? `next at ${formatTime(next)} (${formatIn(next.getTime() - now.getTime())})`
			: 'next one due now';
	return `last ${formatDate(generatedAt)}, ${formatTime(last)}; ${when}`;
}

/** Sorts nulls last regardless of direction, so unrated bots never top a table. */
export function compareBy<T>(
	get: (row: T) => number | string | null,
	direction: 'asc' | 'desc'
): (a: T, b: T) => number {
	const sign = direction === 'asc' ? 1 : -1;
	return (a, b) => {
		const x = get(a);
		const y = get(b);
		if (x === null && y === null) return 0;
		if (x === null) return 1;
		if (y === null) return -1;
		if (typeof x === 'string' || typeof y === 'string') {
			return sign * String(x).localeCompare(String(y));
		}
		return sign * (x - y);
	};
}

export function shortName(id: BotId): string {
	return id.slice(id.lastIndexOf('.') + 1);
}

export function packageName(id: BotId): string {
	const i = id.lastIndexOf('.');
	return i < 0 ? '' : id.slice(0, i);
}

/**
 * Pairing score as a diverging heat colour over the panel: red (losing) through neutral (50%)
 * to blue (winning). Blue/red stays distinguishable for colour-blind readers; every heat cell
 * also shows or titles its number, so colour is never the only cue.
 */
export function scoreHeat(score: number | null): string {
	if (score === null) return 'transparent';
	const t = Math.max(-1, Math.min(1, (score - 50) / 30));
	const pole = t >= 0 ? '57 135 229' : '230 103 103';
	return `rgb(${pole} / ${(Math.abs(t) * 0.8).toFixed(3)})`;
}

/** A jar size, e.g. `12.3 KB` (1 KB = 1,000 bytes). */
export function formatBytes(bytes: number): string {
	if (bytes < 1000) return `${bytes} B`;
	if (bytes < 1_000_000) return `${(bytes / 1000).toFixed(1)} KB`;
	return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/** Where a version's jar is served (same origin), and the name a download is saved under. */
export const jarHref = (sha256: string): string => `/jars/${sha256}.jar`;
export const jarFileName = (id: BotId, version: string): string => `${id}_${version}.jar`;
