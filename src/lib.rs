//! Code Arch — analyze a repository locally and emit a compact navigation map.
//!
//! Pipeline (M0 covers stages 0-3, 5-7, 9, 10; git signals and flow extraction
//! arrive at M2 and M3):
//!
//! ```text
//! 0  Inventory   walk, classify, exclude
//! 1  Profile     manifests -> ecosystem and framework priors
//! 2  Parse       tree-sitter -> symbols, references
//! 3  Resolve     specifier strings -> concrete file targets
//! 5  Graph       weighted multi-signal graph
//! 6  Cluster     modularity clustering with a connectivity guarantee
//! 7  Rank        importance scoring
//! 9  Label       names and summaries          <- the only model stage
//! 10 Render      budget-aware assembly
//! ```
//!
//! Every stage but 9 is deterministic: a run on unchanged input reproduces
//! byte for byte.

pub mod cluster;
pub mod graph;
pub mod inventory;
pub mod label;
pub mod parse;
pub mod profile;
pub mod rank;
pub mod render;
pub mod resolve;
pub mod types;

use anyhow::{Context, Result};
use label::{DerivedLabeler, Label, Labeler};
use std::path::PathBuf;

/// A root map larger than this stops being cheaper than the repository.
pub const DEFAULT_BUDGET: usize = 4_000;

/// More domains than this is a listing, not a map.
pub const DEFAULT_MAX_DOMAINS: usize = 12;

/// Which stage 9 implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelerKind {
    /// Deterministic, no model, no network. The default.
    #[default]
    Derived,
    /// Local GGUF model via llama.cpp. Requires the `llm` build feature.
    Llm,
}

pub struct Options {
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub budget: usize,
    pub max_domains: usize,
    pub seed: u64,
    pub write_index: bool,
    pub labeler: LabelerKind,
    pub model_path: Option<PathBuf>,
    pub llm_threads: Option<i32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            root: PathBuf::from("."),
            out: None,
            budget: DEFAULT_BUDGET,
            max_domains: DEFAULT_MAX_DOMAINS,
            seed: 0x5EED,
            write_index: true,
            labeler: LabelerKind::Derived,
            model_path: None,
            llm_threads: None,
        }
    }
}

pub struct RunReport {
    pub files: usize,
    pub loc: usize,
    pub domains: usize,
    pub map_tokens: usize,
    pub source_tokens_estimate: usize,
    pub resolution_rate: f64,
    pub unresolved: usize,
    pub out_path: PathBuf,
    pub index_path: Option<PathBuf>,
    pub used_directory_fallback: bool,
    pub truncated: bool,
    /// The domain cap could not be met without merging groups of files that
    /// share no dependency at all. Reported rather than forced.
    pub over_domain_cap: bool,
    /// Clusters the chosen labeler could not name itself. Always 0 for the
    /// derived labeler; reported rather than hidden for the model path.
    pub labels_fell_back: usize,
}

pub fn run(opts: &Options) -> Result<RunReport> {
    // 0 — Inventory
    let inv = inventory::collect(&opts.root)?;
    if inv.is_empty() {
        anyhow::bail!(
            "no JavaScript or TypeScript source files found under {}",
            opts.root.display()
        );
    }

    // 1 — Profile
    let profile = profile::detect(&inv);
    let routes = profile::route_hints(&profile, &inv);

    // 2 — Parse
    let parsed = parse::parse_all(&inv);

    // 3 — Resolve
    let res = resolve::resolve_all(&inv, &parsed, &profile.mappings);

    // 5 — Graph
    let g = graph::build(&inv, &res);

    // 6 — Cluster. With no edges at all there is nothing to cluster, which is
    // stage 6's declared failure path rather than an error.
    let used_directory_fallback = g.edge_count() == 0;
    let part = if used_directory_fallback {
        cluster::directory_partition(&inv)
    } else {
        cluster::partition_targeting(&g, opts.max_domains, opts.seed)
    };

    // 7 — Rank
    let scores = rank::score(&g, &routes);

    // 9 — Label
    let summaries = label::summarize(&inv, &parsed, &res, &g, &part, &scores, &routes);
    let labeler: Box<dyn Labeler> = match opts.labeler {
        LabelerKind::Derived => Box::new(DerivedLabeler),
        #[cfg(feature = "llm")]
        LabelerKind::Llm => {
            let path = opts
                .model_path
                .as_ref()
                .context("--labeler llm requires --model <path.gguf>")?;
            let threads = opts.llm_threads.unwrap_or_else(|| {
                std::thread::available_parallelism().map_or(4, |n| n.get() as i32)
            });
            Box::new(label::llm::LlmLabeler::load(path, threads)?)
        }
        #[cfg(not(feature = "llm"))]
        LabelerKind::Llm => anyhow::bail!(
            "this binary was built without the `llm` feature.
Rebuild with: cargo build --release --features llm"
        ),
    };
    let mut taken: Vec<String> = Vec::new();
    let mut labels: Vec<Label> = Vec::with_capacity(summaries.len());
    // `summarize` already ordered domains most-important first, so the domain
    // a reader cares about most wins the unqualified name.
    for s in &summaries {
        let l = labeler.label(s, &taken);
        taken.push(l.name.clone());
        labels.push(l);
    }

    // 10 — Render
    let rendered = render::render_map(&render::MapInput {
        inv: &inv,
        profile: &profile,
        res: &res,
        parsed: &parsed,
        summaries: &summaries,
        labels: &labels,
        budget: opts.budget,
    });

    let out_path = opts
        .out
        .clone()
        .unwrap_or_else(|| inv.root.join("CODEBASE.md"));
    std::fs::write(&out_path, &rendered.markdown)
        .with_context(|| format!("cannot write {}", out_path.display()))?;

    let mut index_path = None;
    if opts.write_index {
        let dir = inv.root.join(".codearch");
        std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let p = dir.join("index.json");
        let json = render::index_json(&inv, &res, &summaries, &labels, rendered.tokens);
        std::fs::write(&p, json).with_context(|| format!("cannot write {}", p.display()))?;
        index_path = Some(p);
    }

    let source_bytes: u64 = inv.files.iter().map(|f| f.bytes).sum();

    Ok(RunReport {
        files: inv.len(),
        loc: inv.total_loc(),
        domains: summaries.len(),
        map_tokens: rendered.tokens,
        source_tokens_estimate: (source_bytes / 4) as usize,
        resolution_rate: res.resolution_rate,
        unresolved: res.unresolved.len(),
        out_path,
        index_path,
        used_directory_fallback,
        truncated: rendered.truncated,
        over_domain_cap: summaries.len() > opts.max_domains,
        labels_fell_back: labeler.fell_back(),
    })
}
