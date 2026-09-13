# M6 — Multi-model sweep and LoRA path

Date: 2026-09-13
Status: in implementation.
Implements: `code_arch_architecture.md` build order M6 ("model choice made from
the M1 numbers rather than from reasoning") and `code_arch_core_idea.md`
model-selection candidates plus the LoRA fine-tune direction.
Vehicle: the frozen 20-cluster reference set + `python eval/score_sweep.py`.

## Problem

M6-lite closed the guard and stemming gaps on a single model
(Qwen2.5-Coder-1.5B, 60% vs 75% specificity). The core idea names four
candidates to benchmark against the default, and a LoRA fine-tune path as the
longer-term direction. Neither has been run. Model choice is still reasoning,
not numbers.

## Sweep set

All Q4_K_M, all with a built-in chat template (required by `LlmLabeler::generate`,
which formats through `apply_chat_template`). File names verified against the
Hugging Face API on 2026-09-13; registry lives in `eval/models-sweep.json`.

| id | repo | file | license | role |
|---|---|---|---|---|
| qwen25-coder-1.5b | Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF | qwen2.5-coder-1.5b-instruct-q4_k_m.gguf | Apache-2.0 | incumbent default, already in `models/` |
| qwen25-coder-0.5b | lmstudio-community/Qwen2.5-Coder-0.5B-Instruct-GGUF | Qwen2.5-Coder-0.5B-Instruct-Q4_K_M.gguf | Apache-2.0 | LoRA base candidate; does the smallest code model hold? |
| llama32-1b | bartowski/Llama-3.2-1B-Instruct-GGUF | Llama-3.2-1B-Instruct-Q4_K_M.gguf | Llama 3.2 Community | fastest-on-CPU candidate from the core idea; non-Apache license noted |
| gemma2-2b-it | bartowski/gemma-2-2b-it-GGUF | gemma-2-2b-it-Q4_K_M.gguf | Gemma | general-model contrast; tests the code-pretraining claim head-on |

Excluded with reason, not silently: **CodeGemma 2B** ships as a base model
only (chat/instruct exists only at 7B). With no chat template the labeler's
prompt formatting has nothing to format through, so it cannot run behind the
current trait without a separate base-model prompting path — out of scope,
recorded here.

## Method

`eval/score_sweep.py` scores the derived baseline plus every registry model
whose file is present, in a single invocation, by identical v2 code:

- Same frozen `clusters-real.json`, same blind-review hash gate, same
  `evaluate_labels` scorer as `score_llm_labels.py` (which stays untouched as
  the single-model record).
- Same prompt v2, same name + summary guard, same stemmed metric.
- Missing model files are reported as skipped with the exact
  `huggingface-cli download` command to reproduce — never a failure.
- Wall seconds per model are recorded. CPU minutes are a first-class cost for
  this tool (M0.5 constraint), so speed is reported beside quality.

Winner rule, stated before the run: highest name specificity; tie-breaks are
lower summary-fallback rate, then smaller file size, then id order. The script
prints the rule winner and its gap to the derived baseline. The architecture
doc records whether that winner changes the default — the script recommends,
a human decides.

## LoRA path

Training itself is out of scope (needs a teacher model, GPU time, and a
harvested multi-repo pair set). The concrete M6 artifact is
`eval/build_lora_pairs.py`: it exports the frozen clusters into JSONL
`{prompt, completion}` pairs in the exact shape the labeler consumes, one
completion per acceptable reference name, with derived-template summaries
standing in where no gold summary exists. That is the name-head seed format,
not a full SFT set — summary supervision needs teacher distillation, stated
not hidden. A real fine-tune starts by running this exporter across harvested
production clusters, not by redesigning it.

## Exit criteria

1. `cargo test` + `python -m pytest eval` pass (new tests are hermetic: no
   model, no build, no network).
2. Sweep runs for every downloaded model; any missing model is listed with its
   reproduce command.
3. `eval/labels-sweep-report.{json,md}` records the table, per-model fallback
   rates, timing, rule winner, and gap to derived.
4. Architecture doc records the model decision, win or loss, with
   `--labeler derived` staying the default unless a candidate beats it on
   specificity.
5. No change to the derived path or the single-model scorer.

## Risks

**Downloads.** ~2.8 GB total for the three new models. All three repos are
ungated; `huggingface-cli download` resumes cleanly. If the network fails, the
sweep still reports the incumbent plus skipped entries — a partial sweep with
reproduce commands, not a blocked milestone.

**Chat-template drift.** Non-Qwen templates format the same evidence text;
the prompt is plain user content, so the comparison stays fair. A model whose
template errors surfaces as a per-model error row, not a harness failure.

**Small reference set.** 20 clusters cannot separate close models with
authority. The sweep reports raw counts beside percentages so a 1-cluster gap
reads as what it is.
