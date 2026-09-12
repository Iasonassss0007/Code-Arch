# Reverse Import Index

Date: 2026-09-11
Status: implemented. Exit criteria 1–3 met, and 5 is written as a status section
in `code_arch_architecture.md`. Criterion 4 (the M1v2 agent run) is **deferred** by
the owner's decision on 2026-09-11. The OpenRouter run's first call returned
HTTP 402 (the account has no credits); nothing was spent. Free-tier agents were
then probed (Google AI Studio, NVIDIA API), and none could run the multi-turn
JSON agent loop reliably; see `eval/README.md`, "Agent providers". The
deterministic ceiling is the recorded result. Whether an agent with tools uses
the index remains unmeasured. The oracle-scope defect is fixed (madge now scans the whole checkout).
The `.mts` inventory mismatch is fixed on the harness side, but madge cannot
parse `.mts` imports; see the ceiling result below.
Extends: `code_arch_architecture.md` stage 10 (Render) and the output layout of Decision 2
Evidence: `research/codearch-representation/report/rebuild/` (the audited rebuild, not the original DOCX)

## Problem

The map cannot answer "what depends on X?".

`CODEBASE.md` carries dependency information only at domain level
(`Depends on: Core, Jsx`). Which *files* import a given file is not written down
anywhere an agent can read it. Stage 3 resolves every import to a file edge
(`Resolution.edges`), stage 5 builds `in_edges` from them, and then stage 10
throws both away.

Two measurements say this is the gap that matters:

1. **Representation QA study** (rebuilt report, category results). On direct
   dependency questions an explicit edge list scored 1.000 (n=7). The prose map
   scored 0.000 (n=10) and the file inventory scored 0.000 (n=14). No other
   category showed a structural difference that large.
2. **M1 real-agent run.** On single-file navigation the map did not help (45/48
   with vs 46/48 without, +98.8% provider tokens). That task set had no headroom:
   the path inventory already contained the answer. The replacement benchmark,
   M1v2 (`eval/tasks-nav-gated.json`), asks for transitive impact. Every task is
   gated so that returning the most-imported files scores F1 ≤ 0.35. Nobody has
   run it.

Caveats carried forward, not hidden. The QA study was single-shot with no tools,
used one answering model and had small per-category samples. It shows an edge
list *contains* dependency answers, not that an agent with tools will use one.
That second question is what M1v2 exists to measure.

## Goals

- Write every resolved internal import, reversed (imported file → importers), to
  an on-demand file an agent can open.
- Advertise it from `CODEBASE.md` inside the existing 4,000-token budget.
- Make M1v2 runnable against it, and measure the free deterministic ceiling
  before spending anything on the paid agent run.

## Non-goals

- Precomputed transitive closure. Its size is O(files × reachable files) and
  hub files reach most of the repository.
- A forward index ("what does X import"). An agent gets that by opening X.
- Import edges in `index.json`. Nothing consumes them yet.
- Stable-order rendering for the rest of the map. The research's churn
  measurement behind that proposal is a confirmed bug
  (`measure_incremental.py:56`), so there is no evidence to act on.
- Git co-change, flows, the hierarchical split. M2, M3, M5.

## Design

### Why not put the edges in `CODEBASE.md`

Hono has 960 resolved import edges. Its map is 3,126 tokens against a 4,000
budget (3,266 with the Imports section added). The full index measures 9,772
tokens, several times the headroom. A capped list does not help either. M1v2 tasks are admitted
*only* if the most-imported-files heuristic scores F1 ≤ 0.35 on them, so a list
truncated to the top hubs fails the benchmark by construction.

This is the same situation Decision 2 already resolves for domain detail: a
budgeted root file that routes to on-demand files. The index follows that
pattern.

### Output layout

```text
CODEBASE.md                  budget unchanged (4,000); gains a small Imports section
.codearch/
    index.json               unchanged
    imports.md               new, on demand, outside the root budget
```

### `CODEBASE.md` — Imports section

Placed after Domains, before Task Navigation. Counted inside the budget.

