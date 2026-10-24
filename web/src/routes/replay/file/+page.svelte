<svelte:head>
	<title>Open a replay file</title>
	<meta name="description" content="Play a Robocode-S .track file in the browser." />
</svelte:head>

<script lang="ts">
	import Replay from '#lib/replay/Replay.svelte';
	import { loadTrack, type ReplayTrack } from '#lib/replay/track.ts';

	let track: ReplayTrack | undefined = $state.raw();
	let fileName = $state('');
	let error = $state('');
	let loading = $state(false);
	let dragging = $state(false);
	/** Bumped per file, so the viewer starts afresh. */
	let opened = $state(0);

	const MAX_BYTES = 64 * 1024 * 1024;

	async function open(file: File | undefined) {
		if (!file) return;
		error = '';
		loading = true;
		try {
			if (file.size > MAX_BYTES) throw new Error('That file is too large for a replay track.');
			const bytes = new Uint8Array(await file.arrayBuffer());
			// A desktop recording (.rsr) is gzip, which starts with 0x1f; the viewer plays tracks only.
			if (bytes[0] === 0x1f) {
				throw new Error(
					'This is a full recording (gzip), not a replay track. Open recordings in the desktop app.'
				);
			}
			const decoded = await loadTrack(bytes);
			if (decoded.frameCount === 0 || decoded.robotCount === 0) throw new Error('This track has no turns.');
			track = decoded;
			fileName = file.name;
			opened++;
		} catch (e) {
			error =
				e instanceof Error && !e.message.startsWith('not a recording')
					? e.message
					: 'This file is not a replay track (.track).';
		} finally {
			loading = false;
		}
	}

	function ondrop(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		void open(e.dataTransfer?.files[0]);
	}
</script>

<div class="container page">
	<h1>Open a replay file</h1>
	<p class="lead">
		Play a <code>.track</code> file, the kind the desktop app and the rumble produce, right here.
		The file is read in your browser and goes nowhere.
	</p>

	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="drop"
		class:dragging
		ondragover={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragleave={() => (dragging = false)}
		{ondrop}
	>
		<label class="button" for="track-file">{track ? 'Open another file' : 'Choose a .track file'}</label>
		<input
			id="track-file"
			class="file"
			type="file"
			accept=".track,application/octet-stream"
			onchange={(e) => {
				void open(e.currentTarget.files?.[0]);
				e.currentTarget.value = '';
			}}
		/>
		<span class="or">or drop it here</span>
	</div>

	<p class="error" role="alert">{error}</p>
	{#if loading}<p>Reading the file&hellip;</p>{/if}

	{#if track}
		<h2 class="name">{fileName}</h2>
		{#key opened}
			<Replay {track} />
		{/key}
	{/if}
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.lead {
		max-width: 40rem;
		color: var(--text-2);
	}
	.drop {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.8rem;
		padding: 1.2rem;
		border: 2px dashed var(--line-strong);
		border-radius: var(--radius);
		background: var(--panel);
	}
	.drop.dragging {
		border-color: var(--radar);
		background: var(--panel-2);
	}
	/* Visually hidden, but still focusable: the label is the button. */
	.file {
		position: absolute;
		width: 1px;
		height: 1px;
		opacity: 0;
		overflow: hidden;
	}
	.file:focus-visible + .or {
		outline: 2px solid var(--amber);
	}
	.drop:has(.file:focus-visible) .button {
		outline: 2px solid var(--amber);
		outline-offset: 2px;
	}
	.or {
		color: var(--text-2);
	}
	.error {
		margin: 0.8rem 0 0;
		min-height: 1.6em;
		color: var(--danger);
	}
	.name {
		font-size: 1.1rem;
		font-family: var(--font-mono);
		margin: 1rem 0 0.6rem;
		overflow-wrap: anywhere;
	}
</style>
