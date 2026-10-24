#!/usr/bin/env bash
# Builds the replay-track decoder (crates/replay-wasm) for the browser into src/lib/replay/wasm/
# (gitignored). Run by the package.json scripts that dev, build and test the site.
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install wasm-bindgen-cli --version $VERSION --locked
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(cd .. && pwd)

# The version crates/replay-wasm pins: wasm-bindgen's glue and the CLI have to agree.
VERSION=$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' "$root/crates/replay-wasm/Cargo.toml")

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "build-wasm: the wasm32 target is missing: rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi
if [ "$(wasm-bindgen --version 2>/dev/null | cut -d' ' -f2)" != "$VERSION" ]; then
  echo "build-wasm: wasm-bindgen-cli $VERSION is missing: cargo install wasm-bindgen-cli --version $VERSION --locked" >&2
  exit 1
fi

out=src/lib/replay/wasm
cd "$root"
cargo build -p robocode-s-replay-wasm --target wasm32-unknown-unknown --release --quiet
cd web
rm -rf "$out"
wasm-bindgen --target web --out-dir "$out" \
  "$root/target/wasm32-unknown-unknown/release/robocode_s_replay_wasm.wasm"
echo "build-wasm: $(wc -c < "$out/robocode_s_replay_wasm_bg.wasm") bytes -> web/$out"
