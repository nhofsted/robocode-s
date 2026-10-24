import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { ServerResponse } from 'node:http';
import type { Connect, Plugin } from 'vite';
import { mdsvex } from 'mdsvex';
import rehypeSlug from 'rehype-slug';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { createFixtures, type Fixtures } from './src/lib/rumble/fixtures.ts';

/**
 * Serves synthetic rumble data at /data/v1 in `vite dev` and `vite preview`, until the
 * rankings Lambda writes the real files. Builds never contain it.
 * RUMBLE_FIXTURES=off disables it, e.g. to point VITE_RUMBLE_DATA at real data.
 * Also serves any /rec/**.track as one of the fixture tracks (src/lib/replay/fixtures): a real
 * 1v1 for /rec/e<E>/1v1, an 8-robot melee for /rec/e<E>/melee. Paths ending in -5.track are
 * missing, to try the viewer's missing state.
 */
function rumbleFixtures(): Plugin {
	let fixtures: Fixtures | undefined;
	const middleware: Connect.NextHandleFunction = (req, res, next) => {
		const url = req.url ?? '';
		if (url.startsWith('/rec/')) {
			const path = url.split('?')[0];
			if (!path.endsWith('.track')) return next();
			if (path.endsWith('-5.track')) {
				res.statusCode = 404;
				return void res.end('not found');
			}
			const fixture = path.includes('/melee/') ? 'melee8.track' : 'acero-mailmanx.track';
			readFile(fileURLToPath(new URL(`./src/lib/replay/fixtures/${fixture}`, import.meta.url))).then(
				(bytes) => {
					res.setHeader('content-type', 'application/octet-stream');
					res.setHeader('cache-control', 'public, max-age=31536000, immutable');
					res.end(bytes);
				},
				() => {
					res.statusCode = 500;
					res.end('fixture track missing');
				}
			);
			return;
		}
		if (!url.startsWith('/data/v1/')) return next();
		fixtures ??= createFixtures({ now: new Date() });
		const body = fixtures.get(url.slice('/data/v1/'.length).split('?')[0]);
		if (body === undefined) {
			res.statusCode = 404;
			res.end('not found');
			return;
		}
		res.setHeader('content-type', 'application/json');
		res.end(JSON.stringify(body));
	};
	return {
		name: 'rumble-fixtures',
		apply: () => process.env.RUMBLE_FIXTURES !== 'off',
		configureServer: (server) => void server.middlewares.use(middleware),
		configurePreviewServer: (server) => void server.middlewares.use(middleware)
	};
}

/**
 * RUMBLE_API_MOCK=on: a tiny in-memory stand-in for /api/v1 in `vite dev` and `vite preview`,
 * to work on the account, submit and bots pages without the backend. Not the real thing: it
 * checks nothing but the shapes (docs/rumble-backend.md §5.4). Without the flag, /api/v1 is
 * simply absent and the pages show their signed-out or unavailable state.
 */
