// The published release the download page links to. A new release changes only this file.
import { REPO_URL } from './site.ts';

export const VERSION = '0.2.5';
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
	setup: file('windows-x86_64-setup.exe', 19_981_136, 'Installer (.exe)'),
	zip: file('windows-x86_64.zip', 26_931_540, 'Archive (.zip)')
};

export const LINUX = {
	x86_64: [
		file('linux-x86_64.deb', 13_525_352, '.deb'),
		file('linux-x86_64.rpm', 14_967_424, '.rpm'),
		file('linux-x86_64.tar.gz', 19_592_876, '.tar.gz')
	],
	aarch64: [
		file('linux-aarch64.deb', 12_701_216, '.deb'),
		file('linux-aarch64.rpm', 13_663_808, '.rpm'),
		file('linux-aarch64.tar.gz', 18_794_908, '.tar.gz')
	]
};

export const MACOS = {
	aarch64: [
		file('macos-aarch64.dmg', 19_801_888, 'Disk image (.dmg)'),
		file('macos-aarch64.app.zip', 18_473_254, 'App (.app.zip)'),
		file('macos-aarch64.tar.gz', 18_400_828, 'Archive (.tar.gz)')
	],
	x86_64: [
		file('macos-x86_64.dmg', 20_542_272, 'Disk image (.dmg)'),
		file('macos-x86_64.app.zip', 19_129_602, 'App (.app.zip)'),
		file('macos-x86_64.tar.gz', 19_052_036, 'Archive (.tar.gz)')
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
