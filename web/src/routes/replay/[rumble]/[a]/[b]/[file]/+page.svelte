<script lang="ts">
	import { resolve } from '$app/paths';
	import RoundPage from '#lib/replay/RoundPage.svelte';
	import { rumbleName, shortName } from '#lib/rumble/format.ts';
	import { displayNames, trackUrl1v1 } from '#lib/rumble/recordings.ts';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();

	const found = $derived(data.found);
	const round = $derived(found.rounds[data.at]);
	// The track's robots by index: A has aIndex, B the other.
	const robots = $derived(round.aIndex === 0 ? [found.a, found.b] : [found.b, found.a]);
	const [nameA, nameB] = $derived(displayNames([found.a.bot, found.b.bot]));
	const rumble = $derived(data.summary.id);

	const roundHref = (file: string) =>
		resolve('/replay/[rumble]/[a]/[b]/[file]', { rumble, a: params.a, b: params.b, file });
	const link = (i: number) =>
		found.rounds[i] ? { href: roundHref(found.rounds[i].file), label: `Round ${found.rounds[i].round + 1}` } : undefined;
	const pairingHref = $derived(
		resolve('/pairing/[rumble]/[a]/[b]', { rumble, a: params.a, b: params.b }) + '#replays'
	);
</script>

<RoundPage
	title="{nameA} vs {nameB} — round {round.round + 1}"
	crumbs={[
		{ href: resolve('/rankings'), label: 'Rankings' },
		{ href: resolve('/rankings/[rumble]', { rumble }), label: rumbleName(data.summary) },
		{ href: pairingHref, label: `${shortName(found.a.bot)} vs ${shortName(found.b.bot)}` }
	]}
	url={trackUrl1v1(found.engine, found.a, found.b, round.file)}
	names={displayNames(robots.map((r) => r.bot))}
	colorKeys={robots.map((r) => r.bot)}
	fighters={robots.map((r) => ({
		id: r.bot,
		version: r.version,
		href: resolve('/bot/[id]', { id: r.bot }) + `?rumble=${rumble}`
	}))}
	outcome="{round.winner === found.a.bot ? nameA : nameB} won this round."
	round={round.round + 1}
	seed={round.seed}
	engine={found.engine}
	size={round.size}
	rules={{ rounds: data.summary.rounds, ...data.summary.battlefield }}
	downloadName="{shortName(found.a.bot)}_vs_{shortName(found.b.bot)}_round{round.round + 1}_{round.file}.track"
	back={{ href: pairingHref, label: `All rounds of ${nameA} vs ${nameB}` }}
	prev={link(data.at - 1)}
	next={link(data.at + 1)}
/>
