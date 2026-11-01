#!/usr/bin/env bash
# SPDX-License-Identifier: EPL-2.0
#
# Records a fixed corpus of classic battles with the sample robots, and with test robots
# that skip turns, for the engine replay test (crates/difftest).
# Usage: tools/difftest/record-samples.sh <output dir> [extra jars]
# Requires ./gradlew :tools:difftest:installDist :robots:samples:jar :robots:testbots:jar
#   :clients:java-determinism:jar
set -euo pipefail

out=${1:?usage: record-samples.sh <output dir> [extra robot jars, path list]}
jars=${2:-}
root=$(cd "$(dirname "$0")/../.." && pwd)
lib=$root/tools/difftest/build/install/difftest/lib
mkdir -p "$out"

java_args=(
  -Xmx512M
  --add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED
  --add-opens=java.base/java.lang.reflect=ALL-UNNAMED
  -XX:+UnlockDiagnosticVMOptions
  -XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow
  -Ddebug=true
  "-javaagent:$(ls "$root"/clients/java-determinism/build/libs/java-determinism-*.jar)"
  "-javaagent:$(ls "$lib"/difftest-*.jar)"
  "-Drobocodes.difftest.robotJars=$(ls "$root"/robots/samples/build/libs/samples-*.jar):$(ls "$root"/robots/testbots/build/libs/testbots-*.jar)"
  -cp "$lib/*"
  io.github.nhofsted.robocodes.difftest.Oracle
)

record() { # name robots seed rounds
  local extra=()
  if [[ -n "$jars" ]]; then extra=(--jars "$jars"); fi
  java "${java_args[@]}" --robots "$2" --seed "$3" --rounds "$4" "${extra[@]}" \
    --out "$out/$1.jsonl.gz" >/dev/null 2>"$out/$1.log" || { echo "oracle failed: $1" >&2; return 1; }
}

pids=()
record duck-walls sample.SittingDuck,sample.Walls 1 3 & pids+=($!)
record spinbot-tracker sample.SpinBot,sample.Tracker 2 5 & pids+=($!)
record ramfire-crazy sample.RamFire,sample.Crazy 3 5 & pids+=($!)
record fire-trackfire sample.Fire,sample.TrackFire 4 5 & pids+=($!)
record corners-velocirobot sample.Corners,sample.VelociRobot 5 5 & pids+=($!)
record melee-samples sample.Corners,sample.Crazy,sample.Fire,sample.RamFire,sample.SpinBot,sample.Tracker,sample.TrackFire,sample.Walls,sample.SittingDuck,sample.VelociRobot 6 5 & pids+=($!)
record team-vs-walls sampleteam.MyFirstTeam,sample.Walls,sample.SpinBot 7 5 & pids+=($!)
record sentry samplesentry.BorderGuard,sample.Walls,sample.Crazy,sample.Tracker 8 5 & pids+=($!)
record junior-target sample.MyFirstJuniorRobot,sample.Target,sample.PaintingRobot 9 5 & pids+=($!)
# Robots that sleep on their main thread skip turns (ADR 0025); Oversleeper is disabled for it.
record sleeper-crazy testbots.Sleeper,sample.Crazy 11 10 & pids+=($!)
record oversleeper-fire testbots.Oversleeper,sample.Fire 12 6 & pids+=($!)
status=0
for pid in "${pids[@]}"; do wait "$pid" || status=1; done
exit $status
