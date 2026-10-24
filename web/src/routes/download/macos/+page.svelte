<script lang="ts">
	import { VERSION, MACOS, url, mb } from '#lib/release.ts';
	import DownloadNotes from '#lib/components/DownloadNotes.svelte';
</script>

<svelte:head>
	<title>Download for macOS · robocode-s</title>
	<meta
		name="description"
		content="Download robocode-s {VERSION} for macOS, Apple Silicon or Intel, or install it with Homebrew."
	/>
</svelte:head>

<h1>robocode-s for macOS</h1>
<p class="lead">Version {VERSION}. Apple Silicon (arm64) or Intel (x86-64).</p>

<div class="cards">
	<section class="panel">
		<h2>Disk image</h2>
		<p>
			<a class="button" href={url(MACOS.aarch64[0])}>Download for Apple Silicon</a>
			<span class="size mono">{mb(MACOS.aarch64[0].size)}</span>
		</p>
		<p>
			<a class="button" href={url(MACOS.x86_64[0])}>Download for Intel</a>
			<span class="size mono">{mb(MACOS.x86_64[0].size)}</span>
		</p>
		<p>
			Open the <code>.dmg</code> and drag <code>robocode-s.app</code> to Applications. The command
			line is <code>robocode-s.app/Contents/MacOS/robocode-s</code>.
		</p>
		<h3>Homebrew</h3>
		<pre>brew install --cask nhofsted/robocode-s/robocode-s</pre>
		<p>
			Installs the app and links <code>robocode-s</code> on the <code>PATH</code>;
			<code>brew upgrade --cask robocode-s</code> brings later versions. The cask clears the
			quarantine flag, so Gatekeeper doesn't stop it.
		</p>
		<h3>All files</h3>
		<table>
			<thead><tr><th>File</th><th>Apple Silicon</th><th>Intel</th></tr></thead>
			<tbody>
				{#each MACOS.aarch64 as a, i (a.name)}
					{@const b = MACOS.x86_64[i]}
					<tr>
						<td class="mono">{a.label}</td>
						<td><a href={url(a)}>download</a> <span class="size mono">{mb(a.size)}</span></td>
						<td><a href={url(b)}>download</a> <span class="size mono">{mb(b.size)}</span></td>
					</tr>
				{/each}
			</tbody>
		</table>
		<p>
			A double-clicked recording opens the window but not the recording yet: open it from the
			window (Open, or drop the file on it).
		</p>
		<p class="warn">
			<strong>Unsigned.</strong> The app is not signed or notarized, so Gatekeeper refuses it after
			a download: right-click it and choose Open, or "Open Anyway" in System Settings, Privacy &amp;
			Security. Or, once: <code>xattr -dr com.apple.quarantine /Applications/robocode-s.app</code>.
		</p>
	</section>

	<section class="panel">
		<h2>Sandbox</h2>
		<p>
			macOS has no built-in sandbox for robots. Battles you run for yourself can use
			<code>--sandbox docker</code> (Docker installed and running), or <code>--sandbox none</code>,
			a plain process. Contributing to the rumble needs Docker, or
			<code>--unsandboxed</code> if you accept running robots with your user's rights.
		</p>
	</section>
</div>

<DownloadNotes />
