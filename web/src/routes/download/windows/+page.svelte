<script lang="ts">
	import { VERSION, WINDOWS, url, mb } from '#lib/release.ts';
	import DownloadNotes from '#lib/components/DownloadNotes.svelte';
</script>

<svelte:head>
	<title>Download for Windows · robocode-s</title>
	<meta name="description" content="Download robocode-s {VERSION} for Windows: the installer, a zip archive or winget." />
</svelte:head>

<h1>robocode-s for Windows</h1>
<p class="lead">Version {VERSION}. Windows 10 or 11, x64.</p>

<div class="cards">
	<section class="panel">
		<h2>Installer</h2>
		<p>
			<a class="button" href={url(WINDOWS.setup)}>Download the installer</a>
			<span class="size mono">{mb(WINDOWS.setup.size)}</span>
		</p>
		<p>
			Installs for your user, without administrator rights, with a Start menu entry and an
			optional <code>PATH</code> entry.
		</p>
		<h3>Archive</h3>
		<p>
			Or the <a href={url(WINDOWS.zip)}>zip archive</a>
			<span class="size mono">{mb(WINDOWS.zip.size)}</span>: unpack it on an NTFS drive (the
			sandbox needs NTFS) and run <code>robocode-s-gui.exe</code>.
		</p>
		<h3>winget</h3>
		<pre>winget install NickHofstede.robocode-s</pre>
		<p>
			Works once Microsoft accepts the package (submitted to
			<a href="https://github.com/microsoft/winget-pkgs/pull/459630">winget-pkgs</a>, in review).
		</p>
		<p class="warn">
			<strong>Unsigned.</strong> The installer and the programs are not code-signed, so SmartScreen
			says "Windows protected your PC": choose "More info", then "Run anyway".
		</p>
	</section>

	<section class="panel">
		<h2>Sandbox</h2>
		<p>
			Robots run in a sandbox built into Windows (an AppContainer and a Job Object): no network, no
			files but the robot's own. Nothing to install. It needs the installation on NTFS, which the
			installer's folder and an archive unpacked on an NTFS drive are.
		</p>
	</section>
</div>

<DownloadNotes />
