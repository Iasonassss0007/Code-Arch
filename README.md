# codearch

Local code-graph lookups for coding agents. `codearch` analyzes a repository on
your machine and answers two questions an agent otherwise answers by grepping and
guessing:

- **Who imports this file?** Directly and transitively, with hop depth.
- **Which frontend files call this backend endpoint?** Across the language
  boundary, from a Django view to the TypeScript files that request it.

Lookups are what measurably help agents (see [What helps agents](#what-helps-agents)),
so that is all `codearch` builds.

## Install

Requires Rust 1.85 or newer (edition 2024). Pure Rust, no native toolchain for the
default build.

```
git clone https://github.com/Iasonassss0007/Code-Arch.git
cd Code-Arch
cargo install --path .
```

## Quick start

```
cd /path/to/repo
codearch                                  # build the lookup indexes (seconds)
codearch importers src/lib/auth.ts        # who depends on this file
codearch callers TagViewSet               # which frontend files call this view
codearch agents --write AGENTS.md         # tell your agents these lookups exist
```

```
$ codearch callers WorkflowViewSet
src/documents/views.py  (routes: workflows)
  src-ui/src/app/services/rest/workflow.service.ts
```

### Use from an agent

Any agent that can run shell commands can call the two lookups directly, once it
knows they exist. `codearch agents` prints a short block saying so;
`codearch agents --write AGENTS.md` (or `CLAUDE.md`, or whichever file your agent
reads) adds it between `<!-- codearch:start -->` and `<!-- codearch:end -->` markers.
Re-running replaces only that block and leaves the rest of the file alone.

For MCP clients, the same lookups are served as tools (`importers`, `route_callers`)
over stdio:

```
claude mcp add codearch -- codearch mcp --repo /path/to/repo
```

The index files are loaded per call, so re-running `codearch` is picked up without
restarting the server.

## Commands

```
codearch [PATH] [--codearch-dir DIR]
codearch importers <file> [--depth N] [--json] [--repo PATH] [--codearch-dir DIR]
codearch callers <View> [--json] [--repo PATH] [--codearch-dir DIR]
codearch agents [--write FILE] [--repo PATH] [--codearch-dir DIR]
codearch mcp [--repo PATH] [--codearch-dir DIR]
```

```
--codearch-dir <path> where the indexes are written (default: <repo>/.codearch)
```

A directory literally named `importers` or `callers` is analyzed with
`codearch ./importers`.

### Lookups

- `importers` prints one `depth  path` line per importing file. `--depth 1` limits
  the answer to direct importers.
- `callers` prints the view's file and routes, then its frontend callers. It needs
  `.codearch/routes.md`, which is written only when route links were found.
- `--json` prints one object for scripting, in the same shape the benchmark's tools
  returned.
- Both read the index as written and never re-analyze. stderr reports
  `index written <N> minutes ago`; re-run `codearch` when the code has moved on.
- A miss exits 0 with a one-line reason on stderr. A missing index, a path outside
  the repository, or bad arguments exit 2.

### What it writes

| File | Written | Contents |
|---|---|---|
| `.codearch/imports.md` | always | Reverse import index: for every imported file, each file that imports it |
| `.codearch/routes.md` | when route links exist | Backend views and their frontend callers |
| `.codearch/cache/` | always | Parse and git caches for fast re-runs; ignored by the tool's own `.gitignore` |

**What to commit:** `.codearch/*.md`, not the
cache. A fresh clone can then answer lookups immediately.

## Supported code

| Area | Coverage |
|---|---|
| Languages | TypeScript, JavaScript (incl. `.mts/.cts/.mjs/.cjs`), Python |
| Import resolution | Relative imports, tsconfig `paths` and `baseUrl` (root, workspace packages, and standalone apps such as `frontend/tsconfig.json`), npm/pnpm workspaces, Python packages, Django `include()` |
| Cross-language routes | Django `path`/`re_path`/`url` with nested `include()`, DRF `router.register`, matched to TypeScript files that send HTTP requests (`HttpClient`, `fetch`, `axios`), including services that inherit their client |
| Frameworks recognized | Next.js, React, Django, Flask, and others from manifests |

Route matching is static. On paperless-ngx it finds every caller the benchmark oracle
finds (recall 1.00) at precision 0.98. URLs assembled at runtime can be missed, and a
route named by a common word can match a file that uses the word for something else.

## What helps agents

**In short: giving an agent a summary of the codebase does not help. Giving it a
precise lookup does.**

We tested this with real AI models on public repositories. Each task asks a
question a developer asks before changing code: *"if I change this file, which other
files are affected?"* The model explores the repository with search and file-open
tools, then answers with a list of files. It tries each task in one of three setups:

- **No help:** only search and open.
- **Map in the prompt:** the same, plus a generated codebase overview pasted into its
  instructions.
- **Lookup tool:** the same, plus `importers` (and, for cross-language tasks,
  `route_callers`) to call whenever it wants.

Answers are scored by **F1**: 100% means exactly the right files, no misses and no
extras.

![Answer quality: lookup tool 98% vs map 58% vs no help 61% on navigation; lookup tool 100% vs no help 87% on cross-language](docs/images/agent-quality.svg)

- With the **lookup tool**, the model found the right files almost every time:
  98% on navigation, where 64 of 68 answers were exactly right (12 of 68 with no
  help), and 100% on the cross-language tasks.
- With the **map in the prompt**, it did slightly *worse* than with no help at all.

![Context read per task: lookup tool 3.5k tokens (34% less) vs map 9.4k (81% more) vs no help 5.2k on navigation; lookup tool 12.2k (54% less) vs no help 26.6k on cross-language](docs/images/agent-context.svg)

- With the **lookup tool**, the agent's context holds **34% less** on navigation and
  **54% less** on cross-language tasks. One lookup answer replaces the search results
  and opened files the agent would otherwise read to work out the same thing, which
  leaves more room in the context window for the actual work.
- With the **map in the prompt**, the context holds **81% more**: the map is added,
  and the agent still searches and opens files.

![Model tokens billed relative to no help: lookup tool 82% and map 183% on navigation; lookup tool 28% on cross-language](docs/images/agent-tokens.svg)

- The smaller context also makes tasks **cheaper**. Models are billed for the whole
  conversation on every step, so the saving compounds: the lookup tool cost 82% of
  no help on navigation and **28%** on cross-language tasks.
- The map made every navigation task **almost twice as expensive** (183%).

<details>
<summary>Exact numbers, confidence intervals and caveats</summary>

Change in F1 against no help, averaged per task, with a 95% bootstrap confidence
interval. An interval that does not include 0 is a clear effect.

| Benchmark | Setup | Change in F1 | 95% CI | Tasks |
|---|---|---:|---|---:|
| Navigation | Lookup tool | +0.372 | [+0.28, +0.47] | 34 |
| Navigation | Map in the prompt | −0.024 | [−0.12, +0.08] | 34 |
| Cross-language | Lookup tool | +0.126 | [+0.05, +0.21] | 8 |
| Cross-language | Map in the prompt | −0.096 | [−0.46, +0.27] | 7 |

- **Navigation:** 34 tasks on Hono, Next.js Commerce and TypeDI, 2 attempts each,
  `gemini-3.5-flash-lite`, all three setups in the same runs
  ([`results-nav-rescored`](eval/results-nav-rescored/)).
- **Cross-language:** a Django view changes; which Angular files are affected? 8 tasks
  on paperless-ngx, `gemini-3.6-flash`, 1 attempt per setup
  ([`results-xlang-routes`](eval/results-xlang-routes/),
  [`results-xlang-routes-fixed`](eval/results-xlang-routes-fixed/)). The map-in-prompt
  row comes from a separate run and is compared only with that run's own no-help
  setup ([`results-xlang-36flash-paid`](eval/results-xlang-36flash-paid/)), so it has
  no bar in the charts.
- **The cross-language answer key uses the same idea as the tool** (URL callers plus
  their direct importers). The result shows the lookup works as served; it does not
  independently prove the links are right. It rests on one repository.
- The navigation tasks were rebuilt after a first version turned out to be solvable
  by plain grep; every task now passes adversary gates (see `eval/README.md`).
- Charts and table are generated from the result files by
  `python eval/make_readme_charts.py`. Full write-ups are in each results folder and
  in [`code_arch_architecture.md`](code_arch_architecture.md).

</details>

## Development

```
cargo test
python -m pytest -q eval/test_*.py
```

218 Rust tests and 65 harness tests pass. `code_arch_architecture.md` records the
design, every milestone, and every measured result. The benchmark harness, task
builders and reproduction commands are documented in `eval/README.md`; paid runs
need a provider key (`GEMINI_API_KEY`, `OPENROUTER_API_KEY` or `GROQ_API_KEY`), which
is never written to artifacts.

## Status

A working tool at version 0.3. The lookups (`importers`, `callers`, `agents`, MCP) are the
recommended interface. Coverage is TypeScript, JavaScript and Python; other
ecosystems and route frameworks are not supported yet.

## License

[MIT](LICENSE)
