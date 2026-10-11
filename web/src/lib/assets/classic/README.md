# Classic Robocode images

`body.png`, `turret.png`, `radar.png`, `explosion/explosion1-*.png` and `ground/blue_metal_*.png` are the robot, bullet-explosion and battlefield-floor images of classic Robocode 1.11.1 (`robocode.ui`, `net/sf/robocode/ui/images/`), unmodified.

Copyright (c) 2001-2025 Mathew A. Nelson and Robocode contributors. Originally made available under the Eclipse Public License v1.0; distributed here under the Eclipse Public License v2.0 as permitted by section 7 of EPL-1.0, like the bundled sample robots (ADR 0008).

The robot images are recoloured in the browser with classic's own algorithm (`ImageUtil.ColorFilter`), ported in `../../components/sprites.ts`; the floor tiles are laid out as in classic's `BattleView` (`../../components/Ground.svelte`).

`static/favicon-32.png` and `favicon-256.png` are classic's application icon (`robocode.content/src/main/resources/robocode.ico`, also the favicon of robocode.sourceforge.io) at those sizes, with the black background made transparent: `node tools/favicon.mjs <robocode.ico> static 32 256`, then rename `favicon-*.png`.
