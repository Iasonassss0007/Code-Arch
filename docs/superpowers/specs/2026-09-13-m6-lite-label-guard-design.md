# M6-lite — Label guard: summary grounding + plural stemming

Date: 2026-09-13
Status: in implementation.
Implements: `code_arch_architecture.md` "Known defect, unfixed" (`validate.rs`
checks only the name) and the hono-2 stemming artifact (`Router` vs `Routers`).
Vehicle: the frozen 20-cluster reference set + `python eval/score_llm_labels.py`
against Qwen2.5-Coder-1.5B-Instruct Q4_K_M in `models/`.

## Problem

1. The generated *summary* reaches `CODEBASE.md` with no grounding check. The
   guard validates the name and lets the sentence underneath it through
   unvalidated; the fallback counter cannot see it.
2. Neither Rust `tokenize` nor Python `words()` stems. The model said `Router`
   for a `Routers` reference and lost a full specificity point to a plural.
   Stemmed, the recorded 55% would read 60%.

## Design

### Stemming (both sides, same rules)

`stem()`: `ies` → `y` (`utilities` → `utility`); trailing `s` stripped when
len > 3 and the word does not end in `ss`/`us` (`routers` → `router`,
`services` → `service`; `class`, `status`, `analysis` unchanged). Known edge:
`news` → `new`. Both sides stem, so errors are false accepts only, never
false rejects.

Rust: stem inside `tokenize` (evidence and candidate pass through it
together, so comparisons stay consistent). Python: stem inside `words()` in
`eval/run.py`. The scorer change versions the metric: v1 numbers
(75% / 55%) stay on record as superseded-but-kept; the sweep reports v2.
Both arms are re-scored by identical code in a single invocation
(`score_llm_labels.py` already re-runs derived), so the comparison stays fair.

### Summary guard (Rust only; the scorer already scores summaries)

`SUMMARY_BOILERPLATE` const mirroring Python `BOILERPLATE`, plus STOP-word
filtering for parity with `words()`. `check_summary()`: every summary token
must be in `evidence_tokens ∪ boilerplate` after stop removal. New
`Rejection::UngroundedSummary`.

`LlmLabeler::label` becomes two-stage: name reject → full derived fallback
(unchanged); summary reject → keep the generated name, use
`derive_summary(s)` (free function, no refactor). New `summary_fell_back`
counter: `Labeler` trait gains `summary_fell_back()` (default 0), report field
`labels_summary_fell_back`, additive `eval-support` JSON key, `main.rs` prints
it when > 0.

Cache: LLM labels are keyed by evidence hash, so bump the cache `FORMAT`
version — labels stored under the old guard must regenerate, never serve.

## Measurement

1. `cargo test` (existing + new) and `python -m pytest eval`.
2. Sweep re-run with the pinned model. Build needs the VS-bundled cmake on
   PATH and `LIBCLANG_PATH` set; inference is CPU minutes. Report the v2
   table (derived vs model, specificity / groundedness / collisions,
   both fallback rates).
3. Derived-path maps byte-identical (guard is LLM-only; existing tests assert).
4. Results into `code_arch_architecture.md` win or loss.

## Testing

Hermetic, no model: stem unit tests (routers, services, utilities, class,
status, analysis, news documented); summary-guard tests (grounded passes,
unlisted tech word fails, stopwords/boilerplate allowed); summary-only
fallback composition is 3 lines of glue in feature-gated `llm.rs`,
un-unit-testable without a model file — covered by the sweep run, not by
`cargo test`.

## Exit criteria

1. `cargo test` + `python -m pytest eval` pass.
2. Sweep re-run recorded as v2, win or loss.
3. Derived-path output byte-identical to pre-change binaries.
4. Architecture doc updated.

## Risks

**Guard too strict.** If `summary_fell_back` exceeds ~50%, the boilerplate
list is strangling fluent summaries; loosen and re-run. Rate reported always.

**Stemming over-strips.** `news` → `new` and friends conflate distinct words.
Consistent both sides, false accepts only. Documented here, not hidden.
