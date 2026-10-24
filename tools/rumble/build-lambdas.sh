#!/usr/bin/env bash
# SPDX-License-Identifier: EPL-2.0
#
# Builds the rumble Lambdas (docs/rumble-backend.md §2) for provided.al2023 on arm64 with
# cargo-zigbuild, and copies each binary to rumble/lambda/target/lambda/<name>/bootstrap,
# where CDK's Code.fromAsset picks it up.
#
#   tools/rumble/build-lambdas.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
lambda="$root/rumble/lambda"
target=aarch64-unknown-linux-gnu
glibc=2.34

if ! cargo zigbuild --help >/dev/null 2>&1; then
  echo "cargo-zigbuild is missing: cargo install cargo-zigbuild (and zig on PATH)" >&2
  exit 1
fi
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

cd "$lambda"
cargo zigbuild --release --target "$target.$glibc" --bins

# Every [[bin]] of the crate: api and batch.
bins=$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; m=json.load(sys.stdin); print(" ".join(t["name"] for p in m["packages"] for t in p["targets"] if "bin" in t["kind"]))')
for name in $bins; do
  out="$lambda/target/lambda/$name"
  mkdir -p "$out"
  # install sets the mode (a plain cp kept a non-executable mode on WSL once).
  install -m 0755 "$lambda/target/$target/release/$name" "$out/bootstrap"
  echo "$out/bootstrap ($(du -h "$out/bootstrap" | cut -f1))"
done
