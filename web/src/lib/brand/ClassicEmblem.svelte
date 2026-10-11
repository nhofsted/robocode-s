<script lang="ts">
	// Classic Robocode's logo (RobocodeLogo.paintLogoWithTanks): the radar ellipse with its
	// decorations and the "Robocode" letters, optionally with the S and classic's tanks and
	// explosions around it. 570 × 213, like the original.
	import { drawTank, loadImage, loadSprites } from '#lib/components/sprites.ts';
	import { ROBOCODE_D, S_D, S_OFFSET } from './classic-logo.ts';

	interface Props {
		width?: number;
		/** The S after "Robocode" inside the ellipse, as a seal on the ellipse's rim, or none. */
		s?: 'inline' | 'badge' | 'none';
		tanks?: boolean;
		sColor?: string;
	}
	let { width = 570, s = 'inline', tanks = true, sColor = '#ffb31f' }: Props = $props();

	const id = $props.id();
	const W = 570;
	const H = 213;

	// Ray cuts of the middle decoration: Rectangle2D(180, 69, 500, 3) rotated about (151, 72).
	const rays: number[] = [];
	for (let deg = 120; deg <= 335; deg += 4.8) rays.push(deg);

	// With the S inline the letters move left so the word stays inside the inner ellipse.
	const textX = $derived(s === 'inline' ? 101 : 121);

	const EXPLOSION = import.meta.glob<string>('../assets/classic/explosion/explosion1-{1,6,8,9}.png', {
		eager: true,
		query: '?url',
		import: 'default'
	});
	const expl = (n: number) => EXPLOSION[`../assets/classic/explosion/explosion1-${n}.png`];

	let canvas: HTMLCanvasElement | undefined = $state();

	$effect(() => {
		if (!tanks || !canvas) return;
		const c = canvas;
		const dpr = Math.min(3, devicePixelRatio || 1);
		const k = (width / W) * dpr;
		c.width = Math.round(W * k);
		c.height = Math.round(H * k);
		const ctx = c.getContext('2d')!;
		let cancelled = false;
		Promise.all([loadSprites(), loadImage(expl(9)), loadImage(expl(8)), loadImage(expl(1)), loadImage(expl(6))]).then(
			([sprites, e9, e8, e1, e6]) => {
				if (cancelled) return;
				ctx.setTransform(k, 0, 0, k, 0, 0);
				// drawTanks(): positions, angles and (dark) colours as in classic.
				const robot = (x: number, y: number, body: number, gun: number, rgb: [number, number, number]) =>
					drawTank(ctx, sprites, { body: rgb, gun: rgb, radar: rgb }, x, y, body, gun, gun);
				robot(22, 192, Math.PI / 2, -0.2, [0x30, 0x00, 0x10]);
				robot(22, 92, Math.PI / 2, Math.PI, [0x16, 0x00, 0x2c]);
				robot(212, 173, 0.75, 0.75, [0x02, 0x01, 0x00]);
				robot(455, 50, 2.4, 2, [0x02, 0x00, 0x01]);
				robot(492, 82, -0.3, -0.27, [0x00, 0x00, 0x01]);
				const blast = (img: HTMLImageElement, x: number, y: number, f: number) =>
					ctx.drawImage(img, x - 64 * f, y - 64 * f, 128 * f, 128 * f);
				// Classic uses explosion2-24 at (270, -25); explosion1-9 is the closest frame we ship.
				blast(e9, 334, 39, 0.9);
				blast(e8, 23, 102, 0.3);
				blast(e1, 464, 55, 0.5);
				blast(e6, 488, 72, 0.4);
				ctx.fillStyle = '#c0c0c0';
				ctx.beginPath();
				ctx.arc(21.5, 155.5, 1.5, 0, Math.PI * 2);
				ctx.fill();
			}
		);
		return () => (cancelled = true);
	});
</script>

