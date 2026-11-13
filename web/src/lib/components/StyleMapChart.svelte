<script lang="ts">
	// The style map (types.ts `StyleMap`): one dot per bot, coloured by APS on a one-hue ramp
	// (dim: weak, bright: strong). Hover finds the nearest dot within reach and shows a tooltip;
	// clicking selects it, and the selected bot is joined to its true nearest neighbours (by
	// style distance, which the layout only approximates). Wheel or pinch zooms around the
	// pointer, dragging pans, double-click resets. Dots keep their screen size when zooming.
	// Dense on purpose: dots are smaller than a chart's markers would be, with a thin surface
	// ring so overlapping dots stay apart.
	import { untrack } from 'svelte';
	import { formatNumber, shortName } from '#lib/rumble/format.ts';
	import type { MapBot } from '#lib/rumble/types.ts';

	interface Info {
		rank: number;
		aps: number;
	}
	interface Props {
		bots: MapBot[];
		/** Rank and APS by bot id (from the rankings). */
		info: Map<string, Info>;
		/** Index of the selected bot, or null. */
		selected: number | null;
		onselect: (index: number | null) => void;
	}
	let { bots, info, selected, onselect }: Props = $props();

	let width = $state(600);
	const height = $derived(Math.round(Math.min(760, Math.max(360, width * 0.9))));
	const pad = 14;

	// The view: centre and half-extent in map units (-1..1 is the whole map).
	let view = $state({ cx: 0, cy: 0, half: 1.08 });
	const scale = $derived(Math.min(width, height) / 2 / view.half);
	const sx = (x: number) => width / 2 + (x - view.cx) * scale;
	// Map y grows upwards.
	const sy = (y: number) => height / 2 - (y - view.cy) * scale;

	// The ramp spans the 5th to 95th percentile of APS, so a few extreme bots don't flatten it.
	const apsRange = $derived.by(() => {
		const values = bots.map((b) => info.get(b.id)?.aps ?? 0).sort((a, b) => a - b);
		const at = (q: number) => values[Math.min(values.length - 1, Math.floor(q * values.length))] ?? 0;
		return [at(0.05), at(0.95)] as const;
	});
	/** Dim to bright in one hue; `t` 0..1. */
	function ramp(t: number): string {
		const c = Math.max(0, Math.min(1, t));
		return `color-mix(in oklab, #24477a ${((1 - c) * 100).toFixed(1)}%, #b6dbff)`;
	}
	const color = (id: string) => {
		const [lo, hi] = apsRange;
		const aps = info.get(id)?.aps ?? lo;
		return ramp(hi > lo ? (aps - lo) / (hi - lo) : 0.5);
	};

	// Draw order: weaker first, so the strong bots' dots end up on top.
	const order = $derived(
		bots.map((_, i) => i).sort((a, b) => (info.get(bots[a].id)?.aps ?? 0) - (info.get(bots[b].id)?.aps ?? 0))
	);
	const neighbours = $derived(selected === null ? [] : bots[selected].near.map(([j]) => j));

	let hovered: number | null = $state(null);

	function nearestDot(mx: number, my: number): number | null {
		let best: number | null = null;
		let bestD = 12 * 12;
		for (let i = 0; i < bots.length; i++) {
			const dx = sx(bots[i].x) - mx;
			const dy = sy(bots[i].y) - my;
			const d = dx * dx + dy * dy;
			if (d < bestD) {
				bestD = d;
				best = i;
			}
		}
		return best;
	}

	// Pointer handling: one pointer pans (or clicks, if it did not move), two pinch.
	const pointers = new Map<number, { x: number; y: number }>();
	let moved = false;
	let pinch: number | null = null;

	function local(event: PointerEvent | WheelEvent | MouseEvent) {
		const r = (event.currentTarget as Element).getBoundingClientRect();
		return { x: event.clientX - r.left, y: event.clientY - r.top };
	}

	function zoomAt(px: number, py: number, factor: number) {
		const half = Math.max(0.03, Math.min(1.5, view.half * factor));
		// Keep the map point under the pointer where it is.
		const mx = view.cx + (px - width / 2) / scale;
		const my = view.cy - (py - height / 2) / scale;
		const k = half / view.half;
		view = { cx: mx - (mx - view.cx) * k, cy: my - (my - view.cy) * k, half };
	}

	function onWheel(event: WheelEvent) {
		event.preventDefault();
		const p = local(event);
		zoomAt(p.x, p.y, Math.exp(event.deltaY * (event.deltaMode === 1 ? 0.05 : 0.0015)));
	}

	function onDown(event: PointerEvent) {
		(event.currentTarget as Element).setPointerCapture(event.pointerId);
		pointers.set(event.pointerId, local(event));
		moved = false;
		if (pointers.size === 2) {
			const [a, b] = [...pointers.values()];
			pinch = Math.hypot(a.x - b.x, a.y - b.y);
		}
	}

	function onMove(event: PointerEvent) {
		const p = local(event);
		const last = pointers.get(event.pointerId);
		if (!last) {
			hovered = nearestDot(p.x, p.y);
			return;
		}
		pointers.set(event.pointerId, p);
		if (pointers.size === 2 && pinch !== null) {
			const [a, b] = [...pointers.values()];
			const d = Math.hypot(a.x - b.x, a.y - b.y);
			if (d > 0) zoomAt((a.x + b.x) / 2, (a.y + b.y) / 2, pinch / d);
			pinch = d;
			moved = true;
			return;
		}
		const dx = p.x - last.x;
		const dy = p.y - last.y;
		if (!moved && Math.hypot(dx, dy) < 3) return;
		moved = true;
		hovered = null;
		view = { ...view, cx: view.cx - dx / scale, cy: view.cy + dy / scale };
	}

	function onUp(event: PointerEvent) {
		const wasClick = pointers.size === 1 && !moved;
		pointers.delete(event.pointerId);
		if (pointers.size < 2) pinch = null;
		if (wasClick) {
			const p = local(event);
			onselect(nearestDot(p.x, p.y));
		}
	}

	function reset() {
		view = { cx: 0, cy: 0, half: 1.08 };
	}

	// Bring a bot selected elsewhere (the search, the URL) into view.
	$effect(() => {
		if (selected === null) return;
		const b = bots[selected];
		// Only when the selection changes, not while the visitor pans away from it.
		untrack(() => {
			const x = sx(b.x);
			const y = sy(b.y);
			if (x < 20 || x > width - 20 || y < 20 || y > height - 20) view = { ...view, cx: b.x, cy: b.y };
		});
	});

	const tip = $derived.by(() => {
		if (hovered === null) return null;
		const b = bots[hovered];
		const i = info.get(b.id);
		const x = sx(b.x);
		const y = sy(b.y);
		return {
			name: shortName(b.id),
			line: i ? `#${i.rank} · APS ${formatNumber(i.aps, 2)}` : '',
			left: x > width - 180 ? x - 12 : x + 12,
			top: y < 50 ? y + 12 : y - 12,
			right: x > width - 180,
			below: y < 50
		};
	});
	const visible = (x: number, y: number) => x > -pad && x < width + pad && y > -pad && y < height + pad;
