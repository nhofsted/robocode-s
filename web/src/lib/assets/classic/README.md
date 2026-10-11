# Classic Robocode images

`body.png`, `turret.png`, `radar.png`, `explosion/explosion1-*.png` and `ground/blue_metal_*.png` are the robot, bullet-explosion and battlefield-floor images of classic Robocode 1.11.1 (`robocode.ui`, `net/sf/robocode/ui/images/`), unmodified.

Copyright (c) 2001-2025 Mathew A. Nelson and Robocode contributors. Originally made available under the Eclipse Public License v1.0; distributed here under the Eclipse Public License v2.0 as permitted by section 7 of EPL-1.0, like the bundled sample robots (ADR 0008).

The robot images are recoloured in the browser with classic's own algorithm (`ImageUtil.ColorFilter`), ported in `../../components/sprites.ts`; the floor tiles are laid out as in classic's `BattleView` (`../../components/Ground.svelte`).

`static/favicon.png` is the logo tank (these sprites in the logo colours), rendered by the site itself.
