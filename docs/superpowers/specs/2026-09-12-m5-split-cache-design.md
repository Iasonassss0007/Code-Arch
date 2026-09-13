# M5 — Hierarchical Split and Incremental Cache

Date: 2026-09-12
Status: approved, in implementation.
Implements: `code_arch_architecture.md` Decision 2 (split by resolution) and
the M5 milestone (split + incremental). Vehicle: `facebook/react` pinned at
019019b (4,367 analyzed files) in gitignored `eval/repos/react`.
Evidence: before-state measured 2026-09-12 (see Problem).

## Problem

React at HEAD renders a 236,702-token "map" against a 4,000 budget in 145s:

```text
4367 files → 2133 domains (1945 singletons, 2124 isolated) → ladder bottoms
out at 1 file/domain → 236k tokens. Flows dropped. Full run 145s, --no-git 27s.
CODEARCH_TIME stages (--no-git): inventory 12.8s, parse 4.5s, label 4.1s,
render 2.8s, stability 0.9s, cluster 0.2s. Git stage ≈ 118s (subprocess 0.7s;
pair accumulation explodes).
```

Two failures, matching the two halves of M5 exactly: no split machinery
(flat map or nothing), and no cache machinery (every stage re-runs).
Singletons concentrate in `compiler/` (1,674 of 1,945), so directory buckets
for the tail converge to a handful of pointers.

## Goals

- Budget-triggered split: flat map exceeding budget → root map (≤12 full
  domains + compact bucket pointers + routing table) + `domains/*.md`.
- Re-run after a small change in seconds via `.codearch/cache/`.
- Small repos byte-identical: no split, no behavior change when flat fits.

## Non-goals

- First-run speed. React's first 145s (git pair explosion included) stands;
  the exit is re-run cost and root budget. A pair-budget cap for stage 4 is
  future work, noted not hidden.
- 40–120 fine subsystems on disconnected graphs. React's honest fine
  structure is thousands of units; the contract's numbers assume connected
  repos. Reported as measured.
- Transitive closure, cross-cluster flows (standing rejections).
- Python impact tasks / live agents (credit-gated, out of scope).

## Design

### Split trigger

Render the flat map first; split iff its tokens exceed budget. Deterministic
(a pure function of content), no size rule, no flag. Fixtures and
hono/commerce/typedi never trigger it; their maps must be byte-identical
before/after M5 (asserted, not assumed).

### Fine partition

`partition_targeting(g, 120, seed)` — the contract's second run, same
algorithm, same confidence machinery. Actual N reported; on react it will
exceed 120 and that is written down.

### Coarse partition (the Q1 decision)

`partition(g, 0.5, seed)` + `merge_small`: connected communities merge big
at low gamma while edgeless files stay single — exactly the split the graph
supports. Full means connected TO THE WORLD (external edge weight > 0), not
merely internally coherent: an isolated 200-file component rides a bucket
(its fine subsections still render there) rather than occupying a root slot,
and over-cap connected units merge smallest-first by strongest edge
(terminates: connected units always have an edge to follow; reported if it
cannot fit).

The unmergeable tail — units with no outward edges, singletons included —
goes to **directory buckets**, rendered with the existing low-band machinery
(files grouped by directory, no claimed relationships):
one bucket per top-level directory (`(root)` for top-level files), unbounded
in size. Subdivision was considered and rejected: splitting a 1,674-file
`compiler/` bucket by segment only produces either still-huge buckets or
file-pointers — a listing wearing a hierarchy costume. A bucket is a pointer
however large; the domain file behind it truncates honestly under its budget
share.

Nesting: fine → coarse by majority files (tie → smallest coarse id).

### Root map

Per full unit: name, summary, top-3 files, confidence, pointer to its domain
file. Buckets: one line each (`directory, N files → domains/<slug>.md`).
Then:

```md
## Routing

subscription, checkout, invoice, stripe, plan   → .codearch/domains/billing.md
```

Keywords per unit: top identifier terms (top_symbols + file stems,
lowercased, split) minus terms in >1 unit, top 8 by frequency (alpha
tiebreak). Task Navigation rows point at domain files. Imports section,
Stack, header unchanged. Overflow guard: routing terms truncate first; a
test forces the split at tiny budget and asserts ≤ budget.

### Domain files

`domains/<slug>.md` per full unit AND per bucket: name, summary (buckets:
directory-grouped statement instead), band, ranked file list under the
budget share, flows, coarse-level depends, fine subsections (connected fine
clusters with ≥2 files as named subsections; singleton members folded into
a by-directory listing — no 500-subsection files).

Budget shares per contract: `size × log(1 + churn)`, floor 300, cap 1200
tokens; without git history, size alone.

### index.json (additive)

