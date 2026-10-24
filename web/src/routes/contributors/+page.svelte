<script lang="ts">
	import { resolve } from '$app/paths';
	import Flag from '#lib/components/Flag.svelte';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import { compareBy, formatCount, formatDate } from '#lib/rumble/format.ts';
	import type { Contributor } from '#lib/rumble/types.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type SortKey = 'rank' | 'login' | 'battles' | 'battlesWeek' | 'tier' | 'since';
	let sortKey: SortKey = $state('rank');
	let direction: 'asc' | 'desc' = $state('asc');
	let query = $state('');

	const TIERS: Record<Contributor['tier'], string> = {
		new: 'New',
		verifier: 'Verifier',
		quarantined: 'Quarantined'
	};

	// The file is sorted by battles; that position is the rank, whatever the table is sorted by.
	const ranked = $derived(data.contributors.contributors.map((c, i) => ({ ...c, rank: i + 1 })));
	type Row = (typeof ranked)[number];

	const getter = (key: SortKey): ((r: Row) => number | string | null) =>
		key === 'login' ? (r) => r.login.toLowerCase() : key === 'since' ? (r) => Date.parse(r.since) : (r) => r[key];

	const rows = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const filtered = q ? ranked.filter((r) => r.login.toLowerCase().includes(q)) : ranked;
		return [...filtered].sort(compareBy(getter(sortKey), direction));
	});

	const total = $derived(ranked.reduce((sum, r) => sum + r.battles, 0));

	function sortBy(key: SortKey) {
		if (sortKey === key) {
			direction = direction === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = key;
			direction = key === 'rank' || key === 'login' || key === 'tier' ? 'asc' : 'desc';
		}
	}

	const arrow = (key: SortKey) => (sortKey === key ? (direction === 'asc' ? '▲' : '▼') : '');
	const sorted = (key: SortKey) =>
		sortKey === key ? (direction === 'asc' ? 'ascending' : 'descending') : undefined;

	const COLUMNS: { key: SortKey; label: string; num: boolean; title?: string }[] = [
		{ key: 'rank', label: '#', num: true },
		{ key: 'login', label: 'Volunteer', num: false },
		{ key: 'battles', label: 'Battles', num: true },
		{ key: 'battlesWeek', label: 'Last 7 days', num: true },
		{ key: 'tier', label: 'Tier', num: false, title: 'New volunteers have a share of their battles re-run to check them' },
		{ key: 'since', label: 'Since', num: false }
	];
</script>

<svelte:head>
	<title>Contributors · robocode-s</title>
	<meta name="description" content="The volunteers who run the robocode-s rumble's battles." />
</svelte:head>

<div class="container">
	<h1>Contributors</h1>
	<SampleNotice />
	<p class="lead">
		The rumble's servers run no robots: these volunteers do. Thank you.
		<a href={resolve('/download')}>Join them</a>. You can hide yourself on your
		<a href={resolve('/account')}>account page</a>.
	</p>

	<div class="meta">
		<p class="muted">
			{formatCount(ranked.length)} volunteers · {formatCount(total)} battles · updated every 6
			hours, last {formatDate(data.contributors.generatedAt)}
		</p>
		<label class="search">
			<span class="visually-hidden">Filter volunteers</span>
			<input type="search" placeholder="Filter by login" bind:value={query} />
		</label>
	</div>

	<div class="table-scroll">
		<table class="data">
			<caption class="visually-hidden">
				Contributors, sorted by {sortKey} {direction === 'asc' ? 'ascending' : 'descending'}
			</caption>
			<thead>
				<tr>
					{#each COLUMNS as c (c.key)}
						<th class:num={c.num} title={c.title} aria-sort={sorted(c.key)}>
							<button class="sort" aria-pressed={sortKey === c.key} onclick={() => sortBy(c.key)}>
								{c.label} {arrow(c.key)}
							</button>
						</th>
					{/each}
				</tr>
			</thead>
			<tbody>
				{#each rows as r (r.login)}
					<tr>
						<td class="num">{r.rank}</td>
						<td class="login">
							<Flag code={r.country} />
							<a href="https://github.com/{encodeURIComponent(r.login)}" rel="external">{r.login}</a>
						</td>
						<td class="num" class:key={sortKey === 'battles'}>{formatCount(r.battles)}</td>
						<td class="num" class:key={sortKey === 'battlesWeek'}>{formatCount(r.battlesWeek)}</td>
						<td class="tier {r.tier}">{TIERS[r.tier]}</td>
						<td>{formatDate(r.since)}</td>
					</tr>
				{:else}
					<tr><td colspan="6" class="muted">{query ? `No volunteer matches “${query}”.` : 'Nobody has contributed yet.'}</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>

<style>
	.lead {
		max-width: 44rem;
		color: var(--text-2);
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
	.login {
		white-space: nowrap;
	}
	td.key {
		color: var(--text);
		font-weight: 600;
	}
	.tier {
		color: var(--text-2);
	}
	.tier.verifier {
		color: var(--radar);
	}
	.tier.quarantined {
		color: var(--amber);
	}
</style>
