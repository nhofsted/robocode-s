<script lang="ts">
	// "Robocode" in classic's logo letters (classic-logo.ts), optionally followed by the S.
	import FighterText from '#lib/components/FighterText.svelte';
	import { FLAME_S, ROBOCODE_D, ROBOCODE_WIDTH, S_D, S_OFFSET, S_WIDTH } from './classic-logo.ts';

	interface Props {
		height?: number;
		/** Classic's translucent green with a glow, or solid fills (crisper when small). */
		look?: 'glow' | 'solid';
		/**
		 * The S: in the letters' style after the word, raised like an exponent, the flame S of
		 * FighterText, or left out.
		 */
		s?: 'inline' | 'super' | 'fighter' | 'none';
		sColor?: string;
		/** Placement of the flame S, in letter heights (defaults: FLAME_S). */
		flame?: { x: number; y: number; size: number };
	}
	let { height = 32, look = 'glow', s = 'inline', sColor = '#ffb31f', flame = FLAME_S }: Props = $props();

	const id = $props.id();
	const PAD = 3;
	const width = $derived(
		s === 'inline' ? S_OFFSET + S_WIDTH : s === 'super' ? S_OFFSET + S_WIDTH * 0.62 : ROBOCODE_WIDTH
	);
</script>

<span class="mark" role="img" aria-label="robocode-s">

<svg
	viewBox="{-PAD} {-PAD} {width + 2 * PAD} {46 + 2 * PAD}"
	style:height="{height}px"
	aria-hidden="true"
>
	{#if look === 'glow'}
		<defs>
			<filter id="{id}-glow" x="-10%" y="-30%" width="120%" height="160%">
				<feGaussianBlur stdDeviation="2.2" result="b" />
				<feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge>
			</filter>
		</defs>
	{/if}
	<g filter={look === 'glow' ? `url(#${id}-glow)` : undefined}>
		<path
			d={ROBOCODE_D}
			fill={look === 'glow' ? 'rgb(0 255 0 / 0.3)' : '#3fd35a'}
			stroke={look === 'glow' ? 'rgb(0 255 0 / 0.85)' : 'none'}
			stroke-width="1.5"
		/>
		{#if s === 'inline' || s === 'super'}
			<path
				d={S_D}
				transform={s === 'inline' ? `translate(${S_OFFSET} 0)` : `translate(${S_OFFSET} -2) scale(0.62)`}
				fill={look === 'glow' ? `color-mix(in srgb, ${sColor} 45%, transparent)` : sColor}
				stroke={look === 'glow' ? sColor : 'none'}
				stroke-width={s === 'super' ? 2.4 : 1.5}
			/>
		{/if}
	</g>
</svg>
{#if s === 'fighter'}
	<span
		class="fighter"
		style:margin-left="{height * flame.x}px"
		style:top="{height * flame.y}px"
	>
		<FighterText text="S" height="{height * flame.size}px" />
	</span>
{/if}
</span>

<style>
	.mark {
		display: inline-flex;
		align-items: center;
		white-space: nowrap;
	}
	/* The S sits behind the end of the word: "Robocode" overlaps it a little. */
	.fighter {
		display: flex;
		position: relative;
		z-index: 0;
	}
	svg {
		position: relative;
		z-index: 1;
	}
	svg {
		display: inline-block;
		vertical-align: middle;
		overflow: visible;
	}
</style>
