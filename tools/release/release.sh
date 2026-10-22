#!/usr/bin/env bash
# SPDX-License-Identifier: EPL-2.0
#
# Local release script (there is no CI): checks, builds, archives, packages, uploads.
#
#   tools/release/release.sh build [--version X] [--targets T,T] [--skip-checks]
#   tools/release/release.sh smoke ARCHIVE.tar.gz | PACKAGE.deb | PACKAGE.rpm | SETUP.exe
#   tools/release/release.sh cask VERSION
#   tools/release/release.sh upload VERSION [--publish]
#   tools/release/release.sh publish VERSION [--dry-run]
#
# build   runs the checks (cargo fmt, clippy -D warnings for Linux and Windows,
#         cargo test, gradle build), builds the release binaries of every target
#         this machine can build, and writes to target/release-dist/<version>/,
#         with SHA256SUMS: one archive per platform, and the packages made from
#         the same files (tools/release/package.py):
#           Linux    .deb (dpkg-deb) and .rpm (written by package.py, no rpmbuild)
#           Windows  robocode-s-<v>-windows-x86_64-setup.exe, a per-user NSIS
#                    installer (makensis: `apt install nsis`, or the NSIS 3 zip
#                    for Windows unpacked in ~/.local/opt/nsis-3.11, run through WSL)
#           macOS    robocode-s-<v>-macos-<arch>.app.zip, and a .dmg where
#                    hdiutil exists, i.e. when this runs on a Mac (a .dmg can't be
#                    made on Linux); `cask` prints a Homebrew cask for the zips
#         A target or package it can't build is skipped with the reason; the
#         others are still made.
# smoke   unpacks a Linux archive into a temporary folder and fights a battle
#         with it from another folder; installs a .deb or .rpm in containers
#         (Docker; SMOKE_DEB_IMAGES, SMOKE_RPM_IMAGES) and removes it again;
#         runs the Windows installer silently through WSL interop, fights a
#         battle from the installed folder and uninstalls it (touches HKCU
#         and %LOCALAPPDATA%\Programs of the Windows user running WSL).
# cask    prints the Homebrew cask of the macOS .app.zip files of VERSION (the
#         template is tools/release/robocode-s.rb.in; nothing is published).
# upload  creates the GitHub release vVERSION (a draft unless --publish) from
#         target/release-dist/<version>/ with `gh release create`. Nothing else
#         in this script publishes anything. The commit must be pushed.
# publish walks through every publication of a release, in order, and asks y/N before each:
#         1. the GitHub release (upload --publish)   2. the apt and dnf repositories (repo.py:
#         build with the signing key, upload to S3, invalidate CloudFront)   3. the winget pull
#         request (winget.py --submit)   4. the Homebrew tap (tap.sh). Anything but y skips the
#         step. --dry-run shows the steps and what each would run, and runs none of them.
#         Needs: gh signed in, ROBOCODES_GPG_KEY and AWS_PROFILE (default robocode-s) for step 2.
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
# For the packages (a missing tool skips just that package):
#   dpkg-deb (Debian, Ubuntu: part of dpkg) for the .deb
#   NSIS 3 for the Windows installer: apt install nsis, or on WSL the Windows zip of
#     https://nsis.sourceforge.io/Download unpacked to ~/.local/opt/nsis-3.11 (makensis.exe
#     is run through WSL interop; no administrator; or set MAKENSIS)
#   python3 (the .rpm and the macOS bundle are written by tools/release/package.py)
#   Docker, for `smoke` of the .deb and .rpm
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

# README.txt for platform $1 in the form $2 (archive, or package for the Linux packages),
# written to $4.
write_readme() {
  local platform="$1" kind="$2" version="$3" dst="$4"
  local start data config ossb extra="" layout
  layout="This folder holds the robot host, the determinism agent and the sample robots
(robots/), and the programs find them next to themselves. The folder can be moved or
deleted as a whole."
  case "$platform" in
    linux-*)
      data='$XDG_DATA_HOME/robocode-s (~/.local/share/robocode-s)'
      config='$XDG_CONFIG_HOME/robocode-s/gui.json (~/.config/robocode-s/gui.json)'
      ossb="seccomp and, where the kernel has it, Landlock: no network, no files but the robot's own"
      extra="
