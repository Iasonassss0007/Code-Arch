# codearch

Local code-graph lookups for coding agents. `codearch` analyzes a repository on
your machine and answers two questions an agent otherwise answers by grepping and
guessing:

- **Who imports this file?** Directly and transitively, with hop depth.
- **Which frontend files call this backend endpoint?** Across the language
  boundary, from a Django view to the TypeScript files that request it.

It also writes a compact map of the repository (`CODEBASE.md`). The lookups are the
part with measured benefit; the map is a secondary output (see
[What helps agents](#what-helps-agents)).

Everything runs locally. The tool makes no network connection, needs no account and
collects nothing.

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
codearch /path/to/repo                    # analyze once; writes the indexes
cd /path/to/repo
codearch importers src/lib/auth.ts        # who depends on this file
codearch callers TagViewSet               # which frontend files call this view
```

```
$ codearch callers WorkflowViewSet
src/documents/views.py  (routes: workflows)
  src-ui/src/app/services/rest/workflow.service.ts
```

### Use from an agent

Any agent that can run shell commands can call the two lookups directly. For
MCP clients, the same lookups are served as tools (`importers`, `route_callers`)
over stdio:

```
claude mcp add codearch -- codearch mcp --repo /path/to/repo
```

The index files are loaded per call, so re-running `codearch` is picked up without
restarting the server.

## Commands

```
codearch [PATH] [OPTIONS]
codearch importers <file> [--depth N] [--json] [--repo PATH] [--codearch-dir DIR]
codearch callers <View> [--json] [--repo PATH] [--codearch-dir DIR]
codearch mcp [--repo PATH] [--codearch-dir DIR]
```

Analysis options:

```
--out <path>          where to write the map (default: <repo>/CODEBASE.md)
--budget <n>          hard token budget for the map (default: 4000)
--max-domains <n>     maximum top-level domains (default: 12)
--seed <n>            clustering seed; changes tie-breaks only
--no-index            skip .codearch/index.json
--no-git              ignore git history (no co-change signal)
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

| File | Contents |
|---|---|
| `CODEBASE.md` | The map: domains, stack, flows, most-imported files, pointers to the indexes |
| `.codearch/imports.md` | Reverse import index: for every imported file, each file that imports it |
| `.codearch/routes.md` | Backend views and their frontend callers (only when found) |
| `.codearch/index.json` | Machine-readable map data (skip with `--no-index`) |
| `.codearch/cache/` | Parse and git caches for fast re-runs; ignored by the tool's own `.gitignore` |

**What to commit:** the map and indexes, not the cache. A fresh clone with
`CODEBASE.md` and `.codearch/*.md` can answer lookups immediately.

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

These results come from the benchmark in [`eval/`](eval/), run against pinned public
repositories with real models. Full write-ups are in the linked folders and in
[`code_arch_architecture.md`](code_arch_architecture.md), including the runs that
went against the tool.

| Benchmark | Setup | Result |
|---|---|---|
| Impact navigation: which files are affected if this file changes? Hono, Next.js Commerce, TypeDI; 34 tasks × 2 trials, `gemini-3.5-flash-lite` | `importers` lookup vs no help | **F1 0.979 vs 0.608**, 64/68 exact vs 12/68, fewer tokens ([`results-nav-rescored`](eval/results-nav-rescored/)) |
| Same tasks | Full map in the prompt vs no help | F1 0.583 vs 0.608, about 2× the tokens: **no benefit** ([`results-nav-3arm`](eval/results-nav-3arm/)) |
| Cross-language impact: a Django view changes, which Angular files are affected? paperless-ngx, 8 tasks, `gemini-3.6-flash` | `route_callers` + `importers` vs no help | **F1 1.00 vs 0.874**, 8/8 exact, paired +0.126 [+0.05, +0.21], −54% tokens ([`results-xlang-routes-fixed`](eval/results-xlang-routes-fixed/)) |
| Same tasks | Map in the prompt vs no help | −0.096 [−0.46, +0.27]: **no benefit** ([`results-xlang-36flash-paid`](eval/results-xlang-36flash-paid/)) |

**Read with the caveats.** The cross-language answer key is built from the same idea
the tool implements (URL callers plus their direct importers), so that result shows
the lookup works as served, not that the link definition is right beyond the oracle.
It rests on one repository. The navigation benchmark's first version was solvable by
grep and was rebuilt with adversary gates; see `eval/README.md`.

The consistent finding across runs: **giving an agent a summary of the codebase as
text does not help; giving it precise lookups does.**

## Optional: local model labeling

By default, domain names in the map are derived deterministically from directory
structure, symbols and dependencies. No model is involved and output is
byte-identical across runs.

A small local model can name domains instead. It is **opt-in**, needs a native
toolchain, and currently produces worse names than the default.

### Prerequisites

1. **CMake**, to build llama.cpp. Visual Studio bundles one at
   `Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin` (add it to PATH);
   otherwise install CMake directly.
2. **LLVM / libclang**: `llama-cpp-sys-2` generates bindings with bindgen, which
   needs libclang (`winget install LLVM.LLVM` on Windows). If the build reports
   `Unable to find libclang`, set `LIBCLANG_PATH` to the directory containing it.

### Build and run

```
cargo build --release --features llm
codearch /path/to/repo --labeler llm --model /path/to/model.gguf
```

The tested model is Qwen2.5-Coder-1.5B-Instruct Q4_K_M (~1.1 GB) from
[Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF).
`--llm-threads <n>` sets inference threads (default: all cores).

### What the model is allowed to do

The model never sees source code. It receives structured facts from earlier
deterministic stages (directory names, top symbols, entry points, external
dependencies) and returns a name and one sentence, constrained by a GBNF grammar.

Every generated name is checked against that evidence. A name whose words trace to
nothing in the cluster, collides with a sibling or exceeds the length cap is replaced
by the derived name. The summary is checked word by word the same way; an ungrounded
summary keeps the generated name and uses the derived sentence. The run reports both
fallback counts.

Before enabling it:

- **It is slower.** CPU inference takes minutes per repository rather than seconds.
- **Determinism becomes conditional** on the model file and llama.cpp version.
- **It currently produces worse names.** On 20 frozen production clusters,
  Qwen2.5-Coder-1.5B scores 60% name specificity against the derived labeler's 75%,
  and it is the best of four swept models (`python eval/score_sweep.py`).

A LoRA fine-tune path is wired (`eval/harvest_lora.py`, `eval/train_lora.py`,
`--lora`), with adoption gated at derived parity (75%). No adapter has been trained;
it is optional research, not needed to use the tool.

## Development

```
cargo test
python -m pytest -q eval/test_*.py
```

230 Rust tests and 79 harness tests pass. `code_arch_architecture.md` records the
design, every milestone, and every measured result. The benchmark harness, task
builders and reproduction commands are documented in `eval/README.md`; paid runs
need a provider key (`GEMINI_API_KEY`, `OPENROUTER_API_KEY` or `GROQ_API_KEY`), which
is never written to artifacts.

## Status

A working tool at version 0.1. The lookups (`importers`, `callers`, MCP) are the
recommended interface. Coverage is TypeScript, JavaScript and Python; other
ecosystems and route frameworks are not supported yet.

## License

[MIT](LICENSE)
