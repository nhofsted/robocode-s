// The published release the download page links to. A new release changes only this file.
import { REPO_URL } from './site.ts';

export const VERSION = '0.2.1';
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
	setup: file('windows-x86_64-setup.exe', 19_426_384, 'Installer (.exe)'),
	zip: file('windows-x86_64.zip', 26_034_459, 'Archive (.zip)')
};

export const LINUX = {
	x86_64: [
		file('linux-x86_64.deb', 13_254_728, '.deb'),
		file('linux-x86_64.rpm', 14_560_676, '.rpm'),
		file('linux-x86_64.tar.gz', 19_022_276, '.tar.gz')
	],
	aarch64: [
		file('linux-aarch64.deb', 12_453_208, '.deb'),
		file('linux-aarch64.rpm', 13_351_404, '.rpm'),
		file('linux-aarch64.tar.gz', 18_251_296, '.tar.gz')
	]
};

export const MACOS = {
	aarch64: [
		file('macos-aarch64.dmg', 19_217_649, 'Disk image (.dmg)'),
		file('macos-aarch64.app.zip', 17_954_202, 'App (.app.zip)'),
		file('macos-aarch64.tar.gz', 17_886_871, 'Archive (.tar.gz)')
	],
	x86_64: [
		file('macos-x86_64.dmg', 19_990_747, 'Disk image (.dmg)'),
		file('macos-x86_64.app.zip', 18_578_193, 'App (.app.zip)'),
		file('macos-x86_64.tar.gz', 18_503_616, 'Archive (.tar.gz)')
	]
};

export const SHA256SUMS_URL = `${DOWNLOAD_BASE}/SHA256SUMS`;

export const url = (a: Asset) => `${DOWNLOAD_BASE}/${a.name}`;
export const mb = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;
