<script lang="ts">
	// Decorative battlefield for the home page: a few tanks wander, sweep their radars and fire.
	// Not the engine (that arrives as WebAssembly for replays, design §12.8), just a backdrop.
	// One still frame with prefers-reduced-motion; paused while off-screen.

	interface Props {
		tanks?: number;
	}
	let { tanks = 4 }: Props = $props();

	let canvas: HTMLCanvasElement;

	const COLORS = [
		['#3f8a4f', '#9fd4a8', '#ffd75e'],
		['#8a3f3f', '#d9a0a0', '#5ee3d6'],
		['#3f5f8a', '#a4bde0', '#ffb35e'],
		['#7a6a35', '#d8cc98', '#ff7af0'],
		['#5a3f8a', '#c0a8e6', '#9cff5e']
	];

	interface Bot {
		x: number;
		y: number;
		heading: number;
		gun: number;
		radar: number;
		speed: number;
		turn: number;
		heat: number;
		energy: number;
		colors: string[];
		target: number;
	}
	interface Bullet {
		x: number;
		y: number;
		dx: number;
		dy: number;
		owner: number;
		power: number;
	}
	interface Blast {
		x: number;
		y: number;
		t: number;
		size: number;
	}

	$effect(() => {
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
		let width = 0;
		let height = 0;
		let frame = 0;
		let visible = true;
		const bots: Bot[] = [];
		const bullets: Bullet[] = [];
		const blasts: Blast[] = [];
		let seed = 7;
		const rand = () => {
			seed = (seed * 16807) % 2147483647;
			return seed / 2147483647;
		};

		const resize = () => {
			const dpr = Math.min(2, devicePixelRatio || 1);
			width = canvas.clientWidth;
			height = canvas.clientHeight;
			canvas.width = Math.round(width * dpr);
			canvas.height = Math.round(height * dpr);
			ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		};
		resize();

		for (let i = 0; i < tanks; i++) {
			bots.push({
				x: 60 + rand() * Math.max(1, width - 120),
				y: 60 + rand() * Math.max(1, height - 120),
				heading: rand() * Math.PI * 2,
				gun: rand() * Math.PI * 2,
				radar: rand() * Math.PI * 2,
				speed: 0,
				turn: 0,
				heat: 1 + rand() * 2,
				energy: 100,
				colors: COLORS[i % COLORS.length],
				target: (i + 1) % tanks
			});
		}

		const angleTo = (a: Bot, b: { x: number; y: number }) => Math.atan2(b.x - a.x, a.y - b.y);
		const norm = (a: number) => Math.atan2(Math.sin(a), Math.cos(a));

		const step = (dt: number) => {
			for (const [i, b] of bots.entries()) {
				if (rand() < 0.02) b.turn = (rand() - 0.5) * 0.08;
				if (rand() < 0.02) b.speed = (rand() < 0.5 ? -1 : 1) * (2 + rand() * 4);
				// Steer away from walls.
				const margin = 50;
				if (b.x < margin || b.x > width - margin || b.y < margin || b.y > height - margin) {
					const toCenter = angleTo(b, { x: width / 2, y: height / 2 });
					b.turn = Math.sign(norm(toCenter - b.heading)) * 0.08;
					b.speed = Math.abs(b.speed) || 3;
				}
				b.heading += b.turn * dt;
				b.x = Math.min(width - 20, Math.max(20, b.x + Math.sin(b.heading) * b.speed * dt));
				b.y = Math.min(height - 20, Math.max(20, b.y - Math.cos(b.heading) * b.speed * dt));

				const target = bots[b.target];
				const want = angleTo(b, target);
				b.gun += Math.max(-0.35, Math.min(0.35, norm(want - b.gun))) * 0.3 * dt;
				b.radar += 0.3 * dt;
				b.heat -= 0.1 * dt;
				if (b.heat <= 0 && Math.abs(norm(want - b.gun)) < 0.15) {
					const power = 1 + rand() * 2;
					const v = 20 - 3 * power;
					bullets.push({
						x: b.x + Math.sin(b.gun) * 20,
						y: b.y - Math.cos(b.gun) * 20,
						dx: Math.sin(b.gun) * v * 0.5,
						dy: -Math.cos(b.gun) * v * 0.5,
						owner: i,
						power
					});
					b.heat = 1 + power / 5 + rand() * 3;
				}
				if (rand() < 0.003) b.target = (b.target + 1 + Math.floor(rand() * (tanks - 1))) % tanks;
				if (b.target === i) b.target = (i + 1) % tanks;
			}
			for (let k = bullets.length - 1; k >= 0; k--) {
				const u = bullets[k];
				u.x += u.dx * dt;
				u.y += u.dy * dt;
				const hit = bots.findIndex(
					(b, i) => i !== u.owner && Math.abs(b.x - u.x) < 18 && Math.abs(b.y - u.y) < 18
				);
				if (hit >= 0 || u.x < 0 || u.y < 0 || u.x > width || u.y > height) {
					if (hit >= 0) blasts.push({ x: u.x, y: u.y, t: 0, size: 8 + u.power * 4 });
					bullets.splice(k, 1);
				}
			}
			for (let k = blasts.length - 1; k >= 0; k--) {
				blasts[k].t += 0.06 * dt;
				if (blasts[k].t >= 1) blasts.splice(k, 1);
			}
		};

		const drawTank = (b: Bot) => {
			const [body, gun, radar] = b.colors;
			ctx.save();
			ctx.translate(b.x, b.y);
			// Radar beam.
			ctx.save();
			ctx.rotate(b.radar);
			const beam = ctx.createLinearGradient(0, 0, 0, -220);
			beam.addColorStop(0, 'rgba(94,227,138,0.18)');
			beam.addColorStop(1, 'rgba(94,227,138,0)');
			ctx.fillStyle = beam;
			ctx.beginPath();
			ctx.moveTo(0, 0);
			ctx.arc(0, 0, 220, -Math.PI / 2 - 0.12, -Math.PI / 2 + 0.12);
			ctx.closePath();
			ctx.fill();
			ctx.restore();

			ctx.rotate(b.heading);
			ctx.fillStyle = '#262a26';
			ctx.fillRect(-16, -17, 8, 34);
			ctx.fillRect(8, -17, 8, 34);
			ctx.fillStyle = '#3d433d';
			for (let y = -13; y <= 11; y += 6) {
				ctx.fillRect(-15, y, 6, 2.5);
				ctx.fillRect(9, y, 6, 2.5);
			}
			ctx.fillStyle = body;
			ctx.fillRect(-9, -13, 18, 26);
			ctx.rotate(b.gun - b.heading);
			ctx.fillStyle = gun;
			ctx.fillRect(-1.6, -21, 3.2, 16);
			ctx.beginPath();
			ctx.arc(0, 0, 6, 0, Math.PI * 2);
			ctx.fill();
			ctx.rotate(b.radar - b.gun);
			ctx.fillStyle = radar;
			ctx.fillRect(-5, -3, 10, 2.5);
			ctx.restore();
		};

		const draw = () => {
			ctx.clearRect(0, 0, width, height);
			for (const b of bots) drawTank(b);
			ctx.fillStyle = '#ffc25c';
			for (const u of bullets) {
				ctx.beginPath();
				ctx.arc(u.x, u.y, 1.5 + u.power * 0.6, 0, Math.PI * 2);
				ctx.fill();
			}
			for (const e of blasts) {
				ctx.fillStyle = `rgba(255, ${Math.round(200 - 140 * e.t)}, 80, ${1 - e.t})`;
				ctx.beginPath();
				ctx.arc(e.x, e.y, e.size * (0.4 + e.t), 0, Math.PI * 2);
				ctx.fill();
			}
		};

		let last = performance.now();
		const loop = (now: number) => {
			const dt = Math.min(3, (now - last) / 16.67);
			last = now;
			if (visible && !document.hidden) {
				step(dt);
				draw();
			}
			frame = requestAnimationFrame(loop);
		};

		if (reduce) {
			for (let i = 0; i < 90; i++) step(1);
			draw();
		} else {
			frame = requestAnimationFrame(loop);
		}

		const observer = new IntersectionObserver(([entry]) => (visible = entry.isIntersecting));
		observer.observe(canvas);
		const onResize = () => {
			resize();
			if (reduce) draw();
		};
		addEventListener('resize', onResize);
		return () => {
			cancelAnimationFrame(frame);
			observer.disconnect();
			removeEventListener('resize', onResize);
		};
	});
</script>

<canvas bind:this={canvas} aria-hidden="true"></canvas>

<style>
	canvas {
		display: block;
		width: 100%;
		height: 100%;
	}
</style>
