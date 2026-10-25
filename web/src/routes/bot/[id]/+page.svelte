<script lang="ts">
	import { resolve } from '$app/paths';
	import HistoryChart from '#lib/components/HistoryChart.svelte';
	import { goto } from '$app/navigation';
	import BotReplays from '#lib/components/BotReplays.svelte';
	import Flag from '#lib/components/Flag.svelte';
	import PairingStrip from '#lib/components/PairingStrip.svelte';
	import ScatterChart from '#lib/components/ScatterChart.svelte';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import Tank from '#lib/components/Tank.svelte';
	import { tankColors } from '#lib/components/tank-colors.ts';
	import {
		CLASS_NAMES,
		METRICS,
		compareBy,
		formatBytes,
		formatCount,
		formatDate,
		formatNumber,
		jarFileName,
		jarHref,
		packageName,
		rumbleName,
		scoreHeat,
		shortName
	} from '#lib/rumble/format.ts';
	import type { Pairing } from '#lib/rumble/types.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const bot = $derived(data.bot);
	const current = $derived(bot.versions[0]);
	const entry = $derived(bot.rumbles[data.summary.id]);
	const rumble = $derived(data.summary.id);

	const opponentRank = $derived(new Map(data.rankings.bots.map((r) => [r.id, r.rank])));
	const pairings = $derived(
		entry.pairings.map((p) => ({ ...p, opponentRank: opponentRank.get(p.opponent) ?? null }))
	);
	type Row = Pairing & { opponentRank: number | null };

	const best = $derived([...pairings].sort((a, b) => b.score - a.score).slice(0, 5));
	const worst = $derived([...pairings].sort((a, b) => a.score - b.score).slice(0, 5));
	const problems = $derived([...pairings].sort((a, b) => a.pbi - b.pbi).slice(0, 5));

	// LiteRumble's bot charts: how this bot does against each opponent, by opponent strength.
	const opponents = $derived(new Map(data.rankings.bots.map((r) => [r.id, r])));
	const dots = (y: (p: Row) => number, x: (o: { aps: number; survival: number }) => number) =>
		pairings.flatMap((p) => {
			const o = opponents.get(p.opponent);
			return o
				? [{ id: p.opponent, name: shortName(p.opponent), x: x(o), y: y(p), href: pairingHref(p.opponent) }]
				: [];
		});
	const scoreDots = $derived(dots((p) => p.score, (o) => o.aps));
	const survivalDots = $derived(dots((p) => p.survival, (o) => o.survival));
	const pbiDots = $derived(dots((p) => p.pbi, (o) => o.aps));
	const pbiRange = $derived.by((): [number, number] => {
		const m = Math.max(10, ...pairings.map((p) => Math.abs(p.pbi)));
		const r = Math.ceil(m / 10) * 10;
		return [-r, r];
	});

	// The opponent hovered in a chart or in the best/worst and problem lists, marked in the charts.
	let spotlight: string | null = $state(null);
	const spot = (id: string) => ({
		onpointerenter: () => (spotlight = id),
		onpointerleave: () => (spotlight = null),
		onfocusin: () => (spotlight = id),
		onfocusout: () => (spotlight = null)
	});

	let compareWith = $state('');
	const compareIds = $derived(new Set(data.rankings.bots.map((r) => r.id)));
	function compare(event: SubmitEvent) {
		event.preventDefault();
		const other = compareWith.trim();
		if (compareIds.has(other) && other !== bot.id) {
			goto(resolve('/compare/[rumble]/[a]/[b]', { rumble, a: bot.id, b: other }));
		}
	}

	const HISTORY = [
		{ key: 'aps', label: 'APS', digits: 2, invert: false },
		{ key: 'rank', label: 'Rank', digits: 0, invert: true },
		{ key: 'glicko', label: 'Glicko-2', digits: 0, invert: false }
	] as const;
	let historyKey: (typeof HISTORY)[number]['key'] = $state('aps');
	const historyDef = $derived(HISTORY.find((h) => h.key === historyKey)!);
	const historyPoints = $derived(
		entry.history.map((h) => ({ date: h.date, value: h[historyKey], version: h.version }))
	);
	const releases = $derived(bot.versions.map((v) => ({ date: v.addedAt, label: `v${v.version}` })));

	type SortKey = 'opponentRank' | 'opponent' | 'score' | 'survival' | 'normalized' | 'pbi' | 'battles';
	let sortKey: SortKey = $state('score');
	let direction: 'asc' | 'desc' = $state('desc');
	let query = $state('');
	const rows = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const get = (r: Row) => (sortKey === 'opponent' ? shortName(r.opponent).toLowerCase() : r[sortKey]);
		return pairings
			.filter((p) => !q || p.opponent.toLowerCase().includes(q))
			.sort(compareBy(get, direction));
	});
	function sortBy(key: SortKey) {
		if (sortKey === key) direction = direction === 'asc' ? 'desc' : 'asc';
		else {
			sortKey = key;
			direction = key === 'opponent' || key === 'opponentRank' ? 'asc' : 'desc';
		}
	}
	const arrow = (key: SortKey) => (sortKey === key ? (direction === 'asc' ? '▲' : '▼') : '');

	const pairingHref = (opponent: string) =>
		resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: bot.id, b: opponent });
