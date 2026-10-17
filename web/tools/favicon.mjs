// Extracts sizes from classic's robocode.ico and writes PNGs where the black background,
// flood-filled from the borders, becomes transparent (soft edge for near-black pixels).
// Usage: node tools/favicon.mjs robocode.ico outdir [sizes...]
// Writes original-<size>.png and favicon-<size>.png; the site uses favicon-32 and favicon-256.
// The functions are also used by crates/gui/tools/app-icon.mjs (the desktop app's icon).
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { deflateSync } from 'node:zlib';

/** The sizes in an .ico file, by index. */
export function sizes(buf) {
	const count = buf.readUInt16LE(4);
	return Array.from({ length: count }, (_, i) => buf[6 + i * 16] || 256);
}

/** Entry `i` of an .ico file holding 32-bit BMP images, as RGBA rows top to bottom. */
export function decode(buf, i) {
	const e = 6 + i * 16;
	const w = buf[e] || 256;
	const off = buf.readUInt32LE(e + 12);
	const hdr = buf.readUInt32LE(off);
	const h = w;
	const px = new Uint8ClampedArray(w * h * 4);
	const base = off + hdr;
	for (let y = 0; y < h; y++) {
		for (let x = 0; x < w; x++) {
			const s = base + ((h - 1 - y) * w + x) * 4;
			const d = (y * w + x) * 4;
			px[d] = buf[s + 2];
			px[d + 1] = buf[s + 1];
			px[d + 2] = buf[s];
			px[d + 3] = buf[s + 3];
		}
	}
	return { w, h, px };
}

export function knockOut({ w, h, px }, hard = 34, soft = 80) {
	const lum = (i) => Math.max(px[i * 4], px[i * 4 + 1], px[i * 4 + 2]);
	const seen = new Uint8Array(w * h);
	const stack = [];
	for (let x = 0; x < w; x++) stack.push(x, (h - 1) * w + x);
	for (let y = 0; y < h; y++) stack.push(y * w, y * w + w - 1);
	while (stack.length) {
		const i = stack.pop();
		if (seen[i]) continue;
		seen[i] = 1;
		const l = lum(i);
		if (px[i * 4 + 3] === 0 || l <= hard) {
			px[i * 4 + 3] = 0;
			const x = i % w;
			const y = (i / w) | 0;
			if (x > 0) stack.push(i - 1);
			if (x < w - 1) stack.push(i + 1);
			if (y > 0) stack.push(i - w);
			if (y < h - 1) stack.push(i + w);
		} else if (l < soft) {
			// Edge pixel: keep it, partly transparent, so the outline stays smooth.
			px[i * 4 + 3] = Math.round((px[i * 4 + 3] * (l - hard)) / (soft - hard));
		}
	}
	return { w, h, px };
}

export function png({ w, h, px }) {
	const crcTable = Array.from({ length: 256 }, (_, n) => {
		let c = n;
		for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
		return c >>> 0;
	});
	const crc = (b) => {
		let c = 0xffffffff;
		for (const v of b) c = crcTable[(c ^ v) & 0xff] ^ (c >>> 8);
		return (c ^ 0xffffffff) >>> 0;
	};
	const chunk = (type, data) => {
		const len = Buffer.alloc(4);
		len.writeUInt32BE(data.length);
		const td = Buffer.concat([Buffer.from(type, 'latin1'), data]);
		const c = Buffer.alloc(4);
		c.writeUInt32BE(crc(td));
		return Buffer.concat([len, td, c]);
	};
	const ihdr = Buffer.alloc(13);
	ihdr.writeUInt32BE(w, 0);
	ihdr.writeUInt32BE(h, 4);
	ihdr[8] = 8;
	ihdr[9] = 6;
	const raw = Buffer.alloc((w * 4 + 1) * h);
	for (let y = 0; y < h; y++) {
		raw[y * (w * 4 + 1)] = 0;
		Buffer.from(px.buffer, y * w * 4, w * 4).copy(raw, y * (w * 4 + 1) + 1);
	}
	return Buffer.concat([
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		chunk('IHDR', ihdr),
		chunk('IDAT', deflateSync(raw, { level: 9 })),
		chunk('IEND', Buffer.alloc(0))
	]);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
	const [icoPath, outDir, ...sizeArgs] = process.argv.slice(2);
	const buf = readFileSync(icoPath);
	const wanted = sizeArgs.map(Number);
	sizes(buf).forEach((w, i) => {
		if (!wanted.includes(w)) return;
		writeFileSync(`${outDir}/original-${w}.png`, png(decode(buf, i)));
		writeFileSync(`${outDir}/favicon-${w}.png`, png(knockOut(decode(buf, i))));
	});
}
