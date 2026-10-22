// The published release the download page links to. A new release changes only this file.
import { REPO_URL } from './site.ts';

export const VERSION = '0.1.0';
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
	setup: file('windows-x86_64-setup.exe', 17_890_825, 'Installer (.exe)'),
	zip: file('windows-x86_64.zip', 23_525_227, 'Archive (.zip)')
};

export const LINUX = {
	x86_64: [
		file('linux-x86_64.deb', 12_701_520, '.deb'),
		file('linux-x86_64.rpm', 13_565_704, '.rpm'),
		file('linux-x86_64.tar.gz', 17_733_515, '.tar.gz')
	],
	aarch64: [
		file('linux-aarch64.deb', 11_956_256, '.deb'),
		file('linux-aarch64.rpm', 12_673_128, '.rpm'),
		file('linux-aarch64.tar.gz', 17_028_867, '.tar.gz')
	]
};

export const SHA256SUMS_URL = `${DOWNLOAD_BASE}/SHA256SUMS`;

export const url = (a: Asset) => `${DOWNLOAD_BASE}/${a.name}`;
export const mb = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;