</script>

<svelte:head>
	<title>{shortName(bot.id)} · robocode-s</title>
</svelte:head>

<div class="container">
	<p class="crumbs">
		<a href={resolve('/rankings')}>Rankings</a> /
		<a href={resolve('/rankings/[rumble]', { rumble })}>{rumbleName(data.summary)}</a> /
	</p>

	<header class="head">
		<Tank colors={tankColors(bot.id, bot.versions[0]?.colors)} size={88} heading={20} gunHeading={60} radarHeading={100} />
		<div>
			<h1>{shortName(bot.id)} <span class="version mono">{current.version}</span></h1>
			<p class="sub">
				<span class="mono muted">{packageName(bot.id)}</span> · by
				<Flag code={bot.author.country} />
				{#if bot.author.github}
					<a href="https://github.com/{bot.author.github}">{bot.author.name}</a>
				{:else}
					{bot.author.name}
				{/if}
				{#if current.codesize !== null}
					· <span title="Codesize in bytes">{CLASS_NAMES[current.weightClass]} ({formatCount(current.codesize)} bytes)</span>
				{/if}
				{#if bot.homepage}
					· <a href={bot.homepage} rel="external nofollow">Homepage</a>
				{/if}
			</p>
			{#if bot.description}<p class="desc">{bot.description}</p>{/if}
		</div>
	</header>

	<SampleNotice />

	{#if data.joined.length > 1}
		<nav class="tabs" aria-label="Rumble">
			{#each data.joined as r (r.id)}
				<a
					href={resolve('/bot/[id]', { id: bot.id }) + `?rumble=${r.id}`}
					aria-current={r.id === rumble ? 'page' : undefined}>{rumbleName(r)}</a
				>
			{/each}
		</nav>
	{/if}

	<section class="tiles" aria-label="Scores in {rumbleName(data.summary)}">
		<div class="tile panel">
			<span class="label">Rank</span>
			<span class="value mono">{entry.rank}<small>/{data.summary.bots}</small></span>
		</div>
		{#each METRICS as m (m.key)}
			<div class="tile panel" title={m.title}>
				<span class="label">{m.label}</span>
				<span class="value mono">
					{formatNumber(entry[m.key], m.digits)}
					{#if m.key === 'glicko'}<small>±{formatNumber(entry.glickoRd, 0)}</small>{/if}
				</span>
			</div>
		{/each}
		<div class="tile panel">
			<span class="label">Battles</span>
			<span class="value mono">{formatCount(entry.battles)}</span>
		</div>
	</section>

	<section>
		<div class="section-head">
			<h2>History</h2>
			<div class="toggle" role="group" aria-label="Metric">
				{#each HISTORY as h (h.key)}
					<button
						class="sort"
						aria-pressed={historyKey === h.key}
						onclick={() => (historyKey = h.key)}>{h.label}</button
					>
				{/each}
			</div>
		</div>
		<div class="panel chart">
			<HistoryChart
				points={historyPoints}
				label={historyDef.label}
				digits={historyDef.digits}
				invert={historyDef.invert}
				markers={releases}
			/>
		</div>
	</section>

	<section>
		<h2>Pairings</h2>
		<PairingStrip bot={bot.id} {rumble} {pairings} />

		<div class="scatters">
			<ScatterChart
				title="Score by opponent strength"
				dots={scoreDots}
				xLabel="Opponent APS"
				yLabel="Score"
				highlight={spotlight}
				onhover={(id) => (spotlight = id)}
			/>
			<ScatterChart
				title="Survival by opponent survival"
				dots={survivalDots}
				xLabel="Opponent survival"
				yLabel="Survival"
				highlight={spotlight}
				onhover={(id) => (spotlight = id)}
			/>
			<ScatterChart
				title="Problem bots (KNN-PBI)"
				dots={pbiDots}
				xLabel="Opponent APS"
				yLabel="Score vs. similar bots"
				yDomain={pbiRange}
				ref={0}
				signed
				highlight={spotlight}
				onhover={(id) => (spotlight = id)}
			/>
		</div>
		<p class="muted note">
			KNN-PBI compares this bot's score against each opponent with the scores of the bots ranked
			closest to it: below zero, the opponent is a problem bot for this one.
		</p>

		<div class="extremes">
			{#each [{ title: 'Best pairings', list: best }, { title: 'Worst pairings', list: worst }] as group (group.title)}
				<div class="panel">
					<h3>{group.title}</h3>
					<ol>
						{#each group.list as p (p.opponent)}
							<li {...spot(p.opponent)}>
								<a href={pairingHref(p.opponent)}>{shortName(p.opponent)}</a>
								<span class="num score" style:background={scoreHeat(p.score)}>{formatNumber(p.score, 1)}</span>
							</li>
						{/each}
					</ol>
				</div>
			{/each}
		</div>

		<div class="extremes problems">
			<div class="panel">
				<h3>Problem bots <span class="muted">(lowest KNN-PBI)</span></h3>
				<ol>
					{#each problems as p (p.opponent)}
						<li {...spot(p.opponent)}>
							<a href={pairingHref(p.opponent)}>{shortName(p.opponent)}</a>
							<span class="num score" style:background={scoreHeat(50 + p.pbi)}>{p.pbi > 0 ? '+' : ''}{formatNumber(p.pbi, 1)}</span>
						</li>
					{/each}
				</ol>
			</div>
			<form class="panel compare" onsubmit={compare}>
				<h3>Compare</h3>
				<label for="compare-with">Compare {shortName(bot.id)} with another bot, opponent by opponent:</label>
				<div class="row">
					<input id="compare-with" type="search" list="compare-bots" placeholder="Bot name" bind:value={compareWith} />
					<button type="submit" disabled={!compareIds.has(compareWith.trim()) || compareWith.trim() === bot.id}>Compare</button>
				</div>
				<datalist id="compare-bots">
					{#each data.rankings.bots as r (r.id)}
						{#if r.id !== bot.id}<option value={r.id}></option>{/if}
					{/each}
				</datalist>
			</form>
		</div>

		<div class="table-tools">
			<span class="muted">{formatCount(pairings.length)} of {formatCount(data.summary.bots - 1)} opponents fought</span>
			<label>
				<span class="visually-hidden">Filter opponents</span>
				<input type="search" placeholder="Filter opponents" bind:value={query} />
			</label>
		</div>
		<div class="table-scroll pairings">
			<table class="data">
				<thead>
					<tr>
						<th class="num"><button class="sort" aria-pressed={sortKey === 'opponentRank'} onclick={() => sortBy('opponentRank')}>Their # {arrow('opponentRank')}</button></th>
						<th><button class="sort" aria-pressed={sortKey === 'opponent'} onclick={() => sortBy('opponent')}>Opponent {arrow('opponent')}</button></th>
						<th class="num" title="This bot's percent score against the opponent"><button class="sort" aria-pressed={sortKey === 'score'} onclick={() => sortBy('score')}>Score {arrow('score')}</button></th>
						<th class="num" title="Rounds survived"><button class="sort" aria-pressed={sortKey === 'survival'} onclick={() => sortBy('survival')}>Surv {arrow('survival')}</button></th>
						<th class="num" title="Score normalised against all bots' scores vs this opponent"><button class="sort" aria-pressed={sortKey === 'normalized'} onclick={() => sortBy('normalized')}>NPP {arrow('normalized')}</button></th>
						<th class="num" title="Score minus the score of the bots ranked closest to this one, against this opponent"><button class="sort" aria-pressed={sortKey === 'pbi'} onclick={() => sortBy('pbi')}>PBI {arrow('pbi')}</button></th>
						<th class="num"><button class="sort" aria-pressed={sortKey === 'battles'} onclick={() => sortBy('battles')}>Battles {arrow('battles')}</button></th>
					</tr>
				</thead>
				<tbody>
					{#each rows as p (p.opponent)}
						<tr>
							<td class="num">{p.opponentRank ?? '–'}</td>
							<td>
								<a class="opp" href={pairingHref(p.opponent)}>
									<Tank colors={tankColors(p.opponent, opponents.get(p.opponent)?.colors)} size={26} heading={270} />
									<Flag code={opponents.get(p.opponent)?.country} />
									{shortName(p.opponent)} <span class="mono muted">{p.opponentVersion}</span>
								</a>
							</td>
							<td class="num score" style:background={scoreHeat(p.score)}>{formatNumber(p.score, 2)}</td>
							<td class="num">{formatNumber(p.survival, 1)}</td>
							<td class="num">{formatNumber(p.normalized, 1)}</td>
							<td class="num">{p.pbi > 0 ? '+' : ''}{formatNumber(p.pbi, 1)}</td>
							<td class="num">{p.battles}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>

	{#if data.summary.division === '1v1' || data.summary.division === 'melee'}
		<section id="replays">
			<h2>Replays</h2>
			{#await data.replays}
				<p class="muted">Looking for recorded rounds&hellip;</p>
			{:then rec}
				<BotReplays bot={bot.id} {rumble} {rec} pairings={entry.pairings} />
			{/await}
		</section>
	{/if}

	<section>
		<h2>Versions</h2>
		<div class="table-scroll">
			<table class="data">
				<thead>
					<tr><th>Version</th><th>Added</th><th>Retired</th><th class="num">Codesize</th><th class="num">Size</th><th>Download</th></tr>
				</thead>
				<tbody>
					{#each bot.versions as v (v.version)}
						<tr>
							<td class="mono">{v.version}</td>
							<td>{formatDate(v.addedAt)}</td>
							<td>{v.retiredAt ? formatDate(v.retiredAt) : 'current'}</td>
							<td class="num">{v.codesize === null ? '–' : formatCount(v.codesize)}</td>
							<td class="num">{formatBytes(v.size)}</td>
							<td>
								<a
									href={jarHref(v.sha256)}
									download={jarFileName(bot.id, v.version)}
									rel="external"
									title="SHA-256 {v.sha256}">{jarFileName(bot.id, v.version)}</a
								>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>
</div>

<style>
	.crumbs {
		margin: 1.5rem 0 0;
		font-size: 0.85rem;
	}
	.head {
		display: flex;
		gap: 1.2rem;
		align-items: center;
		margin: 0.5rem 0 1rem;
	}
	.head h1 {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.version {
		font-family: var(--font-mono);
		font-size: 0.9rem;
		color: var(--text-2);
		text-shadow: none;
	}
	.sub,
	.desc {
		margin: 0.3rem 0 0;
		font-size: 0.9rem;
	}
	.desc {
		color: var(--text-2);
	}
	.tabs {
		display: flex;
		gap: 0.25rem;
		flex-wrap: wrap;
		border-bottom: 1px solid var(--line);
		margin: 1.2rem 0 0;
	}
	.tabs a {
		padding: 0.4rem 0.8rem;
		text-decoration: none;
		color: var(--text-2);
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
	}
	.tabs a[aria-current='page'] {
		color: var(--radar);
		border-bottom-color: var(--radar);
	}
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
		gap: 0.6rem;
		margin: 1.2rem 0;
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
		color: var(--text);
	}
	.value small {
		font-size: 0.75rem;
		color: var(--muted);
		font-weight: 400;
		margin-left: 0.15rem;
	}
	.section-head {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: 1rem;
		flex-wrap: wrap;
	}
	.toggle {
		display: flex;
		gap: 0.9rem;
		font-size: 0.85rem;
	}
	.chart {
		padding: 0.8rem 0.8rem 0.6rem;
	}
	.extremes {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
		margin: 1.2rem 0;
	}
	.extremes > .panel {
		padding: 0.8rem 1rem;
	}
	.extremes h3 {
		margin: 0 0 0.4rem;
		font-size: 0.95rem;
	}
	.extremes ol {
		margin: 0;
		padding-left: 1.2rem;
	}
	.extremes li {
		padding: 0.15rem 0;
	}
	.extremes li .score {
		float: right;
		padding: 0 0.4rem;
		border-radius: 3px;
		font-size: 0.85rem;
	}
	.scatters {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
		margin-top: 1.2rem;
		padding: 1rem;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}
	.note {
		font-size: 0.8rem;
	}
	.compare label {
		display: block;
		font-size: 0.85rem;
		color: var(--text-2);
		margin-bottom: 0.5rem;
	}
	.compare .row {
		display: flex;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.compare input {
		flex: 1;
		min-width: 10rem;
	}
	.table-tools {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-bottom: 0.6rem;
		font-size: 0.85rem;
	}
	.pairings {
		max-height: 32rem;
	}
	.opp {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		white-space: nowrap;
	}
</style>
