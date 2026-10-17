// The desktop app's icon: classic Robocode's application icon, as on the website
// (web/tools/favicon.mjs: the black background made transparent), at the sizes Windows
// uses. classic's .ico has 16, 32, 48, 64, 128 and 256; 24 is the 48 halved.
// Usage: node crates/gui/tools/app-icon.mjs <classic's robocode.ico> crates/gui/assets
// Writes robocode-s.ico (all sizes), the .exe's icon. Its 256 is web/static/favicon-256.png,
// which the app sets as its window's icon at run time.
import { readFileSync, writeFileSync } from 'node:fs';
import { decode, knockOut, png, sizes } from '../../../web/tools/favicon.mjs';

const [icoPath, outDir] = process.argv.slice(2);
const buf = readFileSync(icoPath);
const images = new Map(sizes(buf).map((w, i) => [w, knockOut(decode(buf, i))]));

/** Halves an image, averaging each 2x2 block with premultiplied alpha. */
function half({ w, h, px }) {
	const out = new Uint8ClampedArray((w / 2) * (h / 2) * 4);
	for (let y = 0; y < h / 2; y++) {
		for (let x = 0; x < w / 2; x++) {
			let r = 0,
				g = 0,
				b = 0,
				a = 0;
			for (const [dx, dy] of [
				[0, 0],
				[1, 0],
				[0, 1],
				[1, 1]
			]) {
				const s = ((2 * y + dy) * w + 2 * x + dx) * 4;
				const al = px[s + 3];
				r += px[s] * al;
				g += px[s + 1] * al;
				b += px[s + 2] * al;
				a += al;
			}
			const d = (y * (w / 2) + x) * 4;
			if (a > 0) {
				out[d] = Math.round(r / a);
				out[d + 1] = Math.round(g / a);
				out[d + 2] = Math.round(b / a);
			}
			out[d + 3] = Math.round(a / 4);
		}
	}
	return { w: w / 2, h: h / 2, px: out };
}
images.set(24, half(images.get(48)));

/** A 32-bit BMP icon image: BITMAPINFOHEADER (double height), BGRA rows bottom up, AND mask. */
function bmp({ w, h, px }) {
	const maskRow = Math.ceil(w / 32) * 4;
	const out = Buffer.alloc(40 + w * h * 4 + maskRow * h);
	out.writeUInt32LE(40, 0);
	out.writeInt32LE(w, 4);
	out.writeInt32LE(h * 2, 8);
	out.writeUInt16LE(1, 12);
	out.writeUInt16LE(32, 14);
	out.writeUInt32LE(w * h * 4 + maskRow * h, 20);
	for (let y = 0; y < h; y++) {
		for (let x = 0; x < w; x++) {
			const s = ((h - 1 - y) * w + x) * 4;
			const d = 40 + (y * w + x) * 4;
			out[d] = px[s + 2];
			out[d + 1] = px[s + 1];
			out[d + 2] = px[s];
			out[d + 3] = px[s + 3];
			if (px[s + 3] === 0) {
				const m = 40 + w * h * 4 + y * maskRow + (x >> 3);
				out[m] |= 0x80 >> (x & 7);
			}
		}
	}
	return out;
}

// The usual layout: BMP up to 128, PNG for 256.
const order = [16, 24, 32, 48, 64, 128, 256];
const entries = order.map((w) => (w === 256 ? png(images.get(w)) : bmp(images.get(w))));
const header = Buffer.alloc(6 + 16 * order.length);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(order.length, 4);
let offset = header.length;
order.forEach((w, i) => {
	const e = 6 + 16 * i;
	header[e] = w % 256;
	header[e + 1] = w % 256;
	header.writeUInt16LE(1, e + 4);
	header.writeUInt16LE(32, e + 6);
	header.writeUInt32LE(entries[i].length, e + 8);
	header.writeUInt32LE(offset, e + 12);
	offset += entries[i].length;
});
writeFileSync(`${outDir}/robocode-s.ico`, Buffer.concat([header, ...entries]));
