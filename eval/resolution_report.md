# Import resolution on the large eval repo

Phase 1: measurement only. No resolver code has changed.

## Verdict

The 34% is a real gap, but external packages don't cause it.

- **Externals explain 3% of the failures, not most of them.** 228 of the 7,648
  unresolved specifiers were scoped npm packages (`@testing-library/react`, …)
  counted as failures. Another 126 were CSS module imports. Removing both from
  the denominator moves the rate from 34.2% to 35.2%.
- **95% of the failures are one category:** 7,277 imports of the repo's own
  `@deepseek-ai/*` packages, mapped by tsconfig `paths`. codearch never loads
  those `paths`.
- **HEAD hides the gap instead of fixing it.** The current source reports 99.6%
  on the same repo. It reaches that by filing 7,268 of those first-party imports
  as *external*, which removes them from the denominator. Only 9 more
  references resolve than before, and the 57-importer answer is unchanged.
- **First-party resolution is 35.2% on both builds.**

## Which repo and which binary

| | |
|---|---|
| Repo | `~/deepseek-harness` @ `b150a551b8` (2,796 TS/JS/Py files tracked; README says ~2,800) |
| Binary that reported 34% | `~/.cargo/bin/codearch.exe`, built 2026-09-17, matches commit `1613303` |
| Current source | `956a936` (HEAD) |

`eval/grep_vs_codearch.py` runs whichever `codearch` is on PATH, so the README's
numbers came from the stale installed build. Both builds were re-run here:

| Build | Source files | Reported resolution | Unresolved | Import edges |
|---|---|---|---|---|
| `1613303` (installed) | 2,708 | 34.15% | 7,648 | 3,222 |
| `956a936` (HEAD) | 2,708 | 99.57% | 17 | 3,231 |

## Where the rate is computed

`src/resolve.rs`, `resolve_all` (lines 131–241). It computes
`resolution_rate = internal_hits / internal_attempts` at lines 234–238.

What counts as an attempt:

- **TS/JS:** every relative specifier that isn't an asset (line 207), plus every
  bare specifier that `resolve_bare` resolves (line 183). Bare specifiers that
  `is_external_specifier` or `is_unaliased_scoped` call external are never
  attempts (line 198).
- **Python:** relative imports and `django-include:` always count. An absolute
  import counts only when it resolves; a miss goes to externals (`resolve_py`,
  lines 389–441).

The rate is printed by `src/main.rs:161` and written into `imports.md` by
`src/render.rs:484`.

## Method

1. A throwaway harness (scratchpad crate, built against each commit) runs
   inventory → profile → parse → `resolve_all`. It then re-runs `resolve_all`
   once per reference, so every one of the 15,615 references gets its own
   outcome: resolved, external, unresolved or skipped.
2. The per-reference results match each build's full run exactly: the same
   unresolved multiset, 7,648 and 17.
3. Each reference is then classified against ground truth that doesn't use
   codearch's heuristics:
   - the names in all 256 tracked `package.json` files
   - all 162 `paths` keys in the tracked `tsconfig*.json` files
   - declared dependencies
   - Node's `module.builtinModules`
   - Python's `sys.stdlib_module_names`
   - the module paths of the repo's own 20 `.py` files

## Classification of every reference

Each column shows what codearch did with the references in that category. A
first-party reference marked "external" is a miss that the metric doesn't count.

| Category | Refs | `1613303` resolved / external / **unresolved** | HEAD resolved / external / **unresolved** |
|---|---:|---|---|
| tsconfig paths alias (all are also workspace packages) | 7,277 | 0 / 0 / **7,277** | 9 / **7,268 misfiled** / 0 |
| Relative path | 3,974 | 3,957 / 0 / **17** | 3,957 / 0 / **17** |
| Python absolute first-party | 19 | 0 / **19 misfiled** / 0 | 0 / **19 misfiled** / 0 |
| Python relative import | 9 | 9 / 0 / 0 | 9 / 0 / 0 |
| External: Node builtin | 2,155 | 0 / 2,155 / 0 | 0 / 2,155 / 0 |
| External: npm package | 1,943¹ | 0 / 1,715 / **228** | 0 / 1,943 / 0 |
| External: Python stdlib | 85 | 0 / 85 / 0 | 0 / 85 / 0 |
| External: pip package | 9 | 0 / 9 / 0 | 0 / 9 / 0 |
| Asset (`.module.css`) | 126 | 0 / 0 / **126** | skipped |
| `__future__` | 18 | skipped | skipped |
| **Total** | **15,615** | 3,966 / 3,983 / **7,648** | 3,975 / 11,479 / **17** |