<div class="emblem" style:width="{width}px" style:height="{(width * H) / W}px">
	{#if tanks}<canvas bind:this={canvas}></canvas>{/if}
	<svg viewBox="0 0 {W} {H}" role="img" aria-label="robocode-s">
		<defs>
			<clipPath id="{id}-quadrants">
				<rect x="0" y="0" width="261.5" height="87" />
				<rect x="261.5" y="87" width="261.5" height="87" />
			</clipPath>
			<mask id="{id}-rays" maskUnits="userSpaceOnUse" x="-10" y="-10" width="440" height="160">
				<rect x="-10" y="-10" width="440" height="160" fill="#fff" />
				{#each rays as deg (deg)}
					<rect x="180" y="69" width="500" height="3" fill="#000" transform="rotate({deg} 151 72)" />
				{/each}
			</mask>
			<clipPath id="{id}-middle">
				<ellipse cx="210" cy="70" rx="210.8" ry="70.8" />
			</clipPath>
			<mask id="{id}-inner" maskUnits="userSpaceOnUse" x="-10" y="-10" width="390" height="140">
				<rect x="-10" y="-10" width="390" height="140" fill="#fff" />
				<rect x="-10" y="30" width="390" height="56" fill="#000" />
				<rect x="181" y="-10" width="7" height="140" fill="#000" />
			</mask>
			<mask id="{id}-sub" maskUnits="userSpaceOnUse" x="-10" y="-10" width="380" height="140">
				<rect x="184" y="88" width="200" height="40" fill="#fff" />
				<rect x="209" y="80" width="3" height="50" fill="#000" />
			</mask>
		</defs>

		<!-- drawOuterDecoration -->
		<g transform="translate(26 24)">
			<path
				d="M0 87a261.5 87 0 1 0 523 0a261.5 87 0 1 0 -523 0ZM16 87a245.5 82 0 1 0 491 0a245.5 82 0 1 0 -491 0Z"
				fill-rule="evenodd"
				fill="rgb(255 255 255 / 0.5)"
				clip-path="url(#{id}-quadrants)"
			/>
			<ellipse cx="261" cy="86.5" rx="245" ry="81.5" fill="none" stroke="rgb(255 255 255 / 0.5)" stroke-width="1.5" />
		</g>
		<!-- drawMiddleEllipse -->
		<ellipse cx="288" cy="111" rx="220" ry="73" fill="rgb(0 112 0 / 0.5)" stroke="rgb(0 255 0 / 0.25)" stroke-width="1.5" />
		<!-- drawMiddleDecoration -->
		<g transform="translate(77 41)" mask="url(#{id}-rays)">
			<path
				d="M0 70a210 70 0 1 0 420 0a210 70 0 1 0 -420 0ZM18 74a204 72 0 1 0 408 0a204 72 0 1 0 -408 0Z"
				fill-rule="evenodd"
				clip-path="url(#{id}-middle)"
				fill="rgb(0 255 0 / 0.125)"
				stroke="rgb(0 255 0 / 0.28)"
				stroke-width="1.5"
			/>
		</g>
		<!-- drawInnerDecoration -->
		<g transform="translate(103 52)" mask="url(#{id}-inner)">
			<ellipse cx="184" cy="60" rx="184" ry="60" fill="rgb(0 112 0 / 0.5)" stroke="rgb(10 255 10 / 0.4)" stroke-width="1.5" />
		</g>
		<!-- drawInnerSubDecoration -->
		<g transform="translate(110 54)" mask="url(#{id}-sub)">
			<ellipse cx="178" cy="57" rx="178" ry="57" fill="rgb(0 255 0 / 0.03)" stroke="rgb(0 255 0 / 0.28)" stroke-width="1.5" />
		</g>
		<!-- drawRobocodeText -->
		<g transform="translate({textX} 88)">
			<path d={ROBOCODE_D} fill="rgb(0 255 0 / 0.25)" stroke="rgb(0 255 0 / 0.5)" stroke-width="1.5" />
			{#if s === 'inline'}
				<path
					d={S_D}
					transform="translate({S_OFFSET} 0)"
					fill="color-mix(in srgb, {sColor} 45%, transparent)"
					stroke={sColor}
					stroke-width="1.5"
				/>
			{/if}
		</g>
		{#if s === 'badge'}
			<!-- A seal on the ellipse's right rim. -->
			<g transform="translate(486 128)">
				<circle r="31" fill="#1a0d00" stroke={sColor} stroke-width="3" />
				<circle r="25" fill="none" stroke="color-mix(in srgb, {sColor} 50%, transparent)" stroke-width="1.5" />
				<path d={S_D} transform="translate(-20 -23)" fill={sColor} />
			</g>
		{/if}
	</svg>
</div>

<style>
	.emblem {
		position: relative;
		display: inline-block;
		background: #000;
		border-radius: 4px;
	}
	canvas,
	svg {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
	}
</style>
