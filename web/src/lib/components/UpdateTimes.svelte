<script lang="ts">
	import { updateTimes } from '#lib/rumble/format.ts';

	interface Props {
		/** The data file's `generatedAt`. */
		generatedAt: string;
	}

	let { generatedAt }: Props = $props();

	// The "in 2 h 5 min" part counts down while the page is open.
	let now = $state(new Date());
	$effect(() => {
		const timer = setInterval(() => (now = new Date()), 30_000);
		return () => clearInterval(timer);
	});
</script>

{updateTimes(generatedAt, now)}
