# M1 — evaluation before breadth

Run from the `codearch` directory:

```powershell
python eval/run.py
python -m unittest discover -s eval -p test_harness.py -v
cargo test --locked
```

Python 3.11+ and the existing Rust toolchain are sufficient. No Python packages,
model credentials, downloads or model calls are required for the default run.
The driver builds both Rust binaries, invokes the production map generator, and
uses its actual `DerivedLabeler` plus cl100k_base tokenization. The extra binary
is evaluation support; `cargo run -- <repository>` still selects Code Arch.

## What is measured

40 navigation tasks over 20 authored synthetic subsystems: explicit imports
(Tier 1), filesystem convention (Tier 2), and runtime registration (Tier 3).
These fixtures isolate visibility patterns; they are not functioning framework
applications or a representative enterprise sample. There are 20 exact-symbol
queries and 20 intent queries. Gold answers identify the implementation file.

Every task runs in fresh sessions with and without the full production map.
Order alternates, source access and action limits are identical, and the gold
answer never enters the policy transcript. Maps are generated in a temporary
location and are not written into the source tree. The index is never exposed.

The default policy is a **lexical navigation proxy**, not an LLM. It ranks map
file lines by query-word overlap; absent a match, it searches source lines. It
opens the best candidate and answers. Ties are lexical. It does not consult the
gold answer or retry based on correctness. Its deliberate simplicity means an
incorrect map can hurt it and its performance is not a coding-agent estimate.

Metrics in `results/report.json` and `results/report.md`:

- Tokens: cl100k_base over the initial task, complete file inventory, full map
  when present, tool results and action JSON, each counted once. No byte estimate
  or silent tokenizer fallback. This is context volume, **not billed API usage**,
  repeated conversation input, system prompts or hidden reasoning tokens.
- Files opened: distinct source files delivered through `open`.
- Searches: `search` calls, with at most 50 scored matching lines per response.
- Correctness: exact equality of the returned and gold file sets. Failures count
  against the denominator. This evaluates file location, not code changes.
- Primary decision: fewer total tokens with equal or higher accuracy. Results
  also retain the independent accuracy and search tradeoffs, plus tier splits.

Reports retain all action/observation traces, corpus hashes, and driver hash.
Synthetic examples sharing a subsystem are correlated, so there are no claimed
confidence intervals or population significance. No map improvement is made
based on these results in M1.

## Pinned real-repository smoke benchmark

The additional 12 source-inspected Hono tasks target implementation locations,
not test files or re-exports. Prepare the immutable checkout once:

```powershell
git clone https://github.com/honojs/hono.git eval/repos/hono
git -C eval/repos/hono checkout e7b38ee42bfa41f20194ad20fb949b625346be62
python eval/run.py --tasks eval/tasks-hono.json --out eval/results-hono
```

The runner rejects a changed commit or dirty checkout. Repository clones are
ignored by Git. Hono adds real Tier 1 coverage; real Tier 2/3 repositories and
LLM episodes remain necessary for an efficacy claim.

## Label evaluation

`clusters.json` contains 20 explicitly authored structured summaries, acceptable
names, sibling groups, and justified abbreviation aliases. It is a seed labeling
set, **not independently human-reviewed ground truth** and not 20 extracted
production clusters. Scores are:

- Name specificity: a normalized acceptable name and no sibling collision.
- Groundedness: every non-boilerplate lexical token in name and summary must
  trace to the input or an explicit alias. This conservative proxy can reject
  valid synonyms; it does not prove semantic truth or perform noun tagging.
- Collision rate: fraction of clusters whose normalized name duplicates a sibling.

To evaluate a candidate, supply a JSON array with exactly one
`{"id":"auth","name":"Authentication","summary":"..."}` per cluster:

```powershell
python eval/run.py --label-predictions candidate-labels.json --out eval/results-candidate
```

Accepted names and alias lists must be reviewed before looking at candidate
outputs. Do not expand them merely to make a candidate pass. The tests include
unsupported vocabulary and sibling collisions; the easy seed set's ceiling
score does not establish model quality.

## Real-agent adapter protocol

`--agent-command` accepts a JSON argv array for a trusted local adapter. Each
step receives `{"protocol":1,"messages":[...]}` on stdin and must emit exactly
one JSON action on stdout:

```json
{"tool":"search","query":"session expiry"}
{"tool":"open","path":"src/auth/service.ts"}
{"tool":"answer","files":["src/auth/service.ts"]}
```

The harness mediates repository reads and scores answers after the episode.
There are 12 actions per episode and a 120-second timeout per adapter invocation;
errors and exhausted episodes do not disappear from the score. An adapter must
use only the transcript, reset model conversation state per episode, keep model
and decoding settings fixed, and never inspect local gold files. It is a trusted
process, **not an OS sandbox**. Model/provider usage and adapter system-prompt
cost need separate accounting before making a billed-token claim.

