<script lang="ts">
	import { resolve } from '$app/paths';
	import { REPO_URL } from '#lib/site.ts';
	import { VERSION, RELEASE_URL, WINDOWS, LINUX, SHA256SUMS_URL, url, mb } from '#lib/release.ts';

	const PKG = 'https://rumble.planethofstede.be/packages';
	const apt = `sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL ${PKG}/robocode-s.gpg | sudo tee /etc/apt/keyrings/robocode-s.gpg >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/robocode-s.gpg] ${PKG}/apt stable main" | sudo tee /etc/apt/sources.list.d/robocode-s.list
sudo apt update && sudo apt install robocode-s`;
	const dnf = `sudo curl -fsSL -o /etc/yum.repos.d/robocode-s.repo ${PKG}/robocode-s.repo
sudo dnf install robocode-s`;
</script>

<svelte:head>
	<title>Download · robocode-s</title>
	<meta
		name="description"
		content="Download robocode-s {VERSION} for Windows and Linux, or install it from the apt and dnf repositories."
	/>
</svelte:head>

<div class="container page">
	<h1>Download robocode-s</h1>
	<p class="lead">
		Version {VERSION}, for Windows and Linux. <a href={RELEASE_URL}>Release notes</a> ·
		<a href={SHA256SUMS_URL}>SHA256SUMS</a>. New here? Start with
		<a href={resolve('/docs/getting-started')}>Getting started</a>.
	</p>

	<div class="cards">
		<section class="panel">
			<h2>Windows</h2>
			<p class="req">Windows 10 or 11, x64.</p>
			<p>
				<a class="button" href={url(WINDOWS.setup)}>Download the installer</a>
				<span class="size mono">{mb(WINDOWS.setup.size)}</span>
			</p>
			<p>
				Installs for your user, without administrator rights, with a Start menu entry and an
				optional <code>PATH</code> entry.
			</p>
			<p>
				Or the <a href={url(WINDOWS.zip)}>zip archive</a>
				<span class="size mono">{mb(WINDOWS.zip.size)}</span>: unpack it on an NTFS drive (the
				sandbox needs NTFS) and run <code>robocode-s-gui.exe</code>.
			</p>
			<p>
				winget: <code>winget install NickHofstede.robocode-s</code> once it's accepted (submitted to
				<a href="https://github.com/microsoft/winget-pkgs/pull/459630">winget-pkgs</a>, in review).
			</p>
			<p class="warn">
				<strong>Unsigned.</strong> The installer and the programs are not code-signed, so SmartScreen
				says "Windows protected your PC": choose "More info", then "Run anyway".
			</p>
		</section>

		<section class="panel">
			<h2>Linux</h2>
			<p class="req">x86-64 or arm64, glibc 2.31 or newer (Debian 11, Ubuntu 20.04, Fedora 32, RHEL 9).</p>
			<h3>Debian, Ubuntu</h3>
			<pre>{apt}</pre>
			<h3>Fedora, RHEL, Rocky</h3>
			<pre>{dnf}</pre>
			<p>
				Both repositories are signed, and <code>apt upgrade</code> or <code>dnf upgrade</code>
				brings later versions. Or install a downloaded file, which is not signature-checked:
			</p>
			<table>
				<thead><tr><th>Package</th><th>x86-64</th><th>arm64</th></tr></thead>
				<tbody>
					{#each LINUX.x86_64 as a, i (a.name)}
						{@const b = LINUX.aarch64[i]}
						<tr>
							<td class="mono">{a.label}</td>
							<td><a href={url(a)}>download</a> <span class="size mono">{mb(a.size)}</span></td>
							<td><a href={url(b)}>download</a> <span class="size mono">{mb(b.size)}</span></td>
						</tr>
					{/each}
				</tbody>
			</table>
			<p>
				<code>sudo apt install ./robocode-s-{VERSION}-linux-x86_64.deb</code> or
				<code>sudo dnf install ./robocode-s-{VERSION}-linux-x86_64.rpm</code>. The
				<code>.tar.gz</code> unpacks anywhere; run <code>robocode-s-gui</code> or
				<code>robocode-s</code> from it.
			</p>
			<p class="warn">
				<strong>Unsigned.</strong> The programs are not code-signed. Linux doesn't warn, but a
				downloaded <code>.deb</code>, <code>.rpm</code> or <code>.tar.gz</code> isn't
				signature-checked: use the repositories above (signed), or check the file against
				<a href={SHA256SUMS_URL}>SHA256SUMS</a>.
			</p>
		</section>

		<section class="panel">
			<h2>macOS</h2>
			<p>No macOS build yet. Build it from source: see
				<a href={resolve('/docs/getting-started#or-build-robocode-s-from-source')}>Getting started</a>.
				Homebrew follows once there is a build.</p>
			<p class="warn">
				<strong>Unsigned.</strong> The app is not signed or notarized, so Gatekeeper refuses it after
				a download: right-click it and choose Open, or "Open Anyway" in System Settings, Privacy &amp;
				Security.
			</p>
		</section>
	</div>

	<section class="notes">
		<h2>Before you run it</h2>
		<ul>
			<li>
				<strong>First run downloads Java.</strong> The robots' Java runtime (about 50 MB) is
				fetched on first use into your user data folder, not into the installation.
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
			Source and older versions are on <a href={REPO_URL}>GitHub</a>. Next, write a robot with the
			<a href={resolve('/docs/ide')}>IDE guide</a>.
		</p>
	</section>
</div>

<style>
	.page {
		padding-top: 1.5rem;
		padding-bottom: 3rem;
	}
	.lead {
		max-width: 40rem;
		color: var(--text-2);
	}
	.cards {
		display: flex;
		flex-direction: column;
		gap: 1rem;
		max-width: 48rem;
		margin-top: 1.5rem;
	}
	.panel {
		padding: 0.2rem 1.3rem 1.2rem;
		min-width: 0;
	}
	.panel h3 {
		font-size: 1rem;
		margin: 1rem 0 0.3rem;
	}
	.req {
		color: var(--muted);
	}
	.warn {
		margin-top: 1rem;
		padding: 0.6rem 0.8rem;
		border-left: 3px solid var(--amber);
		background: var(--panel-2);
		border-radius: var(--radius);
	}
	.size {
		color: var(--muted);
		font-size: 0.85rem;
		margin-left: 0.4rem;
	}
	pre {
		overflow-x: auto;
	}
	table {
		border-collapse: collapse;
		width: 100%;
		margin: 0.6rem 0;
	}
	th,
	td {
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid var(--line);
	}
	.notes {
		max-width: 48rem;
		margin-top: 2rem;
	}
</style>
