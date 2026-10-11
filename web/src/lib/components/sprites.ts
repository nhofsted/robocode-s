// Classic Robocode's robot images (assets/classic), recoloured the way classic does it:
// a port of net.sf.robocode.ui.gfx.ColorUtil and ImageUtil.ColorFilter, in float arithmetic
// (Math.fround) like the Java original. Browser only.

import bodyUrl from '#lib/assets/classic/body.png';
import turretUrl from '#lib/assets/classic/turret.png';
import radarUrl from '#lib/assets/classic/radar.png';

export type RGB = readonly [number, number, number];

export interface TankColors {
	body: RGB;
	gun: RGB;
	radar: RGB;
}

const f = Math.fround;

/** ColorUtil.fromRGBtoHSL. */
export function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
	const R = f(r / 255);
	const G = f(g / 255);
	const B = f(b / 255);
	const min = Math.min(R, G, B);
	const max = Math.max(R, G, B);
	const delta = f(max - min);
	const L = f(f(max + min) / 2);
	if (delta === 0) return [0, 0, L];
	const S = L < 0.5 ? f(delta / f(max + min)) : f(delta / f(f(2 - max) - min));
	const dR = f(f(f(f(max - R) / 6) + f(delta / 2)) / delta);
	const dG = f(f(f(f(max - G) / 6) + f(delta / 2)) / delta);
	const dB = f(f(f(f(max - B) / 6) + f(delta / 2)) / delta);
	let H: number;
	if (isNear(R, max)) H = f(dB - dG);
	else if (isNear(G, max)) H = f(f(f(1 / 3) + dR) - dB);
	else H = f(f(f(2 / 3) + dG) - dR);
	if (H < 0) H = f(H + 1);
	if (H > 1) H = f(H - 1);
	return [H, S, L];
}

/** Utils.isNear (ColorUtil passes floats, widened to double). */
function isNear(a: number, b: number): boolean {
	return Math.abs(a - b) < 0.00001; // robocode.util.Utils.isNear, in double
}

/** ColorUtil.fromHSLtoRGB. */
export function hslToRgb(h: number, s: number, l: number): RGB {
	const m2 = l <= 0.5 ? f(l * f(s + 1)) : f(f(l + s) - f(l * s));
	const m1 = f(f(2 * l) - m2);
	const channel = (x: number) => Math.trunc(f(255 * hueToRgb(m1, m2, x)));
	return [channel(f(h + f(1 / 3))), channel(h), channel(f(h - f(1 / 3)))];
}

function hueToRgb(m1: number, m2: number, h: number): number {
	if (h < 0) h = f(h + 1);
	if (h > 1) h = f(h - 1);
	if (f(h * 6) < 1) return f(m1 + f(f(f(m2 - m1) * h) * 6));
	if (f(h * 2) < 1) return m2;
	if (f(h * 3) < 2) return f(m1 + f(f(f(m2 - m1) * f(f(2 / 3) - h)) * 6));
	return m1;
}

/**
 * ImageUtil.ColorFilter.filterRGB: a coloured pixel takes the robot colour's hue and saturation
 * and a lightness between both; grey pixels (the tracks) stay as they are.
 */
export function recolorPixel(pixel: RGB, color: RGB): RGB {
	const [ch, cs, cl] = rgbToHsl(...color);
	const [, S, L] = rgbToHsl(...pixel);
	if (S <= 0) return pixel;
	const lum = Math.min(1, f(f(f(cl + L) / 2) + f(cl / 7)));
	return hslToRgb(ch, cs, lum);
}

export interface Sprites {
	body: HTMLImageElement;
	turret: HTMLImageElement;
	radar: HTMLImageElement;
}

let loading: Promise<Sprites> | undefined;

export function loadSprites(): Promise<Sprites> {
	loading ??= Promise.all([bodyUrl, turretUrl, radarUrl].map(loadImage)).then(
		([body, turret, radar]) => ({ body, turret, radar })
	);
	return loading;
}

export function loadImage(url: string): Promise<HTMLImageElement> {
	return new Promise((resolve, reject) => {
		const img = new Image();
		img.onload = () => resolve(img);
		img.onerror = () => reject(new Error(`could not load ${url}`));
		img.src = url;
	});
}

const cache = new Map<string, HTMLCanvasElement>();

/** The image recoloured with `color`, cached per image and colour. */
export function colored(img: HTMLImageElement, color: RGB): HTMLCanvasElement {
	const key = `${img.src}|${color.join(',')}`;
	let canvas = cache.get(key);
	if (canvas) return canvas;
	canvas = document.createElement('canvas');
	canvas.width = img.naturalWidth;
	canvas.height = img.naturalHeight;
	const ctx = canvas.getContext('2d', { willReadFrequently: true })!;
	ctx.drawImage(img, 0, 0);
	const data = ctx.getImageData(0, 0, canvas.width, canvas.height);
	const px = data.data;
	const memo = new Map<number, RGB>();
	for (let i = 0; i < px.length; i += 4) {
		if (px[i + 3] === 0) continue;
		const k = (px[i] << 16) | (px[i + 1] << 8) | px[i + 2];
		let out = memo.get(k);
		if (!out) {
			out = recolorPixel([px[i], px[i + 1], px[i + 2]], color);
			memo.set(k, out);
		}
		px[i] = out[0];
		px[i + 1] = out[1];
		px[i + 2] = out[2];
	}
	ctx.putImageData(data, 0, 0);
	cache.set(key, canvas);
	return canvas;
}

/** Headings in radians, Robocode style: 0 is up, clockwise. Sprites are centred, as in classic. */
export function drawTank(
	ctx: CanvasRenderingContext2D,
	sprites: Sprites,
	colors: TankColors,
	x: number,
	y: number,
	heading: number,
	gunHeading: number,
	radarHeading: number
): void {
	const parts: [HTMLImageElement, RGB, number][] = [
		[sprites.body, colors.body, heading],
		[sprites.turret, colors.gun, gunHeading],
		[sprites.radar, colors.radar, radarHeading]
	];
	for (const [img, color, angle] of parts) {
		const c = colored(img, color);
		ctx.save();
		ctx.translate(x, y);
		ctx.rotate(angle);
		ctx.drawImage(c, -c.width / 2, -c.height / 2);
		ctx.restore();
	}
}

export function css(color: RGB): string {
	return `rgb(${color[0]} ${color[1]} ${color[2]})`;
}
