<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import Ground from '#lib/components/Ground.svelte';
	import Logo from '#lib/components/Logo.svelte';
	import { REPO_URL } from '#lib/site.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const nav = [
		{ href: '/rankings', label: 'Rankings' },
		{ href: '/docs', label: 'Docs' }
	] as const;

	const current = (href: string) =>
		page.url.pathname === href || page.url.pathname.startsWith(href + '/') ||
		(href === '/rankings' && /^\/(bot|pairing)\//.test(page.url.pathname));
</script>

<svelte:head>
	<link rel="icon" href="/favicon-32.png" sizes="32x32" type="image/png" />
	<link rel="icon" href="/favicon-256.png" sizes="256x256" type="image/png" />
	<link rel="apple-touch-icon" href="/favicon-256.png" />
</svelte:head>

<Ground />

<a class="skip" href="#main">Skip to content</a>

<header>
	<div class="container bar">
		<a class="logo" href={resolve('/')} aria-label="robocode-s home">
			<Logo height={28} />
		</a>
		<nav aria-label="Main">
			{#each nav as item (item.href)}
				<a href={resolve(item.href)} aria-current={current(item.href) ? 'page' : undefined}>
					{item.label}
				</a>
			{/each}
		</nav>
	</div>
</header>

<main id="main">
	{@render children()}
</main>

<footer>
	<div class="container">
		<p>
			robocode-s is open source <a href={REPO_URL} rel="external">on GitHub</a> under the
			<a href="https://www.eclipse.org/legal/epl-2.0/">Eclipse Public License 2.0</a>.
			It reimplements <a href="https://robocode.sourceforge.io/">Robocode</a>, created by
			Mathew A. Nelson and maintained by Flemming N. Larsen and contributors.
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
		text-decoration: none;
		padding: 0.2rem 0;
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
