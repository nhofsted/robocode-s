// Classic Robocode's logo as SVG path data: a port of net.sf.robocode.ui.gfx.RobocodeLogo
// (classic 1.11.1, EPL; Flemming N. Larsen). The letters are built from the same pieces as in
// the Java code (a stem, "bows" for the round parts), so the coordinates match the original.
// The "S" of robocode-s is new, drawn in the same vocabulary.

type Point = [number, number];
type Segment = { op: 'M' | 'L'; p: Point } | { op: 'C'; p: [Point, Point, Point] } | { op: 'Z' };
type Path = Segment[];
type Transform = (p: Point) => Point;

const translate =
	(dx: number, dy: number): Transform =>
	([x, y]) => [x + dx, y + dy];
const scale =
	(sx: number, sy: number): Transform =>
	([x, y]) => [x * sx, y * sy];

function transform(path: Path, ...fs: Transform[]): Path {
	const t = (p: Point) => fs.reduce((q, f) => f(q), p);
	return path.map((s) => {
		if (s.op === 'Z') return s;
		if (s.op === 'C') return { op: 'C', p: [t(s.p[0]), t(s.p[1]), t(s.p[2])] };
		return { op: s.op, p: t(s.p) };
	});
}

function toD(path: Path): string {
	const n = (v: number) => +v.toFixed(2);
	return path
		.map((s) => {
			if (s.op === 'Z') return 'Z';
			if (s.op === 'C') return `C${s.p.map(([x, y]) => `${n(x)} ${n(y)}`).join(' ')}`;
			return `${s.op}${n(s.p[0])} ${n(s.p[1])}`;
		})
		.join('');
}

const M = (x: number, y: number): Segment => ({ op: 'M', p: [x, y] });
const L = (x: number, y: number): Segment => ({ op: 'L', p: [x, y] });
const C = (a: number, b: number, c: number, d: number, e: number, f: number): Segment => ({
	op: 'C',
	p: [
		[a, b],
		[c, d],
		[e, f]
	]
});
const Z: Segment = { op: 'Z' };

/** I_SHAPE: Rectangle2D(0, 0, 13, 46). */
const stem = (): Path => [M(0, 0), L(13, 0), L(13, 46), L(0, 46), Z];

function pBow(): Path {
	return [M(0, 0), L(10, 0), C(30, 0, 30, 26, 10, 26), L(0, 26), L(0, 17), L(8, 17), C(14, 18, 14, 9, 8, 9), L(0, 9), L(0, 0), Z];
}

function oBow(): Path {
	return [M(0, 0), C(23, 0, 23, 31, 0, 31), L(0, 20), C(8, 20, 8, 11, 0, 11), L(0, 0), Z];
}

function cBow(): Path {
	return [M(31, 12), C(29, -3.5, 2, -5.5, 0, 14.5), L(11, 14.5), C(11, 8.5, 18, 9, 18, 12), L(31, 12), Z];
}

function letterR(): Path {
	return [
		...stem(),
		...transform(pBow(), translate(15, 0)),
		M(21, 29),
		L(31, 46),
		L(44.5, 46),
		L(33.5, 27),
		C(33.5, 27, 31, 29, 21, 29),
		Z
	];
}

function letterO(): Path {
	return [...transform(oBow(), translate(20, 0)), ...transform(oBow(), scale(-1, 1), translate(18, 0))];
}

function letterB(): Path {
	return [...stem(), ...transform(pBow(), translate(15, 20))];
}

function letterC(): Path {
	return [...cBow(), ...transform(cBow(), scale(1, -1), translate(0, 31))];
}

function letterD(): Path {
	return [...transform(stem(), translate(27, 0)), ...transform(pBow(), scale(-1, 1), translate(25, 20))];
}

function letterE(): Path {
	return [
		M(0, 14.5),
		L(31, 14.5),
		C(31, -4.5, 0, -4.5, 0, 14.5),
		Z,
		M(12, 11),
		L(20, 11),
		C(20, 8, 12, 8, 12, 11),
		Z,
		...transform(cBow(), scale(1, -1), translate(0, 31))
	];
}

/**
 * The robocode-s "S", in the logo's vocabulary: as tall as R, b and d (46), two stacked bows
 * like pBow sharing the middle bar, 9-unit bars and 10.5-unit curves.
 */
function letterS(): Path {
	return [
		M(40, 0),
		L(15, 0),
		C(-5, 0, -5, 27, 15, 27),
		L(25, 27),
		C(31, 27, 31, 37, 25, 37),
		L(0, 37),
		L(0, 46),
		L(25, 46),
		C(45, 46, 45, 18, 25, 18),
		L(15, 18),
		C(9, 18, 9, 9, 15, 9),
		L(40, 9),
		Z
	];
}

/** "Robocode" as in RobocodeLogo.getRobocodeText(): 321 wide, 46 high. */
export const ROBOCODE_D = toD([
	...letterR(),
	...transform(letterO(), translate(42, 16)),
	...transform(letterB(), translate(84, 0)),
	...transform(letterO(), translate(127, 16)),
	...transform(letterC(), translate(170, 16)),
	...transform(letterO(), translate(204, 16)),
	...transform(letterD(), translate(246, 0)),
	...transform(letterE(), translate(290, 16))
]);
export const ROBOCODE_WIDTH = 321;

/** The S on its own, 40 wide and 46 high. */
export const S_D = toD(letterS());
export const S_WIDTH = 40;

/** Where the S follows "Robocode", with the letters' usual spacing. */
export const S_OFFSET = 328;

/**
 * Placement of the flame S (FighterText) after the word, in units of the letters' height:
 * horizontal offset from the end of the word, vertical offset from centred, and its height.
 */
export const FLAME_S = { x: -0.681, y: -0.05, size: 1.6 };