The programs need glibc $GLIBC or newer (Ubuntu 20.04, Debian 11, Fedora 32 and later)."
      if [ "$kind" = package ]; then
        start="Start robocode-s from the application menu (robocode-s-gui), or run robocode-s in a
terminal. Recordings (.rsr, .track) open in the window."
        layout="The package installed the programs' folder in /usr/lib/robocode-s (the robot host,
the determinism agent and the sample robots, robots/); /usr/bin/robocode-s and
robocode-s-gui link to the programs there, which find the rest next to themselves.
Uninstall with the package manager (apt remove robocode-s, dnf remove robocode-s)."
      else
        start="Run ./robocode-s-gui (the window), or ./robocode-s in a terminal. share/install-desktop.sh
adds robocode-s to the application menu of your user (and gives the window its icon
on Wayland)."
      fi ;;
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
robots' folders by ACL entries.

The installer (robocode-s-<version>-windows-x86_64-setup.exe) puts this folder in
%LOCALAPPDATA%\\Programs\\robocode-s, which needs no administrator."
      ;;
    macos-*)
      data='~/Library/Application Support/robocode-s'
      config='~/Library/Application Support/robocode-s/gui.json'
      ossb="not available on macOS yet; the default is 'none' (a plain process)"
      if [ "$kind" = package ]; then
        start="Drag robocode-s.app to Applications and open it. The app is not signed or notarized,
so Gatekeeper refuses it after a download: open it with a right click, Open (or in
System Settings, Privacy & Security, Open Anyway), or once from a terminal:
  xattr -dr com.apple.quarantine /Applications/robocode-s.app
The command line is inside it: robocode-s.app/Contents/MacOS/robocode-s."
        layout="The programs are in the app's Contents/MacOS; the robot host, the determinism agent
and the sample robots (robots/) are in its Contents/Resources, where they find them."
      else
        start="Run ./robocode-s-gui, or ./robocode-s in a terminal. The programs are not signed or
notarized, so Gatekeeper refuses them after a download; once, from this folder:
  xattr -dr com.apple.quarantine ."
      fi ;;
  esac
  START="$start" DATA="$data" CONFIG="$config" OSSB="$ossb" EXTRA="$extra" LAYOUT="$layout" VERSION="$version" PLATFORM="$platform" \
    python3 -I - tools/release/release-readme.txt "$dst" <<'PY'
import os, sys
t = open(sys.argv[1], encoding="utf-8").read()
for k in ("START", "DATA", "CONFIG", "EXTRA", "LAYOUT", "VERSION", "PLATFORM"):
    t = t.replace("@%s@" % k, os.environ[k])
t = t.replace("@OSSANDBOX@", os.environ["OSSB"])
open(sys.argv[2], "w", encoding="utf-8", newline="\r\n" if os.environ["PLATFORM"].startswith("windows") else "\n").write(t)
PY
}

