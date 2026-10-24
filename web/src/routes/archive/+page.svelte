<script lang="ts">
	import SampleNotice from '#lib/components/SampleNotice.svelte';
	import {
		formatBytes,
		formatCount,
		formatDate,
		jarFileName,
		jarHref
	} from '#lib/rumble/format.ts';
	import type { ArchiveVersion } from '#lib/rumble/types.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const STATES: Record<ArchiveVersion['state'], string> = {
		active: 'Active',
		probation: 'On probation',
		retired: 'Retired',
		failed: 'Failed'
	};
	const PAGE = 50;

	let query = $state('');
	let shown = $state(PAGE);

	const bots = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const all = data.archive.bots;
		return q
			? all.filter((b) => b.id.toLowerCase().includes(q) || b.author.toLowerCase().includes(q))
			: all;
	});
	const versionCount = $derived(data.archive.bots.reduce((n, b) => n + b.versions.length, 0));
</script>

<svelte:head>
	<title>Archive · robocode-s</title>
	<meta name="description" content="Download any version of any bot of the robocode-s rumble." />
</svelte:head>

<div class="container">
	<h1>Archive</h1>
	<SampleNotice />
	<p class="lead">
		Every version of every bot in the rumble, current and retired. A download is saved as
		<span class="mono">&lt;id&gt;_&lt;version&gt;.jar</span>. Bots and versions that were taken down
		are not listed.
	</p>

	<div class="meta">
		<p class="muted">
			{formatCount(data.archive.bots.length)} bots · {formatCount(versionCount)} versions · updated
			every 6 hours, last {formatDate(data.archive.generatedAt)}
		</p>
		<label class="search">
			<span class="visually-hidden">Search bots</span>
			<input
				type="search"
				placeholder="Search by bot id or author"
				bind:value={query}
				oninput={() => (shown = PAGE)}
			/>
		</label>
	</div>

	{#each bots.slice(0, shown) as b (b.id)}
		<section class="bot">
			<h2 class="mono">{b.id}</h2>
			<p class="muted author">{b.author}</p>
			<div class="table-scroll">
				<table class="data">
					<caption class="visually-hidden">Versions of {b.id}</caption>
					<thead>
						<tr>
							<th>Version</th><th>State</th><th>Added</th><th class="num">Size</th><th>Download</th>
						</tr>
					</thead>
					<tbody>
						{#each b.versions as v (v.version)}
							<tr>
								<td class="mono">{v.version}</td>
								<td>{STATES[v.state]}</td>
								<td>{formatDate(v.added)}</td>
								<td class="num">{formatBytes(v.size)}</td>
								<td>
									<a
										href={jarHref(v.sha256)}
										download={jarFileName(b.id, v.version)}
										rel="external">{jarFileName(b.id, v.version)}</a
									>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>
	{:else}
		<p class="muted">{query ? `No bot matches “${query}”.` : 'The archive is empty.'}</p>
	{/each}

	{#if bots.length > shown}
		<p>
			<button onclick={() => (shown += PAGE)}>
				Show more ({formatCount(bots.length - shown)} more bots)
			</button>
		</p>
	{/if}
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
	.bot {
		margin: 1.5rem 0;
	}
	.bot h2 {
		margin: 0;
		font-size: 1.05rem;
		overflow-wrap: anywhere;
	}
	.author {
		margin: 0.1rem 0 0.4rem;
		font-size: 0.85rem;
	}
</style>
