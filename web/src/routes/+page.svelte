<script lang="ts">
	import { resolve } from '$app/paths';
	import Battlefield from '#lib/components/Battlefield.svelte';
	import { REPO_URL } from '#lib/site.ts';

	const milestones: { id: string; name: string; state: 'done' | 'active' | 'planned' }[] = [
		{ id: 'M0', name: 'Bootstrap: repository, CI, design', state: 'done' },
		{ id: 'M1', name: 'Headless engine + classic Java robots, bit-exact', state: 'done' },
		{ id: 'M2', name: 'Desktop GUI: viewer, debug paint, replay', state: 'done' },
		{ id: 'M3', name: 'Isolation: containers, remote runners', state: 'planned' },
		{ id: 'M4', name: 'Robots in Python, .NET, Rust, C/C++, Kotlin', state: 'planned' },
		{ id: 'M5', name: 'The rumble: rankings, volunteers, submissions', state: 'planned' },
		{ id: 'M5b', name: 'Battle replays in the browser', state: 'planned' }
	];
	const STATE_LABEL = { done: 'Done', active: 'In progress', planned: 'Planned' };

	// The home page shows more of the battlefield floor than content pages.
	$effect(() => {
		const root = document.documentElement.style;
		root.setProperty('--ground-shade', 'rgb(8 11 18 / 0.5)');
		return () => root.removeProperty('--ground-shade');
	});
</script>

<svelte:head>
	<title>robocode-s · Robocode, rebuilt</title>
	<meta
		name="description"
		content="robocode-s is a reimplementation of Robocode: a fast Rust engine, robots that talk to it over an open protocol, bit-exact with classic robots, and a distributed rumble."
	/>
</svelte:head>

<div class="field"><Battlefield tanks={6} /></div>

<section class="hero">
	<div class="container hero-text">
		<h1>Build a tank.<br />Write its brain.<br />Win the rumble.</h1>
		<p class="lead">
			robocode-s is a reimplementation of <a href="https://robocode.sourceforge.io/">Robocode</a>
			with a Rust engine and robots that run in their own process, talking to the engine over an open
			protocol. Classic Java
			robots run unmodified, turn for turn identical to classic Robocode.
		</p>
		<p class="ctas">
			<a class="button" href={resolve('/docs/getting-started')}>Get started</a>
			<a class="button secondary" href={resolve('/rankings')}>Rankings</a>
		</p>
	</div>
</section>

<div class="container">
	<section class="features" aria-label="Features">
		<article class="panel">
			<h2 class="kicker">Classic</h2>
			<p>
				Existing robot jars run as they are. Every turn matches classic Robocode exactly, checked
				against real RoboRumble robots.
			</p>
		</article>
		<article class="panel">
			<h2 class="kicker">Any language</h2>
			<p>
				Robots talk to the engine over an open protocol, so they can be written in Java, Kotlin,
				Python, C#, Rust or C/C++ and run locally, in a container or on another machine.
			</p>
		</article>
		<article class="panel">
			<h2 class="kicker">Reproducible</h2>
			<p>
				In deterministic mode the same seed gives the same battle on every machine. Find a bug
				once, replay it forever.
			</p>
		</article>
		<article class="panel">
			<h2 class="kicker">The rumble</h2>
			<p>
				A distributed league: volunteers run the battles, results are cross-checked, and every
				matchup can be watched in the browser.
			</p>
		</article>
	</section>

	<section class="status panel">
		<h2>Where we are</h2>
		<p>
			robocode-s is in early development. Classic Java robots, the samples and real RoboRumble
			bots alike, fight identically to classic: headless from the command line, or in the
			desktop GUI with debug painting, recordings and replays. There is no release to download
			yet. Follow along on <a href={REPO_URL}>GitHub</a>.
		</p>
		<ol class="milestones">
			{#each milestones as m (m.id)}
				<li class={m.state}>
					<span class="id mono">{m.id}</span>
					<span class="name">{m.name}</span>
					<span class="state">{STATE_LABEL[m.state]}</span>
				</li>
			{/each}
		</ol>
	</section>
</div>

<style>
	/* The battlefield fills the window behind the whole page. */
	.field {
		position: fixed;
		inset: 0;
		z-index: -1;
		pointer-events: none;
	}
	.hero {
		min-height: 420px;
		display: flex;
		align-items: center;
	}
	.hero-text {
		position: relative;
		width: 100%;
		padding-top: 3rem;
		padding-bottom: 3rem;
		pointer-events: none;
	}
	.hero-text a {
		pointer-events: auto;
	}
	.hero h1 {
		font-size: clamp(2rem, 6vw, 3.4rem);
		line-height: 1.1;
		margin: 0 0 1.2rem;
		text-shadow: 0 2px 12px rgb(0 0 0 / 0.8);
	}
	.lead {
		max-width: 38rem;
		font-size: 1.1rem;
		color: var(--text);
		background: color-mix(in srgb, var(--bg) 80%, transparent);
		padding: 0.6rem 0.9rem;
		border-radius: var(--radius);
	}
	.ctas {
		display: flex;
		gap: 0.75rem;
		flex-wrap: wrap;
		margin-top: 1.5rem;
	}
	/* The battlefield shows through a transparent button; a solid fill keeps its text readable. */
	.ctas .secondary {
		background: var(--panel);
	}

	.features {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr));
		gap: 1rem;
		margin-top: 2.5rem;
	}
	.features article {
		padding: 1.1rem 1.2rem;
	}
	.features h2 {
		margin: 0 0 0.6rem;
	}
	.features p {
		margin: 0;
		color: var(--text-2);
	}

	.status {
		margin-top: 2rem;
		max-width: 48rem;
		padding: 0.2rem 1.4rem 1.4rem;
	}
	.milestones {
		list-style: none;
		padding: 0;
		margin: 1.2rem 0 0;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		background: var(--panel-2);
	}
	.milestones li {
		display: grid;
		grid-template-columns: 3rem 1fr auto;
		gap: 0.75rem;
		padding: 0.55rem 0.9rem;
		border-bottom: 1px solid var(--line);
		align-items: baseline;
	}
	.milestones li:last-child {
		border-bottom: 0;
	}
	.id {
		color: var(--muted);
	}
	.state {
		font-size: 0.8rem;
		color: var(--muted);
	}
	.done .state {
		color: var(--radar);
	}
	.done .state::before {
		content: '✔ ';
	}
	.active .state {
		color: var(--amber);
	}
	.active .state::before {
		content: '▶ ';
	}
	.active .name {
		color: var(--text);
		font-weight: 600;
	}
	.planned .name {
		color: var(--text-2);
	}
</style>