```md
## Imports

Most-imported files (number of analyzed files importing each):

- `src/context.ts` — 120
- `src/hono.ts` — 64
- …eight lines in total

Reverse import index: `.codearch/imports.md` lists, for every imported file,
each analyzed file that imports it (960 imports into 301 files, ~9772 tokens).
```

- Hubs: `IMPORT_HUBS = 8`, ordered by importer count descending, ties by path
  ascending. Files with no importers never appear.
- The token figure is measured with the same cl100k tokenizer as the budget, not
  estimated.
- With zero resolved edges (the directory-fallback path) the section reads
  `No internal imports were resolved, so there is no import index.` and names no
  file.

### `.codearch/imports.md` — format

```md
# Reverse Import Index — hono

Generated by Code Arch. For each file, every analyzed file that imports it.
960 imports into 301 files · import resolution 98% · 19 unresolved specifiers carry no edge.
External packages are not listed. Files nothing imports are omitted.

src/adapter/bun/index.ts ← src/adapter/bun/ssg.test.ts, src/preset/quick.test.ts
src/context.ts ← src/adapter/aws-lambda/conninfo.ts, src/adapter/aws-lambda/handler.ts, …
```

Why this shape:

- **One line per imported file.** A text search for a path lands on a complete
  answer for that file, without reading surrounding context.
- **Full paths, no backticks, no bullets.** Every repeated path is paid for.
  Backticks and a bullet per importer add tokens across 836 edges and carry no
  information here. The root file keeps backticks, matching its existing
  convention.
- **Lines sorted by imported-file path; importers sorted by path.** Output stays
  deterministic, and a one-file change rewrites only the lines that name that
  file.
- **Header states what is missing.** The resolution rate and unresolved count
  appear in the file itself, so an agent reading only this file still learns
  the index is incomplete where resolution failed.

Edges come from `CodeGraph::in_edges`, which is already deduplicated, sorted and
free of self-imports. This is the same edge set PageRank and fan-in use, so the
hubs, the index and importance all read one source.

### Code changes

```text
src/render.rs    ImportsIndex { markdown, imports, imported_files, tokens }
                 fn import_index(title, paths, in_edges, resolution_rate, unresolved)
                     -> ImportsIndex                                          pure
                 fn import_hubs(paths, in_edges, n) -> Vec<(String, usize)>   pure
                 fn imports_section(hubs, index) -> String                    pure
                 MapInput gains `imports` and `hubs`; build() renders the section
src/lib.rs       Options.codearch_dir; writes imports.md; RunReport gains
                 imports_path, import_edges, imports_tokens
src/main.rs      --codearch-dir <path>; prints the new file
```

The new functions take plain paths and adjacency rather than `Inventory` or
`Resolution`, so they are tested as pure functions.

### CLI

```
--codearch-dir <path>   where index.json and imports.md are written
                        default: <repo>/.codearch
```

The eval harness needs this. It generates maps from pinned checkouts, which
must stay clean, and it already passes `--no-index` for that reason.
`--no-index` keeps its meaning: it skips `index.json` only. `imports.md` is
always written, because `CODEBASE.md` points at it.

The pointer text always names the canonical `.codearch/imports.md`. A relocated
directory is a tooling concern; the harness serves the file back under the
canonical path.

### Failure states

| Failure | Behavior |
|---|---|
| No resolved import edges | Section states it; `imports.md` written with header and zero lines |
| Imported file with hundreds of importers | Written in full. The file is on demand and states its size in the root map |
| Cannot write `imports.md` | Run fails with the path, same as `CODEBASE.md` today |
| Unresolved specifiers | Counted in the header, carry no edge. Silent omission is the known risk, stated in both files |

## Evaluation harness changes

M1v2 is not runnable as the harness stands, independent of this feature.

