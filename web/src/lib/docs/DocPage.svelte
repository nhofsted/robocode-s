<script lang="ts">
	// mdsvex layout for every .md page: frontmatter `title` and `description` become the page
	// title and meta description.
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		description?: string;
		children: Snippet;
	}
	let { title, description, children }: Props = $props();
</script>

<svelte:head>
	<title>{title} · robocode-s</title>
	{#if description}<meta name="description" content={description} />{/if}
</svelte:head>

<article class="doc">
	<h1>{title}</h1>
	{@render children()}
</article>

<style>
	.doc {
		max-width: 46rem;
		min-width: 0;
	}
	.doc :global(h2) {
		border-bottom: 1px dashed var(--line-strong);
		padding-bottom: 0.3rem;
		margin-top: 2.2em;
	}
	.doc :global(p),
	.doc :global(li) {
		color: var(--text);
	}
	.doc :global(li + li) {
		margin-top: 0.3em;
	}
	/* Blockquotes are callouts; start one with **Status:** or **Tip:**. */
	.doc :global(blockquote) {
		margin: 1.2rem 0;
		padding: 0.6rem 1rem;
		border-left: 3px solid var(--amber);
		background: color-mix(in srgb, var(--amber) 7%, var(--panel));
		border-radius: 0 var(--radius) var(--radius) 0;
	}
	.doc :global(blockquote p) {
		margin: 0.3rem 0;
	}
	.doc :global(blockquote strong:first-child) {
		color: var(--amber);
	}
	.doc :global(table) {
		width: 100%;
		font-size: 0.9rem;
		display: block;
		overflow-x: auto;
		margin: 1rem 0;
	}
	/* Long inline paths and names wrap instead of widening the page on a phone (code blocks
	   scroll inside themselves). */
	.doc :global(:not(pre) > code) {
		overflow-wrap: anywhere;
	}
	.doc :global(th),
	.doc :global(td) {
		border-bottom: 1px solid var(--line);
		padding: 0.4rem 0.7rem;
		text-align: left;
		vertical-align: top;
	}
	.doc :global(th) {
		color: var(--text-2);
		background: var(--panel-2);
	}

	/* Prism tokens (mdsvex's highlighter), in the site palette. */
	.doc :global(.token.comment),
	.doc :global(.token.prolog) {
		color: var(--muted);
		font-style: italic;
	}
	.doc :global(.token.keyword),
	.doc :global(.token.annotation),
	.doc :global(.token.important) {
		color: var(--radar);
	}
	.doc :global(.token.string),
	.doc :global(.token.char) {
		color: var(--amber);
	}
	.doc :global(.token.number),
	.doc :global(.token.boolean) {
		color: #8fc9ff;
	}
	.doc :global(.token.function),
	.doc :global(.token.class-name) {
		color: #e7c6ff;
	}
	.doc :global(.token.punctuation),
	.doc :global(.token.operator) {
		color: var(--text-2);
	}
</style>
