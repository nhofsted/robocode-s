<script lang="ts">
	// An author's country flag, as on LiteRumble. SVGs from flag-icons (MIT); each flag is its
	// own small file, fetched only when shown. Emoji flags would show as letters on Windows.
	interface Props {
		/** ISO 3166-1 alpha-2, lower case. */
		code: string | null | undefined;
		height?: number;
	}
	let { code, height = 12 }: Props = $props();

	const FLAGS = import.meta.glob<string>('/node_modules/flag-icons/flags/4x3/*.svg', {
		query: '?url',
		import: 'default'
	});

	const names = new Intl.DisplayNames(['en'], { type: 'region' });

	let url: string | undefined = $state();
	const name = $derived.by(() => {
		if (!code) return '';
		try {
			return names.of(code.toUpperCase()) ?? code.toUpperCase();
		} catch {
			return code.toUpperCase();
		}
	});

	$effect(() => {
		url = undefined;
		const load = code ? FLAGS[`/node_modules/flag-icons/flags/4x3/${code.toLowerCase()}.svg`] : undefined;
		let cancelled = false;
		load?.().then((u) => {
			if (!cancelled) url = u;
		});
		return () => (cancelled = true);
	});
</script>

{#if code}
	<span class="flag" style:height="{height}px" style:width="{(height * 4) / 3}px" title={name}>
		{#if url}<img src={url} alt={name} />{:else}<span class="visually-hidden">{name}</span>{/if}
	</span>
{/if}

<style>
	.flag {
		display: inline-block;
		vertical-align: -1px;
		flex: none;
		border-radius: 2px;
		overflow: hidden;
		box-shadow: 0 0 0 1px rgb(0 0 0 / 0.35);
		background: var(--panel-2);
	}
	img {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
</style>
