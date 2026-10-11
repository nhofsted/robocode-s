<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
</script>

<svelte:head>
	<title>{page.status} · robocode-s</title>
</svelte:head>

<div class="container">
	<h1>{page.status === 404 ? 'Lost radar contact' : 'Robot disabled'}</h1>
	<p>{page.error?.message ?? 'Something went wrong.'}</p>
	<p>
		<a href={resolve('/rankings')}>Rankings</a> · <a href={resolve('/docs')}>Docs</a> ·
		<a href={resolve('/')}>Home</a>
	</p>
</div>
