#!/usr/bin/env bash
# SPDX-License-Identifier: EPL-2.0
#
# Local release script (there is no CI): checks, builds, archives, uploads.
#
#   tools/release/release.sh build [--version X] [--targets T,T] [--skip-checks]
#   tools/release/release.sh smoke ARCHIVE.tar.gz
#   tools/release/release.sh upload VERSION [--publish]
#
# build   runs the checks (cargo fmt, clippy -D warnings for Linux and Windows,
#         cargo test, gradle build), builds the release binaries of every target
#         this machine can build, and writes one archive per platform and
#         SHA256SUMS to target/release-dist/<version>/. A target it can't build is
#         skipped with the reason; the others are still made.
# smoke   unpacks a Linux archive into a temporary folder and fights a battle
#         with it from another folder.
# upload  creates the GitHub release vVERSION (a draft unless --publish) from
#         target/release-dist/<version>/ with `gh release create`. Nothing else
#         in this script publishes anything. The commit must be pushed.
#
# Targets (platform names in the archives):
#   linux-x86_64    x86_64-unknown-linux-gnu    cargo-zigbuild + zig: glibc 2.31 baseline
#                                               (else a plain cargo build: the glibc of this machine)
#   linux-aarch64   aarch64-unknown-linux-gnu   cargo-zigbuild + zig, glibc 2.31
#   windows-x86_64  x86_64-pc-windows-gnu       MinGW-w64 (x86_64-w64-mingw32-gcc)
#   macos-aarch64   aarch64-apple-darwin        needs the macOS SDK: not buildable from Linux
#   macos-x86_64    x86_64-apple-darwin         (zig does not ship the frameworks); built on a Mac
#                                               with `cargo build`, or here if SDKROOT names an SDK
#
# Tools to install once:
#   rustup target add aarch64-unknown-linux-gnu x86_64-pc-windows-gnu
#   cargo install cargo-zigbuild --locked
#   zig 0.13 to 0.15 on the PATH (https://ziglang.org/download/; or `pip install ziglang`)
#   apt install mingw-w64
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
GLIBC=2.31
REPO=nhofsted/robocode-s
TARGETS_ALL="linux-x86_64 linux-aarch64 windows-x86_64 macos-aarch64 macos-x86_64"

die() { echo "release.sh: $*" >&2; exit 1; }
say() { echo "== $*" >&2; }

rust_target() {
  case "$1" in
    linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    linux-aarch64) echo aarch64-unknown-linux-gnu ;;
    windows-x86_64) echo x86_64-pc-windows-gnu ;;
    macos-aarch64) echo aarch64-apple-darwin ;;
    macos-x86_64) echo x86_64-apple-darwin ;;
    *) die "unknown platform $1 (one of: $TARGETS_ALL)" ;;
  esac
}

cargo_version() {
  sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -n1
}

have_zigbuild() { command -v zig >/dev/null 2>&1 && cargo zigbuild --help >/dev/null 2>&1; }

# Builds both programs for platform $1 into target/dist/<rust target>/release (or
# target/dist/release for a native build). Prints the folder; returns 1 (with the
# reason on stderr) when the platform can't be built here.
build_target() {
  local platform="$1" t
  t="$(rust_target "$platform")"
  local pkgs=(-p robocode-s-cli -p robocode-s-gui)
  export CARGO_TARGET_DIR="$ROOT/target/dist"
  case "$platform" in
    linux-*)
      if have_zigbuild; then
        rustup target list --installed | grep -qx "$t" || { echo "skipped $platform: rustup target add $t" >&2; return 1; }
        cargo zigbuild --release --locked "${pkgs[@]}" --target "$t.$GLIBC" >&2 || return 1
      elif [ "$platform" = linux-x86_64 ] && [ "$(uname -m)" = x86_64 ] && [ "$(uname -s)" = Linux ]; then
        echo "warning: no zig/cargo-zigbuild: linux-x86_64 is built against this machine's glibc ($(ldd --version | head -n1 | sed 's/.* //')) and will not run on older distributions" >&2
        cargo build --release --locked "${pkgs[@]}" --target "$t" >&2 || return 1
      else
        echo "skipped $platform: needs cargo-zigbuild and zig (see the top of this script)" >&2; return 1
      fi ;;
    windows-x86_64)
      command -v x86_64-w64-mingw32-gcc >/dev/null || { echo "skipped $platform: MinGW-w64 not installed (apt install mingw-w64)" >&2; return 1; }
      rustup target list --installed | grep -qx "$t" || { echo "skipped $platform: rustup target add $t" >&2; return 1; }
      cargo build --release --locked "${pkgs[@]}" --target "$t" >&2 || return 1 ;;
    macos-*)
      if [ "$(uname -s)" = Darwin ]; then
        cargo build --release --locked "${pkgs[@]}" --target "$t" >&2 || return 1
      elif [ -n "${SDKROOT:-}" ] && have_zigbuild; then
        cargo zigbuild --release --locked "${pkgs[@]}" --target "$t" >&2 || return 1
      else
        echo "skipped $platform: cross-linking needs the macOS SDK (build on a Mac, or set SDKROOT with zigbuild)" >&2; return 1
      fi ;;
  esac
  echo "$CARGO_TARGET_DIR/$t/release"
}