1. **The system prompt contradicts the tasks.** `openrouter_agent.SYSTEM` says
   "tests, examples and re-exports alone do not qualify". M1v2 ground truth
   counts test files as dependents (`README.md`, "Test files count as legitimate
   dependents"). Fix: a second prompt, `SYSTEM_IMPACT`, selected when
   `task['kind'] == 'impact'`. It defers to the task's own definition of what
   qualifies. `SYSTEM` itself stays byte-identical, so M1 remains reproducible.
2. **The agent cannot reach an on-demand file.** `Session` only opens inventoried
   `.ts/.js` files. Fix: `Session(..., map_files={'.codearch/imports.md': text})`.
   It is populated only in the `with_map` arm, listed in the task observation as
   `map_files`, openable by exact path, and counted in `files_opened`. In the
   `without_map` arm the observation is unchanged.
3. `run_agent.py` passes `--codearch-dir <tmp>`, records `imports_sha256` in the
   run config, and saves `<repo>-imports.md` beside each saved map.

### Deterministic ceiling — `eval/check_import_index.py`

Before any paid run, measure whether the index *contains* the M1v2 answers. For
each gated task, parse the repository's `imports.md` and walk importers
breadth-first from the target. Score that set against `expected_files` with the
harness's own `set_f1`. This is the score a perfect agent reading only the index
would get.

Reported per task and as a mean, both raw and restricted to `src/`. madge, the
oracle, scanned only `repos/<r>/src`; codearch also analyzes benchmarks and
scripts, so unrestricted precision is expected to be lower.

**Measured.** Against the first, `src/`-only oracle: recall 1.000 on 24/24
tasks but F1 0.873, because correct dependents outside `src/` counted as wrong.
madge was rerun on the whole checkout, and the tasks were rebuilt and re-gated.
Result: 37 tasks, recall 1.000 on all of them, mean F1 0.981, against a free
adversary mean of 0.126. The remaining gap is `.mts` benchmark files. The index
lists them, but neither madge's extensions nor the harness inventory include
them. See `code_arch_architecture.md`.

## Testing

Hermetic, as before. No network, no model, no cloned repositories in the default
suite.

- `import_hubs`: ordering by count and then path; cap respected; files with no
  importers excluded.
- `import_index`: lines sorted by imported path and importers by path; files
  with no importers omitted; header counts match the edges; zero edges produce
  a header and no lines.
- `imports_section`: hub lines and the pointer when edges exist; the no-imports
  sentence, naming no file, when none do.
- `run()` on the tier1 fixture writes `imports.md` to the requested directory,
  links it from a map that stays in budget, and places the section before Task
  Navigation.
- Python: `Session` opens a map file only when one is supplied, and rejects it
  otherwise; the impact prompt is selected by task kind and the nav prompt is
  unchanged; the ceiling BFS follows multiple hops and terminates on cycles.

## Exit criteria

1. `cargo test` passes: the 63 existing tests unmodified plus the new ones.
   `python -m pytest eval` passes.
2. On Hono, Commerce and TypeDI, `CODEBASE.md` stays within 4,000 tokens, and two
   consecutive runs produce byte-identical `CODEBASE.md` and `imports.md`.
3. `check_import_index.py` reports the ceiling for every gated task (37 after
   the oracle rebuild).
   **Gate:** if mean ceiling recall is below 0.80, the paid run waits and the
   resolver gaps get investigated first. The numbers are written down either way.
4. The M1v2 agent run (`run_agent.py --tasks tasks-nav-gated.json`), without_map
   vs with_map. **It spends provider money, so it needs the project owner's
   explicit go-ahead.** Implementation is complete at criterion 3; the finding
   is complete at 4.
5. Results go into `code_arch_architecture.md` whether they are a win or a loss.

## Risks

**The agent may not open the index**, or may open it and walk it badly. Mentally
tracing importers four or five hops through ~9k tokens is exactly the multi-step
reasoning small models get wrong. The ceiling (criterion 3) separates "the index
lacks the answer" from "the agent failed to use it", which is why it runs first.

**Resolver misses become silent omissions.** A re-export chain or alias the
resolver cannot follow drops a whole subtree from an impact answer. The
resolution rate and unresolved count are stated in both files. The ceiling
measures the effect against a third-party oracle.

**Cost.** Opening the index costs thousands of tokens, which is how the map lost
M1. The difference this time is that the alternative is walking the chain one
search at a time, which M1v2's gate makes expensive. Whether that trade pays is
the measurement, not an assumption.

**Harness drift.** Changing the prompt and session invalidates comparison with
any earlier M1v2 run. There is none, so nothing is lost; M1 itself is untouched.
