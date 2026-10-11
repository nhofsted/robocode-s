// A bot's tank colours, derived from its id: the rumble doesn't record the colours robots
// set with setColors(). Same id, same colours, everywhere on the site.

import type { RGB, TankColors } from './sprites.ts';

export function tankColors(id: string): TankColors {
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

function hsl(hue: number, s: number, l: number): RGB {
	const a = s * Math.min(l, 1 - l);
	const channel = (n: number) => {
		const k = (n + hue / 30) % 12;
		return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
	};
	return [channel(0), channel(8), channel(4)];
}
