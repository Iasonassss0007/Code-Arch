# codearch

Local code lookups for coding agents. `codearch` indexes a repository on your machine
and answers two questions agents otherwise answer by grepping and guessing:

- **Who imports this file?** Directly and transitively.
- **Which frontend files call this backend endpoint?** From a Django view to the
  TypeScript files that request it.

Works with TypeScript, JavaScript and Python, including monorepos (workspaces,
tsconfig `paths` and `extends`).

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
codearch                                  # build the index
codearch importers src/lib/auth.ts        # who depends on this file
codearch callers TagViewSet               # which frontend files call this view
codearch agents --write AGENTS.md         # tell your agents these lookups exist
```

Add `--json` for machine-readable output, and re-run `codearch` when the code changes.
Commit `.codearch/*.md` so a fresh clone can answer lookups right away
(`.codearch/cache/` is ignored automatically).

For MCP clients (Claude Code and others):

```
claude mcp add codearch -- codearch mcp --repo /path/to/repo
```

## Results

**Agents answer better with a lookup than with a codebase summary.** Asked "if I
change this file, which files are affected?", models using the `codearch` lookup
found the right files 98% of the time on navigation tasks and 100% on cross-language
ones. With no help they scored 61% and 87%. A codebase map in the prompt did worse
than no help. The lookup also cut the context read by 34–56% and the token cost by
18–77%.

![Answer quality: lookup tool 98% vs map 58% vs no help 61% on navigation; lookup tool 100% vs no help 87% on cross-language](docs/images/agent-quality.svg)

Tested with Gemini models on Hono, Next.js Commerce, TypeDI and paperless-ngx. The
cross-language result rests on one repository. Methods are in
[`eval/README.md`](eval/README.md).

**Cheaper than grep for transitive impact.** Context for one `codearch importers` call
vs a batched grep search, one regex per depth
([`eval/grep_vs_codearch.py`](eval/grep_vs_codearch.py)):

| Repo | grep | codearch | Ratio |
|---|---:|---:|---:|
| ~155 TS/JS files | 2,240 tokens | 1,152 tokens | 1.9x |
| ~2,800 TS/JS/Python files | 148,757 tokens | 3,896 tokens | 38x |

For a single direct-importer lookup, `grep -l` is just as good.

**Accurate on a large monorepo.** On a pnpm monorepo with 245 packages, 99.8% of
first-party imports resolve. Compared with the TypeScript compiler's import graph,
codearch's transitive importers have 100% recall and 99.8–100% precision across five
target files. Details are in
[`eval/resolution_report.md`](eval/resolution_report.md).

## Development

```
cargo test
python -m pytest -q eval/test_*.py
```

## License

[MIT](LICENSE)
