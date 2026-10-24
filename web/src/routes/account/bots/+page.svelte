<script lang="ts">
	import { resolve } from '$app/paths';
	import SignInPrompt from '#lib/components/SignInPrompt.svelte';
	import { ApiError, getMyBots, retireBot, type BotInfo, type VersionState } from '#lib/api.ts';
	import { CLASS_NAMES, formatCount, formatDate } from '#lib/rumble/format.ts';
	import type { WeightClass } from '#lib/rumble/types.ts';
	import { session } from '#lib/session.svelte.ts';

	const STATES: Record<VersionState, string> = {
		probation: 'On probation: its first battles run on trusted volunteers before it is ranked.',
		active: 'Active: ranked.',
		failed: 'Failed: it did not load, crashed or was disabled in too many battles.',
		retired: 'Retired.',
		removed: 'Removed.'
	};

	let bots: BotInfo[] | undefined = $state();
	let error = $state('');
	let problem = $state('');

	const text = (e: unknown) => (e instanceof ApiError ? e.message : 'Something went wrong.');

	async function load() {
		try {
			bots = await getMyBots();
		} catch (e) {
			error = text(e);
		}
	}

	$effect(() => {
		if (session.state === 'in') void load();
	});

	async function retire(bot: BotInfo) {
		if (!confirm(`Retire ${bot.id}? It leaves the rankings; you can submit a new version later.`)) return;
		problem = '';
		try {
			await retireBot(bot.id);
			await load();
		} catch (e) {
			problem = text(e);
		}
	}

	const weight = (w: string | undefined) => (w && w in CLASS_NAMES ? CLASS_NAMES[w as WeightClass] : (w ?? ''));
</script>

<svelte:head>
	<title>My bots · robocode-s</title>
</svelte:head>

<div class="container page">
	<p class="crumbs"><a href={resolve('/account')}>Account</a> /</p>
	<h1>My bots</h1>

	{#if session.state !== 'in'}
		<SignInPrompt reason="to see your bots" />
	{:else}
		<p class="lead"><a class="button" href={resolve('/submit')}>Submit a bot or a new version</a></p>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		{#if error}
			<p class="error" role="alert">{error}</p>
		{:else if !bots}
			<p class="muted">Loading…</p>
		{:else if bots.length === 0}
			<p class="muted">You have no bots yet.</p>
		{/if}

		{#each bots ?? [] as bot (bot.id)}
			<section class="panel bot">
				<header>
					<h2>
						{#if bot.state === 'active'}
							<a href={resolve('/bot/[id]', { id: bot.id })}>{bot.id}</a>
						{:else}
							{bot.id}
						{/if}
						<span class="state {bot.state}" title={STATES[bot.state]}>{bot.state}</span>
					</h2>
					{#if bot.state !== 'retired' && bot.state !== 'removed'}
						<button class="danger" onclick={() => retire(bot)}>Retire</button>
					{/if}
				</header>
				<div class="table-scroll">
					<table class="data">
						<thead>
							<tr><th>Version</th><th>State</th><th>Class</th><th class="num">Codesize</th><th>Submitted</th></tr>
						</thead>
						<tbody>
							{#each bot.versions as v (v.vid)}
								<tr>
									<td class="mono">{v.version}</td>
									<td>
										<span class="state {v.state}" title={STATES[v.state]}>{v.state}</span>
										{#if v.probation}
											<span class="muted small">
												{v.probation.battles} battles, {v.probation.failed} failed
											</span>
										{/if}
									</td>
									<td>{weight(v.weightClass)}</td>
									<td class="num">{v.codesize === undefined ? '' : formatCount(v.codesize)}</td>
									<td>{v.created ? formatDate(v.created) : ''}</td>
								</tr>
								{#if v.failures?.length}
									<tr class="detail">
										<td colspan="5">
											<strong>Why it failed:</strong>
											<ul>
												{#each v.failures as f, i (i)}<li>{f}</li>{/each}
											</ul>
											Fix it and submit a new version (a new <code>robot.version</code>).
										</td>
									</tr>
								{/if}
							{/each}
						</tbody>
					</table>
				</div>
			</section>
		{/each}
	{/if}
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.crumbs {
		margin: 0;
		font-size: 0.85rem;
	}
	.bot {
		max-width: 56rem;
		padding: 0.2rem 1.3rem 1.2rem;
		margin-top: 1rem;
	}
	header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 1rem;
	}
	h2 {
		font-size: 1.15rem;
	}
	.state {
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		margin-left: 0.4rem;
		color: var(--muted);
	}
	.state.active {
		color: var(--radar);
	}
	.state.probation {
		color: var(--amber);
	}
	.state.failed {
		color: var(--danger);
	}
	.small {
		font-size: 0.8rem;
		margin-left: 0.4rem;
	}
	.detail td {
		color: var(--text-2);
		white-space: normal;
	}
	.detail ul {
		margin: 0.3rem 0;
		padding-left: 1.2rem;
	}
	.error {
		color: var(--danger);
	}
</style>