¹ Includes 14 `mdast` imports. That package is types-only, declared as
`@types/mdast` in `packages/client/ui-primitives/package.json`.

### Unresolved specifiers in the build that reported 34%, by requested category

| Category | Unresolved | Share |
|---|---:|---:|
| tsconfig paths alias | 7,277 | 95.1% |
| External package (scoped npm counted as a failure) | 228 | 3.0% |
| Other: asset import (`.module.css`) counted as a failure | 126 | 1.6% |
| Other: relative import whose target the inventory excluded | 17 | 0.2% |
| baseUrl import | 0 | — |
| Workspace or monorepo package (not also a paths alias) | 0 | — |
| Barrel or index re-export | 0 | — |
| Python relative import | 0 | — |
| Python absolute first-party | 0 unresolved (19 misfiled as external) | — |
| Dynamic `import()` / `require()` with a non-literal argument | n/a (never enters the denominator) | — |
| **Total** | **7,648** | |

Notes on the zero rows:

- **baseUrl:** the only `baseUrl` in the repo is in a test fixture's tsconfig.
- **Barrel or index:** no relative miss involves a directory or index import.
  All 17 relative misses name a file explicitly, and the file is missing from
  the inventory.
- **Dynamic imports:** the parser keeps only string literals, so a non-literal
  argument creates no reference at all. A regex scan finds about 141 such call
  sites (133 identifier or expression arguments, 8 template literals). That
  count is **an estimate**: a multi-line literal call would be miscounted. Most
  are in `tests/*.e2e.ts` files loading built bundles by URL.

## The two numbers

| Metric | `1613303` | HEAD |
|---|---:|---:|
| Overall resolution, as codearch reports it | **34.15%** (3,966 / 11,614) | 99.57% (3,975 / 3,992) |
| Externals (and assets) removed from codearch's denominator | 35.22% (3,966 / 11,260) | 99.57% |
| **First-party resolution against ground truth**² | **35.16%** (3,966 / 11,279) | **35.24%** (3,975 / 11,279) |

² Denominator: the 11,279 references whose ground truth is first-party (paths
alias, relative path, Python relative, Python absolute first-party). A
first-party reference codearch files as external counts as a miss.

Target files for the README's 57: two files have exactly 57 transitive
importers under `1613303`. The README doesn't name its target, so both are
tracked:

| Target | `1613303` | HEAD |
|---|---:|---:|
| `packages/client/ui-conversation/src/client/contract/chat-nodes.ts` | 57 | 57 |
| `packages/client/ui-conversation/src/submission-settings.ts` | 57 | 57 |

## Examples (5 per category, `specifier` ← importer)

**tsconfig paths alias:** unresolved in `1613303`, filed external in HEAD
- `@deepseek-ai/dsh-app-boot` ← `apps/cli/src/bin.ts`
- `@deepseek-ai/cordis` ← `apps/cli/src/profile-boot.ts`
- `@deepseek-ai/cordis-plugin-include` ← `apps/cli/src/profile-boot.ts`
- `@deepseek-ai/cordis-plugin-loader` ← `apps/cli/src/profile-boot.ts`
- `@deepseek-ai/dsh-home-paths` ← `apps/cli/src/profile-boot.ts`

The repo has 360 distinct specifiers in this category. The most used:
`@deepseek-ai/cordis` (1,109), `@deepseek-ai/dsh-session` (585),
`@deepseek-ai/dsh-llm` (574), `@deepseek-ai/dsh-client-runtime/client` (335).

