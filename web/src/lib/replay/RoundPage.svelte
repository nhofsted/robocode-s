<script lang="ts">
	// One recorded rumble round: the player, what happened, how to re-run it, and the way to
	// the neighbouring rounds. The routes under /replay work out the title, names and links.
	import { resolve } from '$app/paths';
	import Replay from './Replay.svelte';
	import { TrackError, fetchTrack } from './fetch.ts';
	import { formatBytes, formatCount } from '../rumble/format.ts';

	interface Link {
		href: string;
		label: string;
	}
	interface Props {
		title: string;
		crumbs: Link[];
		/** The track, same origin under /rec. */
		url: string;
		/** Per track robot index. */
		names: string[];
		colorKeys: string[];
		/** The robots in track order, with versions. */
		fighters: { id: string; version: string; href: string }[];
		outcome: string;
		/** 1-based. */
		round: number;
		seed: string;
		engine: number;
		size: number;
		rules: { rounds: number; width: number; height: number };
		downloadName: string;
		back: Link;
		prev?: Link;
		next?: Link;
	}
	let {
		title,
		crumbs,
		url,
		names,
		colorKeys,
		fighters,
		outcome,
		round,
		seed,
		engine,
		size,
		rules,
		downloadName,
		back,
		prev,
		next
	}: Props = $props();

	const track = $derived(fetchTrack(url));
	const message = (e: unknown) =>
		e instanceof TrackError && e.kind === 'missing'
			? 'This recording is no longer available. Recordings are removed when a bot releases a new version, so the pairing may have fresh ones.'
			: e instanceof Error
				? e.message
				: 'This replay could not be loaded.';
</script>

<svelte:head>
	<title>{title} · robocode-s</title>
</svelte:head>

<div class="container page">
	<p class="crumbs">
		{#each crumbs as c (c.href)}<a href={c.href}>{c.label}</a> /{' '}{/each}
	</p>
	<h1>{title}</h1>
	<p class="outcome">{outcome}</p>

	{#key url}
		{#await track}
			<p class="loading" role="status">Loading the replay&hellip;</p>
		{:then t}
			<Replay track={t} {names} {colorKeys} />
		{:catch e}
			<p class="notice" role="alert"><strong>No replay.</strong> {message(e)}</p>
		{/await}
	{/key}

	<nav class="rounds" aria-label="Rounds">
		{#if prev}<a class="button secondary" href={prev.href}>&larr; {prev.label}</a>{/if}
		<a href={back.href}>{back.label}</a>
		{#if next}<a class="button secondary next" href={next.href}>{next.label} &rarr;</a>{/if}
	</nav>

	<section class="panel facts">
		<h2>About this round</h2>
		<dl>
			<dt>Robots</dt>
			<dd>
				{#each fighters as f, i (i)}{i > 0 ? ', ' : ''}<a href={f.href}>{f.id}</a>
					<span class="mono muted">{f.version}</span>{/each}
				<span class="muted">(in the order they were started)</span>
			</dd>
			<dt>Round</dt>
			<dd>{round} of a battle of {rules.rounds} rounds on a {formatCount(rules.width)}&times;{formatCount(rules.height)} field</dd>
			<dt>Seed</dt>
			<dd class="mono">{seed}</dd>
			<dt>Engine</dt>
			<dd class="mono">e{engine}</dd>
		</dl>
		<p>
			Rumble battles run in <a href={resolve('/docs/robot-development/deterministic-mode')}>deterministic mode</a>:
			the same robots (these versions, started in this order), the same seed and the same
			rules and engine version play the same battle again, so you can
			<a href={resolve('/docs/robot-development/debugging')}>re-run it locally</a> under your debugger.
			This replay shows the round as the engine drew it, not every detail of it.
		</p>
		<p>
			<a class="button" href={url} download={downloadName}>Download .track ({formatBytes(size)})</a>
			<span class="muted">Open it with Watch Battle in the <a href={resolve('/download')}>desktop app</a>, or on the
			<a href={resolve('/replay/file')}>replay file page</a>.</span>
		</p>
	</section>
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.crumbs {
		margin: 0;
		font-size: 0.85rem;
	}
	h1 {
		overflow-wrap: anywhere;
	}
	.outcome {
		margin: -0.3rem 0 1rem;
		color: var(--amber);
		font-weight: 600;
	}
	.loading {
		color: var(--text-2);
		margin: 4rem 0;
	}
	.rounds {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.8rem 1.2rem;
		margin: 1.2rem 0;
	}
	.next {
		margin-left: auto;
	}
	.facts {
		padding: 0.2rem 1.2rem 1rem;
		max-width: 48rem;
	}
	dl {
		display: grid;
		grid-template-columns: max-content minmax(0, 1fr);
		gap: 0.3rem 1.2rem;
		margin: 0 0 1rem;
	}
	dt {
		color: var(--text-2);
	}
	dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.facts p {
		color: var(--text-2);
	}
</style>
