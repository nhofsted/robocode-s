<script lang="ts">
	// One metric over time: a 2px line, recessive grid, a crosshair that snaps to the nearest day,
	// a marker at every release, and a table view. Rank is drawn inverted (1 at the top).
	import { formatDate, formatNumber } from '#lib/rumble/format.ts';

	interface Props {
		points: { date: string; value: number | null; version?: string }[];
		label: string;
		digits: number;
		invert?: boolean;
		/** Releases: a vertical marker at the first day on or after `date`. */
		markers?: { date: string; label: string }[];
	}
	let { points, label, digits, invert = false, markers = [] }: Props = $props();

	let width = $state(640);
	const height = 220;
	const pad = { top: 24, right: 16, bottom: 26, left: 52 };
	let hover: number | null = $state(null);

	const valid = $derived(points.filter((p): p is { date: string; value: number } => p.value !== null));

	const domain = $derived.by(() => {
		if (valid.length === 0) return [0, 1] as const;
		let lo = Math.min(...valid.map((p) => p.value));
		let hi = Math.max(...valid.map((p) => p.value));
		if (hi - lo < 1e-9) {
			lo -= 1;
			hi += 1;
		}
		const margin = (hi - lo) * 0.08;
		return [lo - margin, hi + margin] as const;
	});

	const x = (i: number) =>
		pad.left + (points.length <= 1 ? 0 : (i / (points.length - 1)) * (width - pad.left - pad.right));
	const y = (v: number) => {
		const [lo, hi] = domain;
		const t = (v - lo) / (hi - lo);
		const h = height - pad.top - pad.bottom;
		return pad.top + (invert ? t : 1 - t) * h;
	};

	const path = $derived.by(() => {
		let d = '';
		let pen = false;
		points.forEach((p, i) => {
			if (p.value === null) {
				pen = false;
				return;
			}
			d += `${pen ? 'L' : 'M'}${x(i).toFixed(1)},${y(p.value).toFixed(1)}`;
			pen = true;
		});
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

	const releaseMarks = $derived(
		markers
			.filter((m) => m.date.slice(0, 10) > points[0]?.date)
			.map((m) => ({ ...m, i: points.findIndex((p) => p.date >= m.date.slice(0, 10)) }))
			.filter((m) => m.i >= 0)
	);

	const monthTicks = $derived(
		points
			.map((p, i) => ({ i, date: p.date }))
			.filter((p) => p.date.endsWith('-01'))
	);

	function onMove(event: PointerEvent) {
		const rect = (event.currentTarget as SVGElement).getBoundingClientRect();
		const px = event.clientX - rect.left;
		const t = (px - pad.left) / (width - pad.left - pad.right);
		hover = Math.max(0, Math.min(points.length - 1, Math.round(t * (points.length - 1))));
	}

	function onKey(event: KeyboardEvent) {
		if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
		event.preventDefault();
		const start = hover ?? points.length - 1;
		hover = Math.max(0, Math.min(points.length - 1, start + (event.key === 'ArrowLeft' ? -1 : 1)));
	}

	const hovered = $derived(hover === null ? null : points[hover]);
	const valueText = $derived.by(() => {
		const p = points[hover ?? points.length - 1];
		const v = p.version ? `, version ${p.version}` : '';
		return `${formatDate(p.date)}${v}: ${label} ${formatNumber(p.value, digits)}`;
	});
	const tipLeft = $derived(hover === null ? 0 : Math.min(Math.max(x(hover), 70), width - 70));
</script>

{#if points.length < 2}
	<p class="muted">Not enough history yet: rankings are snapshotted once a day.</p>
{:else}
	<div class="chart" bind:clientWidth={width}>
		<svg
			{width}
			{height}
			role="slider"
			aria-label="{label} per day, {formatDate(points[0].date)} to {formatDate(points[points.length - 1].date)}; arrow keys move between days"
			aria-valuemin={0}
			aria-valuemax={points.length - 1}
			aria-valuenow={hover ?? points.length - 1}
			aria-valuetext={valueText}
			tabindex="0"
			onpointermove={onMove}
			onpointerleave={() => (hover = null)}
			onkeydown={onKey}
			onblur={() => (hover = null)}
		>
			{#each ticks as t (t)}
				<line class="grid" x1={pad.left} x2={width - pad.right} y1={y(t)} y2={y(t)} />
				<text class="axis" x={pad.left - 8} y={y(t)} dy="0.32em" text-anchor="end">
					{formatNumber(t, digits > 0 && ticks.some((v) => !Number.isInteger(v)) ? 1 : 0)}
				</text>
			{/each}
			{#each monthTicks as m (m.date)}
				<text class="axis" x={x(m.i)} y={height - 6} text-anchor="middle">
					{new Date(m.date).toLocaleDateString('en-GB', { month: 'short', timeZone: 'UTC' })}
				</text>
			{/each}
			{#each releaseMarks as m (m.date)}
				<line class="release" x1={x(m.i)} x2={x(m.i)} y1={pad.top - 6} y2={height - pad.bottom} />
				<text class="release-label" x={x(m.i) + 4} y={pad.top - 10}>{m.label}</text>
			{/each}
			<path class="line" d={path} />
			{#if hovered && hovered.value !== null && hover !== null}
				<line class="cross" x1={x(hover)} x2={x(hover)} y1={pad.top} y2={height - pad.bottom} />
				<circle class="dot" cx={x(hover)} cy={y(hovered.value)} r="4" />
			{/if}
		</svg>
		{#if hovered}
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
	.chart {
		position: relative;
		width: 100%;
	}
	svg {
		display: block;
		touch-action: pan-y;
	}
	svg:focus-visible {
		outline: 2px solid var(--amber);
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
