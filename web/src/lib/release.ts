// The published release the download page links to. A new release changes only this file.
import { REPO_URL } from './site.ts';

export const VERSION = '0.2.7';
export const TAG = `v${VERSION}`;
export const RELEASE_URL = `${REPO_URL}/releases/tag/${TAG}`;
export const DOWNLOAD_BASE = `${REPO_URL}/releases/download/${TAG}`;

export type Asset = { name: string; size: number; label: string };

const file = (suffix: string, size: number, label: string): Asset => ({
	name: `robocode-s-${VERSION}-${suffix}`,
	size,
	label
});

export const WINDOWS = {
	setup: file('windows-x86_64-setup.exe', 20_602_594, 'Installer (.exe)'),
	zip: file('windows-x86_64.zip', 27_673_700, 'Archive (.zip)')
};

export const LINUX = {
	x86_64: [
		file('linux-x86_64.deb', 14_117_068, '.deb'),
		file('linux-x86_64.rpm', 15_570_984, '.rpm'),
		file('linux-x86_64.tar.gz', 20_297_981, '.tar.gz')
	],
	aarch64: [
		file('linux-aarch64.deb', 13_286_320, '.deb'),
		file('linux-aarch64.rpm', 14_540_408, '.rpm'),
		file('linux-aarch64.tar.gz', 19_486_127, '.tar.gz')
	]
};

export const MACOS = {
	aarch64: [
		file('macos-aarch64.dmg', 20_511_835, 'Disk image (.dmg)'),
		file('macos-aarch64.app.zip', 19_193_300, 'App (.app.zip)'),
		file('macos-aarch64.tar.gz', 19_120_471, 'Archive (.tar.gz)')
	],
	x86_64: [
		file('macos-x86_64.dmg', 21_296_746, 'Disk image (.dmg)'),
		file('macos-x86_64.app.zip', 19_826_457, 'App (.app.zip)'),
		file('macos-x86_64.tar.gz', 19_744_436, 'Archive (.tar.gz)')
	]
};

export const SHA256SUMS_URL = `${DOWNLOAD_BASE}/SHA256SUMS`;

export const url = (a: Asset) => `${DOWNLOAD_BASE}/${a.name}`;
export const mb = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;

/** `/release.json`: what the programs check for updates (crates/update, ADR 0022). It is
 * made from this file, so it names a new release when the download page does. */
export const releaseJson = () => ({
	version: VERSION,
	notes: RELEASE_URL,
	download: DOWNLOAD_BASE,
	files: [
		...Object.values(WINDOWS),
		...Object.values(LINUX).flat(),
		...Object.values(MACOS).flat()
	].map((a) => a.name)
});
