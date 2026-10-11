<script lang="ts">
	// The site logo: classic Robocode's logo letters with their glow, and the flame S of
	// robocode-s tucked behind the end of the word (variation 1h on /brand).
	import ClassicWordmark from '#lib/brand/ClassicWordmark.svelte';

	interface Props {
		/** Height of the letters in px; the S is taller. */
		height?: number;
	}
	let { height = 28 }: Props = $props();
</script>

<ClassicWordmark {height} look="glow" s="fighter" />
