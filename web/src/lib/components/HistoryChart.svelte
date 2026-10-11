<script lang="ts">
	// One metric over time on a time axis: a 2px line, recessive grid, a crosshair that snaps to
	// the nearest point, a marker at every release, zoom, and a table view. Rank is drawn
	// inverted (1 at the top).
	// Zoom: the range buttons, dragging across the chart, or + / − / 0 on the keyboard;
	// double-click shows everything again.
	import { formatDate, formatNumber } from '#lib/rumble/format.ts';

	interface Point {
		date: string;
		value: number | null;
		version?: string;
	}
	interface Props {
		points: Point[];
		label: string;
		digits: number;
		invert?: boolean;
		/** Releases: a vertical marker at `date`. */
		markers?: { date: string; label: string }[];
	}
	let { points, label, digits, invert = false, markers = [] }: Props = $props();

	const DAY = 86_400_000;
	const MIN_SPAN = 14 * DAY;

	let width = $state(640);
	const height = 240;
	const pad = { top: 24, right: 16, bottom: 26, left: 52 };
	const plotW = $derived(Math.max(10, width - pad.left - pad.right));

	const times = $derived(points.map((p) => Date.parse(p.date)));
	const extent = $derived([times[0] ?? 0, times.at(-1) ?? DAY] as const);

	/** The visible time range; null shows everything. */
	let view: [number, number] | null = $state(null);
	const range = $derived.by((): readonly [number, number] => {
		const [a, b] = extent;
		if (!view) return b - a < MIN_SPAN ? [b - MIN_SPAN, b] : [a, b];
		return view;
	});

	const PRESETS = [
		{ label: '3M', days: 91 },
		{ label: '1Y', days: 365 },
		{ label: '3Y', days: 3 * 365 }
	];
	const span = $derived(extent[1] - extent[0]);
	const presets = $derived(PRESETS.filter((p) => p.days * DAY < span));

	function zoomTo(a: number, b: number) {
		const [lo, hi] = extent;
		if (b - a < MIN_SPAN) {
			const mid = (a + b) / 2;
			a = mid - MIN_SPAN / 2;
			b = mid + MIN_SPAN / 2;
		}
		a = Math.max(lo, a);
		b = Math.min(hi, b);
		view = a <= lo && b >= hi ? null : [a, b];
	}

	function zoomBy(factor: number, around?: number) {
		const [a, b] = range;
		const c = around ?? (a + b) / 2;
		zoomTo(c - (c - a) * factor, c + (b - c) * factor);
	}

	/** Visible points, plus one on each side so the line runs to the edges. */
	const first = $derived(Math.max(0, times.findIndex((t) => t >= range[0]) - 1));
	const last = $derived.by(() => {
		let i = times.length - 1;
		while (i > 0 && times[i] > range[1]) i--;
		return Math.min(times.length - 1, i + 1);
	});
	const visible = $derived(
		points.slice(first, last + 1).map((p, k) => ({ ...p, t: times[first + k], i: first + k }))
	);

	const domain = $derived.by(() => {
		const values = visible
			.filter((p) => p.t >= range[0] && p.t <= range[1] && p.value !== null)
			.map((p) => p.value as number);
		if (values.length === 0) return [0, 1] as const;
		let lo = Math.min(...values);
		let hi = Math.max(...values);
		if (hi - lo < 1e-9) {
			lo -= 1;
			hi += 1;
		}
		const margin = (hi - lo) * 0.08;
		return [lo - margin, hi + margin] as const;
	});

	const x = (t: number) => pad.left + ((t - range[0]) / (range[1] - range[0])) * plotW;
	const y = (v: number) => {
		const [lo, hi] = domain;
		const f = (v - lo) / (hi - lo);
		return pad.top + (invert ? f : 1 - f) * (height - pad.top - pad.bottom);
	};
	const tAt = (px: number) =>
		range[0] + ((px - pad.left) / plotW) * (range[1] - range[0]);

	const path = $derived.by(() => {
		let d = '';
		let pen = false;
		for (const p of visible) {
			if (p.value === null) {
				pen = false;
				continue;
			}
			d += `${pen ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.value).toFixed(1)}`;
			pen = true;
		}
		return d;
	});

	const ticks = $derived.by(() => {
		const [lo, hi] = domain;
		const raw = (hi - lo) / 4;
		const mag = 10 ** Math.floor(Math.log10(raw));
		const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
		const out: number[] = [];
		for (let v = Math.ceil(lo / step) * step; v <= hi; v += step) out.push(v);
		return out;
	});

	/** Month ticks for short ranges, years (or every 3 months) for longer ones. */
	const timeTicks = $derived.by(() => {
		const [a, b] = range;
		const months = (b - a) / (30 * DAY);
		const every = months <= 14 ? 1 : months <= 30 ? 3 : 12;
		const d = new Date(a);
		const out: { t: number; text: string }[] = [];
		let cur = Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
		while (cur <= b) {
			const c = new Date(cur);
			if (c.getUTCMonth() % every === 0) {
				const year = c.getUTCMonth() === 0;
				out.push({
					t: cur,
					text:
						every === 12 || (year && every > 1)
							? String(c.getUTCFullYear())
							: c.toLocaleDateString('en-GB', { month: 'short', timeZone: 'UTC' }) +
								(year ? ` ${c.getUTCFullYear()}` : '')
				});
			}
			cur = Date.UTC(c.getUTCFullYear(), c.getUTCMonth() + 1, 1);
		}
		return out;
	});

	const releaseMarks = $derived(
		markers
			.map((m) => ({ ...m, t: Date.parse(m.date.slice(0, 10)) }))
			.filter((m) => m.t >= range[0] && m.t <= range[1])
	);

	// Hover, keyboard and drag-to-zoom.
	let hover: number | null = $state(null);
	let drag: { from: number; to: number } | null = $state(null);

	function nearest(t: number): number {
		let best = first;
		for (const p of visible) {
			if (Math.abs(p.t - t) < Math.abs(times[best] - t)) best = p.i;
		}
		return best;
	}

	function localX(event: PointerEvent | MouseEvent): number {
		return event.clientX - (event.currentTarget as SVGElement).getBoundingClientRect().left;
	}

	function onDown(event: PointerEvent) {
		if (event.button !== 0) return;
		const px = localX(event);
		drag = { from: px, to: px };
		(event.currentTarget as SVGElement).setPointerCapture(event.pointerId);
	}
	function onMove(event: PointerEvent) {
		const px = localX(event);
		if (drag) drag.to = Math.max(pad.left, Math.min(width - pad.right, px));
		hover = nearest(tAt(px));
	}
	function onUp() {
		if (drag && Math.abs(drag.to - drag.from) > 8) {
			const a = tAt(Math.min(drag.from, drag.to));
			const b = tAt(Math.max(drag.from, drag.to));
			zoomTo(a, b);
		}
		drag = null;
	}

	function onKey(event: KeyboardEvent) {
		const h = hover ?? times.length - 1;
		if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
			event.preventDefault();
			const next = Math.max(0, Math.min(times.length - 1, h + (event.key === 'ArrowLeft' ? -1 : 1)));
			hover = next;
			if (times[next] < range[0] || times[next] > range[1]) {
				const w = range[1] - range[0];
				zoomTo(times[next] - w / 2, times[next] + w / 2);
			}
		} else if (event.key === '+' || event.key === '=') {
			zoomBy(0.5, times[h]);
		} else if (event.key === '-') {
			zoomBy(2, times[h]);
		} else if (event.key === '0') {
			view = null;
		}
	}

	const hovered = $derived(hover === null ? null : points[hover]);
	const hoverX = $derived(hover === null ? 0 : x(times[hover]));
	const tipLeft = $derived(Math.min(Math.max(hoverX, 80), width - 80));
	const valueText = $derived.by(() => {
		const p = points[hover ?? points.length - 1];
		const v = p.version ? `, version ${p.version}` : '';
		return `${formatDate(p.date)}${v}: ${label} ${formatNumber(p.value, digits)}`;
	});
	const zoomed = $derived(view !== null);
