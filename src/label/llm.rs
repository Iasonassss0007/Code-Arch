//! Stage 9 — llama.cpp backed labeler.
//!
//! Job    Turn a ClusterSummary into a name and one sentence with a small
//!        local model.
//! In     ClusterSummary, sibling names already assigned
//! Out    Label — generated when it survives the guard, derived otherwise
//! Fails  Every failure is local to one cluster and yields the derived label.
//!        A run never blocks on the model and never aborts because of it.
//!
//! Compiled only under `--features llm`. Everything worth testing — prompt
//! construction, response parsing, the grounding guard — lives in
//! `super::validate`, which is compiled and tested unconditionally. What
//! remains here is FFI glue.

use std::cell::Cell;
use std::num::NonZeroU32;
use std::path::Path;

use anyhow::{Context as _, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel, Special};
use llama_cpp_2::sampling::LlamaSampler;

use super::validate;
use super::{ClusterSummary, DerivedLabeler, Label, Labeler, derive_summary};

/// Output shape, enforced at the sampler rather than checked after the fact.
///
/// Restricting the name to letters and spaces removes JSON escaping entirely:
/// the model cannot emit a quote or a backslash, so a complete reply always
/// parses. `parse_response` still guards the incomplete case.
const GRAMMAR: &str = r#"
root ::= "{\"name\": \"" name "\", \"summary\": \"" summary "\"}"
name ::= [A-Za-z] [A-Za-z ]{0,38}
summary ::= [A-Za-z0-9 ,.'()/-]{1,200}
"#;

/// Generation cap. The grammar bounds a well-formed reply far below this; the
/// cap only stops a pathological run.
const MAX_TOKENS: usize = 160;

/// Context window. The prompt is a short evidence list, never source code.
const N_CTX: u32 = 2048;

pub struct LlmLabeler {
    backend: LlamaBackend,
    model: LlamaModel,
    threads: i32,
    fallback: DerivedLabeler,
    generated: Cell<usize>,
    fell_back: Cell<usize>,
    summary_fell_back: Cell<usize>,
}

impl LlmLabeler {
    /// Load the model. This is the one failure the caller does not absorb: the
    /// user asked for `--labeler llm` explicitly, so a missing or broken model
    /// file is an error, not a silent downgrade to derived output.
    pub fn load(model_path: &Path, threads: i32) -> Result<Self> {
        let backend = LlamaBackend::init().context("initializing the llama.cpp backend")?;
        let model = LlamaModel::load_from_file(&backend, model_path, &LlamaModelParams::default())
            .with_context(|| format!("loading GGUF model from {}", model_path.display()))?;

        Ok(Self {
            backend,
            model,
            threads,
            fallback: DerivedLabeler,
            generated: Cell::new(0),
            fell_back: Cell::new(0),
            summary_fell_back: Cell::new(0),
        })
    }

    /// Clusters whose name came from the model.
    pub fn generated(&self) -> usize {
        self.generated.get()
    }

    /// Clusters that fell back to the derived name, for any reason.
    pub fn fell_back(&self) -> usize {
        self.fell_back.get()
    }

    /// Generated names kept with a derived summary.
    pub fn summary_fell_back(&self) -> usize {
        self.summary_fell_back.get()
    }

    fn generate(&self, prompt: &str) -> Result<String> {
        let template = self
            .model
            .chat_template(None)
            .context("model has no built-in chat template")?;
        let message = LlamaChatMessage::new("user".to_string(), prompt.to_string())?;
        let formatted = self
            .model
            .apply_chat_template(&template, &[message], true)?;

        let tokens = self.model.str_to_token(&formatted, AddBos::Always)?;

        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_threads(self.threads)
            .with_n_threads_batch(self.threads);
        let mut ctx = self.model.new_context(&self.backend, params)?;

        // Only the final prompt token needs logits; the rest is prefill.
        let mut batch = LlamaBatch::new(N_CTX as usize, 1);
        let last = tokens.len().saturating_sub(1);
        for (i, token) in tokens.iter().enumerate() {
            batch.add(*token, i as i32, &[0], i == last)?;
        }
        ctx.decode(&mut batch)?;

        // Grammar first, then greedy: temperature zero, so the only freedom the
        // sampler has is which grammar-legal token is most likely.
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::grammar(&self.model, GRAMMAR, "root")?,
            LlamaSampler::greedy(),
        ]);

        let mut out = String::new();
        let mut pos = tokens.len() as i32;

        for _ in 0..MAX_TOKENS {
            // `sample` accepts internally — see its doc comment. Calling
            // `accept` again advances the grammar through the same piece twice,
            // which drops every parse stack and makes llama.cpp abort the
            // process on the next apply (GGML_ASSERT(!stacks.empty())).
            let token = sampler.sample(&ctx, -1);

            if self.model.is_eog_token(token) {
                break;
            }
            out.push_str(&self.model.token_to_str(token, Special::Plaintext)?);

            batch.clear();
            batch.add(token, pos, &[0], true)?;
            pos += 1;
            ctx.decode(&mut batch)?;
        }

        Ok(out)
    }
}

impl Labeler for LlmLabeler {
    fn label(&self, s: &ClusterSummary, siblings: &[String]) -> Label {
        let raw = match self.generate(&validate::build_prompt(s, siblings)) {
            Ok(raw) => raw,
            Err(_) => return self.derived(s, siblings),
        };

        let Some((name, summary)) = validate::parse_response(&raw) else {
            return self.derived(s, siblings);
        };

        if validate::check(&name, s, siblings).is_err() {
            return self.derived(s, siblings);
        }

        // A good name with an ungrounded sentence keeps the name: the honest
        // sentence is the derived template, not a model invention.
        if validate::check_summary(&summary, s).is_err() {
            self.summary_fell_back.set(self.summary_fell_back.get() + 1);
            return Label {
                name,
                summary: derive_summary(s),
            };
        }

        self.generated.set(self.generated.get() + 1);
        Label { name, summary }
    }

    fn summary_fell_back(&self) -> usize {
        self.summary_fell_back.get()
    }
}

impl LlmLabeler {
    fn derived(&self, s: &ClusterSummary, siblings: &[String]) -> Label {
        self.fell_back.set(self.fell_back.get() + 1);
        self.fallback.label(s, siblings)
    }
}
