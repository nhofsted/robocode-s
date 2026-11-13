# 0029. The style map: a fitted intransitive model, laid out with t-SNE

- Status: Proposed
- Date: 2026-11-13

## Context

LiteRumble has a cluster map (`ClusterMap?game=roborumble`): bots near each other "have similar strengths and weaknesses". It takes each pairing's difference from the score the two bots' overall strengths predict, adds survival at half weight, calls two bots similar when they are unusual against the same opponents (with a small nudge for equally consistent bots), links each bot to its 10 most similar, and lays the graph out with t-SNE. The exact expected score, similarity and t-SNE settings aren't given, and the layout changes from run to run.

The owner asked for the same, more rigorously. robocode-s's pairing matrix is not complete like LiteRumble's: on 2026-11-13 the 1v1 rumble (1,205 bots) had 67% of its pairings, nearly all with one battle (the new #1 had 247 of 1,204). It will fill, but a method that needs many shared opponents per pair of bots is fragile until then.

### Measurements (1v1, prod data of 2026-11-13; scripts outside the repository)

- **Noise.** The pooled within-pairing standard deviation of one battle's score is 4.2 points (5.5 near an even score, 1.4 near 0 or 100), from the 94,637 pairings with two or more battles (`pairings.bin` keeps sums of squares).
- **Held-out pairings** (10% of the pairings, by root mean square error in score points): strengths only 12.1; with 1, 2, 4, 8, 16 intransitive planes 10.6, 9.5, 8.5, 7.7, 7.1. More planes keep helping a little; the structure past strength is real (one battle's noise alone is 4.2) and has many dimensions, so no single plane, and no 2-D linear projection, shows it all: the first two principal components of the style profiles hold 45% of their variance.
- **Layouts** of the same profiles (rank 8): a linear projection (PCA) is 0.85 trustworthy at 10 neighbours, t-SNE 0.975. Refits from other random starts give the same style distances (correlation 0.999999); t-SNE's neighbour lists agree 92.5% between two refits, the true (high-dimensional) ones 99.7%.
- **Cost** (Rust, one thread, a desktop core): a cold run of the 1,205-bot rumble 19 s (fit 166 iterations, t-SNE 1,000), a warm one from the previous model 5 s. On unchanged data a warm start moves bots by 0.5% of the map's width (median).

## Decision

A daily `map` batch (03:55 UTC, after the first publish) writes `data/v1/map/<rumble>.json` per rumble (`rumble_core::map`, `StyleMap` in `types.ts`):

- **Model.** `logit(score_XO / 100) ≈ s_X − s_O + Σ_c (a_Xc b_Oc − b_Xc a_Oc)` with 8 planes `c`, fitted by L-BFGS to all pairings of the rumble's members: binomial cross-entropy weighted by battles (a fractional logistic, so 0 and 100 need no clipping), L2 penalty 1.0 on the plane coordinates. It is the skew-symmetric ("disc", Blade-Chest) decomposition of paired comparisons: strength is the transitive part, each plane one rock-paper-scissors pattern. Survival is left out: APS ranks by score.
- **Distance.** The style distance of two bots is the root mean square, over all members as opponents, of the difference of their fitted results with strength taken out: `d² = (u_X − u_Y)ᵀ J (UᵀU / n) Jᵀ (u_X − u_Y)`, published ×25 (score points at an even match). It comes from the fitted model, not from the raw pairings two bots share, so it is denoised and defined for every pair, however sparse the matrix.
- **Layout.** Exact t-SNE (perplexity 30) of the coordinates whose Euclidean distances are those distances; started from the previous day's layout (new bots at their three nearest known neighbours), so the map moves only as results change. The file lists each bot's 10 true nearest neighbours with their distances and the layout's trustworthiness, and the page shows those, so the layout's distortions can't mislead.
- **Placement.** Bots with fewer pairings than a quarter of the other members are fitted (they inform the others) but not placed: their style is mostly the penalty's. A rumble with fewer than 20 placed bots gets no map.
- **Deterministic.** Starting values from the bot ids, fixed iteration rules, one thread, no randomness.
- **Site.** `/map/<rumble>`, reached from the bot page's Compare box ("Bots that play like X"), not from the navigation or the rankings (owner, 2026-11-13: cool, not important).

## Consequences

- The map is reproducible and explains itself: what a distance means, how much the layout keeps, and which bots are left out and why.
- The rank (8) and penalty are fixed constants, chosen once on the data above; more planes predict a little better at more cost. Re-run the cross-validation when the matrix has filled.
- Not in the model: survival, behaviour (movement, guns: the recordings could give per-bot behaviour features later) and code ancestry (shared classes in the jars). Each would be a different map, not a better one of this.
- About 20–80 s of a 900 s Lambda per day on a cold start of all rumbles, a few seconds warm; one more state file per rumble (`state/e<E>/map/<rumble>.json`, about 0.5 MB for 1v1).
