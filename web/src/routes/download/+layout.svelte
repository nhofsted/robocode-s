<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
</script>

<div class="container page">
	{#if page.url.pathname !== '/download'}
		<p class="back"><a href={resolve('/download')}>&larr; All downloads</a></p>
	{/if}
	{@render children()}
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.back {
		margin: 0 0 0.5rem;
		font-size: 0.9rem;
	}
	.page :global(.lead) {
		max-width: 40rem;
		color: var(--text-2);
	}
	.page :global(.cards) {
		display: flex;
		flex-direction: column;
		gap: 1rem;
		max-width: 48rem;
		margin-top: 1.5rem;
	}
	.page :global(.panel) {
		padding: 0.2rem 1.3rem 1.2rem;
		min-width: 0;
	}
	.page :global(.panel h3) {
		font-size: 1rem;
		margin: 1rem 0 0.3rem;
	}
	.page :global(.req) {
		color: var(--muted);
	}
	.page :global(.warn) {
		margin-top: 1rem;
		padding: 0.6rem 0.8rem;
		border-left: 3px solid var(--amber);
		background: var(--panel-2);
		border-radius: var(--radius);
	}
	.page :global(.size) {
		color: var(--muted);
		font-size: 0.85rem;
		margin-left: 0.4rem;
	}
	.page :global(pre) {
		overflow-x: auto;
	}
	.page :global(table) {
		border-collapse: collapse;
		width: 100%;
		margin: 0.6rem 0;
	}
	.page :global(th),
	.page :global(td) {
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid var(--line);
	}
</style>
