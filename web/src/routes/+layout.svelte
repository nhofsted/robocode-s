<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import Tank from '#lib/components/Tank.svelte';
	import { REPO_URL } from '#lib/site.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const LOGO = { body: [63, 138, 79], gun: [159, 212, 168], radar: [255, 194, 92] } as const;

	const nav = [
		{ href: '/rankings', label: 'Rankings' },
		{ href: '/docs', label: 'Docs' }
	] as const;

	const current = (href: string) =>
		page.url.pathname === href || page.url.pathname.startsWith(href + '/') ||
		(href === '/rankings' && /^\/(bot|pairing)\//.test(page.url.pathname));
</script>

<svelte:head>
	<link rel="icon" href="/favicon.png" type="image/png" />
</svelte:head>

<a class="skip" href="#main">Skip to content</a>

<header>
	<div class="container bar">
		<a class="logo" href={resolve('/')} aria-label="robocode-s home">
			<Tank colors={LOGO} size={36} heading={0} gunHeading={45} radarHeading={80} />
			<span class="pixel">robocode-s</span>
		</a>
		<nav aria-label="Main">
			{#each nav as item (item.href)}
				<a href={resolve(item.href)} aria-current={current(item.href) ? 'page' : undefined}>
					{item.label}
				</a>
			{/each}
			<a href={REPO_URL} rel="external">GitHub</a>
		</nav>
	</div>
</header>

<main id="main">
	{@render children()}
</main>

<footer>
	<div class="container">
		<p>
			robocode-s is open source under the
			<a href="https://www.eclipse.org/legal/epl-2.0/">Eclipse Public License 2.0</a>.
			It reimplements <a href="https://robocode.sourceforge.io/">Robocode</a>, created by
			Mathew A. Nelson and maintained by Flemming N. Larsen and contributors; it is an
			independent project, not affiliated with them.
		</p>
	</div>
</footer>

<style>
	.skip {
		position: absolute;
		left: -999px;
		top: 0;
		background: var(--amber);
		color: var(--bg);
		padding: 0.5rem 1rem;
		z-index: 10;
	}
	.skip:focus {
		left: 0;
	}

	header {
		border-bottom: 1px solid var(--line);
		background: color-mix(in srgb, var(--bg) 88%, transparent);
		backdrop-filter: blur(6px);
		position: sticky;
		top: 0;
		z-index: 5;
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		min-height: 56px;
		flex-wrap: wrap;
	}
	.logo {
		display: inline-flex;
		align-items: center;
		gap: 0.6rem;
		color: var(--radar);
		text-decoration: none;
		font-size: 0.85rem;
	}
	nav {
		display: flex;
		gap: 1.2rem;
		font-size: 0.95rem;
	}
	nav a {
		color: var(--text-2);
		text-decoration: none;
		padding: 0.2rem 0;
		border-bottom: 2px solid transparent;
	}
	nav a:hover {
		color: var(--text);
	}
	nav a[aria-current='page'] {
		color: var(--radar);
		border-bottom-color: var(--radar);
	}

	main {
		min-height: calc(100vh - 56px - 8rem);
		padding-bottom: 4rem;
	}

	footer {
		border-top: 1px solid var(--line);
		color: var(--muted);
		font-size: 0.8rem;
		padding: 1.5rem 0 2rem;
		background: var(--bg);
	}
	footer p {
		margin: 0;
		max-width: 60rem;
	}
</style>
