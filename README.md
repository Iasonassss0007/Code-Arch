# codearch

Fast, local code lookups for AI coding agents. Instead of grepping and guessing, an agent asks:

- **Who imports this file?** Directly or transitively.
- **What does this file import?**
- **Which frontend files call this backend endpoint?** Django, Flask, or FastAPI routes to their TypeScript callers.

Supports TypeScript, JavaScript, and Python, including `.vue`/`.svelte` scripts and monorepos.

## Quick start

```
npm install -g @iasonas/codearch
npx skills add Iasonassss0007/Code-Arch
```

The first line installs the `codearch` command. The second teaches your agent when to use it.

Other installers (no Node needed):

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Iasonassss0007/Code-Arch/releases/latest/download/codearch-installer.sh | sh
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Iasonassss0007/Code-Arch/releases/latest/download/codearch-installer.ps1 | iex"
```

## Use

```
codearch                           # build the index in the current repo
codearch importers src/lib/auth.ts # who depends on this file
codearch deps src/lib/auth.ts      # what this file imports
codearch callers TagViewSet        # frontend files calling this view
codearch callers --path /tags/     # same, by route path
```

- Add `--json` for machine-readable output.
- Every answer says whether the index is `fresh`, `stale`, or `unknown`. Add `--refresh` to rebuild first when needed.
- Commit `.codearch/*.md` so a fresh clone can answer lookups right away.

## Other ways to connect an agent

- **MCP server:** `claude mcp add codearch -- codearch mcp --refresh` (run inside the repo).
- **Per repo:** `codearch agents --write AGENTS.md` adds a short instruction block.

## Results

Asked "if I change this file, which files are affected?", agents using codearch found the right
files 98% of the time (100% on cross-language tasks), versus 61% (87%) with no help. They also
read 34–56% less context.

![Answer quality: lookup tool 98% vs map 58% vs no help 61% on navigation; lookup tool 100% vs no help 87% on cross-language](docs/images/agent-quality.svg)

- **Cheaper than grep:** up to 38x fewer tokens for transitive impact on a 2,800-file repo ([script](eval/grep_vs_codearch.py)).
- **Accurate:** 100% recall and 99.8–100% precision against the TypeScript compiler on a 245-package monorepo ([report](eval/resolution_report.md)).

Tested with Gemini models on Hono, Next.js Commerce, TypeDI and paperless-ngx.

## Development

```
cargo test
python -m pytest -q eval/test_*.py
```

## License

[MIT](LICENSE)