</script>

<div class="map" bind:clientWidth={width}>
	<svg
		{width}
		{height}
		role="img"
		aria-label="Style map: {bots.length} bots; nearby bots have similar strengths and weaknesses"
		onwheel={onWheel}
		onpointerdown={onDown}
		onpointermove={onMove}
		onpointerup={onUp}
		onpointercancel={onUp}
		onpointerleave={() => (hovered = null)}
		ondblclick={reset}
	>
		{#if selected !== null}
			{@const s = bots[selected]}
			{#each neighbours as j (j)}
				<line class="link" x1={sx(s.x)} y1={sy(s.y)} x2={sx(bots[j].x)} y2={sy(bots[j].y)} />
			{/each}
		{/if}
		{#each order as i (bots[i].id)}
			{@const b = bots[i]}
			{@const x = sx(b.x)}
			{@const y = sy(b.y)}
			{#if visible(x, y)}
				<circle cx={x} cy={y} r={3.5} fill={color(b.id)} class:dim={selected !== null && i !== selected && !neighbours.includes(i)} />
			{/if}
		{/each}
		{#if selected !== null}
			{@const s = bots[selected]}
			{#each neighbours as j (j)}
				<circle class="neighbour" cx={sx(bots[j].x)} cy={sy(bots[j].y)} r={5} />
			{/each}
			<circle class="selected" cx={sx(s.x)} cy={sy(s.y)} r={7} />
			<text class="label" x={sx(s.x) + 10} y={sy(s.y) - 8}>{shortName(s.id)}</text>
		{/if}
		{#if hovered !== null}
			<circle class="hover" cx={sx(bots[hovered].x)} cy={sy(bots[hovered].y)} r={6} />
		{/if}
	</svg>
	{#if tip}
		<div class="tip" class:right={tip.right} class:below={tip.below} style:left="{tip.left}px" style:top="{tip.top}px">
			<strong>{tip.name}</strong>
			<span>{tip.line}</span>
		</div>
	{/if}
	<div class="controls">
		<button type="button" onclick={() => zoomAt(width / 2, height / 2, 1 / 1.5)} aria-label="Zoom in">+</button>
		<button type="button" onclick={() => zoomAt(width / 2, height / 2, 1.5)} aria-label="Zoom out">−</button>
		<button type="button" onclick={reset}>Reset</button>
	</div>
	<div class="legend" aria-hidden="true">
		<span>APS ≤{formatNumber(apsRange[0], 0)}</span>
		<span class="bar" style:background="linear-gradient(to right, {ramp(0)}, {ramp(0.5)}, {ramp(1)})"></span>
		<span>≥{formatNumber(apsRange[1], 0)}</span>
	</div>
</div>

<style>
	.map {
		position: relative;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow: hidden;
		touch-action: none;
		user-select: none;
	}
	svg {
		display: block;
		cursor: grab;
	}
	svg:active {
		cursor: grabbing;
	}
	circle {
		stroke: var(--panel);
		stroke-width: 1;
	}
	circle.dim {
		opacity: 0.55;
	}
	.link {
		stroke: var(--amber);
		stroke-width: 1;
		opacity: 0.6;
	}
	circle.neighbour {
		fill: none;
		stroke: var(--amber);
		stroke-width: 2;
	}
	circle.selected {
		fill: none;
		stroke: var(--text);
		stroke-width: 2;
	}
	circle.hover {
		fill: none;
		stroke: var(--text);
		stroke-width: 1.5;
		pointer-events: none;
	}
	.label {
		fill: var(--text);
		font-size: 0.8rem;
		paint-order: stroke;
		stroke: var(--panel);
		stroke-width: 3px;
		pointer-events: none;
	}
	.tip {
		position: absolute;
		transform: translateY(-100%);
		pointer-events: none;
		background: var(--panel-2);
		border: 1px solid var(--line-strong);
		border-radius: var(--radius);
		padding: 0.3rem 0.5rem;
		font-size: 0.8rem;
		display: flex;
		flex-direction: column;
		white-space: nowrap;
	}
	.tip.right {
		transform: translate(-100%, -100%);
	}
	.tip.below {
		transform: none;
	}
	.tip.right.below {
		transform: translateX(-100%);
	}
	.tip span {
		color: var(--text-2);
	}
	.controls {
		position: absolute;
		top: 0.5rem;
		right: 0.5rem;
		display: flex;
		gap: 0.25rem;
	}
	.controls button {
		min-width: 2rem;
		padding: 0.15rem 0.5rem;
	}
	.legend {
		position: absolute;
		left: 0.6rem;
		bottom: 0.5rem;
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.75rem;
		color: var(--text-2);
		background: rgb(16 22 31 / 0.8);
		padding: 0.15rem 0.4rem;
		border-radius: var(--radius);
	}
	.legend .bar {
		width: 6rem;
		height: 0.5rem;
		border-radius: 2px;
	}
</style>
