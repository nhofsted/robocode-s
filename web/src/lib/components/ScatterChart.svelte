<script lang="ts">
	// One dot per pairing, after LiteRumble's bot charts: x is a property of the opponent (its
	// APS or survival), y how this bot does against it. Hover finds the nearest dot; clicking it
	// opens that pairing, and only the hovered dot gets a tooltip. The pairing table on the same
	// page is the table view. `highlight` marks an opponent hovered elsewhere on the page (a list,
	// another chart) without a tooltip; `onhover` reports this chart's hovered opponent for that.

	// Not colour-coded: colouring by y (score, survival, PBI) only repeats the dot's height as an
	// even gradient, and colouring by x repeats its position. Nothing else per pairing is worth a
	// channel: PBI is shown by the third chart and battle counts by the table.
	import { formatNumber } from '#lib/rumble/format.ts';

	interface Dot {
		id: string;
		name: string;
		x: number;
		y: number;
		href: string;
	}
	interface Props {
		title: string;
		dots: Dot[];
		xLabel: string;
		yLabel: string;
		/** y range; defaults to 0–100. */
		yDomain?: [number, number];
		/** Horizontal reference line (50: even; 0: as expected). */
		ref?: number;
		/** Format y values with a sign (differences). */
		signed?: boolean;
		/** Id of a dot to mark, without a tooltip. */
		highlight?: string | null;
		/** Called with the hovered dot's id, or null when the pointer leaves it. */
		onhover?: (id: string | null) => void;
	}
	let {
		title,
		dots,
		xLabel,
		yLabel,
		yDomain = [0, 100],
		ref = 50,
		signed = false,
		highlight = null,
		onhover
	}: Props = $props();

	let width = $state(300);
	const height = $derived(Math.round(Math.min(300, Math.max(200, width * 0.85))));
	const pad = { top: 10, right: 10, bottom: 34, left: 42 };
	const plotW = $derived(width - pad.left - pad.right);
	const plotH = $derived(height - pad.top - pad.bottom);

	const x = (v: number) => pad.left + (v / 100) * plotW;
	const y = (v: number) => {
		const [lo, hi] = yDomain;
		const c = Math.max(lo, Math.min(hi, v));
		return pad.top + (1 - (c - lo) / (hi - lo)) * plotH;
	};
	const yTicks = $derived.by(() => {
		const [lo, hi] = yDomain;
		const step = (hi - lo) / 4;
		return [0, 1, 2, 3, 4].map((k) => lo + k * step);
	});

	let hovered: Dot | null = $state(null);
	const marked = $derived(hovered ? null : (dots.find((d) => d.id === highlight) ?? null));

	function hover(dot: Dot | null) {
		if (dot?.id === hovered?.id) return;
		hovered = dot;
		onhover?.(dot?.id ?? null);
	}

	function onMove(event: PointerEvent) {
		const r = (event.currentTarget as SVGElement).getBoundingClientRect();
		const mx = event.clientX - r.left;
		const my = event.clientY - r.top;
		let best: Dot | null = null;
		let bestD = 12 * 12;
		for (const d of dots) {
			const dx = x(d.x) - mx;
			const dy = y(d.y) - my;
			const dist = dx * dx + dy * dy;
			if (dist < bestD) {
				best = d;
				bestD = dist;
			}
		}
		hover(best);
	}

	const fmtY = (v: number) => (signed && v > 0 ? '+' : '') + formatNumber(v, 1);
</script>

<figure class="scatter">
	<figcaption>{title}</figcaption>
	<div class="plot" bind:clientWidth={width}>
		<svg
			{width}
			{height}
			role="img"
			aria-label="{title}: {dots.length} opponents; the pairing table below lists the same values"
			onpointermove={onMove}
			onpointerleave={() => hover(null)}
		>
			{#each yTicks as t (t)}
				<line class="grid" x1={pad.left} x2={width - pad.right} y1={y(t)} y2={y(t)} />
				<text class="axis" x={pad.left - 6} y={y(t)} dy="0.32em" text-anchor="end">{fmtY(t).replace('.0', '')}</text>
			{/each}
			{#each [0, 25, 50, 75, 100] as t (t)}
				<text class="axis" x={x(t)} y={height - pad.bottom + 14} text-anchor="middle">{t}</text>
			{/each}
			<line class="ref" x1={pad.left} x2={width - pad.right} y1={y(ref)} y2={y(ref)} />
			<text class="label" x={pad.left + plotW / 2} y={height - 4} text-anchor="middle">{xLabel}</text>
			<text class="label" transform="translate(11 {pad.top + plotH / 2}) rotate(-90)" text-anchor="middle">{yLabel}</text>
			{#each dots as d (d.id)}
				<circle class="dot" cx={x(d.x)} cy={y(d.y)} r="3" />
			{/each}
			{#if marked}
				<circle class="dot active" cx={x(marked.x)} cy={y(marked.y)} r="5" />
			{/if}
			{#if hovered}
				<!-- The hovered dot links to its pairing; the big transparent circle is the hit area. -->
				<a href={hovered.href} aria-label="{hovered.name}: open pairing">
					<circle class="hit" cx={x(hovered.x)} cy={y(hovered.y)} r="12" />
					<circle class="dot active" cx={x(hovered.x)} cy={y(hovered.y)} r="5" />
				</a>
			{/if}
		</svg>
		{#if hovered}
			<div class="tip" style:left="{x(hovered.x)}px" style:top="{y(hovered.y)}px">
				<strong class="mono">{fmtY(hovered.y)}</strong>
				<span>{hovered.name} · {xLabel.toLowerCase()} {formatNumber(hovered.x, 1)}</span>
			</div>
		{/if}
	</div>
</figure>

<style>
	.scatter {
		margin: 0;
		min-width: 0;
	}
	figcaption {
		font-size: 0.9rem;
		font-weight: 600;
		margin-bottom: 0.3rem;
	}
	.plot {
		position: relative;
	}
	svg {
		display: block;
		touch-action: pan-y;
	}
	.hit {
		fill: transparent;
		cursor: pointer;
	}
	.grid {
		stroke: var(--line);
	}
	.ref {
		stroke: var(--text-2);
		stroke-dasharray: 4 3;
	}
	.axis,
	.label {
		fill: var(--muted);
		font-family: var(--font-mono);
		font-size: 10px;
	}
	.label {
		font-family: var(--font-sans);
		font-size: 11px;
	}
	.dot {
		fill: color-mix(in srgb, var(--radar) 55%, transparent);
	}
	.dot.active {
		fill: var(--radar);
		stroke: var(--panel);
		stroke-width: 2;
	}
	.tip {
		position: absolute;
		transform: translate(-50%, calc(-100% - 10px));
		background: var(--panel-2);
		border: 1px solid var(--line-strong);
		border-radius: var(--radius);
		padding: 0.2rem 0.5rem;
		font-size: 0.8rem;
		pointer-events: none;
		white-space: nowrap;
		display: flex;
		flex-direction: column;
		align-items: center;
		z-index: 2;
	}
	.tip span {
		color: var(--text-2);
	}
</style>
