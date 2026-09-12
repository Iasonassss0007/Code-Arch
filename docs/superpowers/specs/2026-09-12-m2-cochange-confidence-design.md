# M2 — Git Co-change and the Confidence Model

Date: 2026-09-12
Status: implemented except the live blind run. Exit criteria 1–5 met (5 as a
fixture-replayed pipeline); criterion 6 recorded in `code_arch_architecture.md`.
The live A/B (`cochange_review.py --live`) waits on provider credit.
Implements: `code_arch_architecture.md` stage 4, the 0.55 co-change weight in
stage 5, the churn term in stage 7, and the Confidence Model section.
Evidence: co-change profiling over the three eval checkouts, 2026-09-12.

## Problem

Stage 4 does not exist. `rank.rs` carries `W_CHURN * 0.0` with the comment "git
signals land at M2", `graph.rs` fuses two of four planned signals, and
`render.rs` prints a standing caveat that the map contains no co-change signal.
Confidence is a single global `resolution_rate`; there are no per-cluster bands,
so the tool cannot say "I could not determine structure here" — the failure mode
the confidence model exists to prevent.

Profiling the three eval checkouts (2000 commits each, pair kept at >=2 commits,
commits over 25 files skipped) says the signal is real and is not merely a test
artifact:

```text
repo       strong pairs   src-src   src-test   test-test
hono       1918           54.6%     36.3%       9.0%
commerce   1597          100.0%      0.0%       0.0%
typedi      403           64.0%     21.6%      14.4%
```

Hono's strongest pairs are `deno_dist/context.ts <-> src/context.ts` (81
commits) — generated mirrors of the sources. Those files never reach the
clusters, because stage 0 already classifies them, which is why co-change is
restricted to inventory files.

## Goals

- Stage 4: read git history, emit weighted co-change pairs and per-file churn.
- Fuse co-change into the graph at 0.55 and churn into importance ranking.
- Compute per-cluster confidence from resolution, stability and agreement, and
  render three bands.
- Make the with-git / without-git comparison runnable.

## Non-goals

- Rename following (`git log -M`). A renamed file's history starts over. The
  delta report will show whether it matters.
- Author or ownership signals.
- Embedding similarity (M3).
- Re-labeling the frozen reference clusters. `clusters-real.json` scores names
  against a sha256-pinned set and is untouched by this milestone.

## Design

### Stage 4 — `src/git.rs`

One subprocess: `git log --no-merges --name-only -n 2000 --pretty=...`, capped
at 24 months. `gix` and `git2` were rejected — a large dependency tree and a
libgit2 build respectively, for data one `git log` already prints.

Pair credit per commit is `1 / (files - 1)`, so a two-file commit is strong
evidence and a forty-file refactor is nearly none. Commits touching more than 50
files are dropped outright. Credit decays exponentially with age, 180-day
half-life.

A pair survives when it appears in at least 2 commits, both files are in the
inventory, and the two are not both `FileClass::Test`. Source-test pairs are
kept: they mostly duplicate an import edge, which is exactly what the agreement
term should reward. Test-test pairs are dropped; they have no import basis and
would pull a tests blob out of real subsystems.

Weights are normalized by the 99th-percentile pair weight and clamped to 1.0, so
one runaway pair cannot flatten the rest of the distribution.

Failure is local and silent in the pipeline's sense: no git binary, no `.git`,
a shallow or empty history, or a non-zero exit produces an empty `CoChange`.
The run continues on imports alone and says so.

### Stage 5 — fusion

`W_COCHANGE = 0.55` added to the fused adjacency in `graph.rs`, alongside the
existing import (1.00) and directory (0.25) weights. The co-change edge set is
retained separately, because the agreement term needs it per cluster.

### Stage 7 — churn

Per-file commit counts, log-scaled and normalized, replace the `W_CHURN * 0.0`
placeholder at its existing 0.20 weight.

### Confidence

Per cluster:

```text
resolution   resolved internal refs / attempted, restricted to members
stability    fraction of members that never migrate across 5 seeds,
             clusters matched between runs by maximum overlap
agreement    Jaccard of the cluster's import-edge set and co-change-edge set

confidence = 0.45*resolution + 0.35*stability + 0.20*agreement
```

Bands render as the architecture doc specifies: `>= 0.75` full treatment,
`0.45-0.75` no flows and marked "relationships partially inferred", `< 0.45`
files grouped by directory with no claimed relationships.

With no git history, agreement is undefined. The remaining two terms are
renormalized to sum to 1 and the header states that confidence was computed
without the co-change signal. Scoring agreement as 0 would manufacture low
confidence out of a missing input, which is the opposite of the point.

### Measurement

`--no-git` renders today's output; the default renders with the signal. The
deterministic delta report — pairs merged and split, hidden-coupling pairs
promoted, per-cluster band movement — runs free on all three checkouts.

The quality claim is a blind A/B: both maps rendered, provenance stripped, a
model judging per-cluster coherence, the same shape as
`labels-reference-review.json`. **The harness lands in this milestone; the run
is gated on provider credit** (OpenRouter returned HTTP 402 on 2026-09-11),
exactly as M1v2 criterion 4 is.

## Testing

Hermetic. `git log` output is parsed by a pure function tested against fixture
text, so the default suite needs no git binary and no network. One integration
test does `git init` in a temp directory with scripted commits.

- Parsing: commit boundaries, non-source paths, empty commits, malformed lines.
- Credit: `1/(files-1)` weighting, the 50-file skip, half-life decay,
  the 2-commit floor, test-test exclusion, inventory restriction.
- Normalization: 99th-percentile scaling, single-pair and empty inputs.
- Confidence: each term in isolation, the renormalization when git is absent,
  band boundaries at exactly 0.45 and 0.75.
- Stability: a graph with an obvious partition scores 1.0; a degenerate one does
  not.
- Determinism: two consecutive runs byte-identical; `--no-git` reproduces the
  pre-M2 output exactly.

## Exit criteria

1. `cargo test` passes: the existing tests unmodified plus the new ones.
   `python -m pytest eval` passes.
2. `--no-git` on hono, commerce and typedi reproduces the current `CODEBASE.md`
   byte for byte; with git, two consecutive runs are byte-identical and the map
   stays within 4,000 tokens.
3. The delta report exists for all three checkouts.
4. At least one low-confidence region is emitted on the checkout with the worst
   import resolution (commerce, 76%). If none appears anywhere, the banding is
   not measuring anything and that gets written down.
5. The blind A/B harness runs end to end against a recorded fixture. The live
   run waits on provider credit.
6. Results go into `code_arch_architecture.md` whether they are a win or a loss.

## Risks

**Co-change makes the map worse.** 16% of hono's strong pairs match an import
edge; the rest are coupling the graph has never seen, and some of it is noise.
The `--no-git` flag and the delta report exist so the change is reversible and
visible rather than assumed.

**Stability is measured against the same algorithm that produced the clusters.**
It detects boundary cases, not wrong boundaries. It is 0.35 of the score, so a
confidently-stable-but-wrong partition still reads as high confidence. The blind
A/B is the only check on that, and it is credit-gated.

**Shallow clones return nothing.** Two of the three checkouts were deepened on
2026-09-11 to make this measurable at all. Anyone re-cloning shallow gets the
degraded path, which is correct but easy to mistake for a bug.