# Assembles dist/<name>/ for platform $1 from the binaries in $2; archives it, and makes
# the platform's packages from it (Linux: .deb and .rpm; Windows: the installer; macOS:
# the .app, zipped, and a .dmg where hdiutil is). Prints the files made.
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

  case "$platform" in
    linux-*)
      mkdir -p "$dir/share"
      cp crates/gui/robocode-s.desktop "$dir/share/robocode-s.desktop"
      cp web/static/favicon-256.png "$dir/share/robocode-s.png"
      cp tools/release/robocode-s-mime.xml "$dir/share/robocode-s-mime.xml"
      cp tools/release/install-desktop.sh "$dir/share/install-desktop.sh"
      chmod +x "$dir/share/install-desktop.sh" ;;
  esac
  write_readme "$platform" archive "$version" "$dir/README.txt"

  # Archives: fixed owner, order and times, so the same inputs give the same bytes.
  local stamp epoch
  stamp="@$(git log -1 --format=%ct)"
  epoch="$(git log -1 --format=%ct)"
  if [ "$platform" = windows-x86_64 ]; then
    rm -f "$out/$name.zip"
    (cd "$out" && python3 -I - "$name" "$name.zip" "$epoch" <<'PY'
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

  local pk="python3 -I tools/release/package.py"
  case "$platform" in
    linux-*)
      write_readme "$platform" package "$version" "$out/.README-$platform"
      local deb_arch="amd64" rpm_arch="${platform#linux-}"
      [ "$platform" = linux-aarch64 ] && deb_arch=arm64
      if command -v dpkg-deb >/dev/null; then
        $pk deb --src "$dir" --out "$out/$name.deb" --arch "$deb_arch" --version "$version" --epoch "$epoch" --readme "$out/.README-$platform" \
          && echo "$out/$name.deb" || echo "warning: the .deb of $platform failed" >&2
      else
        echo "skipped the .deb of $platform: dpkg-deb not installed" >&2
      fi
      $pk rpm --src "$dir" --out "$out/$name.rpm" --arch "$rpm_arch" --version "$version" --epoch "$epoch" --readme "$out/.README-$platform" \
        && echo "$out/$name.rpm" || echo "warning: the .rpm of $platform failed" >&2
      rm -f "$out/.README-$platform" ;;
    windows-x86_64)
      if $pk win --src "$dir" --out "$out/$name-setup.exe" --version "$version"; then
        echo "$out/$name-setup.exe"
      else
        echo "skipped the Windows installer: makensis (NSIS 3) is needed, see package.py" >&2
      fi ;;
    macos-*)
      write_readme "$platform" package "$version" "$dir/README.txt"
      local app_dir="$out/.app-$platform/robocode-s.app" arch="${platform#macos-}"
      mkdir -p "$out/.app-$platform"
      if $pk app --src "$dir" --app-dir "$app_dir" --zip "$out/$name.app.zip" --arch "$arch" --version "$version" --epoch "$epoch"; then
        echo "$out/$name.app.zip"
        if command -v hdiutil >/dev/null; then
          rm -f "$out/$name.dmg"
          hdiutil create -quiet -volname "robocode-s $version" -srcfolder "$app_dir" -ov -format UDZO "$out/$name.dmg" \
            && echo "$out/$name.dmg"
        else
          echo "no .dmg for $platform: hdiutil needs a Mac (the .app.zip is made; see README, Installing)" >&2
        fi
      fi
      rm -rf "$out/.app-$platform" ;;
  esac
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

BATTLE=(battle sample.Fire,sample.Walls --rounds 2 --seed 1)

# Installs the .deb or .rpm $1 in a container of each image (SMOKE_DEB_IMAGES,
# SMOKE_RPM_IMAGES), fights a battle under both sandboxes, removes the package and checks
# that nothing is left. Needs Docker and a package for this machine's architecture.
smoke_container() {
  local pkg="$1" kind="$2" images img file dir
  file="$(basename "$pkg")"; dir="$(cd "$(dirname "$pkg")" && pwd)"
  case "$file" in
    *aarch64*|*arm64*) [ "$(uname -m)" = aarch64 ] || { echo "skipped $file: not this machine's architecture" >&2; return 0; } ;;
    *) [ "$(uname -m)" = x86_64 ] || { echo "skipped $file: not this machine's architecture" >&2; return 0; } ;;
  esac
  command -v docker >/dev/null || die "smoke test of $file: Docker is not installed"
  if [ "$kind" = deb ]; then
    images="${SMOKE_DEB_IMAGES:-debian:12 ubuntu:24.04}"
  else
    images="${SMOKE_RPM_IMAGES:-fedora:41 rockylinux:9}"
  fi
  for img in $images; do
    say "$file in $img"
    # Docker's default seccomp profile may refuse what the `os` sandbox sets up (seccomp
    # filters, Landlock): a failure of that battle is reported, not fatal.
    docker run --rm -v "$dir:/pkg:ro" "$img" sh -ec '
      kind='"$kind"'; file='"$file"'
      if [ $kind = deb ]; then
        apt-get update -qq >/dev/null; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "/pkg/$file" >/dev/null
      else
        dnf install -y -q "/pkg/$file" >/dev/null
      fi
      cd /tmp
      robocode-s --version
      test -x /usr/bin/robocode-s-gui; test -f /usr/share/applications/robocode-s.desktop
      robocode-s battle sample.Fire,sample.Walls --rounds 2 --seed 1 --sandbox none
      robocode-s battle sample.Fire,sample.Walls --rounds 2 --seed 1 --sandbox os || echo "WARNING: the os sandbox failed in this container"
      if [ $kind = deb ]; then apt-get remove -y -qq robocode-s >/dev/null; else dnf remove -y -q robocode-s >/dev/null; fi
      test ! -e /usr/lib/robocode-s; test ! -e /usr/bin/robocode-s; test ! -e /usr/share/applications/robocode-s.desktop
      echo "removed cleanly"
    '
  done
}

