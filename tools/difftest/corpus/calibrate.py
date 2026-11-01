#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Calibrates the virtual clock (#15, ADR 0014) on this machine.

Fights battles of earlier corpus plans in classic twice: once with work metering
(counted instructions per robot), once without (the robots' real CPU time, without
the counting overhead). Robots that read the clock, or whose turns differ between the
two runs, are left out. Then fits, over all robots,

    cpu_nanos = a * turns + b * instructions

b is the time per counted instruction (PICOS_PER_INSTRUCTION = 1000 * b); a is classic's
robot-side work per turn (event dispatch, executeImpl), which isn't counted.

  calibrate.py --plan <out>/plan.json --manifest corpus.json --jars DIR --out DIR [--only 00-duel,...]
               [--extra-jar robots/testbots/build/libs/testbots-*.jar] [--jobs N]

The metered run also writes each robot's work profile (WorkProfile: instructions by kind,
JDK calls by method); `samples.jsonl` keeps it with the CPU time for fit-costs.py, which
fits a price per kind. Neither run skips turns, so heavy robots aren't left out.

Run it on an otherwise idle machine: CPU time is measured per robot thread, but caches
and frequency scaling still feel the load.
"""
import argparse, concurrent.futures, json, os, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def oracle(battle, jars, out, meter):
    lib = ROOT / "tools/difftest/build/install/difftest/lib"
    agent = next((ROOT / "clients/java-determinism/build/libs").glob("java-determinism-*.jar"))
    samples = next((ROOT / "robots/samples/build/libs").glob("samples-*.jar"))
    report = out / f"{battle['name']}.{'metered' if meter else 'cpu'}.tsv"
    if report.exists():
        return report
    cmd = ["java", "-Xmx1g",
           "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
           "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
           "-XX:+UnlockDiagnosticVMOptions",
           "-XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow", "-Ddebug=true",
           f"-Drobocodes.calibrate={report.with_suffix('.tmp')}", f"-Drobocodes.meterWork={str(meter).lower()}",
           f"-javaagent:{agent}", f"-javaagent:{next(lib.glob('difftest-*.jar'))}",
           f"-Drobocodes.difftest.robotJars={samples}", "-cp", f"{lib}/*",
           "io.github.nhofsted.robocodes.difftest.Oracle",
           "--robots", ",".join(battle["robots"]), "--seed", str(battle["seed"]),
           "--rounds", str(battle["rounds"]), "--jars", os.pathsep.join(map(str, jars)),
           "--out", str(out / f"{battle['name']}.{'metered' if meter else 'cpu'}.jsonl.gz")]
    report.with_suffix(".tmp").unlink(missing_ok=True)
    r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2700)
    if r.returncode != 0:
        print(f"{battle['name']}: oracle failed (exit {r.returncode})", file=sys.stderr)
        return None
    report.with_suffix(".tmp").rename(report)
    return report


def read(report):
    rows = {}
    for line in report.read_text().splitlines():
        name, turns, fuel, clock, cpu, *rest = line.split("\t")
        profile = {}
        for item in (rest[0].split(",") if rest and rest[0] else []):
            key, _, n = item.rpartition("=")
            profile[key] = int(n)
        rows[name] = (int(turns), int(fuel), int(clock), int(cpu), profile)
    return rows


def fit(samples):
    """Least squares for cpu = a * turns + b * fuel (no intercept)."""
    stt = sum(t * t for t, f, c in samples)
    sff = sum(f * f for t, f, c in samples)
    stf = sum(t * f for t, f, c in samples)
    stc = sum(t * c for t, f, c in samples)
    sfc = sum(f * c for t, f, c in samples)
    det = stt * sff - stf * stf
    return (stc * sff - sfc * stf) / det, (sfc * stt - stc * stf) / det


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--plan", required=True, action="append")
    p.add_argument("--manifest", required=True, action="append")
    p.add_argument("--jars", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--only", help="comma-separated battle names")
    p.add_argument("--limit", type=int, default=0, help="at most this many battles per plan")
    p.add_argument("--extra-jar", action="append", default=[], help="more robot jars (calibration benchmarks)")
    p.add_argument("--jobs", type=int, default=1, help="battles at a time (CPU time is per thread)")
    args = p.parse_args()
    jar_dir = Path(args.jars).resolve()
    jars = sorted({jar_dir / r["file"] for m in args.manifest for r in json.loads(Path(m).read_text())["robots"]})
    jars += [Path(j).resolve() for j in args.extra_jar]
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    battles = []
    for plan in args.plan:
        chosen = json.loads(Path(plan).read_text())
        if args.only:
            chosen = [b for b in chosen if b["name"] in args.only.split(",")]
        # Battle names repeat between plans (00-duel, ...): prefix them with the plan's run.
        prefix = Path(plan).resolve().parent.name
        chosen = [dict(b, name=f"{prefix}-{b['name']}") for b in chosen]
        if args.limit:
            chosen = chosen[:args.limit]
        battles += chosen
    samples, kept, profiles, dropped = [], [], [], 0

    def both(battle):
        return oracle(battle, jars, out, True), oracle(battle, jars, out, False)

    with concurrent.futures.ThreadPoolExecutor(max(1, args.jobs)) as pool:
        for i, (battle, (metered, cpu)) in enumerate(zip(battles, pool.map(both, battles))):
            if not metered or not cpu:
                continue
            m, c = read(metered), read(cpu)
            for name, (turns, fuel, clock, _, profile) in m.items():
                # Robots that budget their work by the clock do other work in the two runs;
                # a few reads (timing a round, logging) don't matter.
                if name not in c or clock > turns / 20 or c[name][0] != turns or turns == 0 or fuel == 0:
                    dropped += 1
                    continue
                samples.append((turns, fuel, c[name][3]))
                kept.append((battle["name"], name, turns, fuel, c[name][3]))
                profiles.append({"battle": battle["name"], "robot": name, "turns": turns, "fuel": fuel,
                                 "cpu": c[name][3], "clock": clock, "profile": profile})
            print(f"[{i + 1}/{len(battles)}] {battle['name']}: {len(samples)} robots so far", flush=True)
    if len(samples) < 2:
        sys.exit("not enough robots to fit")
    a, b = fit(samples)
    (out / "samples.tsv").write_text("".join("\t".join(map(str, k)) + "\n" for k in kept))
    (out / "samples.jsonl").write_text("".join(json.dumps(r) + "\n" for r in profiles))
    ratios = sorted(c / f for t, f, c in samples)
    print(f"\n{len(samples)} robots ({dropped} left out: clock readers or diverged)")
    print(f"fit: {a:.0f} ns per turn + {b * 1000:.0f} ps per counted instruction")
    print(f"cpu/instruction per robot: median {ratios[len(ratios) // 2] * 1000:.0f} ps, "
          f"p10 {ratios[len(ratios) // 10] * 1000:.0f}, p90 {ratios[len(ratios) * 9 // 10] * 1000:.0f}")


if __name__ == "__main__":
    main()
