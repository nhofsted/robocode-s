// The published release the download page links to. A new release changes only this file.
import { REPO_URL } from './site.ts';

export const VERSION = '0.2.8';
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
	setup: file('windows-x86_64-setup.exe', 20_691_396, 'Installer (.exe)'),
	zip: file('windows-x86_64.zip', 27_788_706, 'Archive (.zip)')
};

export const LINUX = {
	x86_64: [
		file('linux-x86_64.deb', 14_136_380, '.deb'),
		file('linux-x86_64.rpm', 15_584_832, '.rpm'),
		file('linux-x86_64.tar.gz', 20_315_159, '.tar.gz')
	],
	aarch64: [
		file('linux-aarch64.deb', 13_301_664, '.deb'),
		file('linux-aarch64.rpm', 14_553_576, '.rpm'),
		file('linux-aarch64.tar.gz', 19_504_536, '.tar.gz')
	]
};

export const MACOS = {
	aarch64: [
		file('macos-aarch64.dmg', 20_564_100, 'Disk image (.dmg)'),
		file('macos-aarch64.app.zip', 19_209_075, 'App (.app.zip)'),
		file('macos-aarch64.tar.gz', 19_137_739, 'Archive (.tar.gz)')
	],
	x86_64: [
		file('macos-x86_64.dmg', 21_318_352, 'Disk image (.dmg)'),
		file('macos-x86_64.app.zip', 19_845_419, 'App (.app.zip)'),
		file('macos-x86_64.tar.gz', 19_762_162, 'Archive (.tar.gz)')
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
