<script lang="ts">
	import { resolve } from '$app/paths';
	import { RUMBLE_LIVE } from '#lib/site.ts';
</script>

{#if !RUMBLE_LIVE}
	<p class="notice">
		<strong>Preview.</strong> The rumble hasn't started yet: the bots, authors and numbers on
		this page are generated sample data. <a href={resolve('/docs/rumble')}>How the rumble will work</a>
	</p>
{/if}
