<script lang="ts">
	// Shown instead of a page that needs an account: loading, signed out, or no backend.
	import { page } from '$app/state';
	import { loginUrl } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';

	interface Props {
		/** What signing in is for, e.g. "to submit a bot". */
		reason: string;
	}
	let { reason }: Props = $props();
</script>

{#if session.state === 'loading'}
	<p class="muted">Loading…</p>
{:else if session.state === 'unavailable'}
	<p class="notice">
		<strong>Not available.</strong> The rumble server can't be reached from here, so there is no
		sign-in. (On a local development server there is no backend; see the web README.)
	</p>
{:else}
	<section class="panel prompt">
		<p>Sign in with your GitHub account {reason}. The site reads only your public profile.</p>
		<p>
			<a class="button" href={loginUrl(page.url.pathname + page.url.search)} rel="external">
				Sign in with GitHub
			</a>
		</p>
	</section>
{/if}

<style>
	.prompt {
		max-width: 36rem;
		padding: 0.2rem 1.3rem 0.4rem;
	}
</style>
