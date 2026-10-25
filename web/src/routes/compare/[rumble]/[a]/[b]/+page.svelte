<script lang="ts">
	// Two bots side by side against every opponent they share, after LiteRumble's BotCompare:
	// where does A do better than B, and against what kind of opponent?
	import { resolve } from '$app/paths';
	import Flag from '#lib/components/Flag.svelte';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import ScatterChart from '#lib/components/ScatterChart.svelte';
	import Tank from '#lib/components/Tank.svelte';
	import { tankColors } from '#lib/components/tank-colors.ts';
	import { compareBy, formatNumber, rumbleName, scoreHeat, shortName } from '#lib/rumble/format.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const rumble = $derived(data.summary.id);
	const nameA = $derived(shortName(data.a.id));
	const nameB = $derived(shortName(data.b.id));
	const opponents = $derived(new Map(data.rankings.bots.map((r) => [r.id, r])));

	const rows = $derived.by(() => {
		const byB = new Map(data.eb.pairings.map((p) => [p.opponent, p]));
		return data.ea.pairings.flatMap((pa) => {
			const pb = byB.get(pa.opponent);
			const o = opponents.get(pa.opponent);
			if (!pb || !o || pa.opponent === data.b.id) return [];
			return [
				{
					opponent: pa.opponent,
					opponentAps: o.aps,
					opponentSurvival: o.survival,
					a: pa.score,
					b: pb.score,
					diff: pa.score - pb.score,
					survivalDiff: pa.survival - pb.survival
				}
			];
		});
	});
	type Row = (typeof rows)[number];

	const better = $derived(rows.filter((r) => r.diff > 0).length);
	const meanDiff = $derived(rows.length ? rows.reduce((t, r) => t + r.diff, 0) / rows.length : 0);
	const head = $derived(data.ea.pairings.find((p) => p.opponent === data.b.id));

	const pairingHref = (opponent: string) =>
		resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: data.a.id, b: opponent });

	const range = (values: number[]): [number, number] => {
		const m = Math.max(10, ...values.map(Math.abs));
		const r = Math.ceil(m / 10) * 10;
		return [-r, r];
	};
	const diffDots = $derived(
		rows.map((r) => ({ id: r.opponent, name: shortName(r.opponent), x: r.opponentAps, y: r.diff, href: pairingHref(r.opponent) }))
	);
	const survivalDots = $derived(
		rows.map((r) => ({ id: r.opponent, name: shortName(r.opponent), x: r.opponentSurvival, y: r.survivalDiff, href: pairingHref(r.opponent) }))
	);

	type SortKey = 'opponent' | 'opponentAps' | 'a' | 'b' | 'diff';
	let sortKey: SortKey = $state('diff');
	let direction: 'asc' | 'desc' = $state('asc');
	const sorted = $derived(
		[...rows].sort(
			compareBy((r: Row) => (sortKey === 'opponent' ? shortName(r.opponent).toLowerCase() : r[sortKey]), direction)
		)
	);
	function sortBy(key: SortKey) {
		if (sortKey === key) direction = direction === 'asc' ? 'desc' : 'asc';
		else {
			sortKey = key;
			direction = key === 'opponent' || key === 'diff' ? 'asc' : 'desc';
		}
	}
	const arrow = (key: SortKey) => (sortKey === key ? (direction === 'asc' ? '▲' : '▼') : '');
	const signed = (v: number, d = 1) => (v > 0 ? '+' : '') + formatNumber(v, d);
</script>

<svelte:head>
	<title>{nameA} vs {nameB}, all opponents · robocode-s</title>
</svelte:head>

