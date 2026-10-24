<script lang="ts">
	import { resolve } from '$app/paths';
	import Flag from '#lib/components/Flag.svelte';
	import SignInPrompt from '#lib/components/SignInPrompt.svelte';
	import {
		ApiError,
		getTokens,
		revokeToken,
		updateMe,
		type TokenInfo,
		type Tier
	} from '#lib/api.ts';
	import { COUNTRIES } from '#lib/countries.ts';
	import { formatCount, formatDate } from '#lib/rumble/format.ts';
	import { session } from '#lib/session.svelte.ts';

	const TIERS: Record<Tier, string> = {
		new: 'New: a share of your battles is re-run by others to check them.',
		verifier: 'Verifier: trusted to run verification and probation battles.',
		quarantined: 'Quarantined: your results are being re-checked.',
		banned: 'Banned.'
	};

	const me = $derived(session.me);

	let tokens: TokenInfo[] | undefined = $state();
	let tokensError = $state('');
	let message = $state('');
	let problem = $state('');
	let busy = $state(false);

	$effect(() => {
		if (session.state !== 'in') return;
		getTokens().then(
			(list) => (tokens = list),
			(e: unknown) => (tokensError = text(e))
		);
	});

	const text = (e: unknown) => (e instanceof ApiError ? e.message : 'Something went wrong.');

	async function change(what: { country?: string | null; hidden?: boolean }, done: string) {
		busy = true;
		message = problem = '';
		try {
			await updateMe(what);
			await session.load(true);
			message = done;
		} catch (e) {
			problem = text(e);
		} finally {
			busy = false;
		}
	}

	async function revoke(t: TokenInfo) {
		const own = t.current;
		if (!confirm(own ? 'Revoke this browser session? You will be signed out.' : `Revoke "${t.label ?? t.id}"? Whatever uses it must sign in again.`)) return;
		problem = '';
		try {
			await revokeToken(t.id);
			if (own) await session.signOut();
			else tokens = tokens?.filter((x) => x.id !== t.id);
		} catch (e) {
			problem = text(e);
		}
	}

	const percent = (v: number) => `${(v * 100).toFixed(1)}%`;
</script>

<svelte:head>
	<title>Account · robocode-s</title>
</svelte:head>

<div class="container page">
	<h1>Account</h1>

	{#if session.state !== 'in' || !me}
		<SignInPrompt reason="to see your account, contribution counters and bots" />
	{:else}
		<p class="lead">
			Signed in as <strong>{me.login}</strong>{me.name ? ` (${me.name})` : ''}, since
			{formatDate(me.since)}.
			<a href={resolve('/account/bots')}>My bots</a> · <a href={resolve('/submit')}>Submit a bot</a>
		</p>

		{#if message}<p class="ok" role="status">{message}</p>{/if}
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}

		<div class="grid">
			<section class="panel">
				<h2>Profile</h2>
				<p>
					<label>
						Country
						<select
							disabled={busy}
							value={me.country ?? ''}
							onchange={(e) => change({ country: e.currentTarget.value || null }, 'Country saved.')}
						>
							<option value="">Not shown</option>
							{#each COUNTRIES as c (c.code)}
								<option value={c.code}>{c.name}</option>
							{/each}
						</select>
					</label>
					{#if me.country}<Flag code={me.country} height={14} />{/if}
				</p>
				<p class="check">
					<label>
						<input
							type="checkbox"
							disabled={busy}
							checked={me.hidden ?? false}
							onchange={(e) =>
								change(
									{ hidden: e.currentTarget.checked },
									e.currentTarget.checked
										? 'You are hidden from the contributors list at its next update.'
										: 'You are listed on the contributors list at its next update.'
								)}
						/>
						Hide me from the <a href={resolve('/contributors')}>contributors list</a>
					</label>
				</p>
				<p class="muted small">
					The list shows your GitHub login, country and battle counts. It is rebuilt every 6
					hours, together with the rankings.
				</p>
			</section>

			<section class="panel">
				<h2>Contribution</h2>
				<p><strong>{me.tier}</strong>. {TIERS[me.tier]}</p>
				<dl>
					<dt>Reputation</dt>
					<dd class="num">{formatCount(me.reputation)}</dd>
					<dt title="The share of your battles that other volunteers re-run to check them">Verify rate</dt>
					<dd class="num">{percent(me.verifyRate)}</dd>
					<dt>Battles contributed</dt>
					<dd class="num">{formatCount(me.battles)}</dd>
					<dt>Verified matches</dt>
					<dd class="num">{formatCount(me.verifiedMatches)}</dd>
					<dt>Mismatches</dt>
					<dd class="num">{formatCount(me.mismatches)}</dd>
				</dl>
				<p class="muted small">
					How to contribute: <a href={resolve('/download')}>Download</a>, then
					<code>robocode-s rumble login</code> and <code>robocode-s rumble run</code>.
				</p>
			</section>
		</div>

		<section class="tokens">
			<h2>Upload tokens and sessions</h2>
			<p class="muted">
				The command line and the GUI sign in with an upload token (<code>robocode-s rumble
				login</code>); browsers use a session. Revoke what you no longer use: it stops working
				within 5 minutes.
			</p>
			{#if tokensError}
				<p class="error" role="alert">{tokensError}</p>
			{:else if !tokens}
				<p class="muted">Loading…</p>
			{:else}
				<div class="table-scroll">
					<table class="data">
						<thead>
							<tr><th>Kind</th><th>Label</th><th>Created</th><th>Last used</th><th></th></tr>
						</thead>
						<tbody>
							{#each tokens as t (t.id)}
								<tr>
									<td>{t.kind === 'upload' ? 'Upload token' : 'Browser session'}{t.current ? ' (this one)' : ''}</td>
									<td>{t.label ?? ''}</td>
									<td>{formatDate(t.created)}</td>
									<td>{t.lastUsed ? formatDate(t.lastUsed) : 'never'}</td>
									<td class="num"><button class="danger" onclick={() => revoke(t)}>Revoke</button></td>
								</tr>
							{:else}
								<tr><td colspan="5" class="muted">No tokens.</td></tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
	{/if}
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.lead {
		color: var(--text-2);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr));
		gap: 1rem;
		margin: 1.5rem 0;
		max-width: 56rem;
	}
	.panel {
		padding: 0.2rem 1.3rem 1rem;
		min-width: 0;
	}
	.check label {
		display: flex;
		gap: 0.5rem;
		align-items: baseline;
	}
	.small {
		font-size: 0.85rem;
	}
	dl {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 0.2rem 1rem;
		margin: 0.5rem 0;
	}
	dt {
		color: var(--text-2);
	}
	dd {
		margin: 0;
	}
	.tokens {
		max-width: 56rem;
	}
	.ok {
		color: var(--radar);
	}
	.error {
		color: var(--danger);
	}
</style>
