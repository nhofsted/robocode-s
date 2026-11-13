<script lang="ts">
	// A bot version's LLM declaration (`robot.llm`) after its author's name: the declared
	// model's vendor logo (a robot for a model the site has no logo for) on a small tile,
	// outlined for `assisted`, solid for `generated`. The logo itself is drawn as its vendor
	// draws it: stroking line-art marks would make them heavier, not lighter. The hover text
	// always says which. `none` shows nothing.
	import type { Llm } from '#lib/rumble/types.ts';
	import { LOGOS } from '#lib/llm/logos.ts';
	import { llmLabel, vendorIcons } from '#lib/llm/vendors.ts';

	interface Props {
		llm: Llm | null | undefined;
		/** The tile's size in pixels. */
		size?: number;
	}
	let { llm, size = 16 }: Props = $props();

	const label = $derived(llm ? llmLabel(llm) : '');
	const icons = $derived(llm && llm.use !== 'none' ? vendorIcons(llm.model) : []);
	const inner = $derived(Math.round(size * 0.7));
</script>

{#if icons.length}
	<span class="llm" class:generated={llm?.use === 'generated'} title={label}>
		{#each icons as vendor (vendor ?? '')}
			<span class="tile" style:width="{size}px" style:height="{size}px">
				{#if vendor}
					<svg viewBox="0 0 24 24" width={inner} height={inner} fill-rule="evenodd" aria-hidden="true">
						{#each LOGOS[vendor].paths as p (p.d)}<path d={p.d} opacity={p.opacity} />{/each}
					</svg>
				{:else}
					<span class="robot" style:font-size="{inner}px" aria-hidden="true">🤖</span>
				{/if}
			</span>
		{/each}
		<span class="visually-hidden">{label}</span>
	</span>
{/if}

<style>
	.llm {
		display: inline-flex;
		gap: 2px;
		align-items: center;
		vertical-align: -3px;
		flex: none;
		color: var(--text-2);
		cursor: help;
	}
	.tile {
		display: inline-grid;
		place-items: center;
		box-sizing: border-box;
		border: 1px solid currentColor;
		border-radius: 4px;
	}
	.generated .tile {
		background: currentColor;
	}
	svg {
		display: block;
		fill: currentColor;
	}
	.generated svg {
		fill: var(--panel);
	}
	.robot {
		line-height: 1;
	}
</style>
