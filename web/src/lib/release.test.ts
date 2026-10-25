import { describe, expect, it } from 'vitest';
import { releaseJson, VERSION } from './release.ts';

describe('release.json', () => {
	it('names the files the programs update from', () => {
		const r = releaseJson();
		expect(r.version).toBe(VERSION);
		expect(r.download).toMatch(new RegExp(`/releases/download/v${VERSION}$`));
		// The names crates/update asks for (Install::how).
		for (const suffix of [
			'windows-x86_64.zip',
			'windows-x86_64-setup.exe',
			'linux-x86_64.tar.gz',
			'linux-aarch64.tar.gz',
			'macos-aarch64.tar.gz',
			'macos-aarch64.app.zip',
			'macos-x86_64.app.zip'
		]) {
			expect(r.files).toContain(`robocode-s-${VERSION}-${suffix}`);
		}
	});
});