checks() {
  say "cargo fmt"; cargo fmt --all --check
  say "cargo clippy (host)"; cargo clippy --workspace --all-targets --locked -- -D warnings
  if rustup target list --installed | grep -qx x86_64-pc-windows-gnu && command -v x86_64-w64-mingw32-gcc >/dev/null; then
    say "cargo clippy (Windows)"
    CARGO_TARGET_DIR=target/windows cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings
  else
    echo "warning: Windows clippy skipped (no x86_64-pc-windows-gnu target or MinGW)" >&2
  fi
  say "cargo test"; cargo test --workspace --locked
  say "gradle build"; ./gradlew build
}

# Assembles dist/<name>/ for platform $1 from the binaries in $2; archives it.
assemble() {
  local platform="$1" bin="$2" version="$3" out="$4" notices="$5"
  local name="robocode-s-$version-$platform" exe="" dir
  [ "$platform" = windows-x86_64 ] && exe=.exe
  dir="$out/$name"
  rm -rf "$dir"; mkdir -p "$dir/lib/agent" "$dir/robots"
  cp "$bin/robocode-s$exe" "$bin/robocode-s-gui$exe" "$dir/"
  cp -r clients/java-compat/build/install/robot-host/lib "$dir/lib/robot-host"
  cp clients/java-determinism/build/libs/java-determinism-*-SNAPSHOT.jar "$dir/lib/agent/java-determinism.jar" 2>/dev/null \
    || cp clients/java-determinism/build/libs/java-determinism-*.jar "$dir/lib/agent/java-determinism.jar"
  cp robots/samples/build/libs/samples-*.jar "$dir/robots/"
  cp LICENSE "$dir/LICENSE"
  cp "$notices" "$dir/THIRD-PARTY-NOTICES.txt"

  local start data config ossb extra=""
  case "$platform" in
    linux-*)
      mkdir -p "$dir/share"
      cp crates/gui/robocode-s.desktop "$dir/share/robocode-s.desktop"
      cp web/static/favicon-256.png "$dir/share/robocode-s.png"
      cp tools/release/install-desktop.sh "$dir/share/install-desktop.sh"
      chmod +x "$dir/share/install-desktop.sh"
      start="Run ./robocode-s-gui (the window), or ./robocode-s in a terminal. share/install-desktop.sh
adds robocode-s to the application menu of your user (and gives the window its icon
on Wayland)."
      data='$XDG_DATA_HOME/robocode-s (~/.local/share/robocode-s)'
      config='$XDG_CONFIG_HOME/robocode-s/gui.json (~/.config/robocode-s/gui.json)'
      ossb="seccomp and, where the kernel has it, Landlock: no network, no files but the robot's own"
      extra="
The programs need glibc $GLIBC or newer (Ubuntu 20.04, Debian 11, Fedora 32 and later)."
      ;;
    windows-x86_64)
      start="Double-click robocode-s-gui.exe. robocode-s.exe is a console program: run it in Command
Prompt or PowerShell (robocode-s.exe battle --help). The programs are not signed, so
SmartScreen asks once: More info, Run anyway."
      data='%LOCALAPPDATA%\robocode-s'
      config='%APPDATA%\robocode-s\gui.json'
      ossb="an AppContainer and a Job Object: no network, no files but the robot's own"
      extra="
Unpack this folder on an NTFS drive of your own (not a network share, a FAT drive or
\\\\wsl\$ paths): the sandbox grants its robots the Java runtime, the robot host and the
robots' folders by ACL entries."
      ;;
    macos-*)
      start="Run ./robocode-s-gui, or ./robocode-s in a terminal. The programs are not signed or
notarized, so Gatekeeper refuses them after a download; once, from this folder:
  xattr -dr com.apple.quarantine ."
      data='~/Library/Application Support/robocode-s'
      config='~/Library/Application Support/robocode-s/gui.json'
      ossb="not available on macOS yet; the default is 'none' (a plain process)"
      extra=""
      ;;
  esac
  START="$start" DATA="$data" CONFIG="$config" OSSB="$ossb" EXTRA="$extra" VERSION="$version" PLATFORM="$platform" \
    python3 -I - tools/release/release-readme.txt "$dir/README.txt" <<'PY'
import os, sys
t = open(sys.argv[1], encoding="utf-8").read()
for k in ("START", "DATA", "CONFIG", "EXTRA", "VERSION", "PLATFORM"):
    t = t.replace("@%s@" % k, os.environ[k])
