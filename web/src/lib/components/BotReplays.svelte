<script lang="ts">
	// A bot page's replays: in 1v1 the opponents with recorded rounds (best pairings first, the
	// rest behind a filter and a button), in melee the rounds won and not won.
	import { resolve } from '$app/paths';
	import Tank from './Tank.svelte';
	import { tankColors } from './tank-colors.ts';
	import { formatCount, scoreHeat, shortName } from '../rumble/format.ts';
	import { meleeRounds, opponentTallies } from '../rumble/recordings.ts';
	import type { BotId, Pairing, Recordings1v1, RecordingsMelee } from '../rumble/types.ts';

	interface Props {
		bot: BotId;
		rumble: string;
		/** The index of the bot's current version, null where there is none (yet). */
		rec: Recordings1v1 | RecordingsMelee | null;
		pairings: readonly Pairing[];
	}
	let { bot, rumble, rec, pairings }: Props = $props();

	// The filter appears once the list is long enough to scroll.
	const FILTER_FROM = 12;
	let query = $state('');

	const scores = $derived(new Map(pairings.map((p) => [p.opponent, p.score])));
	// The index covers the whole division; only opponents met in this rumble have a pairing
	// page here (a nano bot's recorded rounds against a mega bot belong to the mega rumble).
	const opponents = $derived(
		rec && 'pairs' in rec
			? opponentTallies(rec)
					.filter((t) => scores.has(t.opponent))
					.map((t) => ({ ...t, score: scores.get(t.opponent) ?? null }))
					.sort((a, b) => (b.score ?? -1) - (a.score ?? -1))
			: []
	);
	const matching = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return q ? opponents.filter((o) => o.opponent.toLowerCase().includes(q)) : opponents;
	});
	const rounds = $derived(rec && 'rounds' in rec ? meleeRounds(rec) : []);
	const groups = $derived([
		{ title: 'Rounds won', list: rounds.filter((r) => r.won) },
		{ title: 'Rounds not won', list: rounds.filter((r) => !r.won) }
	]);
	let allRounds = $state(false);
	const roundCap = 10;
</script>

{#if !rec || ('pairs' in rec && opponents.length === 0)}
	<p class="muted">
		No rounds of {shortName(bot)} are recorded yet. The rumble records the top 50 bots of a
		division{rumble.startsWith('1v1') ? ', and every bot\'s best, worst and problem pairings' : ''}; the
		slots fill as volunteers run battles.
	</p>
{:else if 'pairs' in rec}
	<div class="table-tools">
		<span class="muted">{formatCount(opponents.length)} opponents with recorded rounds</span>
		{#if opponents.length > FILTER_FROM}
			<label>
				<span class="visually-hidden">Filter opponents with replays</span>
				<input type="search" placeholder="Filter opponents" bind:value={query} />
			</label>
		{/if}
	</div>
	<div class="table-scroll capped">
		<table class="data">
			<thead>
				<tr>
					<th>Opponent</th>
					<th class="num" title="This bot's percent score against the opponent">Score</th>
					<th class="num">Won</th>
					<th class="num">Lost</th>
					<th></th>
				</tr>
			</thead>
			<tbody>
				{#each matching as o (o.opponent)}
					<tr>
						<td>
							<a class="opp" href={resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: bot, b: o.opponent }) + '#replays'}>
								<Tank colors={tankColors(o.opponent, o.colors)} size={26} heading={270} />
								{shortName(o.opponent)} <span class="mono muted">{o.version}</span>
							</a>
						</td>
						<td class="num score" style:background={scoreHeat(o.score)}>{o.score === null ? '–' : o.score.toFixed(1)}</td>
						<td class="num">{o.won}</td>
						<td class="num">{o.lost}</td>
						<td>
							<a href={resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: bot, b: o.opponent }) + '#replays'}>Watch</a>
						</td>
					</tr>
				{:else}
					<tr><td colspan="5" class="muted">No opponent matches.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
{:else}
	<div class="groups">
		{#each groups as g (g.title)}
			<div class="panel">
				<h3>{g.title} <span class="muted">({g.list.length})</span></h3>
				{#if g.list.length > 0}
					<ul class="rounds">
						{#each allRounds ? g.list : g.list.slice(0, roundCap) as r (r.file)}
							<li>
								<a href={resolve('/replay/[rumble]/[bot]/[file]', { rumble, bot, file: r.file })}>Round {r.round + 1}</a>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="muted">None recorded yet.</p>
				{/if}
			</div>
		{/each}
	</div>
	{#if !allRounds && groups.some((g) => g.list.length > roundCap)}
		<p><button type="button" class="secondary" onclick={() => (allRounds = true)}>Show all rounds</button></p>
	{/if}
{/if}

<style>
	.table-tools {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-bottom: 0.6rem;
		font-size: 0.85rem;
	}
	.capped {
		max-height: 32rem;
	}
	.opp {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		white-space: nowrap;
	}
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
	}
	.groups .panel {
		padding: 0.8rem 1rem;
	}
	h3 {
		margin: 0 0 0.5rem;
		font-size: 0.95rem;
	}
	.rounds {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
	}
	.rounds li {
		padding: 0.15rem 0.6rem;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		background: var(--bg);
		font-size: 0.9rem;
		white-space: nowrap;
	}
	.secondary {
		background: transparent;
		color: var(--radar);
		border: 1px solid var(--radar-dim);
	}
	.score {
		font-family: var(--font-mono);
	}
</style>
