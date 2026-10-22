#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""The winget manifest of a release (design §6.9, issue #79).

  winget.py VERSION [--submit [--for-real]] [--dist-dir DIR]

Writes the three manifest files of NickHofstede.robocode-s VERSION into
target/release-dist/VERSION/winget/ (manifest schema 1.6.0): the installer is the NSIS setup,
fetched from the GitHub release v<VERSION>, with its checksum from SHA256SUMS.
Nothing is submitted without --submit --for-real; --submit alone prints what would be done.

--submit opens the pull request to microsoft/winget-pkgs with `gh`, from the user's fork
(`gh repo fork`, a branch, the three files through the contents API, `gh pr create`). It
needs the GitHub release (winget's validation downloads the installer URL) and `gh auth login`.
`wingetcreate submit DIR` does the same from Windows.

Python 3 standard library only.
"""
import argparse
import base64
import datetime
import os
import re
import shlex
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PACKAGE_ID = "NickHofstede.robocode-s"
SCHEMA = "1.6.0"
REPO = "nhofsted/robocode-s"
UPSTREAM = "microsoft/winget-pkgs"
# installer.nsi's UNINST_KEY: HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\robocode-s
PRODUCT_CODE = "robocode-s"


def die(msg):
    sys.exit("winget.py: " + msg)


def manifests(version, sha256, date):
    url = f"https://github.com/{REPO}/releases/download/v{version}/robocode-s-{version}-windows-x86_64-setup.exe"
    head = f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.%s.{SCHEMA}.schema.json\n\n"
    version_yaml = (head % "version" + f"""PackageIdentifier: {PACKAGE_ID}
PackageVersion: {version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {SCHEMA}
""")
    installer_yaml = (head % "installer" + f"""PackageIdentifier: {PACKAGE_ID}
PackageVersion: {version}
InstallerLocale: en-US
MinimumOSVersion: 10.0.0.0
InstallerType: nullsoft
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
InstallerSwitches:
  Silent: /S
  SilentWithProgress: /S
UpgradeBehavior: install
Commands:
- robocode-s
- robocode-s-gui
ReleaseDate: {date}
Installers:
- Architecture: x64
  InstallerUrl: {url}
  InstallerSha256: {sha256.upper()}
  ProductCode: {PRODUCT_CODE}
  AppsAndFeaturesEntries:
  - DisplayName: robocode-s
    Publisher: Nick Hofstede
    DisplayVersion: {version}
    ProductCode: {PRODUCT_CODE}
    InstallerType: nullsoft
ManifestType: installer
ManifestVersion: {SCHEMA}
""")
    locale_yaml = (head % "defaultLocale" + f"""PackageIdentifier: {PACKAGE_ID}
PackageVersion: {version}
PackageLocale: en-US
Publisher: Nick Hofstede
PublisherUrl: https://github.com/nhofsted
PublisherSupportUrl: https://github.com/{REPO}/issues
Author: Nick Hofstede
PackageName: robocode-s
PackageUrl: https://github.com/{REPO}
License: EPL-2.0
LicenseUrl: https://github.com/{REPO}/blob/v{version}/LICENSE
Copyright: Copyright (c) Nick Hofstede and the robocode-s contributors
ShortDescription: Robocode battle engine and viewer
Description: "robocode-s is a Robocode reimplementation: a Rust battle engine with bit-exact classic compatibility, a battlefield viewer (robocode-s-gui) and a command line (robocode-s), robots out of process in a sandbox. It runs classic Java robots; the sample robots and the robot host are included. The Java runtime for the robots is downloaded on the first battle."
Moniker: robocode-s
Tags:
- robocode
- java
- game
- programming-game
- battle
- simulator
ReleaseNotesUrl: https://github.com/{REPO}/releases/tag/v{version}
ManifestType: defaultLocale
ManifestVersion: {SCHEMA}
""")
    return {
        f"{PACKAGE_ID}.yaml": version_yaml,
        f"{PACKAGE_ID}.installer.yaml": installer_yaml,
        f"{PACKAGE_ID}.locale.en-US.yaml": locale_yaml,
    }


def sh(cmd, real, capture=False):
    print("$ " + " ".join(shlex.quote(c) for c in cmd))
    if not real:
        return ""
    r = subprocess.run(cmd, stdout=subprocess.PIPE if capture else None, text=True)
    if r.returncode:
        die("failed: " + " ".join(cmd[:3]))
    return r.stdout.strip() if capture else ""


def submit(version, out, real):
    pub, _, pkg = PACKAGE_ID.partition(".")
    path = f"manifests/{pub[0].lower()}/{pub}/{pkg}/{version}"
    print(f"# {'SUBMITTING' if real else 'DRY RUN (--for-real submits)'}: {len(os.listdir(out))} files to {UPSTREAM}/{path}")
    me = sh(["gh", "api", "user", "--jq", ".login"], real, True) or "<your GitHub login>"
    branch = f"{PACKAGE_ID}-{version}"
    sh(["gh", "repo", "fork", UPSTREAM, "--clone=false"], real)
    sh(["gh", "repo", "sync", f"{me}/winget-pkgs", "--source", UPSTREAM], real)
    base = sh(["gh", "api", f"repos/{me}/winget-pkgs/git/ref/heads/master", "--jq", ".object.sha"], real, True) or "<master sha>"
    sh(["gh", "api", f"repos/{me}/winget-pkgs/git/refs", "-f", f"ref=refs/heads/{branch}", "-f", f"sha={base}"], real)
    for name in sorted(os.listdir(out)):
        with open(os.path.join(out, name), "rb") as f:
            content = base64.b64encode(f.read()).decode() if real else f"<base64 of {name}>"
        sh(["gh", "api", "-X", "PUT", f"repos/{me}/winget-pkgs/contents/{path}/{name}",
            "-f", f"message=New version: {PACKAGE_ID} version {version}", "-f", f"branch={branch}",
            "-f", f"content={content}"], real)
    sh(["gh", "pr", "create", "--repo", UPSTREAM, "--head", f"{me}:{branch}", "--base", "master",
        "--title", f"New version: {PACKAGE_ID} version {version}",
        "--body", f"robocode-s {version}: https://github.com/{REPO}/releases/tag/v{version}"], real)


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("version")
    p.add_argument("--dist-dir", default=os.environ.get("ROBOCODES_DIST_ROOT", os.path.join(ROOT, "target", "release-dist")))
    p.add_argument("--submit", action="store_true")
    p.add_argument("--for-real", action="store_true")
    a = p.parse_args()
    d = os.path.join(a.dist_dir, a.version)
    setup = f"robocode-s-{a.version}-windows-x86_64-setup.exe"
    sha = None
    try:
        with open(os.path.join(d, "SHA256SUMS")) as f:
            for line in f:
                m = re.match(r"^([0-9a-f]{64})  (.+)$", line.strip())
                if m and m.group(2) == setup:
                    sha = m.group(1)
    except FileNotFoundError:
        die(f"no {d}/SHA256SUMS: run release.sh build --version {a.version} first")
    if not sha:
        die(f"{setup} is not in {d}/SHA256SUMS (the Windows installer needs makensis, see release.sh)")
    sfile = os.path.join(d, setup)
    now = datetime.datetime.now(datetime.timezone.utc)
    when = datetime.datetime.fromtimestamp(os.path.getmtime(sfile), datetime.timezone.utc) if os.path.exists(sfile) else now
    out = os.path.join(d, "winget")
    os.makedirs(out, exist_ok=True)
    for name, text in manifests(a.version, sha, when.strftime("%Y-%m-%d")).items():
        with open(os.path.join(out, name), "w", newline="\n") as f:
            f.write(text)
    print(f"wrote {out}", file=sys.stderr)
    if a.submit:
        submit(a.version, out, a.for_real)


if __name__ == "__main__":
    main()
