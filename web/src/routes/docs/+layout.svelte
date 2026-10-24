<script lang="ts">
	import { page } from '$app/state';
	import { REPO_URL } from '#lib/site.ts';
	import { DOCS } from '#lib/docs/nav.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const slug = $derived(page.url.pathname === '/docs' ? '' : page.url.pathname.slice('/docs/'.length));
</script>

<div class="container docs">
	<nav aria-label="Documentation">
		<p class="kicker">Docs</p>
		<ul>
			{#each DOCS as d (d.href)}
				<li>
					<a href={d.href} aria-current={page.url.pathname === d.href ? 'page' : undefined}>{d.title}</a>
					{#if d.children}
						<ul class="sub">
							{#each d.children as c (c.href)}
								<li>
									<a href={c.href} aria-current={page.url.pathname === c.href ? 'page' : undefined}>{c.title}</a>
								</li>
							{/each}
						</ul>
					{/if}
				</li>
			{/each}
		</ul>
	</nav>
	<div class="content">
		{@render children()}
		<p class="edit">
			<a href="{REPO_URL}/edit/main/web/src/routes/docs/{slug ? slug + '/' : ''}+page.md">Edit this page on GitHub</a>
		</p>
	</div>
</div>

<style>
	.docs {
		display: grid;
		grid-template-columns: 13rem minmax(0, 1fr);
		gap: 2.5rem;
		padding-top: 1.5rem;
	}
	nav {
		position: sticky;
		top: 72px;
		align-self: start;
		font-size: 0.9rem;
	}
	nav .kicker {
		margin: 0.6rem 0 0.8rem;
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		border-left: 1px solid var(--line);
	}
	li a {
		display: block;
		padding: 0.3rem 0.8rem;
		color: var(--text-2);
		text-decoration: none;
		margin-left: -1px;
		border-left: 2px solid transparent;
	}
	ul.sub {
		border-left: 0;
	}
	ul.sub li a {
		padding: 0.15rem 0.8rem 0.15rem 1.6rem;
		font-size: 0.85rem;
	}
	li a:hover {
		color: var(--text);
	}
	li a[aria-current='page'] {
		color: var(--radar);
		border-left-color: var(--radar);
	}
	.content {
		background: color-mix(in srgb, var(--panel) 94%, transparent);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		padding: 0.5rem clamp(1rem, 3vw, 2.2rem) 1.5rem;
	}
	.edit {
		margin-top: 3rem;
		font-size: 0.85rem;
	}
	@media (max-width: 760px) {
		.docs {
			grid-template-columns: 1fr;
			gap: 0.5rem;
		}
		nav {
			position: static;
		}
		ul {
			display: flex;
			flex-wrap: wrap;
			gap: 0.2rem 0;
			border-left: 0;
		}
		li a {
			border-left: 0;
			padding: 0.2rem 0.6rem 0.2rem 0;
		}
		ul.sub {
			display: none;
		}
	}
</style>