<div class="container">
	<p class="crumbs">
		<a href={resolve('/rankings')}>Rankings</a> /
		<a href={resolve('/rankings/[rumble]', { rumble })}>{rumbleName(data.summary)}</a> /
	</p>
	<h1>Compare</h1>
	<SampleNotice />

	<section class="duel panel">
		{#each [{ bot: data.a, entry: data.ea }, { bot: data.b, entry: data.eb }] as s, i (s.bot.id)}
			<div class="side" class:right={i === 1}>
				<Tank colors={tankColors(s.bot.id, s.bot.versions[0]?.colors)} size={72} heading={i === 0 ? 90 : 270} />
				<div>
					<a class="name" href={resolve('/bot/[id]', { id: s.bot.id }) + `?rumble=${rumble}`}>{shortName(s.bot.id)}</a>
					<div class="small muted">
						<Flag code={s.bot.author.country} /> {s.bot.author.name} · <span class="mono">{s.bot.versions[0].version} · #{s.entry.rank} · APS {formatNumber(s.entry.aps, 2)}</span>
					</div>
				</div>
			</div>
		{/each}
	</section>

	<section class="tiles">
		<div class="tile panel">
			<span class="label">Shared opponents</span>
			<span class="value mono">{rows.length}</span>
		</div>
		<div class="tile panel">
			<span class="label">{nameA} better against</span>
			<span class="value mono">{better}</span>
		</div>
		<div class="tile panel">
			<span class="label">{nameB} better against</span>
			<span class="value mono">{rows.length - better}</span>
		</div>
		<div class="tile panel">
			<span class="label">Average difference</span>
			<span class="value mono">{signed(meanDiff, 2)}</span>
		</div>
		{#if head}
			<div class="tile panel">
				<span class="label">Head to head</span>
				<span class="value mono"><a href={pairingHref(data.b.id)}>{formatNumber(head.score, 1)}%</a></span>
			</div>
		{/if}
	</section>

	<div class="scatters panel">
		<ScatterChart
			title="Score difference by opponent strength"
			dots={diffDots}
			xLabel="Opponent APS"
			yLabel="{nameA} − {nameB}"
			yDomain={range(rows.map((r) => r.diff))}
			ref={0}
			signed
		/>
		<ScatterChart
			title="Survival difference by opponent survival"
			dots={survivalDots}
			xLabel="Opponent survival"
			yLabel="{nameA} − {nameB}"
			yDomain={range(rows.map((r) => r.survivalDiff))}
			ref={0}
			signed
		/>
	</div>
	<p class="muted note">Above zero: {nameA} scores better against that opponent than {nameB} does.</p>

	<div class="table-scroll list">
		<table class="data">
			<thead>
				<tr>
					<th><button class="sort" aria-pressed={sortKey === 'opponent'} onclick={() => sortBy('opponent')}>Opponent {arrow('opponent')}</button></th>
					<th class="num"><button class="sort" aria-pressed={sortKey === 'opponentAps'} onclick={() => sortBy('opponentAps')}>Their APS {arrow('opponentAps')}</button></th>
					<th class="num"><button class="sort" aria-pressed={sortKey === 'a'} onclick={() => sortBy('a')}>{nameA} {arrow('a')}</button></th>
					<th class="num"><button class="sort" aria-pressed={sortKey === 'b'} onclick={() => sortBy('b')}>{nameB} {arrow('b')}</button></th>
					<th class="num"><button class="sort" aria-pressed={sortKey === 'diff'} onclick={() => sortBy('diff')}>Difference {arrow('diff')}</button></th>
				</tr>
			</thead>
			<tbody>
				{#each sorted as r (r.opponent)}
					<tr>
						<td>
							<a class="opp" href={pairingHref(r.opponent)}>
								<Flag code={opponents.get(r.opponent)?.country} />
								{shortName(r.opponent)}
							</a>
						</td>
						<td class="num">{formatNumber(r.opponentAps, 2)}</td>
						<td class="num" style:background={scoreHeat(r.a)}>{formatNumber(r.a, 2)}</td>
						<td class="num" style:background={scoreHeat(r.b)}>{formatNumber(r.b, 2)}</td>
						<td class="num" style:background={scoreHeat(50 + r.diff)}>{signed(r.diff, 2)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>

<style>
	.crumbs {
		margin: 1.5rem 0 0;
		font-size: 0.85rem;
	}
	.duel {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding: 1.2rem;
		margin: 1rem 0;
		flex-wrap: wrap;
	}
	.side {
		display: flex;
		align-items: center;
		gap: 0.8rem;
	}
	.side.right {
		flex-direction: row-reverse;
		text-align: right;
	}
	.name {
		font-size: 1.15rem;
		font-weight: 600;
	}
	.small {
		font-size: 0.8rem;
	}
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr));
		gap: 0.6rem;
		margin: 1rem 0;
	}
	.tile {
		padding: 0.6rem 0.8rem;
		display: flex;
		flex-direction: column;
	}
	.label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.value {
		font-size: 1.25rem;
		font-weight: 600;
	}
	.scatters {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
		padding: 1rem;
	}
	.note {
		font-size: 0.8rem;
	}
	.list {
		max-height: 32rem;
	}
	.opp {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		white-space: nowrap;
	}
</style>
