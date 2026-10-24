<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import Ground from '#lib/components/Ground.svelte';
	import Logo from '#lib/components/Logo.svelte';
	import { loginUrl } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { REPO_URL } from '#lib/site.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	onMount(() => void session.load());

	const signIn = $derived(loginUrl(page.url.pathname + page.url.search));

	const nav = [
		{ href: '/download', label: 'Download' },
		{ href: '/rankings', label: 'Rankings' },
		{ href: '/contributors', label: 'Contributors' },
		{ href: '/submit', label: 'Submit' },
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
		<div class="account">
			{#if session.state === 'in' && session.me}
				<a href={resolve('/account')} aria-current={current('/account') ? 'page' : undefined}>
					{session.me.login}
				</a>
				<button class="link" onclick={() => session.signOut()}>Sign out</button>
			{:else if session.state !== 'loading'}
				<a class="signin" href={signIn} rel="external">Sign in with GitHub</a>
			{/if}
		</div>
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
			Its community meets on the <a href="https://robowiki.net/">RoboWiki</a> and
			<a href={resolve('/docs/community')}>elsewhere</a>.
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
	/* The links wrap rather than push the page wider than a phone. */
	nav {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem 1.2rem;
		font-size: 0.95rem;
	}
	@media (max-width: 600px) {
		nav {
			column-gap: 0.9rem;
		}
		/* Wrapped onto several rows the header would take a big bite of a phone's screen. */
		header {
			position: static;
		}
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

	.account {
		display: flex;
		align-items: center;
		gap: 0.9rem;
		font-size: 0.95rem;
		min-height: 1.6rem;
	}
	.account a {
		color: var(--text-2);
		text-decoration: none;
	}
	.account a:hover,
	.account a[aria-current='page'] {
		color: var(--radar);
	}
	.account a.signin {
		color: var(--radar);
		border: 1px solid var(--radar-dim);
		border-radius: var(--radius);
		padding: 0.15rem 0.7rem;
	}
	button.link {
		all: unset;
		cursor: pointer;
		color: var(--muted);
	}
	button.link:hover {
		color: var(--text);
		text-decoration: underline;
	}
	button.link:focus-visible {
		outline: 2px solid var(--amber);
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
