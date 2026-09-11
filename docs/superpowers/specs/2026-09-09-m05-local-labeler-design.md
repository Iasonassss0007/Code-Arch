# M0.5 — Local Model Labeler

Date: 2026-09-09
Status: implemented and measured. All five exit criteria met; the measured
outcome is a loss for the model — see the M0.5 section of
`code_arch_architecture.md` and `eval/labels-llm-report.md`.
Extends: `code_arch_architecture.md` stage 9

## Problem

Stage 9 ships `DerivedLabeler` only — derived names and templated summaries, no
model. This was the declared M0 fallback path, not the intended end state.
`src/label.rs:11` records the intent: "The llama.cpp implementation slots in
behind the same trait at M0.5, which is why the trait exists now rather than
later."

Two consequences follow from the gap:

1. The project's defining component is missing. `code_arch_core_idea.md:583-770`
   builds an argument for a small local LLM — structural work is done
   deterministically first, precisely so that a 0.5B-1.5B model is sufficient for
   what remains. None of that is exercised today.
2. The published labeling scores measure the wrong thing.
   `eval/labels-real-report.md` reports 75% name specificity and 90% lexical
   groundedness for the *templated fallback*. There is no model number to
   compare against.

## Goals

- Wire a local generative model behind the existing `Labeler` trait.
- Produce a measured head-to-head against `DerivedLabeler` on frozen references.
- Keep a fresh `git clone` buildable in one command with no C++ toolchain.
- Make no network connection from the shipped product, ever.

## Non-goals

- The eval harness consuming agent. `eval/openrouter_agent.py` is untouched by
  this work. Whether the impact benchmark's agent goes local is a separate
  decision, deliberately deferred until this work produces a number.
- Fine-tuning. `core_idea.md:755` floats a LoRA on Qwen2.5-Coder-0.5B; that is
  M2+ and out of scope.
- The code embedding model, a separate and cheaper component
  (`core_idea.md:764`).
- Any M2+ breadth: new languages, new output tiers, new stages.

## Constraints

**No network in the product.** Stated by the project owner: the final product
must not make API connections. This rules out an HTTP backend against Ollama or
llama.cpp's own `llama-server`, notwithstanding that a loopback socket is
arguably not a vendor API. Inference happens in-process, from a local file.

**The repository is going to be published.** A stranger cloning it must get a
working tool without first fixing a C++ build. This constraint drives the
feature-flag decision below, and it outranks purity of the default path.

**Hardware is CPU-only.** Measured on the development machine: i7-13620H
(10 cores / 16 threads), 32GB RAM, Intel UHD integrated graphics with 2GB, no
discrete GPU. A 4B Q4_K_M model measured 23.4 tok/s prefill and 6.7 tok/s
generation. Every design choice that trades tokens for quality is expensive
here, so the prompt stays small and the output stays short.

## Design

### Inference backend

`llama-cpp-2` (0.1.156) compiled into the binary. GGUF loaded from a path given
on the command line. No sockets, no daemon, no localhost. Fully offline once the
model file exists.

In-process buys one thing an HTTP backend could not: **GBNF grammar
constraints**. Output is constrained at the sampler to exactly the expected JSON
shape, so malformed generation is structurally impossible rather than something
to parse defensively and recover from. `architecture.md:460` specified this.

### Feature gating

The C++ dependency sits behind a Cargo feature, **off by default**.

```
cargo build                    # pure Rust. No CMake, no C++ toolchain.
                               # DerivedLabeler only. Byte-identical to today.
cargo build --features llm     # compiles llama.cpp, enables --labeler llm
```

This inverts the usual failure mode, where every cloner hits the CMake wall
whether or not they wanted local inference. Someone who just wants to see a map
of their repository never pays the toolchain cost. Someone who opts into the
model accepts a heavier build, which is a trade they chose.

`--labeler llm` on a binary built without the feature is a clean error naming the
required build command, not a silent downgrade. A silent downgrade would make it
impossible to tell whether a given map came from the model.

### Module structure

`src/label.rs` is 523 lines and gains a second implementation, so it splits:

