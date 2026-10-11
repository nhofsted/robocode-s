#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Fights a seeded plan of RoboRumble battles in classic Robocode (oracle) and live in
robocode-s, and compares every turn (#11, design §13).

  run-corpus.py --manifest corpus.json --jars DIR --out DIR [--jobs N] [--seed N]

Needs: ./gradlew :tools:difftest:installDist :robots:samples:jar \\
         :clients:java-determinism:jar :clients:java-compat:installDist
       cargo build --release -p robocode-s-difftest
The oracle uses `java` from PATH; the live battles use ROBOCODES_JAVA if set, else the
pinned Temurin JRE (a different JVM build is part of the test).
"""
import argparse, concurrent.futures, gzip, itertools, json, os, random, re, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def plan(manifest, seed, duels, melees, team_battles):
    rng = random.Random(seed)
    # Excluded robots stay in the manifest with the reason, e.g. robots whose battles
    # are not reproducible in classic itself.
    robots = [r for r in manifest["robots"] if "exclude" not in r]
    pool = lambda p: [f"{r['name']} {r['version']}" for r in robots if p in r["pools"]]
    battles = []

    def add(kind, names, rounds):
        battles.append({"name": f"{len(battles):02d}-{kind}", "robots": names, "rounds": rounds,
                        "seed": seed * 1000 + len(battles)})

    duel = pool("duel")
    order = duel[:]
    rng.shuffle(order)
    pairs = [order[i:i + 2] for i in range(0, len(order) - 1, 2)]
    while len(pairs) < duels:
        pairs.append(rng.sample(duel, 2))
    for p in pairs[:duels]:
        add("duel", p, 35)
    melee = pool("melee")
    order = melee[:]
    rng.shuffle(order)
    groups = [order[i:i + 10] for i in range(0, len(order) - 9, 10)]
    while len(groups) < melees:
        groups.append(rng.sample(melee, 10))
    for g in groups[:melees]:
        add("melee", g, 35)
    teams = pool("teams")
    for _ in range(team_battles):
        add("teams", rng.sample(teams, 2), 10)
    return battles


def oracle(battle, jars, recordings, suffix=""):
    out = recordings / f"{battle['name']}{suffix}.jsonl.gz"
    if out.exists():
        return out, None
    lib = ROOT / "tools/difftest/build/install/difftest/lib"
    agent = next((ROOT / "clients/java-determinism/build/libs").glob("java-determinism-*.jar"))
    samples = next((ROOT / "robots/samples/build/libs").glob("samples-*.jar"))
    cmd = ["java", "-Xmx1g",
           "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
           "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
           "-XX:+UnlockDiagnosticVMOptions", "-XX:-InlineMathNatives",
           "-XX:+UnlockExperimentalVMOptions", "-XX:hashCode=2", "-Ddebug=true",
           f"-javaagent:{agent}", f"-javaagent:{next(lib.glob('difftest-*.jar'))}",
           f"-Drobocodes.difftest.robotJars={samples}", "-cp", f"{lib}/*",
           "io.github.nhofsted.robocodes.difftest.Oracle",
           "--robots", ",".join(battle["robots"]), "--seed", str(battle["seed"]),
           "--rounds", str(battle["rounds"]), "--jars", os.pathsep.join(map(str, jars)),
           "--out", str(out.with_suffix(".tmp.gz"))]
    log = recordings / f"{battle['name']}{suffix}.oracle.log"
    with open(log, "w") as f:
        r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=f)
    if r.returncode != 0:
        return None, f"oracle failed (exit {r.returncode}), see {log}"
    out.with_suffix(".tmp.gz").rename(out)
    return out, None


def same_recording(a, b):
    """Where two classic recordings of a battle differ, or None. A dead robot's energy is
    not compared (a known classic race, engine-spec §15.3); a difference after such a race
    in the same round is reported as a race."""
    def turns(path):
        with gzip.open(path, "rt", encoding="utf-8") as f:
            for line in f:
                d = json.loads(line)
                if d.get("type") in ("roundStart", "turn"):
                    yield d

    def race(d, seen):
        for i, r in enumerate(d.get("robots", [])):
            if r["state"] == "DEAD" and r["energy"] > 0:
                seen.setdefault(d["round"], (i, d["turn"]))
        return seen.get(d["round"])

    def normalized(d):
        d = dict(d)
        d["robots"] = [{k: v for k, v in r.items() if not (r["state"] == "DEAD" and k == "energy")}
                       for r in d["robots"]]
        return d

    seen_a, seen_b = {}, {}
    for x, y in itertools.zip_longest(turns(a), turns(b)):
        if x is None or y is None:
            return "ORACLE-NONDETERMINISTIC: one recording is longer"
        known = race(x, seen_a) or race(y, seen_b)
        if normalized(x) != normalized(y):
            where = f"round {x['round']} turn {x['turn']}"
            if known:
                return (f"RACE: classic race: robot {known[0]} was dead with energy at turn {known[1]}; "
                        f"two classic runs differ after it; round {x['round']} turn {x['turn']}")
            return f"ORACLE-NONDETERMINISTIC: two classic runs differ at {where}"
    return None


def live(recording, jars, out):
    env = dict(os.environ)
    env["ROBOCODES_ROBOT_JARS"] = os.pathsep.join(map(str, jars))
    env.setdefault("ROBOCODES_DATA", str(out / "data"))
    env["ROBOCODES_DATA"] = str(Path(env["ROBOCODES_DATA"]) / recording.stem)
    r = subprocess.run([str(ROOT / "target/release/live"), str(recording)], cwd=ROOT, env=env,
                       capture_output=True, text=True)
    return (r.stdout + r.stderr).strip()


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--manifest", required=True)
    p.add_argument("--jars", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--seed", type=int, default=11)
    p.add_argument("--duels", type=int, default=28)
    p.add_argument("--melees", type=int, default=4)
    p.add_argument("--team-battles", type=int, default=4)
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 4) // 2))
    p.add_argument("--check-oracle", action="store_true",
                   help="record every battle twice in classic and require identical recordings")
    args = p.parse_args()
    manifest = json.loads(Path(args.manifest).read_text())
    jar_dir = Path(args.jars).resolve()
    jars = [jar_dir / r["file"] for r in manifest["robots"]]
    out = Path(args.out).resolve()
    recordings = out / "recordings"
    recordings.mkdir(parents=True, exist_ok=True)
    battles = plan(manifest, args.seed, args.duels, args.melees, args.team_battles)
    (out / "plan.json").write_text(json.dumps(battles, indent=1) + "\n")
    total_rounds = sum(b["rounds"] for b in battles)
    print(f"{len(battles)} battles, {total_rounds} rounds; {args.jobs} jobs", flush=True)

    results = {}
    start = time.time()

    def run(battle):
        recording, error = oracle(battle, jars, recordings)
        if error:
            return battle["name"], "ORACLE-FAIL " + error
        if args.check_oracle:
            again, error = oracle(battle, jars, recordings, ".check")
            if error:
                return battle["name"], "ORACLE-FAIL " + error
            differs = same_recording(recording, again)
            if differs:
                return battle["name"], differs
        return battle["name"], live(recording, jars, out)

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for name, result in pool.map(run, battles):
            results[name] = result
            first = result.splitlines()[0] if result else "(no output)"
            print(f"[{time.time() - start:6.0f}s] {name}: {first[:300]}", flush=True)
    (out / "summary.json").write_text(json.dumps(results, indent=1) + "\n")
    verified, kinds = 0, {}
    for b in battles:
        result = results[b["name"]]
        kind = result.split(" ", 1)[0].rstrip(":")
        kinds[kind] = kinds.get(kind, 0) + 1
        if kind == "OK":
            verified += b["rounds"]
        elif kind == "RACE":
            # A known classic race (engine-spec §15.3): the rounds before it count.
            m = re.search(r"; round (\d+) turn", result)
            verified += int(m.group(1)) if m else 0
    print(f"\n{len(battles)} battles: " + ", ".join(f"{n} {k}" for k, n in sorted(kinds.items())))
    print(f"{verified} of {total_rounds} rounds verified identical; details in {out}")
    return 0 if kinds.get("OK", 0) + kinds.get("RACE", 0) == len(battles) else 1


if __name__ == "__main__":
    sys.exit(main())
