#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Fits the work meter's prices per kind of work (#15, ADR 0014) to calibration samples.

  fit-costs.py <out>/samples.jsonl [...] [--min-work N] [--jdk-methods N] [--percentile P] [--json FILE]

Reads the samples calibrate.py writes (each robot's turns, real CPU time and work profile:
instructions by WorkProfile group, JDK calls by method, the static count), and fits

    cpu = a * turns + sum(price[g] * instructions[g]) + sum(price[m] * calls[m]) + price_sized * sized

without negative prices, weighting each robot by its relative error (a light robot counts as
much as a heavy one), with a mild pull of each price towards a prior (--ridge), so that
kinds the samples can't tell apart don't get absurd prices. `sized` is the work charged for
sized JDK calls (fuel minus the static count). The JDK methods called most get prices of
their own; groups and methods too rare to estimate (--min-share) share `other`.

Then compares, per robot doing at least --min-work counted instructions a turn, its real CPU
time with what each model charges: today's flat price (--flat ps per instruction) and the
fit, out of fold (--folds, by battle: a robot's ratio comes from prices fitted without its
battle). A robot charged more than it took skips turns it wouldn't in classic. The generous
scale makes the fit charge no more than the real time for all but --percentile percent of
them; --json writes the scaled prices.
"""
import argparse, json, math, sys
from collections import Counter

PRIOR_INSTRUCTION = 100.0  # ps
PRIOR_JDK_CALL = 11 * 160.0  # today's price of a JDK call: 1 + JDK_CALL_COST instructions
PRIOR_TURN = 64_000.0  # ns: classic's robot-side work per turn (ADR 0014)


def load(paths):
    rows = []
    for path in paths:
        for line in open(path):
            r = json.loads(line)
            if r["profile"]:
                rows.append(r)
    return rows


def features(rows, jdk_methods, min_share):
    total = sum(r["fuel"] for r in rows) or 1
    named = ("jdk:", "sized:", "alloc:")
    groups, calls, sized = Counter(), Counter(), Counter()
    for r in rows:
        for k, n in r["profile"].items():
            if k.startswith("jdk:"):
                calls[k] += n
            elif k.startswith(("sized:", "alloc:")):
                sized[k] += n
            elif k != "static":
                groups[k] += n
    kept_groups = sorted(g for g, n in groups.items() if n >= min_share * total)
    methods = [m for m, n in calls.most_common(jdk_methods) if n * 11 >= min_share * total]
    # Sized calls and allocated elements: those with enough units get prices of their own.
    kept_sized = sorted(k for k, n in sized.items() if n >= min_share * total)
    names = ["turn"] + kept_groups + ["other"] + methods + kept_sized + ["sized_other"]
    priors = ([PRIOR_TURN] + [PRIOR_INSTRUCTION] * (len(kept_groups) + 1)
              + [PRIOR_JDK_CALL] * len(methods) + [PRIOR_INSTRUCTION] * (len(kept_sized) + 1))

    def vector(r):
        p = r["profile"]
        own = [p.get(g, 0) for g in kept_groups]
        if "jdk_call" in kept_groups:
            # Calls with a price of their own leave the shared group.
            own[kept_groups.index("jdk_call")] -= sum(p.get(m, 0) for m in methods)
        other = sum(n for g, n in p.items() if not g.startswith(named) and g != "static" and g not in kept_groups)
        sized_other = sum(n for k, n in p.items() if k.startswith(("sized:", "alloc:")) and k not in kept_sized)
        return ([r["turns"]] + own + [other] + [p.get(m, 0) for m in methods]
                + [p.get(k, 0) for k in kept_sized] + [sized_other])

    # Priors in ns per unit, as the fit's units (cpu is in ns).
    return names, [x / 1000 if i else x for i, x in enumerate(priors)], [vector(r) for r in rows]


def nnls(xs, ys, weights, priors, ridge, iterations=3000):
    """Non-negative weighted least squares with a ridge towards `priors` (relative), by
    coordinate descent: few features, no numpy."""
    n = len(xs[0])
    rows = [[w * v for v in x] for x, w in zip(xs, weights)]
    targets = [w * y for y, w in zip(ys, weights)]
    # The ridge: a pseudo-row per price, penalizing ((beta - prior) / prior)^2 * ridge.
    for j in range(n):
        k = math.sqrt(ridge) / priors[j]
        rows.append([k if i == j else 0.0 for i in range(n)])
        targets.append(k * priors[j])
    scale = [math.sqrt(sum(r[j] ** 2 for r in rows)) or 1 for j in range(n)]
    a = [[r[j] / scale[j] for j in range(n)] for r in rows]
    beta = [priors[j] * scale[j] for j in range(n)]
    resid = [t - sum(c * b for c, b in zip(row, beta)) for row, t in zip(a, targets)]
    cols = [[row[j] for row in a] for j in range(n)]
    norms = [sum(c * c for c in col) for col in cols]
    for _ in range(iterations):
        change = 0.0
        for j in range(n):
            if norms[j] == 0:
                continue
            step = sum(c * r for c, r in zip(cols[j], resid)) / norms[j]
            new = max(0.0, beta[j] + step)
            d = new - beta[j]
            if d:
                beta[j] = new
                for i, c in enumerate(cols[j]):
                    if c:
                        resid[i] -= d * c
                change = max(change, abs(d) / (abs(new) + 1e-30))
        if change < 1e-9:
            break
    return [beta[j] / scale[j] for j in range(n)]


def quantile(values, q):
    v = sorted(values)
    return v[min(len(v) - 1, max(0, int(q * len(v))))]


def summary(label, ratios):
    return (f"  {label:9s} min {min(ratios):.2f}  p5 {quantile(ratios, .05):.2f}  p25 {quantile(ratios, .25):.2f}"
            f"  median {quantile(ratios, .5):.2f}  p75 {quantile(ratios, .75):.2f}  max {max(ratios):.2f}"
            f"  spread p95/p5 {quantile(ratios, .95) / quantile(ratios, .05):.1f}x")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("samples", nargs="+")
    p.add_argument("--min-work", type=float, default=100_000, help="counted instructions a turn for the comparison")
    p.add_argument("--jdk-methods", type=int, default=24, help="JDK methods with a price of their own")
    p.add_argument("--min-share", type=float, default=1e-4, help="share of all work a kind needs for a price")
    p.add_argument("--ridge", type=float, default=0.02, help="pull towards the priors")
    p.add_argument("--folds", type=int, default=5)
    p.add_argument("--percentile", type=float, default=5, help="robots the generous prices may still overcharge, in %%")
    p.add_argument("--flat", type=float, default=160, help="today's price, ps per counted instruction")
    p.add_argument("--json", help="write the scaled prices here")
    args = p.parse_args()
    rows = load(args.samples)
    if len(rows) < 3:
        sys.exit("not enough samples with profiles")
    names, priors, xs = features(rows, args.jdk_methods, args.min_share)
    ys = [r["cpu"] for r in rows]
    weights = [1 / max(y, 1.0) for y in ys]
    beta = nnls(xs, ys, weights, priors, args.ridge)
    print(f"{len(rows)} robots, {len(names)} prices; per turn {beta[0] / 1000:.1f} us")
    totals = [sum(x[j] for x in xs) for j in range(len(names))]
    for j, name in enumerate(names[1:], 1):
        print(f"  {name:70s} {beta[j] * 1000:9.1f} ps   ({totals[j]:.3g} in all samples)")

    def own_cpu(r):
        # The robot's own work: CPU time less the per-turn part (classic's, not counted).
        return max(1.0, r["cpu"] - beta[0] * r["turns"])

    def charged(b, x):
        return max(1.0, sum(bj * v for bj, v in zip(b[1:], x[1:])))

    heavy = [i for i, r in enumerate(rows) if r["fuel"] / r["turns"] >= args.min_work]
    if not heavy:
        sys.exit(f"no robot does {args.min_work:.0f} instructions a turn")
    # Out of fold: each battle's robots are rated by prices fitted without that battle.
    battles = sorted({r["battle"] for r in rows})
    fold_of = {b: i % args.folds for i, b in enumerate(battles)}
    out_of_fold = {}
    for f in range(args.folds):
        train = [i for i, r in enumerate(rows) if fold_of[r["battle"]] != f]
        b = nnls([xs[i] for i in train], [ys[i] for i in train], [weights[i] for i in train], priors, args.ridge)
        for i in heavy:
            if fold_of[rows[i]["battle"]] == f:
                out_of_fold[i] = own_cpu(rows[i]) / charged(b, xs[i])
    flat = {i: own_cpu(rows[i]) / (rows[i]["fuel"] * args.flat / 1000) for i in heavy}
    fit = {i: own_cpu(rows[i]) / charged(beta, xs[i]) for i in heavy}
    print(f"\nreal / charged, {len(heavy)} robots doing {args.min_work:.0f}+ instructions a turn "
          "(below 1: charged too much):")
    print(summary("flat", list(flat.values())))
    print(summary("fit", list(fit.values())))
    print(summary("held out", list(out_of_fold.values())))
    # The benchmarks are each the only sample of their kind: held out, the fit can't know
    # them. What matters is the rumble's robots.
    robots = {i: v for i, v in out_of_fold.items() if not rows[i]["robot"].startswith("calib.")}
    if robots:
        print(summary("robots", list(robots.values())) + f"  ({len(robots)}, held out, no benchmarks)")
        print(summary("flat", [flat[i] for i in robots]) + "  (the same robots)")
        out_of_fold = robots
    scale = quantile(list(out_of_fold.values()), args.percentile / 100)
    print(f"\ngenerous scale (held-out p{args.percentile:g}): {scale:.3f}: the fit's prices times this overcharge "
          f"{sum(v < scale for v in out_of_fold.values())} of {len(out_of_fold)} held-out robots")
    print("most overcharged, held out (real/fit, real/flat):")
    for i in sorted(out_of_fold, key=out_of_fold.get)[:15]:
        r = rows[i]
        print(f"  {out_of_fold[i]:5.2f} {flat[i]:5.2f}  {r['robot']} ({r['battle']}, {r['fuel'] / r['turns'] / 1e6:.2f}M a turn)")
    if args.json:
        prices = {name: beta[j] * 1000 * scale for j, name in enumerate(names) if j}
        json.dump({"psPerUnit": prices, "turnNanos": beta[0], "scale": scale, "robots": len(rows)},
                  open(args.json, "w"), indent=1)


if __name__ == "__main__":
    main()
