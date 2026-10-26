<script lang="ts">
	import { VERSION, LINUX, SHA256SUMS_URL, url, mb } from '#lib/release.ts';
	import DownloadNotes from '#lib/components/DownloadNotes.svelte';

	const PKG = 'https://rumble.planethofstede.be/packages';
	const apt = `sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL ${PKG}/robocode-s.gpg | sudo tee /etc/apt/keyrings/robocode-s.gpg >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/robocode-s.gpg] ${PKG}/apt stable main" | sudo tee /etc/apt/sources.list.d/robocode-s.list
sudo apt update && sudo apt install robocode-s`;
	const dnf = `sudo curl -fsSL -o /etc/yum.repos.d/robocode-s.repo ${PKG}/robocode-s.repo
sudo dnf install robocode-s`;
</script>

<svelte:head>
	<title>Download for Linux · robocode-s</title>
	<meta
		name="description"
		content="Install robocode-s {VERSION} on Linux from the apt and dnf repositories, or download a .deb, .rpm or .tar.gz."
	/>
</svelte:head>

<h1>robocode-s for Linux</h1>
<p class="lead">
	Version {VERSION}. x86-64 or arm64, glibc 2.31 or newer (Debian 11, Ubuntu 20.04, Fedora 32, RHEL 9).
</p>

<div class="cards">
	<section class="panel">
		<h2>Package repositories</h2>
		<h3>Debian, Ubuntu</h3>
		<pre>{apt}</pre>
		<h3>Fedora, RHEL, Rocky</h3>
		<pre>{dnf}</pre>
		<p>
			Both repositories are signed, and <code>apt upgrade</code> or <code>dnf upgrade</code>
			brings later versions. The package puts <code>robocode-s</code> and
			<code>robocode-s-gui</code> on your <code>PATH</code> and adds an application menu entry.
		</p>
	</section>

	<section class="panel">
		<h2>Downloaded files</h2>
		<p>These are not signature-checked.</p>
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
		<h2>Sandbox</h2>
		<p>
			Robots run in a sandbox built into Linux (seccomp, and Landlock where the kernel has it): no
			network, and no files but the robot's own. Without Landlock the battle's header says
			<code>filesystem not limited</code>. <code>--sandbox bwrap</code> uses bubblewrap, and
			<code>--sandbox docker</code> and <code>--sandbox podman</code> a container.
		</p>
	</section>
</div>

<DownloadNotes />