function rumbleApiMock(): Plugin {
	let signedIn = true;
	const me = {
		uid: 'g1', login: 'mock-user', name: 'Mock User', country: 'be', hidden: false, reputation: 61,
		tier: 'verifier', verifyRate: 0.044, battles: 1834, verifiedMatches: 61, mismatches: 0,
		admin: false, since: new Date(Date.now() - 40 * 86_400_000).toISOString()
	};
	const now = () => new Date().toISOString();
	const tokens = [
		{ id: 't1', kind: 'upload', label: 'desktop', created: now(), lastUsed: now() },
		{ id: 's1', kind: 'session', label: null, created: now(), lastUsed: now(), current: true }
	];
	const bots: { id: string; state: string; versions: Record<string, unknown>[] }[] = [
		{
			id: 'mock.Kestrel', state: 'active',
			versions: [
				{ version: '1.1', vid: 2, state: 'probation', codesize: 1234, weightClass: 'mini', created: now(), probation: { battles: 4, failed: 0 } },
				{ version: '1.0', vid: 1, state: 'active', codesize: 1190, weightClass: 'mini', created: now() }
			]
		},
		{
			id: 'mock.Broken', state: 'failed',
			versions: [{ version: '0.1', vid: 3, state: 'failed', codesize: 400, weightClass: 'micro', created: now(), failures: ['class not found: mock.Broken'] }]
		}
	];
	const reply = (res: ServerResponse, status: number, body?: unknown) => {
		res.statusCode = status;
		if (body !== undefined) res.setHeader('content-type', 'application/json');
		res.end(body === undefined ? undefined : JSON.stringify(body));
	};
	const middleware: Connect.NextHandleFunction = (req, res, next) => {
		const url = new URL(req.url ?? '', 'http://localhost');
		if (req.method === 'PUT' && url.pathname.startsWith('/mock-s3/')) return reply(res, 200);
		if (!url.pathname.startsWith('/api/v1/')) return next();
		const route = `${req.method} ${url.pathname.slice('/api/v1/'.length)}`;
		if (route === 'GET auth/login') {
			signedIn = true;
			const back = url.searchParams.get('return') ?? '/';
			res.statusCode = 302;
			res.setHeader('location', back.startsWith('/') ? back : '/');
			return void res.end();
		}
		if (route === 'GET info') {
			return reply(res, 200, { api: 1, engineVersion: 1, minClient: '0.2.0', paused: false, divisions: [] });
		}
		if (!signedIn) return reply(res, 401, { error: 'unauthorized', message: 'Sign in first.' });
		let body = '';
		req.on('data', (chunk) => (body += chunk));
		req.on('end', () => {
			const json = body ? JSON.parse(body) : {};
			switch (route) {
				case 'GET me': return reply(res, 200, me);
				case 'POST me': return reply(res, 200, Object.assign(me, json));
				case 'GET me/tokens': return reply(res, 200, { tokens });
				case 'POST me/tokens/revoke': {
					const i = tokens.findIndex((t) => t.id === json.id);
					if (i >= 0) tokens.splice(i, 1);
					if (json.id === 's1') signedIn = false;
					return reply(res, 200, {});
				}
				case 'GET me/bots': return reply(res, 200, { bots });
				case 'POST bots/retire': {
					const bot = bots.find((b) => b.id === json.bot);
					if (bot) bot.state = 'retired';
					return reply(res, 200, {});
				}
				case 'POST bots/upload-url':
					return reply(res, 200, { url: `/mock-s3/${json.sha256}.jar`, headers: { 'Content-Length': String(json.size), 'x-amz-checksum-sha256': 'bW9jaw==' }, expires: now() });
				case 'POST bots/submit':
					if (json.source === 'url' && String(json.url).includes('bad')) {
						return reply(res, 400, { error: 'invalid', message: 'robot.version is missing\nmock.Bad.class is not in the jar' });
					}
					return reply(res, 200, { bot: 'mock.Submitted', version: '1.0', vid: 9, state: 'probation', codesize: 900, weightClass: 'mini' });
				case 'POST auth/logout': signedIn = false; return reply(res, 204);
				default: return reply(res, 404, { error: 'not_found', message: 'No such endpoint.' });
			}
		});
	};
	return {
		name: 'rumble-api-mock',
		apply: () => process.env.RUMBLE_API_MOCK === 'on',
		configureServer: (server) => void server.middlewares.use(middleware),
		configurePreviewServer: (server) => void server.middlewares.use(middleware)
	};
}

export default defineConfig({
	plugins: [
		rumbleFixtures(),
		rumbleApiMock(),
		sveltekit({
			compilerOptions: {
				// mdsvex 0.12 still emits <script context="module"> for frontmatter.
				warningFilter: (w) =>
					!(w.code === 'script_context_deprecated' && w.filename?.endsWith('.md')),
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Docs and the home page are prerendered; data pages (rankings, bots, pairings) render
			// in the browser from 200.html, which CloudFront serves for unknown paths (design §12.1).
			adapter: adapter({ fallback: '200.html' }),
			preprocess: [
				mdsvex({
					extensions: ['.md'],
					layout: fileURLToPath(new URL('./src/lib/docs/DocPage.svelte', import.meta.url)),
					layoutPropForwarding: 'runes',
					rehypePlugins: [rehypeSlug]
				})
			],
			extensions: ['.svelte', '.md']
		})
	],
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
