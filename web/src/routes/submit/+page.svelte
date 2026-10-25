<script lang="ts">
	import { resolve } from '$app/paths';
	import SignInPrompt from '#lib/components/SignInPrompt.svelte';
	import {
		ApiError,
		getUploadUrl,
		putUpload,
		sha256Hex,
		submitBot,
		type SubmitResult
	} from '#lib/api.ts';
	import { CLASS_NAMES, formatCount } from '#lib/rumble/format.ts';
	import type { WeightClass } from '#lib/rumble/types.ts';
	import { session } from '#lib/session.svelte.ts';

	/** Jar size limit (`jar.maxBytes`, backend §3); the server checks it again. */
	const JAR_MAX = 5 * 1024 * 1024;

	// Melee and 1v1 only for now; teams are not open yet.
	const DIVISIONS = [
		{ id: '1v1', label: '1v1' },
		{ id: 'melee', label: 'Melee' }
	];

	let mode: 'upload' | 'url' = $state('upload');
	let file: File | undefined = $state();
	let url = $state('');
	let homepage = $state('');
	let divisions = $state(['1v1', 'melee']);
	let step = $state('');
	let busy = $state(false);
	let error: ApiError | Error | undefined = $state();
	let result: SubmitResult | undefined = $state();

	const problem = $derived.by(() => {
		if (divisions.length === 0) return 'Choose at least one division.';
		if (mode === 'upload') {
			if (!file) return 'Choose a jar file.';
			if (file.size > JAR_MAX) return `The jar is ${formatCount(file.size)} bytes; the limit is 5 MiB.`;
			if (file.size === 0) return 'The file is empty.';
		} else if (!/^https?:\/\/\S+$/.test(url.trim())) {
			return 'Enter an http or https URL of the jar.';
		}
		if (homepage.trim() && !/^https?:\/\/\S+$/.test(homepage.trim())) {
			return 'The homepage must be an http or https URL.';
		}
		return '';
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (problem || busy) return;
		busy = true;
		error = result = undefined;
		try {
			let source;
			if (mode === 'upload' && file) {
				step = 'Computing the checksum…';
				const sha256 = await sha256Hex(await file.arrayBuffer());
				step = 'Uploading the jar…';
				const upload = await getUploadUrl(sha256, file.size);
				await putUpload(upload, file);
				source = { source: 'upload', sha256 } as const;
			} else {
				source = { source: 'url', url: url.trim() } as const;
			}
			step = 'Checking and registering the bot…';
			result = await submitBot({ source, divisions, homepage: homepage.trim() || undefined });
		} catch (e) {
			error = e instanceof Error ? e : new Error('Something went wrong.');
		} finally {
			busy = false;
			step = '';
		}
	}

	const hint = $derived.by(() => {
		if (!(error instanceof ApiError)) return '';
		if (error.signedOut) return 'Your session has ended: sign in again.';
		if (error.unavailable) return 'The rumble server is not available from here.';
		if (error.status === 429 && error.retryAfter) return `Try again in ${error.retryAfter} seconds.`;
		return '';
	});

	const weight = (w: string) => (w in CLASS_NAMES ? CLASS_NAMES[w as WeightClass] : w);
</script>

<svelte:head>
	<title>Submit a bot · robocode-s</title>
	<meta name="description" content="Submit a Robocode robot jar to the robocode-s rumble." />
</svelte:head>

<div class="container page">
	<h1>Submit a bot</h1>
	<p class="lead">
		Upload a robot jar, or give the URL of one. The rumble checks it, keeps a copy, and runs its
		first battles on trusted volunteers (probation) before it is ranked. Package a robot as
		described in <a href={resolve('/docs/robot-development')}>Developing a robot</a>; how the rumble
		treats it is in <a href={resolve('/docs/rumble')}>The rumble</a>.
	</p>

	{#if session.state !== 'in'}
		<SignInPrompt reason="to submit a bot" />
	{:else}
		<form class="panel" onsubmit={submit}>
			<fieldset>
				<legend>The jar</legend>
				<p class="choice">
					<label><input type="radio" bind:group={mode} value="upload" /> Upload a file</label>
					<label><input type="radio" bind:group={mode} value="url" /> From a URL</label>
				</p>
				{#if mode === 'upload'}
					<p>
						<input
							type="file"
							accept=".jar,application/java-archive"
							aria-label="Robot jar"
							onchange={(e) => (file = e.currentTarget.files?.[0])}
						/>
					</p>
				{:else}
					<p>
						<input type="url" placeholder="https://example.com/jd.MyRobot_1.0.jar" aria-label="URL of the jar" bind:value={url} />
					</p>
					<p class="muted small">The rumble fetches it once (10 s limit, up to 3 redirects).</p>
				{/if}
			</fieldset>

			<fieldset>
				<legend>Divisions</legend>
				<p class="choice">
					{#each DIVISIONS as d (d.id)}
						<label><input type="checkbox" bind:group={divisions} value={d.id} /> {d.label}</label>
					{/each}
				</p>
				<p class="muted small">Teams and twin duel are not open yet.</p>
			</fieldset>

			<p>
				<label>
					Homepage (optional)
					<input type="url" placeholder="https://robowiki.net/wiki/MyRobot" bind:value={homepage} />
				</label>
			</p>

			<p>
				<button type="submit" disabled={busy || !!problem}>{busy ? 'Working…' : 'Submit'}</button>
				{#if step}<span class="muted" role="status">{step}</span>{/if}
				{#if problem && !busy && (file || url || homepage)}<span class="muted small">{problem}</span>{/if}
			</p>

			<div class="limits muted small">
				<strong>Limits.</strong> A jar of up to 5 MiB (compressed). Up to 5 bots per account and 5
				new versions of a bot per day. A GitHub account younger than 30 days needs the owner's
				approval first: <a href="https://github.com/nhofsted/robocode-s/issues">open an issue</a>.
				A bot can only get new versions from its owner, and every release needs a new
				<code>robot.version</code>.
			</div>
		</form>

		{#if error}
			<div class="panel error" role="alert">
				<h2>Not accepted</h2>
				<p class="messages">{error.message}</p>
				{#if hint}<p>{hint}</p>{/if}
				{#if error instanceof ApiError && error.status === 403}
					<p class="muted small">This is a limit of the account or the bot, not of the jar.</p>
				{/if}
			</div>
		{/if}

		{#if result}
			<div class="panel success" role="status">
				<h2>Accepted</h2>
				<dl>
					<dt>Bot</dt>
					<dd class="mono">{result.bot}</dd>
					<dt>Version</dt>
					<dd class="mono">{result.version}</dd>
					<dt>Codesize</dt>
					<dd class="mono">{formatCount(result.codesize)} bytes</dd>
					<dt>Weight class</dt>
					<dd>{weight(result.weightClass)}</dd>
					<dt>State</dt>
					<dd>{result.state}</dd>
				</dl>
				<p>
					<a href={resolve('/account/bots')}>See your bots</a>: the version is ranked after its
					probation battles.
				</p>
			</div>
		{/if}
	{/if}
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.lead {
		max-width: 44rem;
		color: var(--text-2);
	}
	form,
	.panel.error,
	.panel.success {
		max-width: 44rem;
		padding: 0.5rem 1.3rem 1rem;
		margin-top: 1rem;
	}
	fieldset {
		border: 0;
		padding: 0;
		margin: 0 0 0.8rem;
	}
	legend {
		font-weight: 600;
		padding: 0;
	}
	.choice {
		display: flex;
		gap: 1.2rem;
		flex-wrap: wrap;
	}
	label input[type='url'] {
		display: block;
	}
	input[type='url'] {
		width: 100%;
	}
	.small {
		font-size: 0.85rem;
	}
	.limits {
		border-top: 1px dashed var(--line-strong);
		padding-top: 0.8rem;
	}
	.error {
		border-color: var(--danger);
	}
	.error h2 {
		color: var(--danger);
		font-size: 1.1rem;
	}
	.messages {
		white-space: pre-line;
	}
	.success h2 {
		color: var(--radar);
		font-size: 1.1rem;
	}
	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.2rem 1.2rem;
		margin: 0.5rem 0;
	}
	dt {
		color: var(--text-2);
	}
	dd {
		margin: 0;
	}
</style>
