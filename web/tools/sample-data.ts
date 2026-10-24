// Writes the synthetic rumble data set (src/lib/rumble/fixtures.ts) as static files, for hosting
// the site before the rankings Lambda exists. The site shows its "sample data" notice with it.
// Usage: node tools/sample-data.ts <outdir>    (writes <outdir>/rumbles.json, contributors.json, archive.json, rankings/, bots/)
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createFixtures } from '../src/lib/rumble/fixtures.ts';

const outDir = process.argv[2];
if (!outDir) throw new Error('usage: node tools/sample-data.ts <outdir>');

// As of the last daily rankings run (06:00 UTC), so runs on the same day give the same files.
const now = new Date(Date.now() - 6 * 3600_000);
now.setUTCHours(6, 0, 0, 0);
const fixtures = createFixtures({ now });
const write = (path: string, body: unknown) => {
	mkdirSync(join(outDir, path, '..'), { recursive: true });
	writeFileSync(join(outDir, path), JSON.stringify(body));
};

write('contributors.json', fixtures.contributors());
write('archive.json', fixtures.archive());
const index = fixtures.index();
write('rumbles.json', index);
const bots = new Set<string>();
for (const { id } of index.rumbles) {
	const rankings = fixtures.rankings(id)!;
	write(`rankings/${id}.json`, rankings);
	for (const row of rankings.bots) bots.add(row.id);
}
// File names are the decoded ids: CloudFront passes the URL-encoded path, S3 decodes it.
for (const id of bots) write(`bots/${id}.json`, fixtures.bot(id));
console.log(`${index.rumbles.length} rumbles, ${bots.size} bots → ${outDir}`);
