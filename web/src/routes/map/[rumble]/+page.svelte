<script lang="ts">
	// The style map of a rumble (types.ts `StyleMap`). Reached from a bot's page (`?bot=<id>`
	// selects it), not from the navigation: it is a curiosity, not a ranking.
	import { resolve } from '$app/paths';
	import { replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import StyleMapChart from '#lib/components/StyleMapChart.svelte';
	import { CLASS_NAMES, formatCount, formatDate, formatNumber, rumbleName, shortName } from '#lib/rumble/format.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const title = $derived(rumbleName(data.summary));
	const rumble = $derived(data.summary.id);
	const bots = $derived(data.map?.bots ?? []);
	const indexOf = $derived(new Map(bots.map((b, i) => [b.id, i])));
	const info = $derived(new Map(data.rankings.bots.map((r) => [r.id, { rank: r.rank, aps: r.aps }])));

	const wanted = $derived(page.url.searchParams.get('bot'));
	let picked: number | null | undefined = $state(undefined);
	// The URL's bot until the visitor picks another (or none).
	const selected = $derived(picked !== undefined ? picked : wanted !== null ? (indexOf.get(wanted) ?? null) : null);
	const unplacedWanted = $derived(wanted !== null && !indexOf.has(wanted) && data.map?.unplaced.includes(wanted));

	function select(index: number | null) {
		picked = index;
		const url = new URL(page.url.href);
		if (index === null) url.searchParams.delete('bot');
		else url.searchParams.set('bot', bots[index].id);
		replaceState(url, {});
	}

	let search = $state('');
	function find(event: SubmitEvent) {
		event.preventDefault();
		const i = indexOf.get(search.trim());
		if (i !== undefined) {
			select(i);
			search = '';
		}
	}

	const chosen = $derived(selected === null ? null : bots[selected]);
	const chosenInfo = $derived(chosen ? info.get(chosen.id) : undefined);
</script>

<svelte:head>
	<title>{title} style map · robocode-s</title>
</svelte:head>

<div class="container">
	<p class="crumbs">
		<a href={resolve('/rankings')}>Rankings</a> /
		<a href={resolve('/rankings/[rumble]', { rumble })}>{title}</a> /
	</p>
	<h1>{title} style map</h1>
	<SampleNotice />

	{#if data.siblings.length > 1}
		<nav class="classes" aria-label="Weight class">
			{#each data.siblings as s (s.id)}
				<a href={resolve('/map/[rumble]', { rumble: s.id })} aria-current={s.id === rumble ? 'page' : undefined}>
					{CLASS_NAMES[s.weightClass]}
				</a>
			{/each}
		</nav>
	{/if}

	{#if !data.map}
		<p>This rumble has no style map yet: too few of its bots have fought enough opponents.</p>
	{:else}
		<p class="lede">
			Bots close together have similar strengths and weaknesses: they beat, and lose to, the same
			opponents by more than their ratings predict. Colour is APS. Click a bot to see the bots that
			play most like it.
		</p>

		<div class="tools">
			<p class="muted">
				{formatCount(bots.length)} bots · updated daily, last on {formatDate(data.map.generatedAt)}
			</p>
			<form class="search" onsubmit={find}>
				<label>
					<span class="visually-hidden">Find a bot</span>
					<input type="search" list="map-bots" placeholder="Find a bot" bind:value={search} />
				</label>
				<datalist id="map-bots">
					{#each bots as b (b.id)}<option value={b.id}></option>{/each}
				</datalist>
			</form>
		</div>

		{#if unplacedWanted}
			<p class="notice">
				{shortName(wanted ?? '')} isn't on the map yet: it has fought fewer than a quarter of the
				other bots. It appears once it has.
			</p>
		{/if}

		<div class="layout">
			<StyleMapChart {bots} {info} {selected} onselect={select} />

			<aside class="panel">
				{#if chosen}
					<h2>
						<a href={resolve('/bot/[id]', { id: chosen.id }) + `?rumble=${encodeURIComponent(rumble)}`}>{shortName(chosen.id)}</a>
					</h2>
					{#if chosenInfo}
						<p class="muted">#{chosenInfo.rank} · APS {formatNumber(chosenInfo.aps, 2)}</p>
					{/if}
					<h3>Plays most like</h3>
					<ol class="near">
						{#each chosen.near as [j, distance] (j)}
							{@const other = bots[j]}
							<li>
								<button type="button" class="link" onclick={() => select(j)}>{shortName(other.id)}</button>
								<a
									class="num"
									href={resolve('/compare/[rumble]/[a]/[b]', { rumble, a: chosen.id, b: other.id })}
									title="Compare opponent by opponent. Style distance: their results differ by about {formatNumber(distance, 1)} points per opponent once strength is taken out"
									>{formatNumber(distance, 1)}</a
								>
							</li>
						{/each}
					</ol>
					<p class="note muted">
						By style distance: how much their results differ per opponent, in score points, once
						strength is taken out. The map only approximates it; this list is exact. Click a distance
						to compare the two.
					</p>
				{:else}
					<p class="muted">Click a dot, or find a bot, to see the bots that play most like it.</p>
				{/if}
			</aside>
		</div>

		<details class="method">
			<summary>How the map is made</summary>
			<p>
				A model explains every pairing score by the two bots' strengths plus {data.map.rank}
				rock-paper-scissors patterns: in each, some kinds of bots do better against others than
				strength alone predicts. It is fitted to all pairings, weighted by battles. The number of
				patterns was checked against pairings the model was not shown: more patterns predict a little
				better still, but cost more to compute every day. A bot's style is
				what the patterns say about it, with its strength taken out, so a weak bot and a strong bot
				can play alike.
			</p>
			<p>
				The style distance of two bots is how much their expected results differ, opponent by
				opponent, after taking their strengths out (root mean square over the whole field, in score
				points at an even match). The map is a t-SNE layout of these distances. It keeps
				neighbourhoods: {formatNumber(data.map.trustworthiness * 100, 0)}% trustworthy at 10
				neighbours. It does not keep the distances between groups, so read clusters, not the
				space between them. Each day's map starts from the previous one, so bots move only as their
				results change.
			</p>
			{#if data.map.unplaced.length > 0}
				<p>
					Not on the map yet ({formatCount(data.map.unplaced.length)}): bots that have fought fewer
					than a quarter of the field, whose style the model can't pin down yet:
					{#each data.map.unplaced as id, k (id)}<a href={resolve('/bot/[id]', { id })}>{shortName(id)}</a
						>{k < data.map.unplaced.length - 1 ? ', ' : '.'}{/each}
				</p>
			{/if}
		</details>
	{/if}
</div>

<style>
	.classes {
		display: flex;
		gap: 0.25rem;
		flex-wrap: wrap;
		margin-bottom: 1rem;
	}
	.classes a {
		padding: 0.25rem 0.75rem;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		color: var(--text-2);
		text-decoration: none;
	}
	.classes a[aria-current='page'] {
		background: var(--panel-2);
		border-color: var(--line-strong);
		color: var(--text);
	}
	.lede {
		max-width: 48rem;
		color: var(--text-2);
	}
	.tools {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-bottom: 0.6rem;
		font-size: 0.85rem;
	}
	.tools p {
		margin: 0;
	}
	.layout {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 16rem;
		gap: 1rem;
		align-items: start;
	}
	@media (max-width: 52rem) {
		.layout {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	aside {
		padding: 0.8rem 1rem;
	}
	aside h2 {
		margin: 0 0 0.2rem;
		font-size: 1.1rem;
	}
	aside h3 {
		font-size: 0.85rem;
		margin: 0.8rem 0 0.3rem;
		color: var(--text-2);
	}
	.near {
		margin: 0;
		padding-left: 1.4rem;
	}
	.near li {
		padding: 0.12rem 0;
	}
	.near .num {
		float: right;
		font-size: 0.85rem;
	}
	button.link {
		background: none;
		border: none;
		padding: 0;
		color: var(--link, var(--text));
		cursor: pointer;
		font: inherit;
		text-align: left;
	}
	button.link:hover {
		text-decoration: underline;
	}
	.note {
		font-size: 0.8rem;
	}
	.notice {
		font-size: 0.9rem;
	}
	.method {
		margin-top: 1.5rem;
		max-width: 48rem;
		color: var(--text-2);
		font-size: 0.9rem;
	}
	.method summary {
		cursor: pointer;
		color: var(--text);
	}
</style>
