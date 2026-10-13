#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Writes the sample-robot corpus plan for run-corpus.py: every classic sample robot over
more than 1000 seeded rounds (design §16, M1 exit criterion).

  sample-plan.py > samples-plan.json
  run-corpus.py --manifest tools/difftest/corpus/samples-manifest.json \\
      --jars robots/samples/build/libs --plan samples-plan.json --out DIR ...

Every pair of ordinary robots fights a duel; melees, sentry and team battles cover the rest.
"""
import itertools, json, random

ROBOTS = [f"sample.{n}" for n in [
    "Corners", "Crazy", "Fire", "Interactive", "Interactive_v2", "MyFirstJuniorRobot",
    "MyFirstRobot", "PaintingRobot", "RamFire", "SittingDuck", "SpinBot", "Target",
    "TrackFire", "Tracker", "VelociRobot", "Walls"]] + [
    "sampleex.Alien", "sampleex.MasterAndSlave", "sampleex.ProxyOfGreyEminence"]
SENTRY = "samplesentry.BorderGuard"
TEAM = "sampleteam.MyFirstTeam"
SEED = 1100

def main():
    rng = random.Random(SEED)
    battles = []
    def add(kind, robots, rounds):
        battles.append({"name": f"{len(battles):03d}-{kind}", "robots": robots,
                        "rounds": rounds, "seed": SEED * 1000 + len(battles)})
    for pair in itertools.combinations(ROBOTS, 2):
        add("duel", list(pair), 10)
    for _ in range(6):
        add("melee", rng.sample(ROBOTS, 10), 10)
    for _ in range(5):
        add("sentry", [SENTRY] + rng.sample(ROBOTS, 3), 10)
    for _ in range(5):
        add("teams", [TEAM] + rng.sample(ROBOTS, 2), 10)
    print(json.dumps(battles, indent=1))

if __name__ == "__main__":
    main()
