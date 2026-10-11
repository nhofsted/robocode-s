// A bot's tank colours, derived from its id until the rumble records the colours robots set
// with setColors(). Same id, same colours, everywhere on the site.

export interface TankColors {
	body: string;
	gun: string;
	radar: string;
}

export function tankColors(id: string): TankColors {
	let h = 2166136261;
	for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619);
	h >>>= 0;
	const hue = h % 360;
	const hue2 = (hue + 120 + ((h >>> 9) % 120)) % 360;
	return {
		body: `hsl(${hue} 45% 42%)`,
		gun: `hsl(${hue} 30% 62%)`,
		radar: `hsl(${hue2} 70% 60%)`
	};
}
