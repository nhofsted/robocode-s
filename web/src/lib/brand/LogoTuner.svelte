<script lang="ts">
	// Place the flame S by hand: drag on the large preview (or use the arrow keys), scale it with
	// the slider (or + and −), and copy the numbers into FLAME_S (classic-logo.ts).
	import ClassicWordmark from './ClassicWordmark.svelte';
	import { FLAME_S } from './classic-logo.ts';

	const BIG = 110;
	const KEY = 'robocode-s:flame-s';

	function initial() {
		try {
			const saved = JSON.parse(localStorage.getItem(KEY) ?? 'null');
			if (saved && ['x', 'y', 'size'].every((k) => typeof saved[k] === 'number')) return saved;
		} catch {
			// No storage: start from the defaults.
		}
		return { ...FLAME_S };
	}

	let flame = $state(initial());
	let copied = $state(false);

	$effect(() => {
		const value = JSON.stringify(flame);
		try {
			localStorage.setItem(KEY, value);
		} catch {
			// Not saved; the values are still shown below.
		}
	});

	const round = (v: number) => Math.round(v * 1000) / 1000;
	const code = $derived(`{ x: ${round(flame.x)}, y: ${round(flame.y)}, size: ${round(flame.size)} }`);

	let drag: { px: number; py: number; x: number; y: number } | null = null;
	function down(event: PointerEvent) {
		drag = { px: event.clientX, py: event.clientY, x: flame.x, y: flame.y };
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}
	function move(event: PointerEvent) {
		if (!drag) return;
		flame.x = round(drag.x + (event.clientX - drag.px) / BIG);
		flame.y = round(drag.y + (event.clientY - drag.py) / BIG);
	}
	function key(event: KeyboardEvent) {
		const step = event.shiftKey ? 0.05 : 0.01;
		const moves: Record<string, () => void> = {
			ArrowLeft: () => (flame.x = round(flame.x - step)),
			ArrowRight: () => (flame.x = round(flame.x + step)),
			ArrowUp: () => (flame.y = round(flame.y - step)),
			ArrowDown: () => (flame.y = round(flame.y + step)),
			'+': () => (flame.size = round(flame.size + step)),
			'=': () => (flame.size = round(flame.size + step)),
			'-': () => (flame.size = round(Math.max(0.5, flame.size - step)))
		};
		const f = moves[event.key];
		if (f) {
			event.preventDefault();
			f();
		}
	}

	async function copy() {
		try {
			await navigator.clipboard.writeText(code);
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			copied = false;
		}
	}
</script>

<section class="tuner panel">
	<h2>Place the S</h2>
	<p class="muted">
		Drag on the large logo to move the S (arrow keys nudge it, Shift for bigger steps), scale it
		with the slider or + / −. The small versions update live. Send me the numbers at the bottom.
	</p>

	<button
		type="button"
		class="stage"
		aria-label="Drag to move the S; arrow keys nudge, plus and minus scale"
		onpointerdown={down}
		onpointermove={move}
		onpointerup={() => (drag = null)}
		onpointercancel={() => (drag = null)}
		onkeydown={key}
	>
		<ClassicWordmark height={BIG} look="glow" s="fighter" {flame} />
	</button>

	<div class="controls">
		<label>
			Size of the S
			<input type="range" min="0.8" max="3" step="0.01" bind:value={flame.size} />
			<span class="mono">{round(flame.size)}</span>
		</label>
		<button class="button secondary" type="button" onclick={() => (flame = { ...FLAME_S })}>Reset</button>
	</div>

	<div class="previews">
		<div class="bar"><ClassicWordmark height={28} look="glow" s="fighter" {flame} /><span class="muted">header size</span></div>
		<div class="bar"><ClassicWordmark height={48} look="glow" s="fighter" {flame} /></div>
	</div>

	<div class="out">
		<code class="mono">FLAME_S = {code}</code>
		<button type="button" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
	</div>
</section>

<style>
	.tuner {
		padding: 1rem 1.2rem 1.2rem;
		margin: 1.5rem 0;
	}
	.tuner h2 {
		margin-top: 0.2rem;
	}
	.stage {
		display: block;
		width: 100%;
		text-align: left;
		color: inherit;
		padding: 2rem 2.5rem;
		background: #080b10;
		border: 1px dashed var(--line-strong);
		border-radius: var(--radius);
		cursor: grab;
		touch-action: none;
		user-select: none;
		overflow: hidden;
	}
	.stage:active {
		cursor: grabbing;
	}
	.controls {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin: 1rem 0;
		flex-wrap: wrap;
	}
	.controls label {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		font-size: 0.9rem;
	}
	.controls input {
		width: min(20rem, 50vw);
	}
	.previews {
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}
	.bar {
		display: flex;
		align-items: center;
		gap: 1.5rem;
		padding: 0.6rem 1rem;
		background: var(--bg);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		font-size: 0.8rem;
	}
	.out {
		display: flex;
		align-items: center;
		gap: 0.8rem;
		margin-top: 1rem;
		flex-wrap: wrap;
	}
	.out code {
		font-size: 0.95rem;
	}
</style>
