<script lang="ts">
	// Plays a replay track (ADR 0017) on a canvas, drawn like the desktop GUI draws a battle:
	// classic's floor, scan arcs, tanks with energy and names, bullets, explosions. What to draw
	// each turn is frame.ts; this is the canvas, the clock and the controls.
	import { untrack } from 'svelte';
	import { loadImage, loadSprites, drawTank, css, type Sprites } from '../components/sprites.ts';
	import { tankColors, tankColorsByIndex } from '../components/tank-colors.ts';
	import { RADAR_RANGE, SCAN_LINE_BELOW, frameView } from './frame.ts';
	import type { ReplayTrack } from './track.ts';

	const EXPLOSION = Object.values(
		import.meta.glob<string>('../assets/classic/explosion/*.png', {
			eager: true,
			query: '?url',
			import: 'default'
		})
	).sort((a, b) => frameNo(a) - frameNo(b));
	const TILES = Object.values(
		import.meta.glob<string>('../assets/classic/ground/blue_metal_*.png', {
			eager: true,
			query: '?url',
			import: 'default'
		})
	).sort();

	function frameNo(url: string): number {
		return Number(/explosion1-(\d+)/.exec(url)?.[1] ?? 0);
	}

	/** The GUI's default speed: 30 turns a second. */
	const BASE_TPS = 30;
	const SPEEDS = [0.25, 0.5, 1, 2, 4, 8];

	interface Props {
		track: ReplayTrack;
		/** A name per track robot index; "Robot N" where missing. */
		names?: readonly string[];
		/** A bot id per track robot index, to derive the tank colours from; a hue per robot if missing. */
		colorKeys?: readonly string[];
		/** Starts playing (not with prefers-reduced-motion). */
		autoplay?: boolean;
	}
	let { track, names = [], colorKeys = [], autoplay = true }: Props = $props();

	const robots = $derived(
		Array.from({ length: track.robotCount }, (_, i) => {
			const name = names[i] || `Robot ${i + 1}`;
			// Bot ids give a bot the same colours everywhere on the site; without them, one hue each.
			return { name, colors: colorKeys[i] ? tankColors(colorKeys[i]) : tankColorsByIndex(i) };
		})
	);
	/** The most energy a robot starts with, for the energy bars. */
	const maxEnergy = $derived(
		Math.max(100, ...Array.from(frameView(track, 0).energy))
	);

	/** Position in turns, fractional while playing. */
	let pos = $state(0);
	let playing = $state(false);
	let speed = $state(1);
	const last = $derived(track.frameCount - 1);
	const frame = $derived(Math.max(0, Math.min(last, Math.floor(pos))));
	const view = $derived(frameView(track, frame));

	let canvas: HTMLCanvasElement | undefined = $state();
	let cssWidth = $state(0);
	let dpr = $state(1);
	let sprites: Sprites | undefined = $state.raw();
	let blasts: HTMLImageElement[] = $state.raw([]);
	let floor: HTMLCanvasElement | undefined = $state.raw();

	$effect(() => {
		untrack(() => {
			playing = autoplay && !matchMedia('(prefers-reduced-motion: reduce)').matches;
		});
	});

	$effect(() => {
		let gone = false;
		Promise.all([loadSprites(), Promise.all(EXPLOSION.map(loadImage)), Promise.all(TILES.map(loadImage))])
			.then(([s, e, tiles]) => {
				if (gone) return;
				sprites = s;
				blasts = e;
				floor = mosaic(tiles, track.width, track.height);
			})
			.catch(() => {});
		return () => (gone = true);
	});

	/** Classic's floor: one of five tiles per 64 px cell, from a fixed seed (as the GUI does). */
	function mosaic(tiles: HTMLImageElement[], width: number, height: number): HTMLCanvasElement {
		const cell = 64;
		const c = document.createElement('canvas');
		c.width = width;
		c.height = height;
		const g = c.getContext('2d')!;
		let seed = 20010101;
		for (let row = 0; row < Math.ceil(height / cell); row++) {
			for (let col = 0; col < Math.ceil(width / cell); col++) {
				seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
				g.drawImage(tiles[Math.round((seed / 4294967296) * 4)], col * cell, row * cell);
			}
		}
		return c;
	}

	// The clock: a fixed turn rate, whatever the display refresh.
	$effect(() => {
		if (!playing) return;
		let raf = 0;
		let before = performance.now();
		const tick = (now: number) => {
			const dt = Math.min(now - before, 250);
			before = now;
			pos = Math.min(last, pos + (dt / 1000) * BASE_TPS * speed);
			if (pos >= last) playing = false;
			else raf = requestAnimationFrame(tick);
		};
		raf = requestAnimationFrame(tick);
		return () => cancelAnimationFrame(raf);
	});

	$effect(() => {
		if (!canvas) return;
		const el = canvas;
		const measure = () => {
			cssWidth = el.clientWidth;
			dpr = Math.min(3, window.devicePixelRatio || 1);
		};
		measure();
		const observer = new ResizeObserver(measure);
		observer.observe(el);
		return () => observer.disconnect();
	});

	$effect(() => {
		const el = canvas;
		const ctx = el?.getContext('2d');
		if (!el || !ctx || cssWidth <= 0) return;
		const w = Math.round(cssWidth * dpr);
		const h = Math.round((w * track.height) / track.width);
		if (el.width !== w || el.height !== h) {
			el.width = w;
			el.height = h;
		}
		draw(ctx, w / track.width, h);
	});

	function draw(ctx: CanvasRenderingContext2D, k: number, h: number) {
		const H = track.height;
		const v = view;
		ctx.setTransform(1, 0, 0, 1, 0, 0);
		ctx.fillStyle = '#12161d';
		ctx.fillRect(0, 0, ctx.canvas.width, h);
		// Field coordinates: x right, y up from the bottom, so the screen's y is H - y.
		ctx.setTransform(k, 0, 0, k, 0, 0);
		if (floor) ctx.drawImage(floor, 0, 0);
		// Scan arcs under the tanks: blue at 20% opacity, a line when the radar barely turned.
		ctx.fillStyle = ctx.strokeStyle = 'rgb(0 0 255 / 0.2)';
		ctx.lineWidth = 1 / k;
		for (const s of v.scans) {
			const cx = s.x;
			const cy = H - s.y;
			// Canvas angles run clockwise from east; headings from north.
			const a0 = s.start - Math.PI / 2;
			ctx.beginPath();
			ctx.moveTo(cx, cy);
			if (Math.abs(s.extent) < SCAN_LINE_BELOW) {
				ctx.lineTo(cx + RADAR_RANGE * Math.sin(s.start), cy - RADAR_RANGE * Math.cos(s.start));
				ctx.stroke();
			} else {
				ctx.arc(cx, cy, RADAR_RANGE, a0, a0 + s.extent, s.extent < 0);
				ctx.closePath();
				ctx.fill();
			}
		}
		if (sprites) {
			for (const t of v.tanks) {
				const c = robots[t.robot].colors;
				drawTank(ctx, sprites, c, t.x, H - t.y, t.body, t.gun, t.radar);
			}
		}
		ctx.fillStyle = '#dcdcdc';
		for (const b of v.bullets) {
			ctx.beginPath();
			ctx.arc(b.x, H - b.y, Math.max(b.diameter, 2 / k) / 2, 0, Math.PI * 2);
			ctx.fill();
		}
		for (const e of v.explosions) {
			const img = blasts[e.sprite];
			if (!img) continue;
			const sw = img.naturalWidth * e.scale;
			const sh = img.naturalHeight * e.scale;
			ctx.drawImage(img, e.x - sw / 2, H - e.y - sh / 2, sw, sh);
		}
		// Energy above and name below each tank, in screen pixels like the GUI's 11 px, 8 to 14.
		ctx.setTransform(1, 0, 0, 1, 0, 0);
		const px = Math.min(14, Math.max(8, 11 * k)) * dpr;
		ctx.font = `${px}px "IBM Plex Sans", system-ui, sans-serif`;
		ctx.textAlign = 'center';
		ctx.lineJoin = 'round';
		ctx.lineWidth = Math.max(2, px / 4);
		ctx.strokeStyle = 'rgb(0 0 0 / 0.7)';
		ctx.fillStyle = '#fff';
		const label = (text: string, x: number, y: number, base: CanvasTextBaseline) => {
			ctx.textBaseline = base;
			ctx.strokeText(text, x, y);
			ctx.fillText(text, x, y);
		};
		for (const t of v.tanks) {
			const x = t.x * k;
			const y = (H - t.y) * k;
			label(t.energy.toFixed(1), x, y - 24 * k, 'bottom');
			label(robots[t.robot].name, x, y + 24 * k, 'top');
		}
	}

	const result = $derived(
		!view.over
			? ''
			: view.winner >= 0
				? `${robots[view.winner].name} wins.`
				: view.survivors === 0
					? 'Round over: no robot survived.'
					: 'Round over.'
	);

	function toggle() {
		if (playing) {
			playing = false;
			return;
		}
		if (frame >= last) pos = 0;
		playing = true;
	}
	function restart() {
		pos = 0;
		playing = false;
	}
	function step(by: number) {
		playing = false;
		pos = Math.max(0, Math.min(last, frame + by));
	}
	function onkeydown(e: KeyboardEvent) {
		if (e.altKey || e.ctrlKey || e.metaKey) return;
		const target = e.target as HTMLElement;
		const onButton = target instanceof HTMLButtonElement;
		const onField = target instanceof HTMLInputElement || target instanceof HTMLSelectElement;
		if (e.key === ' ' && !onButton && !(target instanceof HTMLSelectElement)) toggle();
		else if (e.key === 'ArrowLeft' && !onField) step(-1);
		else if (e.key === 'ArrowRight' && !onField) step(1);
		else if (e.key === 'Home' && !onField) restart();
		else return;
		e.preventDefault();
	}