# Runs the Windows installer $1 through WSL interop: a silent install for the current
# user, the programs from the installed folder (a battle under the `os` sandbox), a silent
# uninstall, and checks of the files, the shortcut and the registry (PATH included).
smoke_windows() {
  local setup="$1" tmp local_app dir path_before path_after
  [ -e /proc/sys/fs/binfmt_misc/WSLInterop ] || die "smoke test of the installer: needs WSL (Windows interop)"
  tmp="$(mktemp -d)"; cp "$setup" "$tmp/setup.exe"; chmod +x "$tmp/setup.exe"
  cd /mnt/c
  local_app="$(wslpath "$(cmd.exe /c 'echo %LOCALAPPDATA%' 2>/dev/null | tr -d '\r')")"
  dir="$local_app/Programs/robocode-s"
  [ ! -e "$dir" ] || die "$dir exists: not touching an installation"
  path_before="$(reg.exe query 'HKCU\Environment' /v Path)"
  "$tmp/setup.exe" /S
  [ -f "$dir/robocode-s.exe" ] && [ -f "$dir/robocode-s-gui.exe" ] && [ -f "$dir/uninstall.exe" ] || die "the installer did not install the programs"
  reg.exe query 'HKCU\Environment' /v Path | grep -q 'Programs\\robocode-s' || die "the installer did not add robocode-s to PATH"
  reg.exe query 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\robocode-s' >/dev/null || die "no uninstall entry"
  reg.exe query 'HKCU\Software\Classes\.rsr' >/dev/null || die "no file association"
  "$dir/robocode-s.exe" --version
  "$dir/robocode-s.exe" "${BATTLE[@]}" --sandbox os
  "$dir/uninstall.exe" /S
  sleep 3
  [ ! -e "$dir/robocode-s.exe" ] && [ ! -e "$dir/uninstall.exe" ] || die "the uninstaller left files in $dir"
  rmdir "$dir/robots" "$dir" 2>/dev/null || true
  for k in 'Software\Microsoft\Windows\CurrentVersion\Uninstall\robocode-s' 'Software\robocode-s' 'Software\Classes\.rsr' 'Software\Classes\robocode-s.recording'; do
    ! reg.exe query "HKCU\\$k" >/dev/null 2>&1 || die "registry key $k is left"
  done
  path_after="$(reg.exe query 'HKCU\Environment' /v Path)"
  [ "$path_before" = "$path_after" ] || die "PATH differs after the uninstall"
  rm -rf "$tmp"
  echo "the installer works: installed, ran a battle, uninstalled, left nothing (the Java runtime and the battle folders are in $local_app/robocode-s)"
}

cmd_smoke() {
  local archive="${1:?archive or package}" tmp
  case "$archive" in
    *.deb) smoke_container "$archive" deb; return ;;
    *.rpm) smoke_container "$archive" rpm; return ;;
    *-setup.exe) smoke_windows "$archive"; return ;;
  esac
  tmp="$(mktemp -d)"
  tar -C "$tmp" -xzf "$archive"
  local d; d="$(echo "$tmp"/robocode-s-*)"
  mkdir "$tmp/elsewhere"; cd "$tmp/elsewhere"
  "$d/robocode-s" --version
  for sb in os none; do
    "$d/robocode-s" "${BATTLE[@]}" --sandbox "$sb"
  done
  echo "smoke test passed in $tmp"
}