```
src/label.rs            module root, unchanged: trait Labeler,
                        ClusterSummary, summarize(), DerivedLabeler
src/label/validate.rs   always compiled: grounding guard, prompt
                        construction, response parsing
src/label/llm.rs        #[cfg(feature = "llm")] model load, GBNF,
                        generation loop, LlmLabeler
```

Two corrections to the original plan, both made during implementation:

`src/label.rs` stays where it is rather than moving to `mod.rs`. Rust 2018+
allows a module root file to sit beside its own directory, so the 523 lines and
their 44 tests do not move at all. Same separation, no churn.

The pure logic lives in `validate.rs`, **not** in the feature-gated `llm.rs`.
The spec asked for hermetic tests that run without the feature; had the guard
been written inside `#[cfg(feature = "llm")]`, its tests would never run in the
default suite, which is precisely the code most worth testing.

This matches the tree already documented at `architecture.md:477`. The trait
signature does not change:

```rust
fn label(&self, summary: &ClusterSummary, siblings: &[String]) -> Label;
```

No `Result`, because "failure produces the derived label" is the documented
contract (`label.rs:5-7`), not an error the caller handles. `LlmLabeler` owns a
`DerivedLabeler` and delegates to it on every failure path.

`src/lib.rs:118` — `let labeler = DerivedLabeler;` — is the only call site that
changes, selecting an implementation from config.

### CLI

```
--labeler <derived|llm>   default: derived
--model <path.gguf>       required when --labeler llm
--llm-threads <n>         default: available_parallelism()
```

`--model` over a bundled or hardcoded path so the model is swappable, per
`core_idea.md:744`. The default stays `derived`, so existing behavior, existing
output and existing determinism are all unchanged unless explicitly asked for.

### Model

Qwen2.5-Coder-1.5B-Instruct, Q4_K_M, from `Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF`
(official upstream repository, ~1.1GB). Chosen in `core_idea.md:694` over newer
general small models because reasoning-tuning, multimodality and very long
context are all wasted or actively harmful for a short constrained-output task.

Not vendored into the repository. Documented as a download step.

### Prompt

One cluster per call. The model receives only the structured `ClusterSummary` —
directories, top symbols, entry points, external dependencies, and the sibling
names already taken — and never raw source. This preserves the invariant from
`label.rs:9`: "The model never sees source code and never invents a
relationship: every field of `ClusterSummary` is produced by an earlier
deterministic stage."

Sibling names are passed so the model can avoid collisions, the same input
`derive_name` already receives.

Output is a JSON object with `name` and `summary`, grammar-constrained.

### Grounding guard

`core_idea.md:445` is explicit that a confident wrong name is worse than no map,
because the agent will trust it and be actively misdirected. So a generated name
is not accepted on faith.

A name is rejected, and that cluster falls back to `derive_name`, when:

- no token in it traces lexically to the cluster's own dirs, symbols, entry
  points, or external dependencies
- it collides with a sibling name already assigned
- it is empty, or exceeds a new `MAX_NAME_CHARS` constant. `label.rs` has no
  name-length cap today (it caps symbols, entry points and externals but not
  names), so this constant is introduced by this work. Its value is set during
  implementation from the observed distribution of `derive_name` output across
  the eval corpus, so the guard cannot reject a length the derived path itself
  produces.

This is the same property `eval/score_real_labels.py` already measures as lexical
groundedness, so the guard and the metric agree by construction.

Rejections are counted and surfaced in `RunReport`, following the precedent set
by `over_domain_cap`: report the limit rather than hide it.

### Failure states

Every failure is local to one cluster and degrades to the derived label. A run
never blocks on the model and never aborts because of it.

| Failure | Behavior |
|---|---|
| Model file missing or unreadable | Clean startup error — the user asked for it explicitly |
| Model load failure | Clean startup error |
| Generation error on cluster N | Derived label for N, run continues |
| Ungrounded or colliding name | Derived label for N, run continues |
| Empty or oversized output | Derived label for N, run continues |

### Determinism

