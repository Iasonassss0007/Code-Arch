# codearch

Analyze a repository locally and write a compact map of it, so a coding agent
can start from a mental model of the project instead of rediscovering it by
reading files.

Everything runs on your machine. The tool makes no network connection.

## Build

```
cargo build --release
```

Pure Rust, no native toolchain required. This is the whole setup for the
default configuration.

## Use

```
codearch /path/to/repo
```

Writes `CODEBASE.md` at the repository root, `.codearch/imports.md` (a reverse
import index: for every file, each file that imports it, read on demand and
outside the map budget), and `.codearch/index.json` unless `--no-index` is passed.

```
--out <path>          where to write the map (default: <repo>/CODEBASE.md)
--budget <n>          hard token budget for the map (default: 4000)
--max-domains <n>     maximum top-level domains (default: 12)
--seed <n>            clustering seed; changes tie-breaks only
--no-index            skip .codearch/index.json
--codearch-dir <path> write index.json and imports.md here (default: <repo>/.codearch)
```

Currently parses TypeScript, JavaScript and Python.

## Optional: local model labeling

By default, domain names are derived deterministically from directory
structure, symbols and dependencies. No model is involved and output is
byte-identical across runs.

A small local model can name domains instead. It is **opt-in**, because it
needs a native toolchain that the default build does not.

### Prerequisites

Both are required, and both are only needed for this feature:

1. **CMake** — builds llama.cpp. Visual Studio bundles one at
   `Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin` (add it to PATH);
   otherwise install CMake directly.
2. **LLVM / libclang** — `llama-cpp-sys-2` generates its bindings with bindgen,
   which needs `libclang.dll`. Visual Studio does not ship it by default.

   ```
   winget install LLVM.LLVM          # Windows
   ```

   If the build reports `Unable to find libclang`, set `LIBCLANG_PATH` to the
   directory containing the library (e.g. `C:\Program Files\LLVM\bin`).

### Build and run

```
cargo build --release --features llm
```

Download a GGUF model — the tested one is Qwen2.5-Coder-1.5B-Instruct Q4_K_M
(~1.1GB) from
[Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF):

```
codearch /path/to/repo --labeler llm --model /path/to/model.gguf
```

`--llm-threads <n>` sets inference threads; the default is all available cores.

### What the model is and is not allowed to do

The model never sees source code. It receives only structured facts an earlier
deterministic stage produced — directory names, top symbols, entry points,
external dependencies — and returns a name and one sentence, constrained by a
GBNF grammar.

Every generated *name* is then checked against that same evidence. A name whose
words trace to nothing in the cluster is discarded and the derived name used
instead, as are names that collide with a sibling or exceed the length cap. A
confident but wrong name is worse than a plain one, because the agent will trust
it. The run reports how many domains fell back.

The generated *summary* is not checked. This is a known gap, not a design
choice: the guard validates the name and lets the sentence underneath it through
unvalidated. Measurement shows that is where the model's untraceable vocabulary
actually appears.

Three consequences worth knowing before enabling it:

- **It is slower.** Inference is CPU-bound. Expect minutes per repository rather
  than seconds. It is a one-time cost — the map persists.
- **Determinism becomes conditional.** Output is reproducible for a given model
  file and llama.cpp version, rather than unconditionally. `--labeler derived`
  keeps the stronger guarantee.
- **It currently produces worse names.** Measured head-to-head against the
  derived labeler on 20 frozen production clusters, Qwen2.5-Coder-1.5B scores
  55% name specificity against the derived path's 75%. Its remaining failures
  are naming a cluster after one of its symbols (`Benchmarks Jsx` becomes
  `Content`) and, in one case, still preferring the parent directory to the leaf
  (`Etag` becomes `Middleware`). Every name it produces does trace back to the
  cluster's own evidence, at a rate slightly better than the derived path's.
  Full result in `code_arch_architecture.md`; reproduce with
  `python eval/score_llm_labels.py`.

The feature ships default-off, and `derived` stays the recommendation. It is
kept because the finding is worth reproducing on other models and corpora, not
because it currently wins.

## Status

M0–M5 are built and tested: 160 Rust tests + 44 Python tests pass.
M5 splits a 4,367-file React checkout from a 236,702-token flat map to a
2,957-token root plus domain files, with warm re-runs in ~10s.
What the maps are worth is reported in `code_arch_architecture.md` rather
than summarized favourably here — including a negative one: on a 24-task
file-location benchmark, supplying the full map did not improve a coding
agent's accuracy and roughly doubled token use. The M1v2 agent run and the
transitive-impact replacement are built but still gated on provider credit.

Treat this as a working tool whose central benefit is still unproven.