## Actual LLM runs (M1 continuation)

`run_agent.py` runs the completed 24-task suite against three pinned public
repositories: Hono (explicit dependencies), Next.js Commerce (conventions), and
TypeDI (runtime service/decorator indirection). The immutable revisions and gold
locations are in `tasks-real.json`. Prepare missing checkouts using the URLs and
revisions in that file; the runner refuses dirty or mismatched checkouts.

```powershell
python eval/run_agent.py --repeats 2
python eval/run_agent.py --repeats 2 --resume
python -m unittest discover -s eval -p "test_*.py" -v
```

Requires `OPENROUTER_API_KEY` in the environment. Credentials are never written
to artifacts or included in transcripts. The default model is
`qwen/qwen3.5-flash-02-23`, temperature 0, seed 24301, reasoning disabled. Each
episode is a fresh conversation. Only task, inventory, optional map, and tool
observations reach the model; answers and usage metadata stay outside it.
Requests use the [OpenRouter chat API](https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request).

`results-agent/report.json` contains all 96 episodes, provider/model identifiers,
API usage, source/corpus/map hashes, system prompt, and per-tier summaries.
`report.md` is the compact result. `checkpoint.json` is saved after each episode;
resume requires identical inputs and code. Never overwrite previous runs to hide
failures. Provider failures stop the run with a checkpoint. Malformed actions
and exhausted action budgets remain scored failures.

Provider usage counts repeated context, system prompt, and completion tokens;
the original cl100k_base metric still counts delivered context once. These are
separate columns. USD is provider-reported, including caching effects. The
one-dollar default cap is checked between episodes, so a final episode can cross
it; missing usage stops further calls. No retries conceal additional costs.

A full file inventory is given to both arms. This is a meaningful path-list
baseline, but it makes descriptive-path lookup tasks easy. The suite measures
single-file navigation, not bug fixes, architectural reasoning, or broad
multi-file changes. Public repositories may also be familiar to the model.
Repeated trials are correlated and do not create 48 independent tasks. Do not
generalize the result beyond this model, corpus and access protocol.

## Real-cluster labeling set

`clusters-real.json` replaces the synthetic ceiling test for meaningful label
assessment. It contains the first ten ranked production clusters from Hono and
Commerce, preserving full source member paths and structured summaries. Inputs
were exported by `eval-support` with `{"op":"clusters","root":"..."}` before
label generation. Reference names were authored from those inputs, then reviewed
by GPT-4.1 Mini without candidate labels. `labels-reference-review.json` records
that review and hashes the frozen references. This is **model-reviewed data, not
human-reviewed ground truth**.

```powershell
python eval/score_real_labels.py
```

`labels-real-report.md` and `.json` score the actual `DerivedLabeler`. The blind
reference review is already saved; `review_labels.py` performs a new paid review
only when explicitly run. It is not needed to reproduce the fallback scores.
The lexical groundedness metric treats untraceable generic "Core" as a failure;
this flags lack of traceability, not a claim that the model hallucinated code.

## Head-to-head: local model vs derived labeler

`score_llm_labels.py` scores both labelers over the same frozen references in a
single invocation, so neither arm can be advantaged by a code change made
between runs. It re-runs the derived baseline rather than reading
`labels-real-report.json`, and never overwrites that frozen derived record.

```powershell
cargo build --features llm
python eval/score_llm_labels.py [path\to\model.gguf]
```

The model defaults to `models/qwen2.5-coder-1.5b-instruct-q4_k_m.gguf`. Writes
`labels-llm-report.md` and `.json`. Takes about two minutes on CPU for 20
clusters. Measured result: the derived labeler wins on name specificity, 75% to
55%; see the M0.5 section of `code_arch_architecture.md`.

`labels-llm-report-prompt-v1.md` and `.json` are the superseded first run, kept
deliberately. The only difference between the two is the wording of
`build_prompt` in `src/label/validate.rs`: v1 asked for "a word shown above" and
scored 40%, v2 asks for the most specific word and scores 55%. Fifteen points of
the original gap were prompt design, not model capability. Re-running the scorer
overwrites `labels-llm-report.*` but never the `-prompt-v1` copies.

Read the two groundedness figures carefully. The reported `groundedness` covers
**name and summary together** and requires every word to trace to an input
token, while the in-product guard in `src/label/validate.rs` checks **only the
name** and accepts it if **any** word traces. The generated summary is therefore
unvalidated in the product, and every one of the model's groundedness failures
under prompt v2 comes from summary prose rather than from the name — name-only
groundedness is 100%. `unsupported_tokens` in the JSON attributes each failure
token by token.

