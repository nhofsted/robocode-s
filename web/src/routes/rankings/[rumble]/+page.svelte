<script lang="ts">
	import { resolve } from '$app/paths';
	import Flag from '#lib/components/Flag.svelte';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import Tank from '#lib/components/Tank.svelte';
	import { tankColors } from '#lib/components/tank-colors.ts';
	import {
		CLASS_NAMES,
		METRICS,
		compareBy,
		formatCount,
		formatDate,
		formatNumber,
		packageName,
		rumbleName,
		shortName
	} from '#lib/rumble/format.ts';
	import type { RankingRow } from '#lib/rumble/types.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type SortKey = 'rank' | 'name' | 'author' | 'pairings' | 'battles' | (typeof METRICS)[number]['key'];
	let sortKey: SortKey = $state('rank');
	let direction: 'asc' | 'desc' = $state('asc');
	let query = $state('');

	const title = $derived(data.summary ? rumbleName(data.summary) : data.rankings.rumble);
	const opponents = $derived((data.summary?.bots ?? data.rankings.bots.length) - 1);

	const getter = (key: SortKey): ((r: RankingRow) => number | string | null) =>
		key === 'name' ? (r) => shortName(r.id).toLowerCase() : key === 'author' ? (r) => r.author.toLowerCase() : (r) => r[key];

	const rows = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const filtered = q
			? data.rankings.bots.filter(
					(r) => r.id.toLowerCase().includes(q) || r.author.toLowerCase().includes(q)
				)
			: data.rankings.bots;
		return [...filtered].sort(compareBy(getter(sortKey), direction));
	});

	function sortBy(key: SortKey) {
		if (sortKey === key) {
			direction = direction === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = key;
			// Text and rank read best ascending, metrics descending.
			direction = key === 'rank' || key === 'name' || key === 'author' ? 'asc' : 'desc';
		}
	}

	const arrow = (key: SortKey) => (sortKey === key ? (direction === 'asc' ? '▲' : '▼') : '');

	function change(r: RankingRow): { text: string; cls: string; label: string } {
		if (r.rankWeekAgo === null) return { text: 'new', cls: 'new', label: 'new this week' };
		const d = r.rankWeekAgo - r.rank;
		if (d === 0) return { text: '·', cls: 'same', label: 'unchanged since last week' };
		return d > 0
			? { text: `▲${d}`, cls: 'up', label: `up ${d} since last week` }
			: { text: `▼${-d}`, cls: 'down', label: `down ${-d} since last week` };
	}
</script>

<svelte:head>
	<title>{title} rankings · robocode-s</title>
</svelte:head>

