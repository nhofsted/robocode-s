<script lang="ts">
	// "Robocode" in green Helvetica, with the "S" of robocode-s appended like a version badge.
	interface Props {
		/** Font size of "Robocode" in px; the S is larger. */
		size?: number;
	}
	let { size = 22 }: Props = $props();
</script>

<span class="logo" style:font-size="{size}px" role="img" aria-label="robocode-s">
	<span class="word" aria-hidden="true">Robocode</span><span class="sf" aria-hidden="true">S</span>
</span>

<style>
	.logo {
		display: inline-flex;
		align-items: baseline;
		white-space: nowrap;
	}
	.word {
		font-family: var(--font-logo);
		font-weight: 700;
		letter-spacing: -0.01em;
		color: var(--radar);
	}
	.sf {
		font-size: 1.45em;
		margin-left: 0.04em;
		position: relative;
		top: 0.08em;
	}
</style>
