// A bot's tank colours: those its robot sets with setColors(), where the rumble knows them
// (`BotColors` in the data files), and otherwise derived from its id, part by part. Same id,
// same colours, everywhere on the site, as in the desktop app (`sprites::default_colors`).

import type { BotColors } from '../rumble/types.ts';
import type { RGB, TankColors } from './sprites.ts';

/** `#rrggbb` as RGB; undefined if it isn't one. */
function hex(color: string | undefined): RGB | undefined {
	const m = color ? /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color) : null;
	return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : undefined;
}

/** A bot's tank colours: `published` (the robot's own) where given, else derived from the id. */
export function tankColors(id: string, published?: BotColors | null): TankColors {
	const derived = idColors(id);
	if (!published) return derived;
	return {
		body: hex(published.body) ?? derived.body,
		gun: hex(published.gun) ?? derived.gun,
		radar: hex(published.radar) ?? derived.radar
	};
}

function idColors(id: string): TankColors {
	let h = 2166136261;
	for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
	h >>>= 0;
	const hue = h % 360;
	const hue2 = (hue + 120 + ((h >>> 9) % 120)) % 360;
	return {
		body: hsl(hue, 0.55, 0.38),
		gun: hsl(hue, 0.35, 0.55),
		radar: hsl(hue2, 0.7, 0.55)
	};
}

/** Colours for robot number `i` of a battle with no bot ids: each a golden angle on from the last, as the desktop app does. */
export function tankColorsByIndex(i: number): TankColors {
	const hue = (i * 137.508 + 120) % 360;
	return {
		body: hsl(hue, 0.55, 0.38),
		gun: hsl(hue, 0.35, 0.55),
		radar: hsl((hue + 180) % 360, 0.7, 0.55)
	};
}

function hsl(hue: number, s: number, l: number): RGB {
	const a = s * Math.min(l, 1 - l);
	const channel = (n: number) => {
		const k = (n + hue / 30) % 12;
		return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
	};
	return [channel(0), channel(8), channel(4)];
}