<div class="container">
	<p class="crumbs"><a href={resolve('/rankings')}>Rankings</a> /</p>
	<h1>{title}</h1>
	<SampleNotice />

	{#if data.siblings.length > 1}
		<nav class="classes" aria-label="Weight class">
			{#each data.siblings as s (s.id)}
				<a
					href={resolve('/rankings/[rumble]', { rumble: s.id })}
					aria-current={s.id === data.rankings.rumble ? 'page' : undefined}
				>
					{CLASS_NAMES[s.weightClass]}
				</a>
			{/each}
		</nav>
	{/if}

	<div class="meta">
		<p class="muted">
			{#if data.summary}
				{formatCount(data.summary.bots)} bots · {formatCount(data.summary.battles)} battles ·
				{data.summary.rounds} rounds on {data.summary.battlefield.width}×{data.summary.battlefield
					.height} ·
			{/if}
			updated every 6 hours, last {formatDate(data.rankings.generatedAt)}
		</p>
		<label class="search">
			<span class="visually-hidden">Filter bots</span>
			<input type="search" placeholder="Filter by bot or author" bind:value={query} />
		</label>
	</div>

	<div class="table-scroll">
		<table class="data">
			<caption class="visually-hidden">
				{title} rankings, sorted by {sortKey} {direction === 'asc' ? 'ascending' : 'descending'}
			</caption>
			<thead>
				<tr>
					<th class="num" aria-sort={sortKey === 'rank' ? (direction === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button class="sort" aria-pressed={sortKey === 'rank'} onclick={() => sortBy('rank')}>
							# {arrow('rank')}
						</button>
					</th>
					<th class="num"><span title="Change in rank since last week">±</span></th>
					<th>
						<button class="sort" aria-pressed={sortKey === 'name'} onclick={() => sortBy('name')}>
							Bot {arrow('name')}
						</button>
					</th>
					<th>
						<button class="sort" aria-pressed={sortKey === 'author'} onclick={() => sortBy('author')}>
							Author {arrow('author')}
						</button>
					</th>
					{#each METRICS as m (m.key)}
						<th class="num" title={m.title}>
							<button class="sort" aria-pressed={sortKey === m.key} onclick={() => sortBy(m.key)}>
								{m.label} {arrow(m.key)}
							</button>
						</th>
					{/each}
					<th class="num" title="Opponents fought, of all ranked bots">
						<button class="sort" aria-pressed={sortKey === 'pairings'} onclick={() => sortBy('pairings')}>
							Pairings {arrow('pairings')}
						</button>
					</th>
					<th class="num">
						<button class="sort" aria-pressed={sortKey === 'battles'} onclick={() => sortBy('battles')}>
							Battles {arrow('battles')}
						</button>
					</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as r (r.id)}
					{@const c = change(r)}
					<tr>
						<td class="num">{r.rank}</td>
						<td class="num change {c.cls}" title={c.label}>{c.text}</td>
						<td class="bot">
							<a
								href={resolve('/bot/[id]', { id: r.id }) + `?rumble=${data.rankings.rumble}`}
								title="{r.id} {r.version}"
							>
								<Tank colors={tankColors(r.id)} size={28} heading={90} />
								<span class="name">{shortName(r.id)}</span>
								<span class="pkg">{packageName(r.id)}</span>
								<span class="ver mono">{r.version}</span>
							</a>
						</td>
						<td class="author"><span class="who" title={r.author}><Flag code={r.country} /> {r.author}</span></td>
						{#each METRICS as m (m.key)}
							<td class="num" class:key={m.key === sortKey}>{formatNumber(r[m.key], m.digits)}</td>
						{/each}
						<td class="num" class:incomplete={r.pairings < 0.9 * opponents}>
							{formatCount(r.pairings)}
						</td>
						<td class="num">{formatCount(r.battles)}</td>
					</tr>
				{:else}
					<tr><td colspan={6 + METRICS.length} class="muted">No bots match “{query}”.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
	<p class="muted legend">
		Pairing counts in amber: the bot has met fewer than 90% of its opponents, so its scores are
		still settling. <a href={resolve('/docs/rumble')}>How scores are computed</a>
	</p>
</div>

<style>
	.crumbs {
		margin: 1.5rem 0 0;
		font-size: 0.85rem;
	}
	.classes {
		display: flex;
		gap: 0.4rem;
		flex-wrap: wrap;
		margin: 1rem 0;
	}
	.classes a {
		padding: 0.25rem 0.75rem;
		border: 1px solid var(--line-strong);
		border-radius: 999px;
		text-decoration: none;
		color: var(--text-2);
		font-size: 0.9rem;
	}
	.classes a[aria-current='page'] {
		color: var(--bg);
		background: var(--radar);
		border-color: var(--radar);
	}
	.meta {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin: 0.5rem 0 1rem;
	}
	.meta p {
		margin: 0;
		font-size: 0.85rem;
	}
	.search input {
		width: min(20rem, 100%);
	}
	.table-scroll {
		max-height: 75vh;
	}
	/* Long ids and author names are cut with an ellipsis (the full text is the title), so the
	   table fits without scrolling sideways. The short name gives way last. The bot column takes
	   the room the others leave (width 100% with max-width 0 lets a cell shrink below its
	   content); on a phone its 9rem minimum makes the table scroll instead. */
	td.bot {
		width: 100%;
		max-width: 0;
		min-width: 10rem;
	}
	.bot a {
		overflow: hidden;
		display: flex;
		align-items: baseline;
		gap: 0.4rem;
		min-width: 0;
		text-decoration: none;
		white-space: nowrap;
	}
	.name,
	.pkg,
	.ver {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	/* The package and version give way first; the short name only once they are gone. */
	.name {
		flex-shrink: 0;
		max-width: 100%;
	}
	.pkg,
	.ver {
		flex-shrink: 1;
	}
	.bot :global(canvas) {
		align-self: center;
	}
	.name {
		color: var(--text);
		font-weight: 600;
	}
	.bot a:hover .name {
		color: var(--radar);
		text-decoration: underline;
	}
	.pkg,
	.ver {
		color: var(--muted);
		font-size: 0.8rem;
	}
	.author {
		color: var(--text-2);
	}
	.who {
		display: block;
		max-width: 10rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	td.key {
		color: var(--text);
		font-weight: 600;
	}
	.change {
		font-size: 0.75rem;
	}
	.change.up {
		color: var(--radar);
	}
	.change.down {
		color: var(--danger);
	}
	.change.same {
		color: var(--muted);
	}
	.change.new {
		color: var(--amber);
	}
	.incomplete {
		color: var(--amber);
	}
	.legend {
		font-size: 0.8rem;
	}
</style>
