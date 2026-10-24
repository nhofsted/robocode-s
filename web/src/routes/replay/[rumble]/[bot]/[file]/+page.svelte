<script lang="ts">
	import { resolve } from '$app/paths';
	import RoundPage from '#lib/replay/RoundPage.svelte';
	import { rumbleName, shortName } from '#lib/rumble/format.ts';
	import { displayNames, trackUrlMelee } from '#lib/rumble/recordings.ts';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();

	const round = $derived(data.rounds[data.at]);
	const rumble = $derived(data.summary.id);
	const name = $derived(shortName(params.bot));

	const roundHref = (file: string) =>
		resolve('/replay/[rumble]/[bot]/[file]', { rumble, bot: params.bot, file });
	const link = (i: number) =>
		data.rounds[i] ? { href: roundHref(data.rounds[i].file), label: `Round ${data.rounds[i].round + 1}` } : undefined;
	const botHref = $derived(resolve('/bot/[id]', { id: params.bot }) + `?rumble=${rumble}`);
</script>

<RoundPage
	title="{name} melee — round {round.round + 1}, {round.won ? 'won' : 'not won'}"
	crumbs={[
		{ href: resolve('/rankings'), label: 'Rankings' },
		{ href: resolve('/rankings/[rumble]', { rumble }), label: rumbleName(data.summary) },
		{ href: botHref, label: name }
	]}
	url={trackUrlMelee(data.rec.engine, data.rec, round.file)}
	names={displayNames(round.robots.map((r) => r.bot))}
	colorKeys={round.robots.map((r) => r.bot)}
	fighters={round.robots.map((r) => ({
		id: r.bot,
		version: r.version,
		href: resolve('/bot/[id]', { id: r.bot }) + `?rumble=${rumble}`
	}))}
	outcome="{name} {round.won ? 'won' : 'did not win'} this round."
	round={round.round + 1}
	seed={round.seed}
	engine={data.rec.engine}
	size={round.size}
	rules={{ rounds: data.summary.rounds, ...data.summary.battlefield }}
	downloadName="{name}_melee_round{round.round + 1}_{round.file}.track"
	back={{ href: botHref + '#replays', label: `All recorded rounds of ${name}` }}
	prev={link(data.at - 1)}
	next={link(data.at + 1)}
/>