t = t.replace("@OSSANDBOX@", os.environ["OSSB"])
open(sys.argv[2], "w", encoding="utf-8", newline="\r\n" if os.environ["PLATFORM"].startswith("windows") else "\n").write(t)
PY

  # Archives: fixed owner, order and times, so the same inputs give the same bytes.
  local stamp
  stamp="@$(git log -1 --format=%ct)"
  if [ "$platform" = windows-x86_64 ]; then
    rm -f "$out/$name.zip"
    (cd "$out" && python3 -I - "$name" "$name.zip" "$(git -C "$ROOT" log -1 --format=%ct)" <<'PY'
import os, sys, zipfile, time
src, dst, ts = sys.argv[1], sys.argv[2], int(sys.argv[3])
date = time.gmtime(max(ts, 315532800))[:6]
with zipfile.ZipFile(dst, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(root, f)
            zi = zipfile.ZipInfo(p.replace(os.sep, "/"), date)
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o644 << 16
            z.writestr(zi, open(p, "rb").read())
PY
    )
    echo "$out/$name.zip"
  else
    rm -f "$out/$name.tar.gz"
    tar -C "$out" --sort=name --owner=0 --group=0 --numeric-owner --mtime="$stamp" -cf - "$name" | gzip -9n > "$out/$name.tar.gz"
    echo "$out/$name.tar.gz"
  fi
  rm -rf "$dir"
}


cmd_build() {
  local version="" targets="$TARGETS_ALL" skip_checks=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --version) version="$2"; shift 2 ;;
      --targets) targets="${2//,/ }"; shift 2 ;;
      --skip-checks) skip_checks=1; shift ;;
      *) die "unknown option $1" ;;
    esac
  done
  local cv; cv="$(cargo_version)"
  version="${version:-$cv}"
  [ "$version" = "$cv" ] || echo "warning: --version $version differs from Cargo.toml's $cv (the programs report $cv)" >&2
  [ -z "$(git status --porcelain)" ] || echo "warning: the working tree has uncommitted changes" >&2
  [ "$skip_checks" = 1 ] && echo "warning: checks skipped" >&2 || checks
  say "Java parts"
  ./gradlew :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
  local out="$ROOT/target/release-dist/$version"
  rm -rf "$out"; mkdir -p "$out"
  say "THIRD-PARTY-NOTICES.txt"
  python3 -I tools/release/notices.py "$out/THIRD-PARTY-NOTICES.txt" 2>/dev/null
  local made=() skipped=() p bin
  for p in $targets; do
    say "build $p"
    if bin="$(build_target "$p")"; then
      made+=("$(assemble "$p" "$bin" "$version" "$out" "$out/THIRD-PARTY-NOTICES.txt")")
    else
      skipped+=("$p")
    fi
  done
  rm -f "$out/THIRD-PARTY-NOTICES.txt"
  [ ${#made[@]} -gt 0 ] || die "nothing was built"
  (cd "$out" && sha256sum -- * > SHA256SUMS)
  say "done: $out"
  ls -l "$out" >&2
  [ ${#skipped[@]} -eq 0 ] || echo "skipped: ${skipped[*]}" >&2
}

cmd_smoke() {
  local archive="${1:?archive}" tmp
  tmp="$(mktemp -d)"
  tar -C "$tmp" -xzf "$archive"
  local d; d="$(echo "$tmp"/robocode-s-*)"
  mkdir "$tmp/elsewhere"; cd "$tmp/elsewhere"
  "$d/robocode-s" --version
  for sb in os none; do
    "$d/robocode-s" battle sample.Fire,sample.Walls --rounds 2 --seed 1 --sandbox "$sb"
  done
  echo "smoke test passed in $tmp"
}

cmd_upload() {
  local version="${1:?VERSION}"; shift
  local draft=--draft
  [ "${1:-}" = --publish ] && draft=""
  local dir="$ROOT/target/release-dist/$version"
  [ -f "$dir/SHA256SUMS" ] || die "no $dir/SHA256SUMS: run build --version $version first"
  (cd "$dir" && sha256sum --check --quiet SHA256SUMS) || die "the archives don't match SHA256SUMS"
  [ -z "$(git status --porcelain)" ] || die "uncommitted changes"
  git branch -r --contains HEAD | grep -q . || die "HEAD is not pushed"
  local files=(); local f
  for f in "$dir"/*; do files+=("$f"); done
  gh release create "v$version" --repo "$REPO" --target "$(git rev-parse HEAD)" \
    --title "robocode-s $version" --generate-notes $draft "${files[@]}"
}

case "${1:-}" in
  build) shift; cmd_build "$@" ;;
  smoke) shift; cmd_smoke "$@" ;;
  upload) shift; cmd_upload "$@" ;;
  *) sed -n '2,/^set -e/p' "$0" | sed 's/^# \{0,1\}//' | head -n 40; exit 2 ;;
esac
