<script lang="ts">
	import { resolve } from '$app/paths';
	import { REPO_URL } from '#lib/site.ts';

	const build = `git clone ${REPO_URL}.git
cd robocode-s
./gradlew :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
cargo build --release -p robocode-s-cli -p robocode-s-gui`;
	const run = `target/release/robocode-s battle sample.Fire,sample.Walls,sampleteam.MyFirstTeam --rounds 10
target/release/robocode-s-gui`;
	const test = `cargo test --workspace     # engine, protocol, server, runner, cli
./gradlew build            # Java robot host, sample robots, test oracle`;
</script>

<svelte:head>
	<title>Build from source · robocode-s</title>
	<meta
		name="description"
		content="Build robocode-s from its source: the Rust toolchain, JDK 21 and the commands."
	/>
</svelte:head>

<h1>Build robocode-s from source</h1>
<p class="lead">
	The installers are the easy way. Build from source for a platform without one, or to change
	robocode-s itself.
</p>

<div class="cards">
	<section class="panel">
		<h2>Who it is for</h2>
		<p>
			Installers hold the same programs, built and tested for a release, with menu entries and
			updates through your package manager. A source build gives you the
			latest code and the freedom to change it, and nothing updates it for you. If you only want to
			write robots, <a href={resolve('/download')}>install it</a> instead.
		</p>
	</section>

	<section class="panel">
		<h2>You need</h2>
		<ul>
			<li>Rust, stable, via <a href="https://rustup.rs">rustup</a> (1.85 or newer), and a C toolchain.</li>
			<li>JDK 21 for the Gradle build. The robots themselves run on a Java runtime of about 50 MB that robocode-s downloads on first use.</li>
			<li>Git.</li>
		</ul>
	</section>

	<section class="panel">
		<h2>Build</h2>
		<pre>{build}</pre>
		<p>
			The Gradle tasks build the Java side that battles need at run time: the robot host, the
			determinism agent and the sample robots. The programs are then
			<code>target/release/robocode-s</code> (the command line) and
			<code>target/release/robocode-s-gui</code> (the window). Leave out
			<code>-p robocode-s-gui</code> if you only want the command line.
		</p>
	</section>

	<section class="panel">
		<h2>Run</h2>
		<pre>{run}</pre>
		<p>
			Start the programs from the repository's folder: a build from source looks for the robot host
			and the sample robots relative to the current folder (or where
			<code>ROBOCODES_HOST_LIB</code>, <code>ROBOCODES_AGENT_JAR</code> and
			<code>ROBOCODES_ROBOT_JARS</code> point). Its data folder is <code>target/robocodes-data</code>.
			Add <code>target/release</code> to your <code>PATH</code> to type <code>robocode-s</code>
			elsewhere. Then follow <a href={resolve('/docs/getting-started')}>Getting started</a>.
		</p>
	</section>

	<section class="panel">
		<h2>Test it</h2>
		<pre>{test}</pre>
		<p>
			Windows, the sandboxes and the rest of the build are described in the
			<a href="{REPO_URL}#building">README</a> and the
			<a href="{REPO_URL}/blob/main/docs/design.md">design document</a>.
		</p>
	</section>
</div>