Treat the name+summary figure with care when comparing a generator against the
derived template. `BOILERPLATE` in `run.py:14` is the derived template's own
connective vocabulary, so the derived arm cannot fail the metric on prose while
any free-text sentence must. It measures conformity to that template, not
accuracy.

## Multi-model sweep (M6)

`score_sweep.py` scores the derived baseline plus every model in
`models-sweep.json` whose GGUF file is present, in one invocation by identical
v2 code. Missing files are skipped with the exact `hf download` command, never
a failure. The winner rule (specificity, then fewer summary fallbacks, then
smaller file) is fixed in the script, not chosen after seeing numbers.

```powershell
cargo build --features llm
python eval/score_sweep.py [--models ID ...] [--models-dir DIR]
```

Writes `labels-sweep-report.md` and `.json` (table, fallback rates, CPU wall
time per model, rule winner, gap to derived). Never touches
`labels-real-report.*` or `labels-llm-report.*`. Measured result on four
models: derived 75% still beats the best model (Qwen2.5-Coder-1.5B, 60%),
so `derived` stays the default. See the M6 section of
`code_arch_architecture.md` for the table and findings.

`build_lora_pairs.py` exports the frozen clusters as `lora-pairs.jsonl`
(`{input, completion}` in the labeler's exact shape, prompts stored
structured so rendering cannot drift). The references carry acceptable names
but no gold summaries, so this is the name-head seed format — summary
supervision needs teacher distillation over harvested production clusters,
which is deferred. Hermetic tests live in `test_sweep.py` (registry shape,
winner rule, markdown rendering, pair export; no model, no build, no
network).

## Exit status

Actual paired LLM runs and a measured real-cluster labeling baseline now exist.
Read `results-agent/report.md` for the measured answer to whether the map helps
under this protocol. Human label review and more demanding downstream tasks
remain limitations, not completed work. No M2+ production features were added.

## M1v2 — impact benchmark (`tasks-nav-gated.json`)

The first task set could not measure the hypothesis. 22 of its 24 answers
contained a query token in their own path ("find the file for **cors**" ->
`src/middleware/**cors**/index.ts`), and `Session` hands both arms the complete
file listing, so the baseline was already holding a 100%-coverage index. It
scored 46/48 with ~1.06 files opened and ~0.04 searches per episode: no
exploration to reduce, no headroom to win.

The replacement asks for transitive impact -- "the behavior of X is changing,
what else needs review?" -- which no path match reveals and which a grep-only
agent can assemble only by walking the chain a round at a time.

    # --ts-config is required: without it madge drops bare baseUrl imports and Commerce lost 82 of 121 edges (fixed 2026-09-15)
    npx --yes madge@8 --json --extensions ts,tsx,js,jsx,mts,cts,mjs,cjs --ts-config repos/<r>/tsconfig.json repos/<r> > oracle/<r>.json
    python build_tasks_nav.py --out tasks-nav.json --per-repo 500
    python gate_tasks_nav.py --tasks tasks-nav.json --out tasks-nav-gated.json
    python run_agent.py --tasks tasks-nav-gated.json --out results-nav

Ground truth comes from madge, a third-party dependency-graph tool, so codearch
is not graded against its own resolver. madge scans the whole checkout, not
just `src/`. The task asks for every dependent, and the agent may answer any
inventoried path. A `src/`-only oracle therefore marked correct answers in
benchmarks and tests as wrong. That was the first oracle, replaced 2026-09-11.

One known gap remains. madge 8 lists `.mts`/`.mjs` files but does not parse
their imports: 0 of Hono's 29 such files have an edge, while codearch finds 52.
Dependents reachable only through those files are missing from ground truth.
In the ceiling this affects 2 of 37 tasks.

**Every task must beat a free adversary.** `gate_tasks_nav.py` gives two
heuristics the query and the path inventory -- nothing else -- and the answer's
cardinality, then admits a task only if both score F1 <= 0.35. `path_guess`
ranks files by shared path tokens; `hub_guess` returns the most-imported files.
Of 81 candidates, 37 survive (mean adversary F1 0.13, max 0.33). The first
gated set, built from the `src/`-only oracle, kept 24.

Scoring is set F1, not exact match: answers hold 2-20 files, and exact match
would floor every arm at zero -- the mirror of the ceiling effect that broke
the first run.

Known limits: typedi (39 graph nodes) contributes only four tasks. In a
repository that small the most-imported files are often the answer, so
`hub_guess` wins by construction. Small repositories host few tasks of this type. Test files
count as legitimate dependents. Impact is one verb; flow and
convention-indirection tasks are not built yet.