`"hierarchy": {"coarse": [{id, name, fine: [ids], bucket: bool}],
"parent": [coarse_idx per fine cluster]}`. Flat `clusters` retained
byte-compatible for eval tooling. `imports.md` unchanged. `domains/` only
exists when split.

### Cache (the Q2 decision: parse + git + LLM-labels)

`.codearch/cache/store.json` (single file, tmp+rename atomicity), plus
`.codearch/.gitignore` created-if-absent containing `cache/`. New `sha2`
dependency for content hashing.
```text
version:      FORMAT const; mismatch → cache ignored wholesale
files:        rel → {mtime_ns, size, hash, loc, class, language,
                    symbols, refs, partial, parse_hash}
git:          {head, pairs: [[rel, rel, w]], churn: {rel: n}, commits_read}
labels_llm:   {evidence_hash: {name, summary}}
split_decision: {fingerprint, split}
```

Inventory stats every file but reads only on (mtime_ns, size) miss;
unchanged files reuse stored hash → parse-cache hit without reading.
Changed files are read, rehashed, and parsed only on hash miss. Git reuses
stored pairs/churn (mapped through the current inventory, missing dropped)
when HEAD matches. Labels: LLM path only, keyed by evidence hash
(summary fields + labeler id + model id); derived always recomputes
(milliseconds per domain — one labeling path, no subset-dedup divergence).

`parse_hash` (the content hash the stored parse was built from) guards the
crash window: inventory refreshes `hash` before parse runs, so only a
matching pair is a hit. A store saved by a crashed run re-parses, never
serves stale.

`split_decision` memoizes the verdict (flat render or split) keyed by a
fingerprint of file rels+hashes, git HEAD, budget, domain cap, seed,
labeler and model id. Same fingerprint decides identically without
rebuilding the flat map to measure it — this is what removes the 236k-token
probe render from warm re-runs. Any content, history or option change
re-probes exactly. Labels still run uniformly (subsection names need them),
which is why the memo saves the render, not the labeling.

Cache hits reproduce stored values bit-for-bit: cached runs are
byte-identical to uncached ones (tested, both directions).

### CLI

No new flags. Split is a budget outcome; cache lives under the existing
`--codearch-dir`. `--no-git` zeroes the git term as before (churn share
falls back to size).

## Measurement

1. React: root ≤ 4,000 tokens, domain files sane, routing terms
   discriminative (spot-checked), fine-N reported.
2. Re-run with no changes: ~145s cold-disk → ~10s warm (single-digit on a
   quiet disk; this machine meters 9–14s), map byte-identical to the full
   run. The remaining fat is uniform labeling (compatibility over cleverness)
   and disk speed (environment), neither worth correctness risk.
3. No-split regression: hono/commerce/typedi maps byte-identical to pre-M5
   binaries; fixtures + M1 harness + ceiling unchanged.
4. `cargo test` (existing unmodified + new) and `python -m pytest eval`.
5. Results into `code_arch_architecture.md` win or loss.

## Testing

Hermetic. Split forced by tiny `--budget` on synthetic repos (existing
ladder-test pattern):

- Trigger: over-budget fixture splits, fitting fixture does not.
- Buckets: disconnected files land in directory buckets marked low, never
  in a named community; isolated-but-internally-connected blobs bucket too
  (Full faces outward), keeping fine subsections.
- Nesting: majority rule over post-reorder summary positions (partition ids
  are meaningless after `reorder_by_importance`; the file set is the join
  key), deterministic tiebreak; every fine cluster has exactly one parent.
- Routing: terms unique across units, never dotted or extensions; overflow
  truncates terms first.
- index.json: hierarchy present iff split; flat clusters unchanged.
- Cache: version mismatch ignored; mtime/size hit skips reads; hash change
  re-parses; HEAD change re-collects git; missing files drop from stored
  pairs/churn; cached run byte-identical to clean run; fingerprint change
  re-probes, fingerprint match skips the probe.
- `.gitignore` created-if-absent, never clobbered.

## Exit criteria

1. `cargo test` + `python -m pytest eval` pass.
2. React root map ≤ 4,000 tokens routing to domain files; re-run in seconds.
3. No-split repos byte-identical to pre-M5 output.
4. Results recorded win or loss.

## Risks

**Buckets dominate the root map.** If a repo is 90% disconnected tail, the
root is mostly pointers. That is the honest shape of such a repo (the flat
map was 2,133 domains); the routing table still routes. Recorded per repo.

**mtime/size trust.** A change preserving both (same-byte rewrite aside,
clock granularity) reuses stale parse. Content hash is checked on read;
unchanged-stat files are never read, so a same-size same-mtime content
change is missed until mtime moves — the standard make-class tradeoff,
documented here.

**Cache grows without bound.** One JSON object; react-scale stores are a few
MB. No eviction in M5; file count is bounded by repo size. Stated.
