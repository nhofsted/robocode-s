<script lang="ts">
	// Every pairing of one bot as a cell, opponents ordered by their rank (strongest left), coloured
	// by score: a whole-field view of where a bot wins and loses. Hover or focus shows the value.
	import { resolve } from '$app/paths';
	import { formatNumber, scoreHeat, shortName } from '#lib/rumble/format.ts';
	import type { BotId, Pairing, RumbleId } from '#lib/rumble/types.ts';

	interface Props {
		bot: BotId;
		rumble: RumbleId;
		pairings: (Pairing & { opponentRank: number | null })[];
	}
	let { bot, rumble, pairings }: Props = $props();

	const cells = $derived(
		[...pairings].sort((a, b) => (a.opponentRank ?? Infinity) - (b.opponentRank ?? Infinity))
	);
	let active: (typeof cells)[number] | null = $state(null);
</script>

<div class="strip" role="group" aria-label="Pairing scores by opponent rank">
	{#each cells as p (p.opponent)}
		<a
			href={resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: bot, b: p.opponent })}
			style:background={scoreHeat(p.score)}
			aria-label="vs {shortName(p.opponent)} (rank {p.opponentRank ?? '?'}): {formatNumber(p.score, 1)}%"
			onpointerenter={() => (active = p)}
			onfocus={() => (active = p)}
			onpointerleave={() => (active = null)}
			onblur={() => (active = null)}
		></a>
	{/each}
</div>
<div class="readout" aria-hidden="true">
	{#if active}
		<strong class="mono">{formatNumber(active.score, 1)}%</strong>
		<span>vs {shortName(active.opponent)} · rank {active.opponentRank ?? '?'} · {active.battles} battles</span>
	{:else}
		<span class="scale">
			<span class="swatch" style:background={scoreHeat(20)}></span> losing
			<span class="swatch" style:background={scoreHeat(50)}></span> even
			<span class="swatch" style:background={scoreHeat(80)}></span> winning
			<span class="muted">· strongest opponents on the left</span>
		</span>
	{/if}
</div>

<style>
	.strip {
		display: flex;
		height: 36px;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		background: var(--panel-2);
		overflow: hidden;
	}
	.strip a {
		flex: 1 1 0;
		min-width: 1px;
	}
	.strip a:hover,
	.strip a:focus-visible {
		outline: 2px solid var(--amber);
		outline-offset: -2px;
		z-index: 1;
	}
	.readout {
		min-height: 1.6rem;
		font-size: 0.85rem;
		margin-top: 0.35rem;
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
		color: var(--text-2);
	}
	.scale {
		display: inline-flex;
		gap: 0.35rem;
		align-items: center;
		flex-wrap: wrap;
	}
	.swatch {
		display: inline-block;
		width: 0.9rem;
		height: 0.9rem;
		border: 1px solid var(--line-strong);
		border-radius: 2px;
		margin-left: 0.4rem;
	}
</style>
