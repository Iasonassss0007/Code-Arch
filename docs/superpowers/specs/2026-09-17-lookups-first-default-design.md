# Lookups first: index-only default, map opt-in, agent pointer

Date: 2026-09-17. Status: ready to implement. No model calls, no spend.

## Why

The tool's purpose is to be useful to coding agents and to developers who work with
them. Measured so far:

| What the agent got | Effect vs no help |
|---|---|
| Lookups (`importers`, `route_callers`) | F1 +0.372 navigation, +0.126 cross-language, fewer tokens |
| The map pasted into its prompt | F1 −0.024 navigation, −0.096 cross-language, ~1.8× tokens |

(`README.md` "What helps agents"; `eval/make_readme_charts.py` regenerates the numbers.)

Yet `codearch .` still spends most of its work on the map: git history, graph fusion,
clustering, ranking, naming, flows, and a 4,000-token `CODEBASE.md`. Agents benefit from
the indexes and from knowing the lookups exist. This change makes that the default.

## Scope

| Part | What |
|---|---|
| **A** | `codearch [PATH]` writes only the lookup indexes. The full map moves behind `--map`. |
| **B** | `codearch agents` prints (or writes) a short block telling agents the lookups exist. |
| **C** | Eval harness, README, architecture doc and version updated for the new default. |

Out of scope: new lookups (`imports`, `cochange`, `entrypoints`, more route frameworks);
those are the next spec. Removing map code. Changing the query or MCP interfaces.

## Acceptance gates

1. **`--map` is today's behaviour, byte for byte.** On hono, commerce, typedi, realworld
   and paperless-ngx, `codearch <repo> --map --codearch-dir <tmp>` writes `CODEBASE.md`,
   `index.json`, `imports.md`, `routes.md` (and domain files where split) identical to the
   pre-change binary run with the same flags minus `--map`. Record hashes in the log.
2. **The default writes the same indexes.** `imports.md` and `routes.md` from the default
   run are byte-identical to the `--map` run's on the same five repos. The default writes
   no `CODEBASE.md` and no `index.json`.
3. **Lookups unchanged.** `eval/check_query_parity.py` passes 42/42 using default runs.
4. **Faster.** Record wall time, default vs `--map`, cold and warm, on hono and
   paperless-ngx (`CODEARCH_TIME=1` stage timings). The default must skip every stage
   after contracts; there is no target number, just report it.
5. **Cache safety.** Run `--map` → default → `--map` on one repo: the third run is warm
   and its outputs equal the first run's. The default run must not drop stored git, label
   or split data from the cache.
6. **No user file is modified without being asked.** The default run never deletes or
   rewrites an existing `CODEBASE.md`. `codearch agents` writes only with `--write`, and
   only inside its own marked block.
7. `cargo test` and `python -m pytest -q eval/test_*.py` pass.

## What exists today (read before starting)

`lib::run` (`src/lib.rs`) runs, in order: cache load → inventory → profile → parse →
resolve → contracts (`contract::join`, `semantic_join`, `route_join`) → git → graph →
cluster → rank → stability → summarize → label → flows → `render::import_index` → map
render → write (map, cache `store.save`, `imports.md`, `routes.md`, `index.json`).

`imports.md` needs only `inv`, `profile` (for `render::map_title`), `res` and the file
paths. `routes.md` needs only `inv` and `parsed`. Everything from git onward is map-only.

`cache::Store` is loaded once and saved once; `.codearch/.gitignore` is created by the
store and never clobbered (`src/cache.rs`).

Eval scripts that run the binary and read the map (all need `--map`):

```text
eval/run.py:263            M1 harness (map text in context)
eval/run_agent.py:139      real-agent runs (with_map arm, CODEBASE copies)
eval/check_import_index.py:79
eval/cochange_delta.py:34
eval/impact_proxy.py:241
```

`eval/check_query_parity.py` reads only the indexes, so it should use the default.

## Part A: index-only default

### A1. CLI

```text
codearch [PATH] [--codearch-dir DIR]                   # new default: indexes only
codearch [PATH] --map [map options] [--codearch-dir DIR]
```

- Add `--map`. Every option that only affects the map requires it, via clap `requires`:
  `--out`, `--budget`, `--max-domains`, `--seed`, `--no-index`, `--no-git`, `--labeler`,
  `--model`, `--llm-threads`, `--lora`, `--lora-scale`. Passing one without `--map` is a
  usage error that names `--map`, not a silent no-op. Test one of them.
