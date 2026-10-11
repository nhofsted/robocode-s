import type { BotId, Division, Metrics, RumbleSummary, WeightClass } from './types.ts';

export type MetricKey = keyof Omit<Metrics, 'glickoRd'>;

export const METRICS: { key: MetricKey; label: string; title: string; digits: number }[] = [
	{ key: 'aps', label: 'APS', title: 'Average percent score over all pairings', digits: 2 },
	{ key: 'pwin', label: 'PWIN', title: 'Percentage of pairings won', digits: 1 },
	{ key: 'survival', label: 'Surv', title: 'Average percentage of rounds survived', digits: 1 },
	{ key: 'glicko', label: 'Glicko-2', title: 'Glicko-2 rating (daily)', digits: 0 },
	{ key: 'anpp', label: 'ANPP', title: 'Average normalised percentage pairs (daily)', digits: 1 },
	{ key: 'vote', label: 'Vote', title: 'Share of opponents this bot scores best against (daily)', digits: 2 }
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
