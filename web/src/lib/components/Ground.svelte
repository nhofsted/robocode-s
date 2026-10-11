<script lang="ts">
	// Classic's battlefield floor as the page background: a mosaic of its five blue-metal tiles,
	// one picked per 64 px cell as in classic's BattleView (round(nextDouble() * 4)), but from a
	// fixed seed so the floor looks the same on every visit. Sets --ground for app.css.
	import { loadImage } from './sprites.ts';

	const TILES = Object.values(
		import.meta.glob<string>('../assets/classic/ground/blue_metal_*.png', {
			eager: true,
			query: '?url',
			import: 'default'
		})
	).sort();

	const CELLS = 8;
	const SIZE = 64;

	$effect(() => {
		let url: string | undefined;
		let cancelled = false;
		Promise.all(TILES.map(loadImage)).then((tiles) => {
			if (cancelled) return;
			const canvas = document.createElement('canvas');
			canvas.width = canvas.height = CELLS * SIZE;
			const ctx = canvas.getContext('2d')!;
			let seed = 20010101;
			const next = () => {
				seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
				return seed / 4294967296;
			};
			for (let y = 0; y < CELLS; y++) {
				for (let x = 0; x < CELLS; x++) {
					ctx.drawImage(tiles[Math.round(next() * 4)], x * SIZE, y * SIZE);
				}
			}
			canvas.toBlob((blob) => {
				if (!blob || cancelled) return;
				url = URL.createObjectURL(blob);
				document.documentElement.style.setProperty('--ground', `url(${url})`);
			});
		});
		return () => {
			cancelled = true;
			if (url) URL.revokeObjectURL(url);
		};
	});
</script>