</script>

{#snippet icon(d: string)}
	<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path {d} fill="currentColor" /></svg>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<section
	class="replay"
	aria-label="Battle replay"
	tabindex="0"
	style:--cap="calc(78vh * {track.width / track.height})"
	{onkeydown}
>
	<div class="stage">
		<!-- svelte-ignore a11y_no_interactive_element_to_noninteractive_role -->
		<canvas
			bind:this={canvas}
			role="img"
			aria-label="Battlefield, turn {frame} of {last}, {view.survivors} of {track.robotCount} robots alive"
			style:aspect-ratio="{track.width} / {track.height}"
		></canvas>
		<div class="controls">
			<div class="buttons">
				<button
					type="button"
					onclick={toggle}
					aria-label={playing ? 'Pause' : frame >= last ? 'Play again' : 'Play'}
					title={playing ? 'Pause' : frame >= last ? 'Play again' : 'Play'}
				>
					{#if playing}
						{@render icon('M7 5h3.5v14H7zM13.5 5H17v14h-3.5z')}
					{:else if frame >= last}
						{@render icon('M12 5a7 7 0 1 1-6.6 4.7l1.9.6A5 5 0 1 0 12 7v3L7.5 6 12 2z')}
					{:else}
						{@render icon('M8 5l11 7-11 7z')}
					{/if}
				</button>
				<button type="button" class="secondary" onclick={restart} aria-label="Restart from the first turn" title="Restart">
					{@render icon('M6 5h2.5v14H6zM19 5v14L9.5 12z')}
				</button>
				<button type="button" class="secondary" onclick={() => step(-1)} disabled={frame <= 0} aria-label="Back one turn" title="Back one turn">
					{@render icon('M15.5 5h2.5v14h-2.5zM14 5v14L5 12z')}
				</button>
				<button type="button" class="secondary" onclick={() => step(1)} disabled={frame >= last} aria-label="Forward one turn" title="Forward one turn">
					{@render icon('M6 5h2.5v14H6zM10 5v14l9-7z')}
				</button>
				<label class="speed">
					<span>Speed</span>
					<select bind:value={speed} aria-label="Playback speed">
						{#each SPEEDS as s (s)}
							<option value={s}>{s === 0.25 ? '¼' : s === 0.5 ? '½' : s}×</option>
						{/each}
					</select>
				</label>
			</div>
			<div class="scrub">
				<input
					type="range"
					min="0"
					max={last}
					step="1"
					value={frame}
					oninput={(e) => (pos = Number(e.currentTarget.value))}
					aria-label="Turn"
					aria-valuetext="turn {frame} of {last}"
				/>
				<output class="turn" aria-hidden="true">turn {frame} / {last}</output>
			</div>
			<p class="result" role="status">{result}</p>
			<p class="hint">Space plays or pauses, &larr; &rarr; step a turn, Home restarts.</p>
		</div>
	</div>

	<ul class="legend" aria-label="Robots">
		{#each robots as r, i (i)}
			<li class:dead={!view.alive[i]} class:winner={view.winner === i}>
				<span class="swatch" style:background={css(r.colors.body)} style:border-color={css(r.colors.radar)} aria-hidden="true"></span>
				<span class="name">{r.name}</span>
				<span class="energy">
					{#if view.alive[i]}
						{view.energy[i].toFixed(1)}
					{:else}
						dead
					{/if}
				</span>
				<span
					class="bar"
					role="meter"
					aria-label="{r.name} energy"
					aria-valuemin="0"
					aria-valuemax={maxEnergy}
					aria-valuenow={view.energy[i]}
				>
					<span style:width="{Math.min(100, (100 * view.energy[i]) / maxEnergy)}%"></span>
				</span>
			</li>
		{/each}
	</ul>
</section>

<style>
	/* The canvas, the controls under it and the legend are one block: the stage is as wide as the
	   canvas may get (the height cap --cap), the legend sits right next to it. */
	.replay {
		display: grid;
		gap: 1rem;
		grid-template-columns: minmax(0, 1fr);
		align-items: start;
	}
	.stage {
		max-width: var(--cap);
		min-width: 0;
	}
	.replay:focus-visible {
		outline-offset: 4px;
	}
	@media (min-width: 56rem) {
		.replay {
			grid-template-columns: minmax(0, var(--cap)) 17rem;
			justify-content: start;
		}
	}
	canvas {
		display: block;
		width: 100%;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius);
		background: #12161d;
	}
	.controls {
		margin-top: 0.7rem;
		padding: 0.6rem 0.8rem;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}
	.buttons {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: center;
	}
	.buttons button {
		min-width: 3rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.buttons svg {
		display: block;
	}
	.speed {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 0.4rem;
		color: var(--text-2);
		font-size: 0.9rem;
	}
	.scrub {
		display: flex;
		align-items: center;
		gap: 0.8rem;
		margin-top: 0.6rem;
	}
	.scrub input {
		flex: 1;
		min-width: 0;
		accent-color: var(--radar);
	}
	.turn {
		font-family: var(--font-mono);
		font-size: 0.85rem;
		color: var(--text-2);
		white-space: nowrap;
	}
	.result {
		margin: 0.5rem 0 0;
		min-height: 1.6em;
		color: var(--amber);
		font-weight: 600;
	}
	.hint {
		margin: 0;
		color: var(--muted);
		font-size: 0.8rem;
	}
	.legend {
		list-style: none;
		margin: 0;
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		column-gap: 1.2rem;
		padding: 0.2rem 0.8rem;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}
	.legend li {
		display: grid;
		grid-template-columns: 1rem minmax(0, 1fr) auto;
		grid-template-areas:
			'sw name energy'
			'bar bar bar';
		column-gap: 0.5rem;
		align-items: center;
		padding: 0.3rem 0;
		border-bottom: 1px solid var(--line);
	}
	.legend li:last-child,
	.legend li:nth-last-child(2):nth-child(odd) {
		border-bottom: 0;
	}
	.swatch {
		grid-area: sw;
		width: 1rem;
		height: 1rem;
		border-radius: 2px;
		border: 2px solid;
	}
	.name {
		grid-area: name;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.energy {
		grid-area: energy;
		font-family: var(--font-mono);
		font-size: 0.85rem;
		color: var(--text-2);
	}
	.bar {
		grid-area: bar;
		display: block;
		height: 5px;
		margin-top: 0.3rem;
		background: var(--panel-2);
		border-radius: 3px;
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--amber);
	}
	.dead .name,
	.dead .energy {
		color: var(--muted);
		text-decoration: line-through;
	}
	.dead .energy {
		text-decoration: none;
		font-style: italic;
	}
	.dead .swatch {
		opacity: 0.4;
	}
	.winner .name {
		color: var(--radar);
		font-weight: 600;
	}
	@media (min-width: 56rem) {
		.legend {
			grid-template-columns: minmax(0, 1fr);
			padding: 0.4rem 0.8rem;
			position: sticky;
			top: 1rem;
		}
		.legend li {
			padding: 0.4rem 0;
		}
		.legend li:nth-last-child(2):nth-child(odd) {
			border-bottom: 1px solid var(--line);
		}
	}
</style>
