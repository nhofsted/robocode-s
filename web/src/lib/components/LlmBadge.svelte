<script lang="ts">
	// A bot version's LLM declaration (`robot.llm`) after its author's name: the declared
	// model's vendor logo, in the vendor's colours where it has them (a robot for a model the
	// site has no logo for). Assisted and generated look the same; the hover text says which.
	// `none` shows nothing.
	import type { Llm } from '#lib/rumble/types.ts';
	import { LOGOS, type Vendor } from '#lib/llm/logos.ts';
	import { llmLabel, vendorIcons } from '#lib/llm/vendors.ts';

	interface Props {
		llm: Llm | null | undefined;
		/** The space each logo takes, in pixels. */
		size?: number;
	}
	let { llm, size = 16 }: Props = $props();

	// The logos' gradient ids, made unique to this badge: a page shows many badges, and an id
	// that resolves to another badge's copy breaks when that copy is hidden.
	const uid = $props.id();

	const label = $derived(llm ? llmLabel(llm) : '');
	const icons = $derived(llm && llm.use !== 'none' ? vendorIcons(llm.model) : []);
	const inner = $derived(Math.round(size * 0.85));

	function body(vendor: Vendor): string {
		return LOGOS[vendor].body.replaceAll(`llm-${vendor}-`, `llm-${uid}-${vendor}-`);
	}
</script>

{#if icons.length}
	<span class="llm" title={label}>
		{#each icons as vendor (vendor ?? '')}
			<span class="slot" style:width="{size}px" style:height="{size}px">
				{#if vendor}
					<svg
						viewBox="0 0 24 24"
						width={inner}
						height={inner}
						fill="currentColor"
						fill-rule={LOGOS[vendor].evenodd ? 'evenodd' : undefined}
						aria-hidden="true"
					>
						<!-- Generated from LobeHub's SVGs (logos.ts), not user input. -->
						{@html body(vendor)}
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
		color: var(--text);
	}
	.slot {
		display: inline-grid;
		place-items: center;
	}
	svg {
		display: block;
	}
	.robot {
		line-height: 1;
	}
</style>