- `--codearch-dir` works in both modes.
- Subcommands (`importers`, `callers`, `mcp`, and Part B's `agents`) are unchanged.

### A2. Pipeline split

Split `lib::run` so the index-only path stops after contracts:

- Add `Options::map: bool` (CLI `--map`). `Options::default()` keeps `map: true`, so every
  existing library test keeps exercising the map unchanged; only the CLI default flips.
- Factor the shared front (cache load through contracts, plus `import_index` and
  `routes.md`) into one function both modes call. Do not duplicate stage code.
- Index-only writes `imports.md`, `routes.md` (or removes a stale one, as today), and saves
  the cache. It does not run git, graph, cluster, rank, label or flows, and does not write
  the map or `index.json`.
- `RunReport` records the mode; map-only fields become `Option` or zero in index-only
  runs. Keep `RunReport` changes minimal and update `eval-support` if it reads them.

### A3. Cache safety

The default run loads the store, refreshes file parses, and saves. It must write back
every field it did not touch (git signal, labels, split memo) exactly as loaded. Add a
`cache.rs` or `lib.rs` test: a store holding git and label entries still holds them after
an index-only run. Then run gate 5 on a real repo.

### A4. Output

```text
$ codearch C:\src\site
Files:               412 analyzed
Import resolution:   96% (18 unresolved)
Import index:        1,204 imports into 301 files
Route callers:       none found
Wrote:
  C:\src\site\.codearch\imports.md
Next:
  codearch importers <file>            who depends on a file
  codearch agents --write AGENTS.md    tell your agents about these lookups
  codearch --map                       also write the CODEBASE.md overview
```

(Illustrative numbers.) If `<root>/CODEBASE.md` exists and carries the map's generated
header, add one line: `CODEBASE.md is from an earlier run and is no longer updated;
re-run with --map to refresh it, or delete it.` Never delete it.

## Part B: `codearch agents`

Agents only use lookups they know about. This replaces the map's one agent-useful role,
pointing at the lookups, with a few lines in a file agents already read.

### B1. Command

```text
codearch agents [--repo PATH] [--codearch-dir DIR]                   # print the block
codearch agents --write <FILE> [--repo PATH] [--codearch-dir DIR]
```

Printed block (Markdown), built from what the index actually holds:

```markdown
<!-- codearch:start -->
## Code lookups (codearch)

This repository is indexed by codearch. Use these lookups instead of guessing from search:

- `codearch importers <file>`: every file that imports `<file>`, with hop depth. Run it
  before changing a file to see what else is affected.
- `codearch callers <View>`: frontend files that call a backend view.

Answers come from the last `codearch` run; if files moved since, run `codearch` first.
<!-- codearch:end -->
```

The `callers` line appears only when `routes.md` exists. Keep the wording close to the
eval tool descriptions (`eval/openrouter_agent.py` `IMPORTERS_TOOL`, `ROUTES_TOOL`) so the
shipped guidance matches what was measured, and keep it under ~80 words.

### B2. `--write` rules

- `<FILE>` is resolved relative to `--repo`. Typical targets: `AGENTS.md`, `CLAUDE.md`,
  `.github/copilot-instructions.md`. Do not guess the file; the user names it.
- File missing: create it with just the block.
- File present without markers: append a blank line and the block. Never touch other
  content.
- File present with both markers: replace only the text between them, markers included.
  Running it twice gives a byte-identical file (test).
- A start marker without an end marker, or the reverse: refuse with exit 2 and a
  message. Do not guess where the block ends.
- No index under the state directory: exit 2 with `run codearch <repo> first`, like the
  lookups.
- Preserve the file's existing line endings (CRLF stays CRLF).

Tests: create, append, replace idempotently, malformed markers, CRLF preserved, and the
routes line present only when `routes.md` exists.

## Part C: harness, docs, version

1. Add `--map` to the five eval call sites listed above. Re-run the M1 lexical harness
   reproduction (`eval/README.md` records 40/40 with-map, 32/40 without) and
   `check_import_index.py`, and confirm unchanged results in the log.
2. `check_query_parity.py`: use the default (no `--map`), re-run, record.
3. README:
   - Quick start becomes `codearch .` → `codearch importers …` →
     `codearch agents --write AGENTS.md`, plus the MCP line.
   - "What it writes": indexes by default; `CODEBASE.md` and `index.json` only with
     `--map`.
   - A short "Map (optional)" section: a human-readable overview, measured as no help to
     agents when pasted into prompts, so do not paste it; generate it with `--map`.
   - In the options table, mark the map-only options.
4. `code_arch_architecture.md`: one section recording the decision, with the table from
   "Why" and the gate results.
5. `Cargo.toml` version `0.1.0` → `0.2.0`, since the default behaviour changes.

## Rules while working

- Shared stages are factored, never copied. If a refactor changes any `--map` output
  byte, gate 1 fails: fix the refactor, do not re-baseline.
- Record gate 1's baseline before changing code: build the pre-change binary, hash its
  outputs, and keep the hashes in the log.
- One commit per part (A, B, C). Each message states which gates were checked.
- The eval runners must keep reproducing recorded results. If one cannot, stop and record
  why rather than editing the recorded numbers.

## Log

| Gate | Result | Notes |
|---|---|---|
| 1 `--map` byte-identical (5 repos) | **pass** | 21 output files (CODEBASE.md, index.json, imports.md, routes.md, per repo) hashed from the pre-change binary (`cc5a80d`), identical after the change |
| 2 default indexes = `--map` indexes | **pass** | imports.md identical on all 5, routes.md on paperless-ngx; default wrote no CODEBASE.md / index.json |
| 3 parity | **pass** | `check_query_parity.py` on default runs: 42/42 |
| 4 timings default vs `--map` | reported | debug build, ms. Cold: hono 1,419 vs 4,344; commerce 871 vs 1,499; typedi 560 vs 1,154; realworld 596 vs 686; paperless-ngx 2,847 vs 3,254. Warm: hono 755 vs 1,482; paperless-ngx 1,118 vs 1,416. Parsing dominates paperless-ngx, so its gain is smaller |
| 5 cache sequence | **pass** | hono `--map` → default → `--map`: third run parse-cache 377/377 hits, git from cache, CODEBASE.md / imports.md / index.json identical to the first; unit test keeps seeded labels and split memo |
| 6 user files untouched | **pass** | fixture copy: generated CODEBASE.md hash unchanged after a default run, note printed; `agents --write` on a CRLF file appends, keeps CRLF, second run "already up to date"; missing index exits 2 |
| 7 tests | **pass** | cargo 238 + 5 (7 agents, 1 index-only, 2 CLI added); pytest 79 |
| C1 M1 harness reproduction | **pass** | `eval/run.py` with `--map`: without_map 32/40, with_map 40/40 as recorded. `check_import_index.py`: 34 tasks, recall 1.000, F1 0.979 (committed report is the older 37-task set) |
