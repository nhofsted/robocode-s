# The emoji font

`NotoEmoji-Regular.ttf` is Noto Emoji 3.002, the monochrome emoji font (`src/fonts.rs`): it takes the place of the older Noto Emoji (2017) that egui ships, for the emoji robots print. It is the Regular weight (`wght=400`) cut from the variable font `NotoEmoji[wght].ttf` of Google Fonts (https://github.com/google/fonts/tree/main/ofl/notoemoji, sha256 `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551`), which halves its size:

    uvx --from fonttools fonttools varLib.instancer -o NotoEmoji-Regular.ttf 'NotoEmoji[wght].ttf' wght=400

Copyright 2013 Google LLC. SIL Open Font License 1.1 (`OFL.txt`); listed in the release's third-party notices (`tools/release/notices-header.txt`, `notices.py`). Source: https://github.com/googlefonts/emoji-bw