</script>

{#if points.length < 2}
	<p class="muted">Not enough history yet: rankings are snapshotted once a day.</p>
{:else}
	<div class="controls" role="group" aria-label="Zoom">
		{#each presets as p (p.label)}
			<button
				class="sort"
				aria-pressed={view !== null && Math.abs(range[1] - range[0] - p.days * DAY) < DAY && range[1] >= extent[1] - DAY}
				onclick={() => zoomTo(extent[1] - p.days * DAY, extent[1])}>{p.label}</button
			>
		{/each}
		<button class="sort" aria-pressed={!zoomed} onclick={() => (view = null)}>All</button>
		<span class="hint muted">{zoomed ? 'Double-click to show all' : 'Drag across the chart to zoom'}</span>
	</div>
	<div class="chart" bind:clientWidth={width}>
		<svg
			{width}
			{height}
			role="slider"
			aria-label="{label} from {formatDate(points[0].date)} to {formatDate(points[points.length - 1].date)}; arrow keys move between days, plus and minus zoom, 0 shows all"
			aria-valuemin={0}
			aria-valuemax={points.length - 1}
			aria-valuenow={hover ?? points.length - 1}
			aria-valuetext={valueText}
			tabindex="0"
			onpointerdown={onDown}
			onpointermove={onMove}
			onpointerup={onUp}
			onpointercancel={() => (drag = null)}
			onpointerleave={() => {
				if (!drag) hover = null;
			}}
			ondblclick={() => (view = null)}
			onkeydown={onKey}
			onblur={() => (hover = null)}
		>
			<defs>
				<clipPath id="plot-{label}">
					<rect x={pad.left} y={0} width={plotW} height={height} />
				</clipPath>
			</defs>
			{#each ticks as t (t)}
				<line class="grid" x1={pad.left} x2={width - pad.right} y1={y(t)} y2={y(t)} />
				<text class="axis" x={pad.left - 8} y={y(t)} dy="0.32em" text-anchor="end">
					{formatNumber(t, digits > 0 && ticks.some((v) => !Number.isInteger(v)) ? 1 : 0)}
				</text>
			{/each}
			{#each timeTicks as m (m.t)}
				<text class="axis" x={x(m.t)} y={height - 6} text-anchor="middle">{m.text}</text>
			{/each}
			<g clip-path="url(#plot-{label})">
				{#each releaseMarks as m (m.date)}
					<line class="release" x1={x(m.t)} x2={x(m.t)} y1={pad.top - 6} y2={height - pad.bottom} />
				{/each}
				<path class="line" d={path} />
				{#if hovered && hovered.value !== null && hover !== null && !drag}
					<line class="cross" x1={hoverX} x2={hoverX} y1={pad.top} y2={height - pad.bottom} />
					<circle class="dot" cx={hoverX} cy={y(hovered.value)} r="4" />
				{/if}
				{#if drag}
					<rect
						class="brush"
						x={Math.min(drag.from, drag.to)}
						y={pad.top}
						width={Math.abs(drag.to - drag.from)}
						height={height - pad.top - pad.bottom}
					/>
				{/if}
			</g>
			{#each releaseMarks as m (m.date)}
				<text class="release-label" x={x(m.t) + 4} y={pad.top - 10}>{m.label}</text>
			{/each}
		</svg>
		{#if hovered && !drag}
			<div class="tip" style:left="{tipLeft}px" aria-live="polite">
				<strong class="mono">{formatNumber(hovered.value, digits)}</strong>
				<span>{label} · {formatDate(hovered.date)}{hovered.version ? ` · v${hovered.version}` : ''}</span>
			</div>
		{/if}
	</div>
	<details>
		<summary>Show as table</summary>
		<div class="table-scroll history">
			<table class="data">
				<thead><tr><th>Date</th><th>Version</th><th class="num">{label}</th></tr></thead>
				<tbody>
					{#each [...points].reverse() as p (p.date)}
						<tr>
							<td>{formatDate(p.date)}</td>
							<td class="mono">{p.version ?? ''}</td>
							<td class="num">{formatNumber(p.value, digits)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</details>
{/if}

<style>
	.controls {
		display: flex;
		gap: 0.9rem;
		align-items: baseline;
		font-size: 0.85rem;
		margin-bottom: 0.4rem;
		flex-wrap: wrap;
	}
	.hint {
		font-size: 0.75rem;
		margin-left: auto;
	}
	.chart {
		position: relative;
		width: 100%;
	}
	svg {
		display: block;
		touch-action: pan-y;
		cursor: crosshair;
		user-select: none;
	}
	svg:focus-visible {
		outline: 2px solid var(--amber);
	}
	svg:focus:not(:focus-visible) {
		outline: none;
	}
	.grid {
		stroke: var(--line);
		stroke-width: 1;
	}
	.axis {
		fill: var(--muted);
		font-family: var(--font-mono);
		font-size: 11px;
	}
	.line {
		fill: none;
		stroke: var(--radar);
		stroke-width: 2;
		stroke-linejoin: round;
		stroke-linecap: round;
	}
	.release {
		stroke: var(--amber);
		stroke-width: 1.5;
		stroke-dasharray: 4 3;
		opacity: 0.8;
	}
	.release-label {
		fill: var(--amber);
		font-family: var(--font-mono);
		font-size: 11px;
	}
	.cross {
		stroke: var(--text-2);
		stroke-width: 1;
		stroke-dasharray: 3 3;
	}
	.dot {
		fill: var(--radar);
		stroke: var(--panel);
		stroke-width: 2;
	}
	.brush {
		fill: color-mix(in srgb, var(--radar) 15%, transparent);
		stroke: var(--radar);
		stroke-width: 1;
	}
	.tip {
		position: absolute;
		top: 0;
		transform: translateX(-50%);
		background: var(--panel-2);
		border: 1px solid var(--line-strong);
		border-radius: var(--radius);
		padding: 0.25rem 0.55rem;
		font-size: 0.8rem;
		pointer-events: none;
		display: flex;
		flex-direction: column;
		align-items: center;
		white-space: nowrap;
	}
	.tip span {
		color: var(--text-2);
	}
	details {
		margin-top: 0.5rem;
		font-size: 0.85rem;
	}
	summary {
		cursor: pointer;
		color: var(--text-2);
	}
	.history {
		max-height: 16rem;
		margin-top: 0.5rem;
		max-width: 24rem;
	}
</style>
