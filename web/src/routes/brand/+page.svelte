<script lang="ts">
	// Logo variations to choose from (not linked from the site). Option 1: classic Robocode's
	// own logo, ported from its source, with an S in the same letter style. Option 3: the
	// classic tank icon with a simple wordmark.
	import ClassicEmblem from '#lib/brand/ClassicEmblem.svelte';
	import ClassicWordmark from '#lib/brand/ClassicWordmark.svelte';
	import Logo from '#lib/components/Logo.svelte';

	const AMBER = '#ffb31f';
	const RED = '#ff5a3c';
</script>

<svelte:head>
	<title>Logo variations · robocode-s</title>
	<meta name="robots" content="noindex" />
</svelte:head>

{#snippet header(label: string, logo: import('svelte').Snippet)}
	<div class="mock">
		<span class="tag mono">{label}</span>
		<div class="bar">
			{@render logo()}
			<nav><span>Rankings</span><span>Docs</span><span>GitHub</span></nav>
		</div>
	</div>
{/snippet}

<div class="container">
	<h1>Logo variations</h1>
	<p class="muted">
		Each variation in the header as it would appear, and larger where that matters. Option 1 uses
		classic Robocode's own logo, ported shape for shape from its source (<span class="mono"
			>RobocodeLogo.java</span
		>); the S is new, built from the same letter pieces.
	</p>

	<h2>Current</h2>
	{#snippet current()}<Logo size={22} />{/snippet}
	{@render header('now', current)}

	<h2>Option 1: classic letters</h2>

	{#snippet a1()}<ClassicWordmark height={30} look="glow" s="inline" sColor={AMBER} />{/snippet}
	{@render header('1a · classic glow, amber S', a1)}

	{#snippet a2()}<ClassicWordmark height={30} look="solid" s="inline" sColor={AMBER} />{/snippet}
	{@render header('1b · solid, amber S', a2)}

	{#snippet a3()}<ClassicWordmark height={30} look="glow" s="super" sColor={AMBER} />{/snippet}
	{@render header('1c · classic glow, raised S', a3)}

	{#snippet a4()}<ClassicWordmark height={30} look="solid" s="super" sColor={RED} />{/snippet}
	{@render header('1d · solid, raised red S', a4)}

	{#snippet a5()}<ClassicEmblem width={110} s="inline" tanks={false} />{/snippet}
	{@render header('1e · mini emblem', a5)}

	<h3>The emblem, large (home page, social image)</h3>
	<div class="emblems">
		<figure>
			<ClassicEmblem width={570} s="inline" />
			<figcaption class="mono">1f · classic emblem, S inside</figcaption>
		</figure>
		<figure>
			<ClassicEmblem width={570} s="badge" />
			<figcaption class="mono">1g · classic emblem, S as a seal</figcaption>
		</figure>
		<figure>
			<ClassicEmblem width={570} s="none" />
			<figcaption class="mono">reference · classic's logo, unchanged</figcaption>
		</figure>
	</div>

	<h2>Option 3: tank icon and a wordmark</h2>

	{#snippet c1()}
		<span class="iconmark">
			<img src="/favicon-256.png" alt="" width="34" height="34" />
			<span class="wm plex">robocode<span class="dash">-</span>s</span>
		</span>
	{/snippet}
	{@render header('3a · icon + robocode-s', c1)}

	{#snippet c2()}
		<span class="iconmark">
			<img src="/favicon-256.png" alt="" width="34" height="34" />
			<span class="wm helv">Robocode</span><span class="pill">S</span>
		</span>
	{/snippet}
	{@render header('3b · icon + Robocode, S tag', c2)}

	{#snippet c3()}
		<span class="iconmark">
			<img src="/favicon-256.png" alt="" width="34" height="34" />
			<span class="stack">
				<span class="wm plex small">robocode</span>
				<span class="sub mono">S · next generation</span>
			</span>
		</span>
	{/snippet}
	{@render header('3c · icon + stacked', c3)}

	{#snippet c4()}
		<span class="iconmark">
			<img src="/favicon-256.png" alt="" width="34" height="34" />
			<ClassicWordmark height={24} look="solid" s="inline" sColor={AMBER} />
		</span>
	{/snippet}
	{@render header('3d · icon + classic letters (1 and 3 combined)', c4)}
</div>

<style>
	h2 {
		margin-top: 2.5rem;
	}
	.mock {
		margin: 0.8rem 0 1.2rem;
	}
	.tag {
		font-size: 0.75rem;
		color: var(--amber);
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		min-height: 56px;
		padding: 0 1rem;
		background: color-mix(in srgb, var(--bg) 88%, transparent);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		margin-top: 0.3rem;
	}
	nav {
		display: flex;
		gap: 1.2rem;
		color: var(--text-2);
		font-size: 0.95rem;
	}
	.emblems {
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
		align-items: flex-start;
	}
	.emblems figure {
		margin: 0;
		max-width: 100%;
		overflow-x: auto;
	}
	figcaption {
		font-size: 0.75rem;
		color: var(--amber);
		margin-top: 0.3rem;
	}
	.iconmark {
		display: inline-flex;
		align-items: center;
		gap: 0.55rem;
	}
	.wm {
		font-weight: 700;
		color: var(--radar);
		font-size: 1.45rem;
		letter-spacing: -0.01em;
	}
	.plex {
		font-family: var(--font-sans);
	}
	.helv {
		font-family: var(--font-logo);
	}
	.dash {
		color: var(--muted);
		margin: 0 0.02em;
	}
	.pill {
		font-family: var(--font-logo);
		font-weight: 700;
		font-size: 0.95rem;
		color: #1a0d00;
		background: #ffb31f;
		border-radius: 4px;
		padding: 0 0.35rem;
		margin-left: 0.35rem;
		align-self: center;
	}
	.stack {
		display: flex;
		flex-direction: column;
		line-height: 1.05;
	}
	.small {
		font-size: 1.25rem;
	}
	.sub {
		font-size: 0.68rem;
		color: var(--amber);
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}
</style>
