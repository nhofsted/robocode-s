<script lang="ts">
	import { resolve } from '$app/paths';
	import { REPO_URL } from '#lib/site.ts';
	import { SHA256SUMS_URL } from '#lib/release.ts';
</script>

<section class="notes">
	<h2>Before you run it</h2>
	<ul>
		<li>
			<strong>First run downloads Java.</strong> The robots' Java runtime (about 50 MB) is fetched
			on first use into your user data folder, not into the installation.
		</li>
		<li>
			<strong>Check a download</strong> against <a href={SHA256SUMS_URL}>SHA256SUMS</a>:
			<code>sha256sum -c --ignore-missing SHA256SUMS</code>.
		</li>
		<li>
			<strong>Uninstalling</strong> leaves the data folder and the GUI's settings; delete them to
			remove everything.
		</li>
	</ul>
	<p>
		Older versions are on <a href={REPO_URL}>GitHub</a>. Next,
		<a href={resolve('/docs/getting-started')}>run a battle</a>.
	</p>
</section>

<style>
	.notes {
		max-width: 48rem;
		margin-top: 2rem;
	}
</style>