### Reverse import index: ceiling and harness changes

    python check_import_index.py --tasks tasks-nav-gated.json --out results-nav-ceiling

This walks `.codearch/imports.md` from each task's target, with no model
involved. Current result on the 37 tasks: recall 1.000 on every task, and mean
F1 0.981. See `results-nav-ceiling/report.md` for per-task rows.

Against the earlier `src/`-only oracle, the same check scored F1 0.873. That gap
was the oracle's fault, not the index's. The index found real dependents in
Hono's benchmarks and TypeDI's `test/` that madge had never scanned, and they
counted as wrong answers. That is why madge now scans the whole checkout.

`run_agent.py` changes:

- It generates maps with `--codearch-dir` into a temp directory, so pinned
  checkouts stay clean.
- It serves `.codearch/imports.md` to the `with_map` arm only, through
  `Session(map_files=...)`.
- `kind: impact` tasks get `SYSTEM_IMPACT`, which counts test files as
  dependents. `SYSTEM` told the agent they do not qualify.

`SYSTEM` itself is unchanged, so M1's prompt reproduces. A rerun of M1 with
today's binary still serves the new map and index, though, so it is not the
recorded M1 condition.

### Agent providers

    python run_agent.py --tasks tasks-nav-gated.json --out results-nav --provider gemini --model gemini-3.6-flash

The two providers are:

- `--provider openrouter` is the default, using `OPENROUTER_API_KEY` and
  Qwen3.5 Flash. M1 ran on it.
- `--provider gemini` uses the Google AI Studio free tier (`GEMINI_API_KEY`,
  `gemini_agent.py`). Usage is recorded at cost 0.0. Google's free-tier terms
  let submitted content be used to improve Google's products. The benchmark
  sends public repository code, the generated map and the index.

Only models whose settings were probed live are accepted (`THINKING` in
`gemini_agent.py`):

- `gemini-2.5-flash` is closed to new users.
- `gemini-3.6-flash` rejects a zero thinking budget, so it runs with
  `thinkingLevel: minimal`.
- `gemini-3.1-flash-lite` runs with thinking off.

Calls are spaced 6.5 s apart (`CODEARCH_GEMINI_MIN_INTERVAL`). 429, 500 and
503 responses and timeouts are retried with backoff; anything that persists
stops the run as a provider failure, and `--resume` continues it.

Three live-probe findings are built in:

- **Reply schema.** JSON mode alone let `gemini-3.6-flash` answer
  `{"action": ...}` instead of `{"tool": ...}`, so requests carry a
  `responseSchema` for the action shape.
- **Reply limit.** 512 output tokens, inherited from M1's one-file answers,
  cut off a 19-file answer mid-JSON. Raising the Gemini limit to 2048 did not
  fix it: the next attempt generated 2,056 tokens for an answer that needs
  roughly 300 and was cut off again (probably a repetition loop; not verified).
  `openrouter_agent.py` still uses 512 and would truncate impact answers too.
- **Failure detail.** An unparseable reply records its `finishReason` and a
  text excerpt in the checkpoint.

`gemini-3.1-flash-lite` answered in about a second but explored almost nothing.
In three practice episodes it ran one search, then answered, never opening the
index (F1 0.10 on a 19-file task). A benchmark on it measures an agent that
stops early, not the map.

`gemini-3.6-flash` explores properly: it searched, opened the index and began
listing correct files. But neither practice episode on the 19-file task
produced a scoreable answer:

- **With the map**, 168 s: the answer was cut off at the output limit, as above.
- **Without the map**, 2,245 s: calls took 10-65 s each, then one call stalled
  through every transport retry and failed.

At that speed and reliability, 148 episodes would take well over 12 hours with
repeated stalls. As of 2026-09-11 the free tier is not a workable agent for
M1v2.

The NVIDIA API free tier (`integrate.api.nvidia.com`, OpenAI-compatible) was
probed the same day. No adapter was built, because no model produced valid
actions:

- `openai/gpt-oss-20b` answered a one-hop index lookup correctly in 8.8 s. In
  real episodes it returned empty `content`, with its move written inside
  `reasoning_content`, often without the `tool` key. That happened with JSON
  mode, without it, and with `reasoning_effort: low`.
- `nvidia/nemotron-3-super-120b-a12b` did the same lookup correctly in 13.3 s,
  but ignored JSON mode in episodes: it wrote prose into `content` until it hit
  the 4096-token limit, after 168 s. The endpoint rejects
  `nvext.guided_json` (HTTP 400, unknown field), and it sometimes answers 503
  "overloaded".
- `deepseek-ai/deepseek-v4-flash-0731` timed out on a 10k-token prompt.
- `mistral-large-2-instruct` and `kimi-k2.6` are listed but return 404 for a
  free account.
