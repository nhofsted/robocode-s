<script lang="ts">
	// A robot drawn with classic Robocode's own sprites, recoloured like classic (sprites.ts).
	// Headings in degrees, Robocode style: 0 is up, clockwise; gun and radar headings are absolute.
	// `size` is the CSS size of a square frame that fits the tank with its gun in any direction.
	import { drawTank, loadSprites, type TankColors } from './sprites.ts';

	interface Props {
		colors: TankColors;
		size?: number;
		heading?: number;
		gunHeading?: number;
		radarHeading?: number;
		label?: string;
	}

	let { colors, size = 32, heading = 0, gunHeading, radarHeading, label }: Props = $props();

	/** Frame in sprite pixels: the turret image is 54 px long, centred on the tank. */
	const FRAME = 56;
	const RAD = Math.PI / 180;

	let canvas: HTMLCanvasElement;

	$effect(() => {
		const gun = gunHeading ?? heading;
		const radar = radarHeading ?? gun;
		const px = Math.round(size * Math.min(3, devicePixelRatio || 1));
		const c = colors;
		const h = heading;
		let cancelled = false;
		loadSprites().then((sprites) => {
			if (cancelled) return;
			canvas.width = px;
			canvas.height = px;
			const ctx = canvas.getContext('2d')!;
			ctx.clearRect(0, 0, px, px);
			ctx.imageSmoothingQuality = 'high';
			ctx.scale(px / FRAME, px / FRAME);
			drawTank(ctx, sprites, c, FRAME / 2, FRAME / 2, h * RAD, gun * RAD, radar * RAD);
		});
		return () => (cancelled = true);
	});
</script>

<canvas
	bind:this={canvas}
	style:width="{size}px"
	style:height="{size}px"
	role={label ? 'img' : undefined}
	aria-label={label}
	aria-hidden={label ? undefined : 'true'}
></canvas>

<style>
	canvas {
		display: inline-block;
		vertical-align: middle;
		flex: none;
	}
</style>
