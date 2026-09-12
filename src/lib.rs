//! Code Arch — analyze a repository locally and emit a compact navigation map.
//!
//! Pipeline (M3 covers stages 0-10; the LLM labeler in stage 9 is optional):
//!
//! ```text
//! 0  Inventory   walk, classify, exclude
//! 1  Profile     manifests -> ecosystem and framework priors
//! 2  Parse       tree-sitter -> symbols, references
//! 3  Resolve     specifier strings -> concrete file targets
//! 4  Git         co-change pairs and per-file churn
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
pub mod confidence;
pub mod flows;
pub mod git;
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
use std::collections::HashMap;
use std::path::PathBuf;
use types::FileId;

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
    /// Skip stage 4 entirely. The map is then built from imports and directory
    /// structure alone, which is what the with-git / without-git comparison
    /// measures against.
    pub no_git: bool,
    /// Where `.codearch` outputs go (default: `<root>/.codearch`). The root map
    /// still names the canonical location; relocating is for tooling.
    pub codearch_dir: Option<PathBuf>,
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
            no_git: false,
            codearch_dir: None,
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
    pub imports_path: PathBuf,
    pub import_edges: usize,
    /// Measured size of the on-demand import index, outside the map budget.
    pub imports_tokens: usize,
    /// Co-change pairs that survived stage 4 filtering.
    pub cochange_pairs: usize,
    /// Commits stage 4 actually read. Zero means no usable git history.
    pub commits_read: usize,
    /// Size-weighted mean of the per-cluster confidence scores.
    pub confidence: f64,
    /// Clusters rendered in the low band.
    pub low_confidence_domains: usize,
    /// Entry→import-chain flows rendered in the map.
    pub flows: usize,
    /// The budget ladder dropped flows before shrinking file lists.
    pub flows_dropped: bool,
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

    // 4 — Git signals. Absent history is a degraded run, not a failed one.
    let cc = if opts.no_git {
        git::CoChange::default()
    } else {
        git::collect(&inv.root, &inv)
    };

    // 5 — Graph
    let g = graph::build(&inv, &res, &cc);

    // 6 — Cluster. With no edges at all there is nothing to cluster, which is
    // stage 6's declared failure path rather than an error.
    let used_directory_fallback = g.edge_count() == 0;
    let part = if used_directory_fallback {
        cluster::directory_partition(&inv)
    } else {
        cluster::partition_targeting(&g, opts.max_domains, opts.seed)
    };

    // 7 — Rank
    let scores = rank::score(&g, &routes, &cc.churn);

    // 8 — Confidence. Stability re-runs the clustering under other tie-break
    // orders, so it costs a few extra partitions and nothing else.
    let stability = confidence::file_stability(&g, &part, opts.max_domains, opts.seed);

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

    let conf: Vec<confidence::ClusterConfidence> = summaries
        .iter()
        .map(|c| confidence::for_cluster(&c.files, &res, &g, &stability, !cc.is_empty()))
        .collect();

    // 8b — Flows. High band only, per contract: entries best-first by
    // importance, capped, traversed intra-cluster. Medium/low bands and
    // entry-less domains get no flows rather than thin ones.
    let rels: Vec<String> = inv.files.iter().map(|f| f.rel.clone()).collect();
    let mut routes_by_file: HashMap<FileId, Vec<&str>> = HashMap::new();
    for r in &routes {
        routes_by_file.entry(r.file).or_default().push(r.label.as_str());
    }
    let domain_flows: Vec<Vec<flows::Flow>> = summaries
        .iter()
        .zip(&conf)
        .map(|(s, c)| {
            if c.band() != confidence::Band::High {
                return Vec::new();
            }
            let member_set: std::collections::HashSet<FileId> =
                s.files.iter().copied().collect();
            let mut entries: Vec<(FileId, &str)> = routes_by_file
                .iter()
                .filter(|(f, _)| member_set.contains(f))
                .flat_map(|(f, labels)| labels.iter().map(move |l| (*f, *l)))
                .collect();
            entries.sort_by(|&(a, _), &(b, _)| {
                scores[b]
                    .partial_cmp(&scores[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| rels[a].cmp(&rels[b]))
            });
            entries.truncate(flows::MAX_FLOWS);
            flows::domain_flows(&s.files, &entries, &g, &scores, &rels)
        })
        .collect();

    // 10 — Render. The import index is built once, outside the budget ladder:
    // the root map only advertises it.
    let paths: Vec<&str> = inv.files.iter().map(|f| f.rel.as_str()).collect();
    let imports = render::import_index(
        &render::map_title(&profile, &inv),
        &paths,
        &g.in_edges,
        res.resolution_rate,
        res.unresolved.len(),
    );
    let hubs = render::import_hubs(&paths, &g.in_edges, render::IMPORT_HUBS);
    let rendered = render::render_map(&render::MapInput {
        inv: &inv,
        profile: &profile,
        res: &res,
        parsed: &parsed,
        summaries: &summaries,
        labels: &labels,
        imports: &imports,
        hubs: &hubs,
        confidence: &conf,
        flows: &domain_flows,
        cochange_pairs: cc.has_history().then(|| cc.pairs.len()),
        budget: opts.budget,
    });

    let out_path = opts
        .out
        .clone()
        .unwrap_or_else(|| inv.root.join("CODEBASE.md"));
    std::fs::write(&out_path, &rendered.markdown)
        .with_context(|| format!("cannot write {}", out_path.display()))?;

    let dir = opts
        .codearch_dir
        .clone()
        .unwrap_or_else(|| inv.root.join(".codearch"));
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    // Always written: the root map points at it.
    let imports_path = dir.join("imports.md");
    std::fs::write(&imports_path, &imports.markdown)
        .with_context(|| format!("cannot write {}", imports_path.display()))?;

    let mut index_path = None;
    if opts.write_index {
        let p = dir.join("index.json");
        let json = render::index_json(
            &inv,
            &res,
            &summaries,
            &labels,
            &conf,
            &domain_flows,
            rendered.tokens,
        );
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
        imports_path,
        import_edges: imports.imports,
        imports_tokens: imports.tokens,
        confidence: confidence::global(
            &conf,
            &summaries.iter().map(|c| c.size()).collect::<Vec<_>>(),
        ),
        low_confidence_domains: conf
            .iter()
            .filter(|c| c.band() == confidence::Band::Low)
            .count(),
        flows: domain_flows.iter().map(|f| f.len()).sum(),
        flows_dropped: rendered.flows_dropped,
        cochange_pairs: cc.pairs.len(),
        commits_read: cc.commits_read,
        used_directory_fallback,
        truncated: rendered.truncated,
        over_domain_cap: summaries.len() > opts.max_domains,
        labels_fell_back: labeler.fell_back(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_writes_the_import_index_and_links_it_from_the_map() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/fixtures/tier1");
        let tmp = std::env::temp_dir().join(format!("codearch-imports-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let opts = Options {
            root: fixture,
            out: Some(tmp.join("CODEBASE.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: false,
            ..Options::default()
        };

        let report = run(&opts).unwrap();
        let map = std::fs::read_to_string(&report.out_path).unwrap();
        let index = std::fs::read_to_string(&report.imports_path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(report.imports_path, tmp.join("state").join("imports.md"));
        assert!(report.import_edges > 0);
        assert!(index.lines().any(|l| l.contains(" ← ")));
        let imports_at = map.find("## Imports").expect("map has an Imports section");
        let nav_at = map.find("## Task Navigation").unwrap();
        assert!(imports_at < nav_at);
        assert!(map.contains("`.codearch/imports.md`"));
        assert!(report.map_tokens <= DEFAULT_BUDGET);
    }

    fn flow_fixture(tmp: &std::path::Path) -> PathBuf {
        let root = tmp.join("flow-repo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app/shop")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name": "flow-shop", "dependencies": {"next": "*"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("app/shop/page.tsx"),
            "import { cart } from \"../../lib/cart\";\nexport default function Page() { return cart; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("lib/cart.ts"),
            "import { money } from \"./money\";\nexport const cart = money;\n",
        )
        .unwrap();
        std::fs::write(root.join("lib/money.ts"), "export const money = 1;\n").unwrap();
        root
    }

    #[test]
    fn run_renders_entry_flows_and_records_them_in_the_index() {
        let tmp = std::env::temp_dir().join(format!("codearch-flows-test-{}", std::process::id()));
        let root = flow_fixture(&tmp);
        let opts = Options {
            root: root.clone(),
            out: Some(tmp.join("CODEBASE.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: true,
            no_git: true,
            ..Options::default()
        };

        let report = run(&opts).unwrap();
        let map = std::fs::read_to_string(&report.out_path).unwrap();
        let index = std::fs::read_to_string(report.index_path.as_ref().unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(report.flows > 0, "expected at least one flow, got none");
        assert!(!report.flows_dropped);
        assert!(map.contains("Flows:"), "map has no Flows section");
        assert!(map.contains("→"), "no flow chain rendered");
        assert!(
            map.contains("import-chain traversals"),
            "caveat does not describe flows"
        );
        assert!(index.contains("\"flows\""), "index.json has no flows");
    }

    #[test]
    fn budget_pressure_drops_flows_before_file_lists() {
        let tmp = std::env::temp_dir().join(format!("codearch-flows-budget-{}", std::process::id()));
        let root = flow_fixture(&tmp);
        let full = Options {
            root: root.clone(),
            out: Some(tmp.join("full.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: false,
            no_git: true,
            ..Options::default()
        };
        let full_report = run(&full).unwrap();
        assert!(full_report.flows > 0);

        // Just under the full-map cost: flows must go, file lists must stay.
        let tight = Options {
            root,
            out: Some(tmp.join("tight.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: false,
            no_git: true,
            budget: full_report.map_tokens - 1,
            ..Options::default()
        };
        let tight_report = run(&tight).unwrap();
        let tight_map = std::fs::read_to_string(&tight_report.out_path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(tight_report.flows_dropped, "flows survived budget pressure");
        assert!(!tight_map.contains("Flows:"), "flows still rendered");
        assert!(tight_report.map_tokens <= tight.budget);
    }
}
