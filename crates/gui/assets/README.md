# The app's icon

`robocode-s.ico` is the Windows program's icon (`../build.rs`): classic Robocode's application icon (`robocode.content/src/main/resources/robocode.ico`, also the website's favicon, see `web/src/lib/assets/classic/README.md`) with the black background made transparent, at 16, 24, 32, 48, 64, 128 and 256 pixels (24 is the 48 halved; the others are classic's own). Its 256 is `web/static/favicon-256.png`, which is also the window's icon at run time and the one for `../robocode-s.desktop`. Made with `node crates/gui/tools/app-icon.mjs <robocode.ico> crates/gui/assets`.

Copyright (c) 2001-2025 Mathew A. Nelson and Robocode contributors. Originally made available under the Eclipse Public License v1.0; distributed here under the Eclipse Public License v2.0 as permitted by section 7 of EPL-1.0 (ADR 0008).