**External package counted as a failure** (`1613303` only)
- `@testing-library/react` ← `apps/web/tests/assembled-boot.ts` (122 refs in total)
- `@vitejs/plugin-react` ← `apps/web/vite.config.ts`
- `@agentclientprotocol/sdk` ← `examples/acp-agent/tests/acp.e2e.ts`
- `@shikijs/langs/typescript` ← `packages/client/ui-primitives/src/markdown/highlight.ts`
- `@shikijs/langs/shellscript` ← `packages/client/ui-primitives/src/markdown/highlight.ts`

**Asset import counted as a failure** (`1613303` only)
- `./LanguageRow.module.css` ← `packages/client/locale/src/client/LanguageRow.tsx`
- `./AgentPresetLabel.module.css` ← `packages/client/ui-agent-preset/src/client/AgentPresetLabel.tsx`
- `./AgentPresetRow.module.css` ← `packages/client/ui-agent-preset/src/client/AgentPresetRow.tsx`
- `./AgentPresetSeat.module.css` ← `packages/client/ui-agent-preset/src/client/AgentPresetSeat.tsx`
- `./AgentPresetSection.module.css` ← `packages/client/ui-agent-preset/src/client/AgentPresetSection.tsx`

**Relative import whose target the inventory excluded** (both builds; all 17 of
HEAD's unresolved)
- `./icons/index.tsx` ← `packages/client/ui-primitives/src/Menu.tsx`. Target
  excluded as generated: an SVG path line of 3,485 bytes trips the
  minified-line check.
- `./FishLogo.tsx` ← `packages/client/ui-primitives/src/index.ts`. Same reason,
  3,486-byte line.
- `./BrandWordmark.tsx` ← `packages/client/ui-primitives/src/index.ts`. Same
  reason, 3,509-byte line.
- `./known-event-types.ts` ← `packages/core/session/src/index.ts`. Target has a
  "GENERATED … do not edit" header.
- `./api-catalog.ts` ← `packages/extensions/tool-cordis/src/inspect.ts`. Target
  has a "Generated … do not edit" header.
- One more: `./packages/typert/generator/lib/types/tsdown-plugin.js` ←
  `tsdown.config.ts`. The target is untracked build output.

**Python absolute first-party, misfiled as external** (19 refs; only 3 distinct
specifiers exist)
- `deepseek_harness` ← `examples/jsonrpc-agent/minimal.py`
- `deepseek_harness.errors` ← `python/sdk/tests/test_bundled_runtime.py`
- `deepseek_harness_runtime` ← `python/sdk/src/deepseek_harness/client.py`

**Dynamic import with a non-literal argument** (not references; regex sample)
- `import(pathToFileURL(webBundleResolver.resolve('@deepseek-ai/dsh-app-boot')).href)` ← `apps/web/tests/assembled-boot.ts:62`
- `import(buildEnvironmentModulePath)` ← `apps/web/tests/built-boot.snapshot.ts:21`
- `import(urls.agent)` ← `packages/api/remotes/tests/built-lib.e2e.ts:52`
- `import(urls.connectionHost)` ← `packages/api/remotes/tests/built-lib.e2e.ts:53`
- `import(urls.apiGatewayHost)` ← `packages/api/remotes/tests/built-lib.e2e.ts:54`

## Root causes (input to Phase 2)

1. **tsconfig `extends` is not followed** (7,277 refs). `profile::read_tsconfig`
   (`src/profile.rs:290`) reads `compilerOptions` only from `tsconfig.json`
   itself. The root `tsconfig.json` is a solution file: it `extends`
   `./tsconfig.base.json`, which holds all the `paths`. As a result codearch
   loads 0 of the 162 alias keys.
2. **Workspace discovery misses nested globs.** It also can't serve as a
   fallback for the same refs. `profile::discover_packages` (`src/profile.rs:371`)
   expands only a trailing single `*`, so `packages/*/*` reads the group
   directories (which have no manifest) and finds nothing. Discovery is also
   capped at 64 packages; the repo has about 240. HEAD found 18.
3. **HEAD hides misses** (7,268 refs). In `is_unaliased_scoped`
   (`src/resolve.rs:341`), any `@scope/name` that no alias claims becomes
   external, even when the scope belongs to the repo's own packages.
4. **Python src-layout below the repo root** (19 refs). `python_roots`
   (`src/resolve.rs:364`) adds only `src/` at the root and workspace package
   dirs. The `python/sdk/src` root is missing.
5. **Inventory drops hand-written files as generated** (16 refs, plus 1 to build
   output). The minified-line check (more than 3,000 bytes on one line) catches
   SVG-path components. This is an inventory decision, not a resolver one.

Side finding, outside the CLI path: `parse::parse_all` (`src/parse.rs:144`)
returns empty parses. `parse_all_cached` stores fresh results only into map
entries that already exist, and `parse_all` passes an empty map. Nothing in the
binary calls `parse_all`.

## Phase 2: fixes, measured per step

All runs use `target/release/codearch` built from the working tree, on
`~/deepseek-harness` @ `b150a551b8`. Indexes are written to a scratch
directory; `~/.cargo/bin/codearch.exe` is never used.

- **Reported rate:** what codearch prints.
- **Ground-truth rate:** the Phase 1 classifier's first-party resolution,
  resolved out of 11,279 first-party refs.
- **External:** references codearch files as external.
- **Importers:** transitive `codearch importers` count for each target.

| Step | Reported rate | Ground-truth rate | External | Unresolved | `chat-nodes.ts` | `submission-settings.ts` |
|---|---:|---:|---:|---:|---:|---:|
| 0: HEAD `956a936` | 99.57% | 35.24% | 11,479 | 17 | 57 | 57 |
| 1: metric | 40.23% (3,975 / 9,881) | 35.24% | 5,573 | 5,906 | 57 | 57 |
| 1b: walk workspace-declared `vendor/` | 48.05% (5,463 / 11,369) | 47.98% (5,463 / 11,386) | 4,217 | 5,906 | 57 | 57 |
| 2: tsconfig `extends` | 98.16% (11,160 / 11,369) | 98.02% (11,160 / 11,386) | 4,217 | 209 | 172 | 173 |
| 3: nested workspace globs, no cap | 99.63% (11,327 / 11,369) | 99.48% (11,327 / 11,386) | 4,217 | 42 | 173 | 173 |
| 4: Python roots below the repo root | 99.80% (11,346 / 11,369) | 99.65% (11,346 / 11,386) | 4,217 | 23 | 173 | 173 |

Step 1 changes (`src/inventory.rs`, `src/profile.rs`, `src/resolve.rs`,
`src/lib.rs`, `src/main.rs`):

- **Repo package names come from the walk.** The inventory now records every
  `package.json` it walks past. Their `name` values become
  `PathMappings::local_packages`.
- **First-party misses count as failures.**
  - A bare import whose package name is in that set is a first-party attempt.
    On a miss it is unresolved, not external.
  - A Python absolute import whose top-level name is a top-level package dir
    in the inventory is a first-party attempt.
- **Non-failures get their own counts.** Asset imports (`asset_refs`) and
  relative imports of files on disk outside the inventory (`excluded_refs`)
  are reported separately and are not attempts. The CLI prints
  `Not counted: N external, N asset, N into excluded files`.

**Step 1 stopped at 40%, not about 35%.** All 1,381 alias imports that are
still external belong to 7 packages under `vendor/`:

| Package | Refs |
|---|---:|
| `@deepseek-ai/cordis` | 1,109 |
| `@deepseek-ai/schemastery` | 131 |
| `@deepseek-ai/cordis-plugin-loader` | 87 |
| `@deepseek-ai/cordis-plugin-include` | 45 |
| `@deepseek-ai/cordis-plugin-timer` | 4 |
| `@deepseek-ai/cordis-plugin-hmr` | 3 |
| `@deepseek-ai/cordis-plugin-group` | 2 |

`vendor` is in the inventory's `VENDOR_DIRS`, so the walk never enters it. As a
result neither the package names nor the 35 source files are seen. In this
repo `vendor/*` is a first-party pnpm workspace member (`pnpm-workspace.yaml`).
With those 7 names known, the reported rate would be 3,975 / 11,262 = 35.30%
(computed from the step 1 per-ref data, not a separate run).

**Step 1b (approved after the stop).** The inventory now walks a directory
named in `VENDOR_DIRS` when a workspace glob covers it. Workspace globs come
from `package.json` `workspaces` or `pnpm-workspace.yaml`. `node_modules` is
never walked. Pattern reading is shared through
`profile::workspace_patterns` and `profile::glob_covers`.

- **Effect:** the walk adds 35 vendor source files and 107 more first-party
  refs.
- **The rate rose instead of falling to 35.3%.** That prediction was wrong.
  The existing workspace resolver already knew `vendor/*` (a single-level
  glob) and only lacked the files, so 1,432 vendor alias refs resolved at once.
- **The metric now matches ground truth.** codearch reports 48.05% against a
  classifier figure of 47.98%. The only difference is the 17 imports into
  excluded files, which the new metric deliberately leaves out.
- **Regression test:** `inventory::walks_vendor_dirs_only_when_a_workspace_declares_them`.

The ground-truth rate keeps the Phase 1 denominator in every row: first-party
refs, including the 17 imports into excluded files.

**Step 2: follow tsconfig `extends`** (`src/profile.rs`: `read_tsconfig`,
`load_tsconfig`, `extends_target`).

- **Chains:** `extends` is followed recursively. It can be a string or an array
  (later entries win), and recursion is capped at depth 16.
- **Where a parent is found:**
  - A relative specifier resolves against the extending config's directory,
    trying `X`, then `X.json`, then `X/tsconfig.json`.
  - A package specifier is looked up in `node_modules` from that directory up
    to the repo root. If none is found, it falls back to the repo package with
    that name. A bare package name uses its `package.json` `tsconfig` field or
    its `tsconfig.json`.
- **Merging:** the child's `baseUrl` and `paths` each replace the parent's.
- **Targets:** `paths` targets resolve against the effective `baseUrl` if set,
  otherwise against the directory of the config that declared them (TypeScript
  semantics). Before, targets were rebased onto the package dir and `baseUrl`
  was ignored.
- **Regression test:** `profile::tsconfig_extends_chains_carry_paths_aliases`
  covers a three-level relative chain, an array `extends`, a package path
  through a repo package, and a package path through `node_modules`.

What remains unresolved (209):

- Deep subpaths of workspace packages that no alias covers, e.g.
  `@deepseek-ai/dsh-client-locale/src/locales/zh.ts` (37).
- `/client`, `/types` and `/remote` subpath exports.
- 19 Python first-party imports.
- `@fixture/host` (7), a test-fixture workspace.

**Step 3: nested workspace globs, no cap** (`src/profile.rs`:
`discover_packages`, `expand_glob`).

- **Expansion:** workspace patterns expand one segment at a time. `*` matches
  within a segment and `**` matches any depth. `node_modules` and hidden
  directories are never entered.
- **No cap:** the 64-package limit is gone. Workspace packages found went from
  18 to 245.
- **Regression test:**
  `profile::nested_workspace_globs_expand_past_sixty_four_packages`. It builds
  70 packages under `packages/*/*` and 1 under `tools/**`, and checks that a
  manifest-bearing group directory and a `node_modules` package are left out.

**Step 4: Python source roots below the repo root** (`src/resolve.rs`:
`python_roots`, `top_python_packages`).

- **New roots:** the parent of every top-level Python package (a directory
  with `__init__.py` whose parent has none) is now an import root.
  `python/sdk/src` and `python/sdk-runtime/src` come from this rule, and all 19
  Python first-party imports now resolve.
- **Regression test:** `resolve::python_source_root_below_the_repo_root_resolves`.

### grep vs codearch token comparison, re-run after step 4

Command:
`python eval/grep_vs_codearch.py ~/deepseek-harness <target> --codearch-bin target/release/codearch.exe`.
Tokens are counted with the Gemini `countTokens` API. The README was not
edited.

| Target | grep context | codearch context | Ratio | codearch importers |
|---|---|---|---:|---:|
| README (old build, as published) | 148,757 tok, 7 rounds | 1,278 tok, 1 call | 116x | 57 |
| `chat-nodes.ts`, new build | 148,757 tok, 7 rounds, 2,503 files | 3,809 tok, 1 call | 39.1x | 173 |
| `submission-settings.ts`, new build | 150,222 tok, 8 rounds, 2,503 files | 3,772 tok, 1 call | 39.8x | 173 |

- **The README's target was very likely `chat-nodes.ts`.** Its grep figures
  (148,757 tok, 7 rounds) match the README's to the token. That is an
  inference; the README doesn't name the target.
- **Why the ratio fell from 116x to 39x:** codearch now lists 173 importers
  instead of 57, so its answer is about 3x longer.
- **One unexplained difference:** grep now converges on 2,503 files, not the
  README's 2,732, even though the token count is identical. I can't explain
  this from the script alone.
- **Side effect:** the script writes `~/deepseek-harness/.codearch`. The
  Sep 26 index that was there is backed up in the session scratchpad.

## Known limitations (step 5)

None of these is resolved by codearch, and the generated-file filter was not
changed.

| Limitation | Refs on deepseek-harness | Why it is not resolved |
|---|---:|---|
| Relative imports into files the inventory classifies as generated | 16 | The targets carry a "generated / do not edit" header, or have an SVG path line over the 2,000-byte minified-line threshold. They are counted as `excluded`, not as failures. |
| Relative import into untracked build output | 1 | `tsdown.config.ts` → `./packages/typert/generator/lib/types/tsdown-plugin.js`. The file exists on disk but is gitignored. Counted as `excluded`. |
| Alias import into a generated file | 1 | `@deepseek-ai/dsh-tool-cordis/src/api-catalog.ts`, from a test. The `excluded` check only covers relative specifiers, so this one counts as unresolved. |
| `exports` subpaths that point only at build output | 14 | `@deepseek-ai/*/remote` maps to `./lib/typert.remote-client.js`, which the typert generator writes at build time. No source file exists to resolve to. |
| Test-fixture workspaces nested in the repo | 8 | `@fixture/host` and `@fixture/domain` are packages of workspaces under `packages/typert/generator/tests/fixtures/*`, which the root workspace doesn't declare. Their names are known (so they count as first-party), but they aren't resolution targets. |
| Non-literal dynamic `import()` / `require()` | about 141 call sites (estimate) | The parser keeps only string-literal arguments, so these never become references and never enter the denominator. The count is a Phase 1 regex estimate. Most are `tests/*.e2e.ts` files loading built bundles by URL. |


## Phase 3: recall against a TypeScript ground truth

**Ground truth.** `node eval/ts_ground_truth.mjs ~/deepseek-harness truth.json`
uses the repo's own TypeScript 6.0.3:

- **Files:** all 2,737 tracked TS/JS files, excluding `.d.ts`.
- **Specifiers:** `ts.preProcessFile`. That covers imports, re-exports,
  `require`, literal `import()`, and `declare module` augmentations.
- **Resolution:** `ts.resolveModuleName`, using each file's nearest
  `tsconfig.json`, fully parsed by `getParsedCommandLineOfConfigFile`. So
  `extends`, `paths` and `moduleResolution: bundler` all come from TypeScript
  itself.
- **Build output:** a resolution into a package's `lib/` or a `.d.ts` file is
  mapped back to the tracked `src/` file when one exists.

Result: 11,587 source edges. 25 resolutions land only in build output, mainly
the 14 `/remote` exports.

**Comparison.**
`python eval/recall_vs_ground_truth.py truth.json ~/deepseek-harness <targets> --codearch-bin target/release/codearch.exe --codearch-dir <step-4 index>`.
It compares transitive importers on both sides.

**Targets.** The two README targets, plus three picked by ground-truth importer
count: the source file closest to 3 importers, closest to 30, and the one with
the most (2,063).

| Target | Truth | codearch | Both | Recall | Precision | Missed | Extra |
|---|---:|---:|---:|---:|---:|---:|---:|
| `packages/client/ui-conversation/src/client/contract/chat-nodes.ts` | 177 | 173 | 173 | 97.7% | 100.0% | 4 | 0 |
| `packages/client/ui-conversation/src/submission-settings.ts` | 190 | 173 | 173 | 91.1% | 100.0% | 17 | 0 |
| `packages/bundle/web-app/src/index.ts` | 3 | 3 | 3 | 100.0% | 100.0% | 0 | 0 |
| `packages/host/directory-picker/src/index.ts` | 30 | 30 | 30 | 100.0% | 100.0% | 0 | 0 |
| `vendor/cosmokit/src/misc.ts` | 2063 | 2057 | 2052 | 99.5% | 99.8% | 11 | 5 |

### Every missed file and its cause

There is one root cause: **module augmentation is not parsed.** A file
containing `declare module '<first-party module>' { … }` depends on that
module. TypeScript loads the module to merge the declarations, and
`preProcessFile` reports it, but codearch's parser records no reference.

- **Direct** misses contain the augmentation themselves.
- **Transitive** misses reach the target only through a direct miss.

No miss is caused by import resolution.

**`packages/client/ui-conversation/src/client/contract/chat-nodes.ts`** (4 missed)

| Missed file | Cause |
|---|---|
| `packages/client/ui-goal/src/client/GoalCommandInputView.tsx` | transitive, through `packages/client/ui-goal/src/client/goal-command-input.ts` |
| `packages/client/ui-goal/src/client/goal-command-input.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-workflow-run/src/client/WorkflowRunPanel.tsx` | transitive, through `packages/client/ui-workflow-run/src/client/workflow-definition.ts` |
| `packages/client/ui-workflow-run/src/client/workflow-definition.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |

**`packages/client/ui-conversation/src/submission-settings.ts`** (17 missed)

| Missed file | Cause |
|---|---|
| `packages/client/ui-conversation/src/client/conversation-nodes/assistant.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/command.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/compaction.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/fallback.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/message.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/register.ts` | transitive, through 10 direct misses (`assistant.ts`, …) |
| `packages/client/ui-conversation/src/client/conversation-nodes/retry.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/tool.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/turn-error.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/turn-max-tokens.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/src/client/conversation-nodes/turn-tail.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-conversation/tests/conversation-node-definitions.client.spec.ts` | transitive, through 10 direct misses (`assistant.ts`, …) |
| `packages/client/ui-goal/src/client/GoalCommandInputView.tsx` | transitive, through `packages/client/ui-goal/src/client/goal-command-input.ts` |
| `packages/client/ui-goal/src/client/goal-command-input.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |
| `packages/client/ui-goal/tests/goal-command-input.client.spec.tsx` | transitive, through 3 direct misses (`command.ts`, …) |
| `packages/client/ui-workflow-run/src/client/WorkflowRunPanel.tsx` | transitive, through `packages/client/ui-workflow-run/src/client/workflow-definition.ts` |
| `packages/client/ui-workflow-run/src/client/workflow-definition.ts` | direct: `declare module` `'@deepseek-ai/dsh-client-ui-conversation/client'` |

**`vendor/cosmokit/src/misc.ts`** (11 missed)

| Missed file | Cause |
|---|---|
| `packages/api/gateway/src/types.ts` | direct: `declare module` `'@deepseek-ai/cordis'` |
| `packages/api/remotes/src/types.ts` | direct: `declare module` `'@deepseek-ai/dsh-typert-protocol'` |
| `packages/credentials/authorization/src/types.ts` | transitive, through `packages/credentials/credentials/src/types.ts` |
| `packages/credentials/credentials/src/types.ts` | direct: `declare module` `'@deepseek-ai/cordis'` |
| `packages/hooks/hook-protocol/src/codec.ts` | transitive, through `packages/hooks/hook-protocol/src/types.ts` |
| `packages/hooks/hook-protocol/src/matcher.ts` | transitive, through `packages/hooks/hook-protocol/src/types.ts` |
| `packages/hooks/hook-protocol/src/merge.ts` | transitive, through `packages/hooks/hook-protocol/src/types.ts` |
| `packages/hooks/hook-protocol/src/types.ts` | direct: `declare module` `'@deepseek-ai/dsh-session/types'` |
| `packages/interaction/commands/src/types.ts` | direct: `declare module` `'@deepseek-ai/cordis'`, `'@deepseek-ai/dsh-session/types'` |
| `packages/settings/settings/src/types.ts` | direct: `declare module` `'@deepseek-ai/cordis'` |
| `packages/storage/storage-domain/src/events.ts` | direct: `declare module` `'@deepseek-ai/cordis'` |

### Every extra file and its cause

All 5 extras are importers of `vendor/cosmokit/src/misc.ts`. Each lives in a
test-fixture workspace under `packages/typert/generator/tests/fixtures/`.

- **What TypeScript does:** the fixture's own tsconfig maps
  `@deepseek-ai/cordis` and `@deepseek-ai/dsh-typert-protocol` to local
  declaration stubs (`cordis.d.ts`, `typert-protocol.d.ts`). The stubs have no
  implementation, so the ground truth has no edge to the real packages.
- **What codearch does:** it keeps one repo-wide alias table where the first
  registration wins, and the root's comes first. So it resolves the same
  specifiers to `vendor/cordis/src` and `packages/typert/protocol/src`.
- **Root cause:** `paths` aliases are global in codearch rather than scoped to
  the tsconfig that governs each file.

- `packages/typert/generator/tests/fixtures/remote-model/packages/domain/src/index.ts`
- `packages/typert/generator/tests/fixtures/remote-model/packages/remote/src/index.ts`
- `packages/typert/generator/tests/fixtures/type-model/packages/client/src/index.ts`
- `packages/typert/generator/tests/fixtures/type-model/packages/host/src/index.ts`
- `packages/typert/generator/tests/fixtures/type-model/packages/write/src/index.ts`

**Takeaways.**

- Precision is 100% on 4 of 5 targets and 99.8% on the largest.
- Recall is 100% on the two small targets and 91.1% to 99.5% on the large
  ones. Every miss comes from unparsed `declare module` augmentation.
- That is a parser gap, so the fix would belong in `src/parse.rs` rather than
  the resolver. It was not attempted, because it is outside the brief.

## Summary: before and after

| Metric (deepseek-harness) | Before (HEAD `956a936`) | After step 4 |
|---|---:|---:|
| Reported resolution | 99.57% (misleading) | 99.80% of first-party imports |
| Ground-truth first-party resolution | 35.24% | 99.65% |
| Unresolved first-party specifiers | 17 reported, about 7,300 real | 23 |
| Analyzed source files | 2,708 | 2,743 (`vendor/` workspace added) |
| Workspace packages discovered | 18 | 245 |
| `chat-nodes.ts` transitive importers | 57 | 173 (ground truth 177) |
| `submission-settings.ts` transitive importers | 57 | 173 (ground truth 190) |
| grep vs codearch tokens, `chat-nodes.ts` | 116x (old build) | 39.1x |

- **Tests:** `cargo test` passes 232 (225 before, plus 7 new regression tests).
  `pytest` passes 65. The clippy warning count is unchanged at 53.
- **Other eval repos:** import-edge counts are identical to HEAD on all six.
  react's reported rate moved from 97% to 96%: its own package names now count
  as first-party misses instead of external.
- **Not fixed (documented above):**
  - `declare module` augmentation is not parsed. It causes every recall miss.
  - `paths` aliases are global rather than scoped to each file's tsconfig. It
    causes the 5 extras.
  - `/remote` exports point only at build output.
  - Test-fixture workspaces nested in the repo aren't resolution targets.
  - The `parse::parse_all` bug is recorded in `eval/known_issues.md`.
