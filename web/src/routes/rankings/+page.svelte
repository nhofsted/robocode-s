<script lang="ts">
	import { resolve } from '$app/paths';
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import { CLASS_NAMES, DIVISION_NAMES, formatCount, formatDate } from '#lib/rumble/format.ts';
	import type { Division, RumbleSummary } from '#lib/rumble/types.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const ORDER: Division[] = ['1v1', 'melee', 'teams', 'twinduel'];
	const ABOUT: Record<Division, string> = {
		'1v1': 'One on one, 800×600.',
		melee: 'Ten bots at once, 1000×1000.',
		teams: 'Teams of five, 1200×1200.',
		twinduel: 'Teams of two, 800×800.'
	};

	const groups = $derived(
		ORDER.map((division) => ({
			division,
			rumbles: data.index.rumbles.filter((r) => r.division === division)
		})).filter((g) => g.rumbles.length > 0)
	);

	const href = (r: RumbleSummary) => resolve('/rankings/[rumble]', { rumble: r.id });
</script>

<svelte:head>
	<title>Rankings · robocode-s</title>
</svelte:head>

<div class="container">
	<h1>Rankings</h1>
	<SampleNotice />
	<p class="muted">Updated {formatDate(data.index.generatedAt)}.</p>

	<div class="groups">
		{#each groups as g (g.division)}
			<section class="panel">
				<h2>{DIVISION_NAMES[g.division]}</h2>
				<p class="muted about">{ABOUT[g.division]}</p>
				<ul>
					{#each g.rumbles as r (r.id)}
						<li>
							<a href={href(r)}>{CLASS_NAMES[r.weightClass]}</a>
							<span class="mono muted">
								{formatCount(r.bots)} bots · {formatCount(r.battles)} battles
							</span>
						</li>
					{/each}
				</ul>
			</section>
		{/each}
	</div>
</div>

<style>
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
		gap: 1rem;
		margin-top: 1.5rem;
	}
	section {
		padding: 1rem 1.2rem;
	}
	h2 {
		margin: 0;
		font-size: 1.15rem;
	}
	.about {
		margin: 0.2rem 0 0.8rem;
		font-size: 0.85rem;
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	li {
		display: flex;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.35rem 0;
		border-top: 1px solid var(--line);
		flex-wrap: wrap;
	}
	li .mono {
		font-size: 0.8rem;
	}
</style>
