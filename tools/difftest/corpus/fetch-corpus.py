#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""A reproducible corpus of RoboRumble robots for differential testing (#11).

  fetch-corpus.py select --manifest corpus.json --jars DIR [--seed N --duel N --melee N --teams N]
      Picks robots at random (seeded) from the current RoboWiki participant lists,
      downloads and validates them, and writes the manifest (names, sources, SHA-256).
  fetch-corpus.py download --manifest corpus.json --jars DIR
      Downloads exactly the robots in the manifest and verifies their SHA-256.

The manifest is committed; the jars are not (they belong to their authors).
"""
import argparse, hashlib, io, json, random, re, sys, urllib.parse, urllib.request, zipfile
from pathlib import Path

LISTS = {
    "duel": "RoboRumble/Participants",
    "melee": "RoboRumble/Participants/Melee",
    "teams": "RoboRumble/Participants/Teams",
}
ARCHIVE = "https://robocode-archive.strangeautomata.com/robots/{}_{}.jar"
AGENT = {"User-Agent": "robocode-s difftest (https://github.com/nhofsted/robocode-s)"}


def participants(page):
    url = "https://robowiki.net/w/index.php?title=" + urllib.parse.quote(page) + "&action=raw"
    text = urllib.request.urlopen(urllib.request.Request(url, headers=AGENT), timeout=60).read().decode("utf-8", "replace")
    entries = []
    for line in text.splitlines():
        m = re.match(r"^([A-Za-z0-9_.$]+) ([^,]+),(https?://\S+)$", line.strip())
        if m:
            entries.append({"name": m.group(1), "version": m.group(2).strip(), "listed_url": m.group(3)})
    return entries


def fetch(url):
    try:
        return urllib.request.urlopen(urllib.request.Request(url, headers=AGENT), timeout=60).read()
    except Exception:
        return None


def valid(data, name, version, team):
    """A jar with the robot's .properties (or the team's .team file) and matching version."""
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            wanted = name.replace(".", "/") + (".team" if team else ".properties")
            if wanted not in z.namelist():
                return False
            props = z.read(wanted).decode("latin-1")
            m = re.search(r"(?:robot|team)\.version\s*=\s*(.*)", props)
            return m is None or m.group(1).strip() == version
    except zipfile.BadZipFile:
        return False


def download(entry, team):
    for url in [ARCHIVE.format(entry["name"], entry["version"]), entry["listed_url"]]:
        data = fetch(url)
        if data and valid(data, entry["name"], entry["version"], team):
            return url, data
    return None, None


def select(args):
    jars = Path(args.jars)
    jars.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    chosen, names = [], {}
    for pool, count in [("duel", args.duel), ("melee", args.melee), ("teams", args.teams)]:
        entries = participants(LISTS[pool])
        rng.shuffle(entries)
        picked = 0
        for e in entries:
            if picked == count:
                break
            if e["name"] in names:
                # Already chosen for another pool: reuse it if it's the same version.
                c = names[e["name"]]
                if c["version"] == e["version"] and pool not in c["pools"]:
                    c["pools"].append(pool)
                    picked += 1
                continue
            url, data = download(e, pool == "teams")
            if data is None:
                print(f"  skip {e['name']} {e['version']}: no valid jar", file=sys.stderr)
                continue
            file = f"{e['name']}_{e['version']}.jar"
            (jars / file).write_bytes(data)
            c = {"name": e["name"], "version": e["version"], "pools": [pool], "url": url,
                 "sha256": hashlib.sha256(data).hexdigest(), "file": file}
            chosen.append(c)
            names[e["name"]] = c
            picked += 1
            print(f"  {pool:5} {e['name']} {e['version']}", file=sys.stderr)
    # Keep exclusions (with their reasons) from an existing manifest.
    if Path(args.manifest).exists():
        old = {r["name"]: r for r in json.loads(Path(args.manifest).read_text())["robots"]}
        for c in chosen:
            if "exclude" in old.get(c["name"], {}):
                c["exclude"] = old[c["name"]]["exclude"]
    manifest = {"seed": args.seed, "robots": chosen}
    Path(args.manifest).write_text(json.dumps(manifest, indent=1) + "\n")
    print(f"{len(chosen)} robots in {args.manifest}", file=sys.stderr)


def download_all(args):
    jars = Path(args.jars)
    jars.mkdir(parents=True, exist_ok=True)
    manifest = json.loads(Path(args.manifest).read_text())
    failed = 0
    for c in manifest["robots"]:
        target = jars / c["file"]
        data = target.read_bytes() if target.exists() else fetch(c["url"])
        if data is None or hashlib.sha256(data).hexdigest() != c["sha256"]:
            print(f"  FAIL {c['file']}: missing or changed", file=sys.stderr)
            failed += 1
            continue
        target.write_bytes(data)
    print(f"{len(manifest['robots']) - failed} of {len(manifest['robots'])} robots ready in {jars}", file=sys.stderr)
    return 1 if failed else 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("select")
    s.add_argument("--manifest", required=True)
    s.add_argument("--jars", required=True)
    s.add_argument("--seed", type=int, default=20261011)
    s.add_argument("--duel", type=int, default=40)
    s.add_argument("--melee", type=int, default=20)
    s.add_argument("--teams", type=int, default=8)
    d = sub.add_parser("download")
    d.add_argument("--manifest", required=True)
    d.add_argument("--jars", required=True)
    args = p.parse_args()
    return select(args) if args.command == "select" else download_all(args)


if __name__ == "__main__":
    sys.exit(main() or 0)
