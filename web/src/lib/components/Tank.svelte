<script lang="ts">
	// A top-down tank in the classic sprite's proportions. Headings in degrees, Robocode style:
	// 0 is up (north), clockwise. Gun and radar headings are absolute, like getGunHeading().
	import type { TankColors } from './tank-colors.ts';

	interface Props {
		colors: TankColors;
		size?: number;
		heading?: number;
		gunHeading?: number;
		radarHeading?: number;
		label?: string;
	}

	let { colors, size = 32, heading = 0, gunHeading, radarHeading, label }: Props = $props();
	const gun = $derived(gunHeading ?? heading);
	const radar = $derived(radarHeading ?? gun);
</script>

<svg
	width={size}
	height={size}
	viewBox="-20 -20 40 40"
	role={label ? 'img' : undefined}
	aria-label={label}
	aria-hidden={label ? undefined : 'true'}
>
	<g transform="rotate({heading})">
		<rect x="-16" y="-17" width="8" height="34" rx="1.5" fill="#262a26" />
		<rect x="8" y="-17" width="8" height="34" rx="1.5" fill="#262a26" />
		{#each [-13, -7, -1, 5, 11] as y (y)}
			<rect x="-15" {y} width="6" height="2.5" fill="#3d433d" />
			<rect x="9" {y} width="6" height="2.5" fill="#3d433d" />
		{/each}
		<rect x="-9" y="-13" width="18" height="26" rx="2" fill={colors.body} />
	</g>
	<g transform="rotate({gun})">
		<rect x="-1.6" y="-21" width="3.2" height="16" fill={colors.gun} />
		<circle r="6" fill={colors.gun} stroke="#0008" stroke-width="0.8" />
	</g>
	<g transform="rotate({radar})">
		<path d="M -5 -1 Q 0 -6 5 -1 L 3 0.5 Q 0 -3 -3 0.5 Z" fill={colors.radar} />
	</g>
</svg>

<style>
	svg {
		display: inline-block;
		vertical-align: middle;
		flex: none;
	}
</style>
