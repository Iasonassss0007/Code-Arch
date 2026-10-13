---
name: codearch
description: Find who imports a file, what a file imports, and which frontend files call a backend route, from a static index instead of grep. Use in TypeScript, JavaScript, or Python repositories (including .vue/.svelte scripts and monorepos) before changing a file, when tracing the impact of a change, or when linking a Django, Flask, or FastAPI view to its TypeScript callers.
---

# codearch

`codearch` answers dependency questions from an index of the repository's imports and routes.
It is faster and more complete than grep for transitive questions, and it uses far less context.

## Setup

1. Check that the binary exists: `codearch --version`.
2. If the command is missing, install it once with `npm install -g @iasonas/codearch`.
   If npm fails, use the installer for the platform:
   - macOS or Linux: `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Iasonassss0007/Code-Arch/releases/latest/download/codearch-installer.sh | sh`
   - Windows: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/Iasonassss0007/Code-Arch/releases/latest/download/codearch-installer.ps1 | iex"`

   Do not run `npx codearch` for every lookup. It is slow on each call.
3. In the repository root, build the index: `codearch`. It takes seconds.
   Skip this step if `.codearch/imports.md` already exists. Lookups report when the index is stale.

## Lookups

Run every command from the repository root. Add `--json` when you parse the output.

| Question | Command |
|---|---|
| Which files depend on this file? | `codearch importers <file>` |
| Which files import it directly? | `codearch importers <file> --depth 1` |
| What does this file import? | `codearch deps <file>` |
| Which frontend files call this backend view? | `codearch callers <ViewName>` |
| Which frontend files call this route? | `codearch callers --path /api/tags/` |

## Freshness

Each answer reports the index state on stderr, or in the `freshness` object with `--json`.

- `fresh`: trust the answer.
- `stale`: the listed files changed since the last index. Run the lookup again with `--refresh`.
- `unknown`: the index has no cache yet. Run the lookup again with `--refresh`.

## When to use grep instead

- The question is about text, symbols, or call sites inside a file, not about file-level imports.
- The language is not TypeScript, JavaScript, or Python.
- You need only the direct importers of one file in a small repository. `grep -l` is as good there.
