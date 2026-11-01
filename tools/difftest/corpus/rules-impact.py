#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Which rumble bots fight differently under new deterministic rules (#15).

  rules-impact.py --site URL --out DIR --old BINARY:HOST_LIB:AGENT --new BINARY:HOST_LIB:AGENT
                  [--opponent NAME]... [--rounds N] [--jobs N] [--only ID,...]

Fetches the jars of every ranked bot version (the site's rankings and archive), then has
each fight the same seeded battles against fixed opponents (cheap robots that neither skip
turns nor read the clock) twice: with the old rules' robot host and agent, and with the
new ones (each a robocode-s binary with its robot host and agent: a binary refuses a robot
host of other rules). A bot whose results or skipped turns differ between the two fights differently
in the rumble: through skipped turns, or through what its clock reads. Writes
`impact.jsonl` (one line per bot: per opponent both runs' scores and skipped turns) and
prints a summary. Battles already in impact.jsonl are not fought again.
"""
import argparse, concurrent.futures, json, os, subprocess, sys, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def ranked_versions(site):
    rumbles = json.loads(fetch(f"{site}/data/v1/rumbles.json"))["rumbles"]
    versions = {}
    for rumble in rumbles:
        if rumble["weightClass"] != "mega":
            continue  # the weight classes rank the same versions
        rows = json.loads(fetch(f"{site}/data/v1/rankings/{rumble['id']}.json"))
        for row in rows.get("rows") or rows["bots"]:
            versions.setdefault((row["id"], row["version"]), set()).add(rumble["division"])
    return versions


def jars(site, versions, folder):
    archive = json.loads(fetch(f"{site}/data/v1/archive.json"))
    sha = {(b["id"], v["version"]): v["sha256"] for b in archive["bots"] for v in b["versions"]}
    folder.mkdir(parents=True, exist_ok=True)
    found = {}
    for key in sorted(versions):
        if key not in sha:
            print(f"no jar: {key}", file=sys.stderr)
            continue
        path = folder / f"{sha[key]}.jar"
        if not path.exists():
            path.write_bytes(fetch(f"{site}/jars/{sha[key]}.jar"))
        found[key] = path
    return found


def fight(rules, robot, jar, opponent, seed, rounds, data):
    binary, lib, agent = rules.split(":")
    env = dict(os.environ, ROBOCODES_HOST_LIB=lib, ROBOCODES_AGENT_JAR=agent, ROBOCODES_DATA=str(data))
    cmd = [str(binary), "battle", "--json", "--fresh-data", "--rounds", str(rounds), "--seed", str(seed),
           "--jar", str(jar), "--jar", str(next((ROOT / "robots/samples/build/libs").glob("samples-*.jar"))),
           robot, opponent]
    r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=3600)
    if r.returncode != 0:
        return {"error": (r.stderr or r.stdout)[-300:]}
    d = json.loads(r.stdout)
    scores = {x["name"]: round(x["score"], 6) for x in d["results"]}
    return {"scores": scores, "skipped": d.get("skippedTurns", {}), "hardLimit": d.get("hardLimit", [])}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--site", default="https://rumble.planethofstede.be")
    p.add_argument("--out", required=True)
    p.add_argument("--old", required=True, help="binary, robot host lib and agent jar of the old rules, colon-separated")
    p.add_argument("--new", required=True, help="the same for the new rules")
    p.add_argument("--opponent", action="append", help="default: sample.Crazy and sample.Walls")
    p.add_argument("--rounds", type=int, default=35)
    p.add_argument("--jobs", type=int, default=6)
    p.add_argument("--only", help="comma-separated bot ids")
    p.add_argument("--data", help="robocode-s data folder with the pinned JRE (jre/), copied per job")
    args = p.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    opponents = args.opponent or ["sample.Crazy", "sample.Walls"]
    versions = ranked_versions(args.site)
    if args.only:
        versions = {k: v for k, v in versions.items() if k[0] in args.only.split(",")}
    found = jars(args.site, versions, out / "jars")
    done_file = out / "impact.jsonl"
    done = set()
    if done_file.exists():
        done = {tuple(json.loads(l)["bot"]) for l in done_file.read_text().splitlines() if l.strip()}
    todo = [k for k in sorted(found) if k not in done]
    print(f"{len(found)} versions with jars, {len(todo)} to fight", flush=True)

    def one(key):
        bot, version = key
        data = out / "data" / str(os.getpid()) / f"{bot}_{version}"
        data.mkdir(parents=True, exist_ok=True)
        if args.data and not (data / "jre").exists():
            os.symlink(Path(args.data) / "jre", data / "jre")
        row = {"bot": [bot, version], "divisions": sorted(versions[key]), "fights": []}
        for i, opponent in enumerate(opponents):
            seed = 4242 + i
            old = fight(args.old, f"{bot} {version}", found[key], opponent, seed, args.rounds, data)
            new = fight(args.new, f"{bot} {version}", found[key], opponent, seed, args.rounds, data)
            row["fights"].append({"opponent": opponent, "old": old, "new": new})
        subprocess.run(["rm", "-rf", str(data)])
        return row

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool, open(done_file, "a") as f:
        for n, row in enumerate(pool.map(one, todo), 1):
            f.write(json.dumps(row) + "\n")
            f.flush()
            if n % 25 == 0:
                print(f"[{n}/{len(todo)}]", flush=True)
    summarize(done_file)


def summarize(path):
    rows = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
    changed, skips, errors = [], [], []
    for r in rows:
        name = " ".join(r["bot"])
        diff = False
        for f in r["fights"]:
            if "error" in f["old"] or "error" in f["new"]:
                errors.append(name)
                continue
            if f["old"]["scores"] != f["new"]["scores"]:
                diff = True
            so, sn = f["old"]["skipped"].get(name, 0), f["new"]["skipped"].get(name, 0)
            if so != sn:
                skips.append((name, f["opponent"], so, sn))
        if diff:
            changed.append(name)
    print(f"{len(rows)} bots fought; {len(changed)} fight differently; {len(set(s[0] for s in skips))} skip "
          f"other turns; {len(set(errors))} with errors")
    for s in sorted(skips, key=lambda s: -abs(s[2] - s[3]))[:30]:
        print(f"  {s[0]} vs {s[1]}: skipped {s[2]} -> {s[3]}")


if __name__ == "__main__":
    main()
