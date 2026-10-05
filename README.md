# codearch

Fast, local code lookups for coding agents. `codearch` indexes a repository on your
machine and answers two questions agents otherwise answer by grepping and guessing:

- **Who imports this file?** Directly and transitively.
- **Which frontend files call this backend endpoint?** From a Django view to the
  TypeScript files that request it, plus the files that import those.

Works with TypeScript, JavaScript and Python.

## Install

Requires Rust 1.85 or newer.

```
git clone https://github.com/Iasonassss0007/Code-Arch.git
cd Code-Arch
cargo install --path .
```

## Use

```
cd /path/to/repo
codearch                                  # build the index (seconds)
codearch importers src/lib/auth.ts        # who depends on this file
codearch callers TagViewSet               # which frontend files call this view
codearch agents --write AGENTS.md         # tell your agents these lookups exist
```

Add `--json` to a lookup for machine-readable output. Re-run `codearch` when the code
changes.

For MCP clients (Claude Code and others):

```
claude mcp add codearch -- codearch mcp --repo /path/to/repo
```

Commit `.codearch/*.md` so a fresh clone can answer lookups right away; the
`.codearch/cache/` folder is ignored automatically.

## What helps agents

**Giving an agent a summary of the codebase does not help. Giving it a precise
lookup does.**

We gave real AI models the question developers ask before a change: *"if I change
this file, which other files are affected?"* Each task ran three ways: with no help,
with a codebase map in the prompt, and with the `codearch` lookup tool.

![Answer quality: lookup tool 98% vs map 58% vs no help 61% on navigation; lookup tool 100% vs no help 87% on cross-language](docs/images/agent-quality.svg)

With the lookup tool, the model found the right files almost every time (98% and
100%). The map made it slightly *worse* than no help.

![Context read per task: lookup tool 3.5k tokens (34% less) vs map 9.4k (81% more) vs no help 5.2k on navigation; lookup tool 11.8k (56% less) vs no help 26.6k on cross-language](docs/images/agent-context.svg)

The lookup tool also leaves the agent with less to read: 34% and 56% less context.

![Model tokens billed relative to no help: lookup tool 82% and map 183% on navigation; lookup tool 23% on cross-language](docs/images/agent-tokens.svg)

And it makes tasks cheaper: 82% and 23% of the cost of no help. The map nearly
doubled the cost.

Tested on Hono, Next.js Commerce, TypeDI and paperless-ngx with Gemini models. The
cross-language result rests on one repository. Methods are in
[`eval/README.md`](eval/README.md), per-run results and confidence intervals in the
`eval/results-*` folders; the charts are generated from those files by
`python eval/make_readme_charts.py`.

## Versus grep

The context an agent spends on a blast-radius lookup ("if I change file X, which
files are affected, directly and transitively?") was also measured, using a best-effort
grep search, one batched regex per depth, against a single `codearch importers` call.
Tokens are counted with the Gemini `countTokens` API by
[`eval/grep_vs_codearch.py`](eval/grep_vs_codearch.py).

| Repo | grep context | codearch context | Ratio |
|---|---|---|---|
| Small (~155 TS/JS files) | 2,240 tok, 4 rounds | 1,152 tok, 1 call | 1.9x |
| Large (~2,800 TS/JS/Python files) | 148,757 tok, 7 rounds | 3,809 tok, 1 call | 39x |

- **Small repo:** a careful grep matches codearch (72 files each). A naive one stopped
  at depth 2 and silently missed 23 of the 72.
- **Large repo:** common basenames (`types`, `utils`, `client`) made grep match
  unrelated files. It converged on 2,503 files. codearch reported 173 importers; the
  TypeScript compiler's own graph has 177. The lookup took 150–175 ms after a 5.6 s
  index build.
- **Earlier figure:** an earlier version of this table showed 116x for the large
  repo. That build resolved only 34% of the repo's first-party imports, so it
  reported 57 importers and a much shorter answer. The large-repo row was
  re-measured after the resolution fixes below; the small-repo row is from the
  earlier build.

For a single direct-importer lookup, `grep -l` is as good. codearch wins on
transitive impact, and the win grows with repo size.

## Accuracy

Measured on the large repo above, a pnpm monorepo with 245 workspace packages,
tsconfig `paths` inherited through `extends`, and Python under `python/*/src`.

**Import resolution: 99.8% of first-party imports** (11,346 of 11,369), as codearch
reports it. An independent classifier, which checks against the repo's manifests and
tsconfigs, puts it at 99.65%. External packages, asset imports (`.css`) and imports
of files codearch skips as generated are counted separately, not as failures.

**Transitive importers against the TypeScript compiler's import graph:**

| Target file | Ground truth | codearch | Recall | Precision |
|---|---:|---:|---:|---:|
| `contract/chat-nodes.ts` | 177 | 173 | 97.7% | 100% |
| `submission-settings.ts` | 190 | 173 | 91.1% | 100% |
| `web-app/src/index.ts` | 3 | 3 | 100% | 100% |
| `directory-picker/src/index.ts` | 30 | 30 | 100% | 100% |
| `cosmokit/src/misc.ts` | 2,063 | 2,057 | 99.5% | 99.8% |

- **Misses:** every missed file depends on its target through a
  `declare module '…'` block (module augmentation). The TypeScript compiler
  counts that as a dependency; codearch doesn't read it yet.
- **Extras:** all five are test-fixture files whose own tsconfig maps a package
  to a stub.
- **Details:** the per-step numbers, every missed file and the known limitations
  are in [`eval/resolution_report.md`](eval/resolution_report.md).
- **Reproduce** with
  [`eval/ts_ground_truth.mjs`](eval/ts_ground_truth.mjs) and
  [`eval/recall_vs_ground_truth.py`](eval/recall_vs_ground_truth.py).

## Development

```
cargo test
python -m pytest -q eval/test_*.py
```

## License

[MIT](LICENSE)
