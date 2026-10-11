import { fileURLToPath } from 'node:url';
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
 */
function rumbleFixtures(): Plugin {
	let fixtures: Fixtures | undefined;
	const middleware: Connect.NextHandleFunction = (req, res, next) => {
		const url = req.url ?? '';
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

export default defineConfig({
	plugins: [
		rumbleFixtures(),
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
