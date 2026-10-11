<script lang="ts">
	// "Robocode" in green Helvetica, with the "S" of robocode-s appended like a version badge.
	import FighterText from './FighterText.svelte';
	interface Props {
		/** Font size of "Robocode" in px; the S is larger. */
		size?: number;
	}
	let { size = 22 }: Props = $props();
</script>

<span class="logo" style:font-size="{size}px" role="img" aria-label="robocode-s">
	<span class="word" aria-hidden="true">Robocode</span><span class="s"><FighterText text="S" height="1.9em" /></span>
</span>

<style>
	.logo {
		display: inline-flex;
		align-items: center;
		white-space: nowrap;
	}
	.word {
		font-family: var(--font-logo);
		font-weight: 700;
		letter-spacing: -0.01em;
		color: var(--radar);
	}
	.s {
		margin-left: -0.55em;
		position: relative;
		top: 0.06em;
	}
</style>
