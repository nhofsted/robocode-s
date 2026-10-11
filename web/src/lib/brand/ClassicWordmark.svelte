<script lang="ts">
	// "Robocode" in classic's logo letters (classic-logo.ts), optionally followed by the S.
	import FighterText from '#lib/components/FighterText.svelte';
	import { ROBOCODE_D, ROBOCODE_WIDTH, S_D, S_OFFSET, S_WIDTH } from './classic-logo.ts';

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
	}
	let { height = 32, look = 'glow', s = 'inline', sColor = '#ffb31f' }: Props = $props();

	const id = $props.id();
	const PAD = 3;
	const width = $derived(
		s === 'inline' ? S_OFFSET + S_WIDTH : s === 'super' ? S_OFFSET + S_WIDTH * 0.62 : ROBOCODE_WIDTH
	);
</script>

<span class="mark" role="img" aria-label="robocode-s" style:--h="{height}px">

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
	<span class="fighter"><FighterText text="S" height="{height * 1.75}px" /></span>
{/if}
</span>

<style>
	.mark {
		display: inline-flex;
		align-items: center;
		white-space: nowrap;
	}
	/* The flame tails run under the end of the word. */
	.fighter {
		display: flex;
		margin-left: calc(var(--h) * -0.62);
		position: relative;
		top: calc(var(--h) * 0.04);
	}
	svg {
		display: inline-block;
		vertical-align: middle;
		overflow: visible;
	}
</style>
