<script lang="ts">
	import { resolve } from '$app/paths';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import FighterText from '#lib/components/FighterText.svelte';
	import Flag from '#lib/components/Flag.svelte';
	import Tank from '#lib/components/Tank.svelte';
	import { tankColors } from '#lib/components/tank-colors.ts';
	import { css } from '#lib/components/sprites.ts';
	import { formatBytes, formatNumber, rumbleName, shortName } from '#lib/rumble/format.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const rumble = $derived(data.summary.id);
	const sides = $derived([
		{ bot: data.a, entry: data.ea, pairing: data.pa },
		{ bot: data.b, entry: data.eb, pairing: data.pb }
	]);
	const scoreA = $derived(data.pa?.score ?? null);

	const botHref = (id: string) => resolve('/bot/[id]', { id }) + `?rumble=${rumble}`;
</script>

<svelte:head>
	<title>{shortName(data.a.id)} vs {shortName(data.b.id)} · robocode-s</title>
</svelte:head>

<div class="container">
	<p class="crumbs">
		<a href={resolve('/rankings')}>Rankings</a> /
		<a href={resolve('/rankings/[rumble]', { rumble })}>{rumbleName(data.summary)}</a> /
	</p>
	<h1>Pairing</h1>
	<SampleNotice />

	<section class="duel panel">
		{#each sides as s, i (s.bot.id)}
			<div class="side" class:right={i === 1}>
				<Tank colors={tankColors(s.bot.id)} size={88} heading={i === 0 ? 90 : 270} />
				<div>
					<a class="name" href={botHref(s.bot.id)}>{shortName(s.bot.id)}</a>
					<div class="muted small">
						<Flag code={s.bot.author.country} /> {s.bot.author.name} ·
						<span class="mono">{s.bot.versions[0].version} · #{s.entry.rank}</span>
					</div>
				</div>
			</div>
			{#if i === 0}<div class="vs"><FighterText text="VS" height="3.2rem" /></div>{/if}
		{/each}
	</section>

	{#if scoreA === null}
		<p class="notice">These two bots haven't fought yet. The rumble schedules pairings with the fewest battles first.</p>
	{:else}
		<div
			class="bar"
			role="img"
			aria-label="{shortName(data.a.id)} {formatNumber(scoreA, 1)}%, {shortName(data.b.id)} {formatNumber(100 - scoreA, 1)}%"
		>
			<div class="fill a" style:width="{scoreA}%" style:background={css(tankColors(data.a.id).body)}>
				<span>{formatNumber(scoreA, 1)}%</span>
			</div>
			<div class="fill b" style:width="{100 - scoreA}%" style:background={css(tankColors(data.b.id).body)}>
				<span>{formatNumber(100 - scoreA, 1)}%</span>
			</div>
		</div>

		<div class="table-scroll stats">
			<table class="data">
				<thead>
					<tr>
						<th></th>
						<th class="num">{shortName(data.a.id)}</th>
						<th class="num">{shortName(data.b.id)}</th>
					</tr>
				</thead>
				<tbody>
					<tr>
						<th scope="row">Score in this pairing</th>
						{#each sides as s (s.bot.id)}<td class="num">{formatNumber(s.pairing?.score, 2)}</td>{/each}
					</tr>
					<tr>
						<th scope="row">Rounds survived</th>
						{#each sides as s (s.bot.id)}<td class="num">{formatNumber(s.pairing?.survival, 1)}</td>{/each}
					</tr>
					<tr>
						<th scope="row" title="Score scaled between the lowest and highest score any bot got against this opponent">Normalised score</th>
						{#each sides as s (s.bot.id)}<td class="num">{formatNumber(s.pairing?.normalized, 1)}</td>{/each}
					</tr>
					<tr>
						<th scope="row">Overall APS</th>
						{#each sides as s (s.bot.id)}<td class="num">{formatNumber(s.entry.aps, 2)}</td>{/each}
					</tr>
					<tr>
						<th scope="row">Battles together</th>
						<td class="num" colspan="2">{data.pa?.battles ?? 0}</td>
					</tr>
				</tbody>
			</table>
		</div>
	{/if}

	<p>
		<a href={resolve('/compare/[rumble]/[a]/[b]', { rumble, a: data.a.id, b: data.b.id })}
			>Compare {shortName(data.a.id)} and {shortName(data.b.id)} against all their opponents</a
		>
	</p>

	<section id="replays" class="replay panel">
		<h2>Watch a battle</h2>
		{#if data.summary.division !== '1v1'}
			<p>
				Melee rounds are recorded per bot, with all the robots in the field: see the replays on
				<a href={botHref(data.a.id)}>{shortName(data.a.id)}</a>'s and
				<a href={botHref(data.b.id)}>{shortName(data.b.id)}</a>'s pages.
			</p>
		{:else}
			{#await data.rounds}
				<p class="muted">Looking for recorded rounds&hellip;</p>
			{:then found}
				{#if found}
					<p class="muted">
						Rounds recorded from this pairing's battles, played right here in the browser.
					</p>
					<div class="groups">
						{#each [found.a, found.b] as side (side.bot)}
							{@const won = found.rounds.filter((r) => r.winner === side.bot)}
							<div>
								<h3>Won by {shortName(side.bot)} <span class="muted">({won.length})</span></h3>
								{#if won.length > 0}
									<ul class="rounds">
										{#each won as r (r.file)}
											<li>
												<a href={resolve('/replay/[rumble]/[a]/[b]/[file]', { rumble, a: data.a.id, b: data.b.id, file: r.file })}>
													Round {r.round + 1}
												</a>
												<span class="muted">{formatBytes(r.size)}</span>
											</li>
										{/each}
									</ul>
								{:else}
									<p class="muted">None recorded yet.</p>
								{/if}
							</div>
						{/each}
					</div>
				{:else}
					<p>No rounds of this pairing are recorded yet.</p>
					<p class="muted">
						The rumble records five rounds won by each side for every pairing that involves one
						of the top 50 bots of a division, and for each bot its best, worst and problem
						pairings. The slots fill as volunteers run battles, so check back later.
					</p>
				{/if}
			{/await}
		{/if}
	</section>
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
	.vs {
		line-height: 0;
	}
	.bar {
		display: flex;
		height: 2.2rem;
		gap: 2px;
		border-radius: var(--radius);
		overflow: hidden;
		margin: 1.2rem 0;
		background: var(--bg);
	}
	.fill {
		display: flex;
		align-items: center;
		padding: 0 0.6rem;
		min-width: 3.5rem;
		font-family: var(--font-mono);
		font-weight: 600;
		color: #fff;
		text-shadow: 0 1px 2px #000;
	}
	.fill.b {
		justify-content: flex-end;
	}
	.stats {
		max-width: 36rem;
	}
	.stats th[scope='row'] {
		font-weight: 400;
		color: var(--text-2);
	}
	.replay {
		margin-top: 2rem;
		padding: 1rem 1.2rem;
		max-width: 48rem;
	}
	.replay h2 {
		margin-top: 0;
	}
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
		gap: 1rem;
	}
	.groups h3 {
		margin: 0.4rem 0;
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
		padding: 0.2rem 0.6rem;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		background: var(--bg);
		font-size: 0.9rem;
		white-space: nowrap;
	}
	.rounds li span {
		font-size: 0.8rem;
		margin-left: 0.3rem;
	}
</style>
