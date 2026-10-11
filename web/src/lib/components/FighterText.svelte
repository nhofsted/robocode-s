<script lang="ts">
	// Letters in the style of the Street Fighter II logo: heavy italic, a pale-yellow-to-red
	// gradient, a dark red rim and black outline, horizontal flame streaks cut through the lower
	// half, and tapered flame tails trailing to the left. Used for the logo's "S" and "VS".
	interface Props {
		text: string;
		/** Height of the drawing in CSS units. */
		height?: string;
		label?: string;
	}
	let { text, height = '1.6em', label }: Props = $props();

	const id = $props.id();

	/** Advance per letter in viewBox units (Archivo Black at font-size 112). */
	const ADVANCE = 80;
	const LEFT = 34;
	const viewWidth = $derived(LEFT + text.length * ADVANCE + 14);

	/** Flame streaks: vertical centre and half-thickness of each cut, in viewBox units. */
	const STREAKS = [
		{ y: 70, h: 3.8 },
		{ y: 83, h: 4.2 },
		{ y: 96, h: 3.8 }
	];
	/** Flame tails between and below the streaks: top, bottom, how far they reach left. */
	const TAILS = [
		{ top: 60, bottom: 67, reach: 40 },
		{ top: 73, bottom: 80.5, reach: 58 },
		{ top: 86, bottom: 93.5, reach: 48 },
		{ top: 99, bottom: 106, reach: 30 }
	];
	/** Where the tails start: inside the first letter, which covers their roots. */
	const tailX = LEFT + 26;
</script>

<svg
	viewBox="0 0 {viewWidth} 118"
	style:height
	role={label ? 'img' : undefined}
	aria-label={label}
	aria-hidden={label ? undefined : 'true'}
>
	<defs>
		<linearGradient id="{id}-fill" x1="0" y1="20" x2="0" y2="106" gradientUnits="userSpaceOnUse">
			<stop offset="0" stop-color="#fffbe2" />
			<stop offset="0.16" stop-color="#ffe95e" />
			<stop offset="0.4" stop-color="#ffb21c" />
			<stop offset="0.62" stop-color="#ff6b12" />
			<stop offset="0.84" stop-color="#e2260f" />
			<stop offset="1" stop-color="#a20d0a" />
		</linearGradient>
		<linearGradient id="{id}-tail" x1="1" y1="0" x2="0" y2="0">
			<stop offset="0" stop-color="#ffd23a" />
			<stop offset="0.35" stop-color="#ff7a12" />
			<stop offset="1" stop-color="#c3150c" />
		</linearGradient>
		<mask id="{id}-cuts" maskUnits="userSpaceOnUse" x="-50" y="0" width={viewWidth + 100} height="130">
			<rect x="-50" y="0" width={viewWidth + 100} height="130" fill="#fff" />
			{#each STREAKS as s (s.y)}
				<!-- A wedge: full thickness at the left, thinning out to the right. -->
				<polygon
					points="{-50},{s.y - s.h} {LEFT + text.length * ADVANCE * 0.8},{s.y} {-50},{s.y + s.h}"
					fill="#000"
				/>
			{/each}
		</mask>
	</defs>
	<g transform="skewX(-14) translate(24 0)">
		{#each TAILS as t (t.top)}
			<polygon
				points="{tailX},{t.top} {tailX - t.reach},{(t.top + t.bottom) / 2 + 1} {tailX},{t.bottom}"
				fill="url(#{id}-tail)"
				stroke="#1a0400"
				stroke-width="2.5"
				stroke-linejoin="round"
			/>
		{/each}
		<!-- Black outline; the streaks cut through rim and fill, so they show black. -->
		<text class="t" x={LEFT} y="104" stroke="#140300" stroke-width="15" fill="#140300">{text}</text>
		<g mask="url(#{id}-cuts)">
			<text class="t" x={LEFT} y="104" stroke="#8a1607" stroke-width="6" fill="#8a1607">{text}</text>
			<text class="t" x={LEFT} y="104" fill="url(#{id}-fill)">{text}</text>
		</g>
	</g>
</svg>

<style>
	svg {
		display: inline-block;
		vertical-align: middle;
		overflow: visible;
	}
	.t {
		font-family: 'Archivo Black', Helvetica, Arial, sans-serif;
		font-size: 112px;
		letter-spacing: -4px;
		stroke-linejoin: round;
		paint-order: stroke fill;
	}
</style>