# Prints the Homebrew cask (tools/release/robocode-s.rb.in) for the macOS packages of VERSION.
cmd_cask() {
  local version="${1:?VERSION}" dir sum_a sum_x
  dir="${ROBOCODES_DIST_ROOT:-$ROOT/target/release-dist}/$version"
  sum_a="$(sed -n "s/^\([0-9a-f]*\)  robocode-s-$version-macos-aarch64.app.zip\$/\1/p" "$dir/SHA256SUMS")"
  sum_x="$(sed -n "s/^\([0-9a-f]*\)  robocode-s-$version-macos-x86_64.app.zip\$/\1/p" "$dir/SHA256SUMS")"
  [ -n "$sum_a$sum_x" ] || die "no macOS .app.zip in $dir/SHA256SUMS (they are built on a Mac, or with SDKROOT set)"
  sed -e "s/@VERSION@/$version/g" -e "s/@SHA256_AARCH64@/${sum_a:-MISSING}/" -e "s/@SHA256_X86_64@/${sum_x:-MISSING}/" \
    tools/release/robocode-s.rb.in
}

cmd_upload() {
  local version="${1:?VERSION}"; shift
  local draft=--draft
  [ "${1:-}" = --publish ] && draft=""
  local dir="${ROBOCODES_DIST_ROOT:-$ROOT/target/release-dist}/$version"
  [ -f "$dir/SHA256SUMS" ] || die "no $dir/SHA256SUMS: run build --version $version first"
  (cd "$dir" && sha256sum --check --quiet SHA256SUMS) || die "the archives don't match SHA256SUMS"
  [ -z "$(git status --porcelain)" ] || die "uncommitted changes"
  git branch -r --contains HEAD | grep -q . || die "HEAD is not pushed"
  local files=(); local f
  for f in "$dir"/*; do files+=("$f"); done
  gh release create "v$version" --repo "$REPO" --target "$(git rev-parse HEAD)" \
    --title "robocode-s $version" --generate-notes $draft "${files[@]}"
}

confirm() {
  local ans
  read -r -p "$1 [y/N] " ans || ans=n
  [ "$ans" = y ] || [ "$ans" = Y ]
}

cmd_publish() {
  local version="${1:?VERSION}" dry=0 tools="$ROOT/tools/release"
  [ "${2:-}" = --dry-run ] && dry=1
  local n=0 names=("GitHub release v$version (public, with all files)"
    "apt and dnf repositories on https://rumble.planethofstede.be/packages/ (signed with ROBOCODES_GPG_KEY)"
    "winget pull request to microsoft/winget-pkgs (NickHofstede.robocode-s $version)"
    "Homebrew tap nhofsted/homebrew-robocode-s (cask $version)")
  step() {
    n=$((n + 1))
    local title="${names[$((n - 1))]}"; shift
    echo; echo "== Step $n of 4: $title"
    if [ $dry = 1 ]; then
      echo "   would run: $*"
      return 0
    fi
    if confirm "Publish step $n (${title%% (*})?"; then "$@"; else echo "   skipped"; fi
  }
  if [ $dry = 1 ]; then
    echo "DRY RUN: nothing is run or sent."
    step upload "$0" upload "$version" --publish
    step repo "$tools/repo.py" build "$version" --pull '&&' "$tools/repo.py" upload --for-real
    step winget "$tools/winget.py" "$version" --submit --for-real
    step tap "$tools/tap.sh" "$version" --for-real
    echo; echo "The commands each step runs, printed by the tools themselves:"
    "$tools/repo.py" upload || true
    return 0
  fi
  step upload "$0" upload "$version" --publish
  step_repo() { python3 -I "$tools/repo.py" build "$version" --pull && python3 -I "$tools/repo.py" upload && confirm "Upload for real?" && python3 -I "$tools/repo.py" upload --for-real; }
  step repo step_repo
  step winget python3 -I "$tools/winget.py" "$version" --submit --for-real
  step tap "$tools/tap.sh" "$version" --for-real
}

case "${1:-}" in
  publish) shift; cmd_publish "$@" ;;
  build) shift; cmd_build "$@" ;;
  smoke) shift; cmd_smoke "$@" ;;
  cask) shift; cmd_cask "$@" ;;
  upload) shift; cmd_upload "$@" ;;
  *) sed -n '2,/^set -e/p' "$0" | sed 's/^# \{0,1\}//' | head -n 70; exit 2 ;;
esac
