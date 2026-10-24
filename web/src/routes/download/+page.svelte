<script lang="ts">
	import { onMount } from 'svelte';
	import { resolve } from '$app/paths';
	import { VERSION, RELEASE_URL, SHA256SUMS_URL } from '#lib/release.ts';

	const options = [
		{
			id: 'windows',
			href: '/download/windows',
			name: 'Windows',
			text: 'An installer for Windows 10 or 11 (x64), or the winget package.'
		},
		{
			id: 'linux',
			href: '/download/linux',
			name: 'Linux',
			text: 'The apt and dnf repositories, or a .deb, .rpm or .tar.gz, for x86-64 and arm64.'
		},
		{
			id: 'macos',
			href: '/download/macos',
			name: 'macOS',
			text: 'A disk image for Apple Silicon or Intel, or Homebrew.'
		},
		{
			id: 'source',
			href: '/download/source',
			name: 'Build from source',
			text: 'For other platforms, or to change robocode-s itself: build it with Rust and Java.'
		}
	] as const;

	// The visitor's system, to point at its page; a guess, never a redirect.
	let detected = $state<string | null>(null);
	onMount(() => {
		const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
		if (/android|iphone|ipad|ipod/i.test(navigator.userAgent)) return;
		const s = (nav.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
		detected = /win/.test(s) ? 'windows' : /mac/.test(s) ? 'macos' : /linux|x11|cros/.test(s) ? 'linux' : null;
	});
</script>

<svelte:head>
	<title>Download · robocode-s</title>
	<meta
		name="description"
		content="Download robocode-s {VERSION} for Windows, Linux and macOS, or build it from source."
	/>
</svelte:head>

<h1>Download robocode-s</h1>
<p class="lead">
	Version {VERSION}. Choose your system. <a href={RELEASE_URL}>Release notes</a> ·
	<a href={SHA256SUMS_URL}>SHA256SUMS</a>. New here? Start with
	<a href={resolve('/docs/getting-started')}>Getting started</a>.
</p>

<ul class="choices">
	{#each options as o (o.id)}
		<li>
			<a class="choice panel" class:you={detected === o.id} href={resolve(o.href)}>
				<span class="name">{o.name}</span>
				{#if detected === o.id}<span class="badge">Your system</span>{/if}
				<span class="text">{o.text}</span>
			</a>
		</li>
	{/each}
</ul>

<style>
	.choices {
		list-style: none;
		margin: 1.5rem 0 0;
		padding: 0;
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 1rem;
		max-width: 48rem;
	}
	.choice {
		display: block;
		height: 100%;
		padding: 1rem 1.3rem 1.1rem !important;
		color: var(--text);
		text-decoration: none;
	}
	.choice:hover {
		border-color: var(--radar-dim);
	}
	.choice.you {
		border-color: var(--radar);
	}
	.name {
		font-size: 1.2rem;
		font-weight: 600;
		color: var(--radar);
	}
	.badge {
		margin-left: 0.6rem;
		font-size: 0.75rem;
		color: var(--muted);
		border: 1px solid var(--line-strong);
		border-radius: 999px;
		padding: 0.05rem 0.5rem;
	}
	.text {
		display: block;
		margin-top: 0.4rem;
		color: var(--text-2);
	}
	@media (max-width: 600px) {
		.choices {
			grid-template-columns: 1fr;
		}
	}
</style>
