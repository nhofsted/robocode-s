#!/usr/bin/env node
// SPDX-License-Identifier: EPL-2.0
//
// Type-checks published rumble data files against src/lib/rumble/types.ts (docs/rumble-data.md):
// each file becomes a typed constant of a generated TypeScript module that the TypeScript
// compiler checks in strict mode, so missing, extra and mistyped fields are all errors.
//
//   node web/scripts/check-rumble-data.mjs <dir>
//
// <dir> holds the files as under /data/v1: rumbles.json, rankings/*.json, bots/*.json and
// contributors.json (each optional). Exits 1 on any error. Used by the rumble batch Lambda's
// tests (rumble/lambda/tests).

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const dir = process.argv[2];
if (!dir) {
	console.error('usage: check-rumble-data.mjs <dir>');
	process.exit(2);
}
const here = path.dirname(fileURLToPath(import.meta.url));
const types = path.resolve(here, '../src/lib/rumble/types.ts');

const files = [];
const add = (file, type) => {
	if (fs.existsSync(file)) files.push([file, type]);
};
add(path.join(dir, 'rumbles.json'), 'RumbleIndex');
add(path.join(dir, 'contributors.json'), 'Contributors');
for (const [sub, type] of [
	['rankings', 'Rankings'],
	['bots', 'BotDetails']
]) {
	const folder = path.join(dir, sub);
	if (!fs.existsSync(folder)) continue;
	for (const name of fs.readdirSync(folder).sort()) {
		if (name.endsWith('.json')) add(path.join(folder, name), type);
	}
}
if (files.length === 0) {
	console.error(`no data files in ${dir}`);
	process.exit(1);
}

let source = `import type { RumbleIndex, Rankings, BotDetails, Contributors } from ${JSON.stringify(types)};\n`;
files.forEach(([file, type], i) => {
	source += `// ${file}\nexport const v${i}: ${type} = ${fs.readFileSync(file, 'utf8')};\n`;
});
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rumble-data-'));
const module = path.join(tmp, 'check.ts');
fs.writeFileSync(module, source);
const program = ts.createProgram([module], {
	strict: true,
	noEmit: true,
	target: ts.ScriptTarget.ES2022,
	module: ts.ModuleKind.ESNext,
	moduleResolution: ts.ModuleResolutionKind.Bundler,
	allowImportingTsExtensions: true,
	skipLibCheck: true,
	types: []
});
const diagnostics = ts.getPreEmitDiagnostics(program);
for (const d of diagnostics) {
	const text = ts.flattenDiagnosticMessageText(d.messageText, '\n');
	if (d.file && d.start !== undefined) {
		const { line } = d.file.getLineAndCharacterOfPosition(d.start);
		const lines = source.split('\n');
		// The comment line before the constant names the file.
		let at = line;
		while (at > 0 && !lines[at].startsWith('// ')) at--;
		console.error(`${lines[at].slice(3)}: ${text}`);
	} else {
		console.error(text);
	}
}
fs.rmSync(tmp, { recursive: true, force: true });
if (diagnostics.length > 0) process.exit(1);
console.log(`${files.length} files match types.ts`);