Temperature 0 and a fixed seed. The existing guarantee — byte-identical output
across runs — is preserved, but weakens from unconditional to conditional on
(model file, llama.cpp version). This is a real reduction and gets written into
the architecture document's deviation table rather than left implicit.

`--labeler derived` remains unconditionally deterministic.

### Performance

Roughly 12-16 clusters per repository, each about 400 prefill and 50 generated
tokens. At measured CPU rates, expect single-digit minutes per repository,
against 4.1s today for Hono (377 files).

This is a real wall-clock regression and it is accepted: it is one-time per
repository, the map persists, and `core_idea.md:1216` frames the whole design as
"analyze the repository locally once." It is also why `derived` stays the
default.

## Testing

Unit tests stay hermetic and must not require the feature, the toolchain, or the
model file. All 44 existing tests keep passing untouched.

- Trait-level tests use a fake `Labeler` returning scripted output, exercising
  validation and fallback logic with no inference.
- Grounding guard tests are pure functions over `ClusterSummary`: ungrounded name
  rejected, collision rejected, empty rejected, grounded name accepted.
- Prompt construction is tested as a pure string function.
- `#[cfg(feature = "llm")]` tests that actually load a model are marked
  `#[ignore]` and run explicitly, never in the default suite.

## Exit criteria

M0.5 is done when all of the following hold:

1. `cargo build` with no features succeeds, the 44 existing tests still pass
   unmodified, and the new hermetic tests pass alongside them.
2. `cargo build --features llm` succeeds on the development machine.
3. `codearch --labeler llm --model <path>` produces a map on Hono and on
   Next.js Commerce, and the run reports its per-cluster fallback count. A run
   that falls back on every cluster is a failed criterion, not a passing run
   with derived output.
4. `eval/score_real_labels.py` reports name specificity, lexical groundedness and
   sibling collisions for `LlmLabeler` on the frozen 20-cluster
   `clusters-real.json` references, head-to-head against DerivedLabeler's
   75% / 90% / 0%.
5. The result is written into `code_arch_architecture.md` whether it is a win or
   a loss.

Criterion 5 admits failure as an outcome. The purpose of M0.5 is to find out
whether a 1.5B model beats string manipulation at this task, not to ship a model
because the specification named one. A model that loses to the templated
fallback is a publishable finding and stays reported.

## Risks

**The `llama-cpp-sys-2` build is the single largest risk.** Confirmed heavier
than this spec first estimated. It needs *two* native prerequisites, not one:

- **CMake**, to compile llama.cpp. Found bundled with VS 2026 at
  `Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`, along with Ninja.
  No install needed on this machine, but it is not on PATH by default.
- **libclang**, because `llama-cpp-sys-2` generates its FFI bindings with
  bindgen. This was missed in the original estimate and is what actually failed
  the first build attempt:
  `Unable to find libclang: couldn't find any valid shared libraries matching:
  ['clang.dll', 'libclang.dll']`. No libclang shipped with VS here, so LLVM was
  installed separately (`winget install LLVM.LLVM`).

This raises the real cost of the opt-in feature for anyone cloning the
repository: CMake **and** LLVM, roughly 1GB of tooling, before
`--features llm` will build. It strengthens rather than weakens the case for
default-off, but the README must state both prerequisites plainly instead of
letting a cloner discover the second one from a bindgen panic.

**A 1.5B model may simply lose.** 75% specificity is not a low bar for a task
whose structural work is already done. Handled by exit criterion 5: the number
gets published either way.

**Feature-gated code rots.** Code behind a default-off feature is not compiled by
default and breaks silently. Mitigation: CI, once it exists, builds both
configurations; until then, both are built before any commit touching stage 9.

## Deviations from the original specification

| Original | This design | Why |
|---|---|---|
| Local LLM as a core component | Opt-in Cargo feature, default off | The repository is being published; a C++ build must not gate the default clone |
| Determinism unconditional | Conditional on model file and llama.cpp version | Inherent to generation; `derived` keeps the strong guarantee |
| (not specified) | Grounding guard rejects ungrounded names | `core_idea.md:445`: a confident wrong name is worse than no name |
