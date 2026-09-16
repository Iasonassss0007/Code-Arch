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

pub mod agents;
pub mod cluster;
pub mod cache;
pub mod confidence;
pub mod contract;
pub mod flows;
pub mod git;
pub mod graph;
pub mod inventory;
pub mod label;
pub mod mcp;
pub mod parse;
pub mod profile;
pub mod query;
pub mod rank;
pub mod render;
pub mod resolve;
pub mod split;
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
    /// Trained LoRA adapter applied on top of `model_path` (M6 path).
    pub lora_path: Option<PathBuf>,
    /// llama.cpp `--lora-scale`; 1.0 is the trained strength.
    pub lora_scale: f32,
    /// Build the full map (`CODEBASE.md`, `index.json`). When false, the run
    /// stops after contracts and writes only the lookup indexes. The library
    /// default stays `true`; the CLI defaults to indexes only (`--map` opts in).
    pub map: bool,
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
            lora_path: None,
            lora_scale: 1.0,
            map: true,
        }
    }
}

/// Per-stage wall clock, printed to stderr when `CODEARCH_TIME` is set.
/// Zero overhead otherwise (one env read per run, one Instant per stage).
struct Timer {
    enabled: bool,
    last: std::cell::Cell<std::time::Instant>,
}

impl Timer {
    fn new() -> Timer {
        Timer {
            enabled: std::env::var_os("CODEARCH_TIME").is_some(),
            last: std::cell::Cell::new(std::time::Instant::now()),
        }
    }

    fn done(&self, stage: &str) {
        if self.enabled {
            let now = std::time::Instant::now();
            eprintln!("time {:<12} {:>8.2}s", stage, (now - self.last.get()).as_secs_f64());
            self.last.set(now);
        }
    }
}

/// Identity of the generative labeler: model path plus size and mtime, so a
/// swapped model file invalidates stored labels without hashing gigabytes.
fn model_identity(path: &PathBuf) -> String {
    let sig = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{}:{size}:{sig}", path.display())
}

/// Cache key for one LLM label: everything the label may depend on, and
/// nothing it cannot see. Member rels are sorted — importance order shifts
/// with scores, membership is what the label describes.
fn label_evidence_key(
    inv: &inventory::Inventory,
    s: &label::ClusterSummary,
    model: &str,
) -> String {
    let mut rels: Vec<&str> = s.files.iter().map(|&f| inv.get(f).rel.as_str()).collect();
    rels.sort_unstable();
    cache::sha_hex(
        format!(
            "llm|{model}|{}|{}|{}|{}|{}",
            rels.join(","),
            s.dirs.join(","),
            s.top_symbols.join(","),
            s.entry_points.join(","),
            s.external_deps.join(","),
        )
        .as_bytes(),
    )
}

/// One label through the shared cache-or-compute path. Used by the flat run
/// and the split run alike, so names agree wherever the same evidence meets
/// the same `taken` order.
fn label_one(
    labeler: &Box<dyn label::Labeler>,
    store: &mut cache::Store,
    llm_model_id: &Option<String>,
    inv: &inventory::Inventory,
    s: &label::ClusterSummary,
    taken: &mut Vec<String>,
) -> Label {
    let key = llm_model_id
        .as_ref()
        .map(|model| label_evidence_key(inv, s, model));
    if let Some(cached) = key.as_ref().and_then(|k| store.labels_llm.get(k)) {
        taken.push(cached.name.clone());
        return Label {
            name: cached.name.clone(),
            summary: cached.summary.clone(),
        };
    }
    let l = labeler.label(s, taken);
    if let Some(k) = key {
        store.labels_llm.insert(
            k,
            cache::StoredLabel {
                name: l.name.clone(),
                summary: l.summary.clone(),
            },
        );
    }
    taken.push(l.name.clone());
    l
}

/// Fingerprint for the split-verdict memo: file set and contents, git HEAD
/// (or its absence), and every option the verdict can depend on. Sorted so
/// HashMap order never leaks into the decision.
fn split_fingerprint(
    store: &cache::Store,
    opts: &Options,
    llm_model_id: &Option<String>,
) -> String {
    let mut parts: Vec<String> = store
        .files
        .iter()
        .map(|(rel, f)| format!("{rel}:{}", f.hash))
        .collect();
    parts.sort();
    parts.push(format!(
        "git:{}",
        store.git.as_ref().map(|g| g.head.as_str()).unwrap_or("none")
    ));
    parts.push(format!("nogit:{}", opts.no_git));
    parts.push(format!("budget:{}:{}", opts.budget, opts.max_domains));
    parts.push(format!("seed:{}", opts.seed));
    parts.push(format!(
        "labeler:{:?}:{}",
        opts.labeler,
        llm_model_id.as_deref().unwrap_or("")
    ));
    cache::sha_hex(parts.join("\n").as_bytes())
}

pub struct RunReport {
    pub files: usize,
    pub loc: usize,
    pub domains: usize,
    pub map_tokens: usize,
    pub source_tokens_estimate: usize,
    pub resolution_rate: f64,
    pub unresolved: usize,
    /// Whether this run built the map. Index-only runs leave every map field
    /// (domains, flows, confidence, git, labels) at zero.
    pub map: bool,
    /// `None` on index-only runs.
    pub out_path: Option<PathBuf>,
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
    /// The flat map exceeded budget: root routes to domain files.
    pub split: bool,
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
    /// Generated names kept with a derived summary (summary guard only).
    pub labels_summary_fell_back: usize,
    /// Cross-language contract pairs fused into the graph: exact URL matches
    /// plus shared rare symbol shapes.
    pub contracts: usize,
    /// The URL-contract subset (exact normalized-path matches).
    pub url_contracts: usize,
    /// The semantic subset (shared rare symbol shapes across languages).
    pub semantic_contracts: usize,
    /// Backend views with at least one frontend caller (`contract::route_join`).
    pub route_views: usize,
    /// Written only when `route_views > 0`.
    pub routes_path: Option<PathBuf>,
}

/// The reverse import index and its hubs. Import-only directed edges: the
/// graph's `in_edges` also carries contract pairs (flows and coupling
/// traverse them), but the index's contract is imports — "every analyzed file
/// that imports it" — and the navigation ceiling measurement assumes it.
fn import_index_and_hubs(
    inv: &inventory::Inventory,
    profile: &profile::Profile,
    res: &resolve::Resolution,
) -> (render::ImportsIndex, Vec<(String, usize)>) {
    let paths: Vec<&str> = inv.files.iter().map(|f| f.rel.as_str()).collect();
    let mut import_in: Vec<Vec<usize>> = vec![Vec::new(); inv.len()];
    for &(from, to) in &res.edges {
        if from < inv.len() && to < inv.len() && from != to {
            import_in[to].push(from);
        }
    }
    for row in &mut import_in {
        row.sort_unstable();
        row.dedup();
    }
    let imports = render::import_index(
        &render::map_title(profile, inv),
        &paths,
        &import_in,
        res.resolution_rate,
        res.unresolved.len(),
    );
    let hubs = render::import_hubs(&paths, &import_in, render::IMPORT_HUBS);
    (imports, hubs)
}

/// Write the lookup indexes. `imports.md` always; `routes.md` only when there
/// is something to say. A stale `routes.md` from an earlier run would claim
/// callers the code no longer has, so it is removed.
fn write_indexes(
    dir: &std::path::Path,
    inv: &inventory::Inventory,
    imports: &render::ImportsIndex,
    route_links: &[contract::RouteLink],
) -> Result<(PathBuf, Option<PathBuf>)> {
    let imports_path = dir.join("imports.md");
    std::fs::write(&imports_path, &imports.markdown)
        .with_context(|| format!("cannot write {}", imports_path.display()))?;
    let routes_file = dir.join("routes.md");
    let routes_path = if route_links.is_empty() {
        let _ = std::fs::remove_file(&routes_file);
        None
    } else {
        std::fs::write(&routes_file, contract::routes_markdown(inv, route_links))
            .with_context(|| format!("cannot write {}", routes_file.display()))?;
        Some(routes_file)
    };
    Ok((imports_path, routes_path))
}

pub fn run(opts: &Options) -> Result<RunReport> {
    // Stage timings to stderr when CODEARCH_TIME is set. Permanent
    // instrumentation, not a debug leftover: the M5 exit ("re-run in
    // seconds") is measured with exactly this.
    let timer = Timer::new();
    // 0 — Inventory. The codearch directory (and its cache) resolves
    // before the walk: warm stat entries skip file reads below. Loading a
    // foreign or corrupt store yields an empty one; the run continues
    // uncached.
    let root = inventory::canonical_root(&opts.root)?;
    let dir = opts
        .codearch_dir
        .clone()
        .unwrap_or_else(|| root.join(".codearch"));
    let mut store = cache::Store::load(&dir);
    timer.done("cache-load");
    let inv = inventory::collect_cached(&root, &mut store.files)?;
    timer.done("inventory");
    if inv.is_empty() {
        anyhow::bail!(
            "no JavaScript, TypeScript or Python source files found under {}",
            root.display()
        );
    }

    // 1 — Profile
    let profile = profile::detect(&inv);
    let routes = profile::route_hints(&profile, &inv);
    timer.done("profile");

    // 2 — Parse
    let parsed = parse::parse_all_cached(&inv, &mut store.files);
    timer.done("parse");

    // 3 — Resolve
    let res = resolve::resolve_all(&inv, &parsed, &profile.mappings, &profile.package_dirs);
    timer.done("resolve");

    // 3.5 — Cross-language contracts. Stage-2 URL evidence joined across
    // languages; feeds stage-5 fusion and the render caveat. Semantic
    // contracts (shared rare symbol shapes, `createUser` ↔ `create_user`)
    // are a second, weaker join — reported separately because the evidence
    // differs and the map says which kind it fused.
    let contracts = contract::join(&inv, &parsed);
    let semantic_contracts = contract::semantic_join(&inv, &parsed);
    let route_links = contract::route_join(&inv, &parsed);

    // Index-only run: the lookups need nothing past this point. The cache is
    // saved as loaded plus refreshed parses, so git, label and split entries
    // from earlier --map runs survive untouched.
    if !opts.map {
        let (imports, _) = import_index_and_hubs(&inv, &profile, &res);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        if let Err(e) = store.save(&dir) {
            eprintln!("warning: cannot write cache: {e:#}");
        }
        let (imports_path, routes_path) = write_indexes(&dir, &inv, &imports, &route_links)?;
        timer.done("indexes+write");
        let source_bytes: u64 = inv.files.iter().map(|f| f.bytes).sum();
        return Ok(RunReport {
            files: inv.len(),
            loc: inv.total_loc(),
            domains: 0,
            map_tokens: 0,
            source_tokens_estimate: (source_bytes / 4) as usize,
            resolution_rate: res.resolution_rate,
            unresolved: res.unresolved.len(),
            map: false,
            out_path: None,
            index_path: None,
            imports_path,
            import_edges: imports.imports,
            imports_tokens: imports.tokens,
            cochange_pairs: 0,
            commits_read: 0,
            confidence: 0.0,
            low_confidence_domains: 0,
            split: false,
            flows: 0,
            flows_dropped: false,
            used_directory_fallback: false,
            truncated: false,
            over_domain_cap: false,
            labels_fell_back: 0,
            labels_summary_fell_back: 0,
            contracts: contracts.len() + semantic_contracts.len(),
            url_contracts: contracts.len(),
            semantic_contracts: semantic_contracts.len(),
            route_views: route_links.len(),
            routes_path,
        });
    }

    // 4 — Git signals. Absent history is a degraded run, not a failed one.
    // HEAD-keyed: an unchanged history reuses the stored signal.
    let cc = if opts.no_git {
        git::CoChange::default()
    } else {
        git::collect_cached(&inv.root, &inv, &mut store.git)
    };
        timer.done("git");

    // Split-verdict memo: identical inputs decide identically, so a warm
    // re-run skips rebuilding the flat map just to measure it again. Any
    // content, history, or option change re-probes exactly.
    let llm_model_id = if opts.labeler == LabelerKind::Llm {
        opts.model_path.as_ref().map(model_identity)
    } else {
        None
    };
    let fingerprint = split_fingerprint(&store, opts, &llm_model_id);
    let memo_split = match &store.split_decision {
        Some(d) if d.fingerprint == fingerprint => Some(d.split),
        _ => None,
    };
    if std::env::var_os("CODEARCH_TIME").is_some() {
        eprintln!(
            "time split-memo     stored={} current={} hit={}",
            store
                .split_decision
                .as_ref()
                .map(|d| d.fingerprint.as_str())
                .unwrap_or("-"),
            fingerprint,
            memo_split.is_some()
        );
    }    // 5 — Graph
    let g = graph::build_with_semantic(&inv, &res, &cc, &contracts, &semantic_contracts);
    timer.done("graph");

    // 6 — Cluster. With no edges at all there is nothing to cluster, which is
    // stage 6's declared failure path rather than an error.
    let used_directory_fallback = g.edge_count() == 0;
    let part = if used_directory_fallback {
        cluster::directory_partition(&inv)
    } else {
        cluster::partition_targeting(&g, opts.max_domains, opts.seed)
    };
    timer.done("cluster");

    // 7 — Rank
    let scores = rank::score(&g, &routes, &cc.churn);
    timer.done("rank");

    // 8 — Confidence. Stability re-runs the clustering under other tie-break
    // orders, so it costs a few extra partitions and nothing else.
    let stability = confidence::file_stability(&g, &part, opts.max_domains, opts.seed);
    timer.done("stability");

    // 9 — Label
    let summaries = label::summarize(&inv, &parsed, &res, &g, &part, &scores, &routes);
    timer.done("summarize");
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
            let lora = opts
                .lora_path
                .as_deref()
                .and_then(|p| p.to_str())
                .map(std::path::Path::new);
            Box::new(label::llm::LlmLabeler::load_with_lora(
                path,
                lora,
                opts.lora_scale,
                threads,
            )?)
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
    //
    // LLM labels consult the cache by evidence hash; derived labels always
    // recompute (milliseconds post-split), so there is exactly one labeling
    // path and no subset-dedup divergence. A hit skips inference, not the
    // fallback accounting below — a stored label already survived it.
    let llm_model_id = if opts.labeler == LabelerKind::Llm {
        opts.model_path.as_ref().map(model_identity)
    } else {
        None
    };
    for s in &summaries {
        labels.push(label_one(
            &labeler,
            &mut store,
            &llm_model_id,
            &inv,
            s,
            &mut taken,
        ));
    }
    timer.done("label");

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
    timer.done("flows");

    // 10 — Render. The import index is built once, outside the budget ladder:
    // the root map only advertises it.
    let (imports, hubs) = import_index_and_hubs(&inv, &profile, &res);
    let map_input = render::MapInput {
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
        contracts: contracts.len(),
        semantic_contracts: semantic_contracts.len(),
        has_routes: !route_links.is_empty(),
        budget: opts.budget,
    };
    let out_path = opts
        .out
        .clone()
        .unwrap_or_else(|| inv.root.join("CODEBASE.md"));

    // Split trigger (Decision 2): the flat map is the probe. Fits → today's
    // path byte-for-byte. Exceeds → root plus domain files. A memoized
    // verdict skips the probe: the fingerprint covers everything it can
    // depend on, so reuse is exact. Only the probe is skipped — labels still
    // run uniformly, which is what subsection names are built from.
    let rendered: Option<render::RenderedMap> = if memo_split == Some(true) {
        None
    } else {
        Some(render::render_map(&map_input))
    };    let split = memo_split
        .unwrap_or_else(|| rendered.as_ref().is_some_and(|r| r.tokens > opts.budget));
    if memo_split.is_none() {
        store.split_decision = Some(cache::SplitDecision { fingerprint, split });
    }

    let (markdown, index_hierarchy, map_tokens, domain_count) = if !split {
        let r = rendered.as_ref().expect("flat path always renders");
        (r.markdown.clone(), None, r.tokens, summaries.len())
    } else {
        render_split(
            &inv,
            &parsed,
            &res,
            &g,
            &scores,
            &routes,
            &rels,
            &cc,
            opts,
            &dir,
            &map_input,
            &summaries,
            &labels,
            &mut taken,
            &labeler,
            &mut store,
            &llm_model_id,
        )?
    };

    std::fs::write(&out_path, &markdown)
        .with_context(|| format!("cannot write {}", out_path.display()))?;

    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    // The stages above refreshed the store this run ran with; persist it for
    // the next one. A write failure warns rather than failing the map — the
    // cache saves time, and losing it only costs time.
    if let Err(e) = store.save(&dir) {
        eprintln!("warning: cannot write cache: {e:#}");
    }
    let (imports_path, routes_path) = write_indexes(&dir, &inv, &imports, &route_links)?;

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
            map_tokens,
            index_hierarchy,
        );
        std::fs::write(&p, json).with_context(|| format!("cannot write {}", p.display()))?;
        index_path = Some(p);
    }
    timer.done("render+write");

    let source_bytes: u64 = inv.files.iter().map(|f| f.bytes).sum();

    Ok(RunReport {
        files: inv.len(),
        loc: inv.total_loc(),
        domains: domain_count,
        map_tokens,
        source_tokens_estimate: (source_bytes / 4) as usize,
        resolution_rate: res.resolution_rate,
        unresolved: res.unresolved.len(),
        map: true,
        out_path: Some(out_path),
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
        // Flat-probe fields. On a memoized split the probe never ran and the
        // flat map does not exist: both read false, which is what "the root
        // routes elsewhere" means for them.
        flows_dropped: rendered.as_ref().map(|r| r.flows_dropped).unwrap_or(false),
        split,
        cochange_pairs: cc.pairs.len(),
        commits_read: cc.commits_read,
        used_directory_fallback,
        truncated: rendered.as_ref().map(|r| r.truncated).unwrap_or(false),
        over_domain_cap: summaries.len() > opts.max_domains,
        labels_fell_back: labeler.fell_back(),
        labels_summary_fell_back: labeler.summary_fell_back(),
        contracts: contracts.len() + semantic_contracts.len(),
        url_contracts: contracts.len(),
        semantic_contracts: semantic_contracts.len(),
        route_views: route_links.len(),
        routes_path,
    })
}

/// The split path: coarse units, nesting, coarse labels, routing, domain
/// files. Runs only when the flat map exceeds budget; small repos never
/// reach it, which is what keeps their output byte-identical.
///
/// Label accounting: flat labels are already taken (the probe needed them).
/// Coarse full units continue the same `taken` order; buckets bypass the
/// labeler with directory statements. Fine subsection names reuse the flat
/// labels for connected clusters of 2+ files under full units — singletons
/// and bucket members render bare.
#[allow(clippy::too_many_arguments)]
fn render_split(
    inv: &inventory::Inventory,
    parsed: &[parse::FileParse],
    res: &resolve::Resolution,
    g: &graph::CodeGraph,
    scores: &[f64],
    routes: &[types::RouteHint],
    rels: &[String],
    cc: &git::CoChange,
    opts: &Options,
    dir: &std::path::Path,
    map_input: &render::MapInput,
    summaries: &[label::ClusterSummary],
    labels: &[Label],
    taken: &mut Vec<String>,
    labeler: &Box<dyn label::Labeler>,
    store: &mut cache::Store,
    llm_model_id: &Option<String>,
) -> anyhow::Result<(String, Option<serde_json::Value>, usize, usize)> {
    let mut units = split::coarse(g, rels, opts.seed, opts.max_domains);

    let mut file_unit = vec![0usize; inv.len()];
    for (u, unit) in units.iter().enumerate() {
        for &f in &unit.files {
            file_unit[f] = u;
        }
    }
    // Post-reorder identity: `summaries` come back from `summarize` sorted
    // by importance with renumbered ids, so partition ids are meaningless
    // from here on. Nesting and every index below run on summary positions.
    let new_members: Vec<Vec<types::FileId>> =
        summaries.iter().map(|s| s.files.clone()).collect();
    let connected = split::fine_connected(g, &new_members);
    let parent = split::nest(&new_members, &file_unit);
    for (fine_id, &u) in parent.iter().enumerate() {
        if let Some(unit) = units.get_mut(u) {
            unit.fine.push(fine_id);
        }
    }

    let part_coarse = cluster::Partition {
        membership: file_unit,
        count: units.len(),
    };
    let coarse_summaries =
        label::summarize(inv, parsed, res, g, &part_coarse, scores, routes);
    // Align units to coarse-summary order (also importance-sorted): the unit
    // whose files match summary `s` moves to position `s`. Depends ids,
    // labels and renders then agree with no translation table.
    let unit_files: Vec<Vec<types::FileId>> =
        units.iter().map(|u| u.files.clone()).collect();
    let coarse_files: Vec<Vec<types::FileId>> = coarse_summaries
        .iter()
        .map(|s| s.files.clone())
        .collect();
    let unit_to_summary = split::align_by_files(&unit_files, &coarse_files);
    let mut new_pos = vec![0usize; units.len()];
    for (u, &s) in unit_to_summary.iter().enumerate() {
        new_pos[u] = s.min(units.len().saturating_sub(1));
    }
    let mut reordered: Vec<split::CoarseUnit> = vec![
        split::CoarseUnit {
            kind: split::UnitKind::Bucket,
            files: Vec::new(),
            fine: Vec::new(),
            dir_key: String::new(),
        };
        units.len()
    ];
    for (u, unit) in units.into_iter().enumerate() {
        reordered[new_pos[u]] = unit;
    }
    let units = reordered;
    let parent: Vec<usize> = parent.iter().map(|&u| new_pos[u]).collect();
    for (u, unit) in units.iter().enumerate() {
        debug_assert!(
            unit.fine.iter().all(|&f| parent.get(f).copied() == Some(u)),
            "nesting disagrees after reorder"
        );
    }
    let mut coarse_labels: Vec<Label> = Vec::with_capacity(units.len());
    for (unit, s) in units.iter().zip(&coarse_summaries) {
        if unit.kind == split::UnitKind::Full {
            coarse_labels.push(label_one(labeler, store, llm_model_id, inv, s, taken));
        } else {
            coarse_labels.push(bucket_label(unit));
        }
    }

    let fine_labels: Vec<Option<String>> = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // Names render wherever subsections do: connected clusters of 2+
            // files, under full units and buckets alike. Singletons and
            // edgeless clusters list bare.
            if s.files.len() >= 2 && connected.get(i).copied().unwrap_or(false) {
                Some(labels[i].name.clone())
            } else {
                None
            }
        })
        .collect();

    let extra: Vec<Vec<String>> = units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            if u.kind == split::UnitKind::Full {
                coarse_summaries[i].top_symbols.clone()
            } else {
                Vec::new()
            }
        })
        .collect();
    let mut terms = split::routing_terms(&units, rels, &extra);
    let names: Vec<String> = units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            if u.kind == split::UnitKind::Full {
                coarse_labels[i].name.clone()
            } else {
                u.dir_key.clone()
            }
        })
        .collect();
    let slugs = split::assign_slugs(&names);
    let budgets = split::domain_budgets(&units, &cc.churn);

    // Root overflow guard: routing terms truncate first (halved to a floor
    // of one term per unit, then accepted). Sections are fixed-size by
    // construction; only terms flex.
    for _ in 0..5 {
        let view = render::SplitView {
            units: &units,
            summaries: &coarse_summaries,
            labels: &coarse_labels,
            terms: &terms,
            slugs: &slugs,
            budgets: &budgets,
            parent: &parent,
            fine_labels: &fine_labels,
        };
        let root = render::render_split_root(map_input, &view);
        if render::count_tokens(&root) <= opts.budget
            || terms.iter().all(|t| t.len() <= 1)
        {
            let root_tokens = render::count_tokens(&root);
            let view = render::SplitView {
                units: &units,
                summaries: &coarse_summaries,
                labels: &coarse_labels,
                terms: &terms,
                slugs: &slugs,
                budgets: &budgets,
                parent: &parent,
                fine_labels: &fine_labels,
            };
            let domains_dir = dir.join("domains");
            std::fs::create_dir_all(&domains_dir).with_context(|| {
                format!("cannot create {}", domains_dir.display())
            })?;
            for (i, slug) in slugs.iter().enumerate() {
                let mut md = String::new();
                for &cap in &[60usize, 30, 15, 8, 4] {
                    md = render::render_domain(map_input, &view, i, cap);
                    if render::count_tokens(&md) <= budgets[i] {
                        break;
                    }
                }
                std::fs::write(domains_dir.join(format!("{slug}.md")), &md)
                    .with_context(|| format!("cannot write domain {slug}"))?;
            }
            let hierarchy =
                split::hierarchy_json(&units, &names, &parent);
            return Ok((root, Some(hierarchy), root_tokens, units.len()));
        }
        for t in terms.iter_mut() {
            t.truncate((t.len() + 1) / 2);
        }
    }
    anyhow::bail!("split root exceeds budget after routing truncation")
}

fn bucket_label(unit: &split::CoarseUnit) -> Label {
    let where_ = if unit.dir_key == "(root)" {
        "at the repository root".to_string()
    } else {
        format!("under `{}/`", unit.dir_key)
    };
    Label {
        name: unit.dir_key.clone(),
        summary: format!(
            "{} files {}, grouped by directory with no claimed relationships.",
            unit.files.len(),
            where_
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_only_run_writes_the_same_index_and_keeps_the_map_cache() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/fixtures/tier1");
        let tmp = std::env::temp_dir().join(format!("codearch-index-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let state = tmp.join("state");
        let map_opts = Options {
            root: fixture.clone(),
            out: Some(tmp.join("CODEBASE.md")),
            codearch_dir: Some(state.clone()),
            ..Options::default()
        };
        let full = run(&map_opts).unwrap();
        let full_index = std::fs::read_to_string(&full.imports_path).unwrap();
        std::fs::remove_file(tmp.join("CODEBASE.md")).unwrap();
        std::fs::remove_file(state.join("index.json")).unwrap();
        // Entries only a map run fills; the index-only run must carry them over.
        let mut seeded = cache::Store::load(&state);
        seeded.labels_llm.insert(
            "k".into(),
            cache::StoredLabel { name: "Kept".into(), summary: "kept".into() },
        );
        let split_before = seeded.split_decision.clone().map(|d| d.fingerprint);
        seeded.save(&state).unwrap();

        let idx = run(&Options { map: false, ..map_opts }).unwrap();
        let idx_index = std::fs::read_to_string(&idx.imports_path).unwrap();
        let after = cache::Store::load(&state);
        let wrote_map = tmp.join("CODEBASE.md").exists() || state.join("index.json").exists();
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(!idx.map && idx.out_path.is_none() && idx.index_path.is_none());
        assert!(!wrote_map, "index-only runs write no map and no index.json");
        assert_eq!(idx_index, full_index);
        assert_eq!(idx.import_edges, full.import_edges);
        assert_eq!(idx.domains, 0);
        assert!(after.labels_llm.contains_key("k"));
        assert_eq!(after.split_decision.map(|d| d.fingerprint), split_before);
    }

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
        let map = std::fs::read_to_string(report.out_path.as_ref().unwrap()).unwrap();
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
        let map = std::fs::read_to_string(report.out_path.as_ref().unwrap()).unwrap();
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

    fn xlang_fixture(tmp: &std::path::Path) -> PathBuf {
        let root = tmp.join("xlang-repo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("frontend")).unwrap();
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::write(
            root.join("frontend/api.ts"),
            "export async function getUsers() { return fetch('/api/users'); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("backend/server.py"),
            "from flask import Flask\napp = Flask(__name__)\n@app.route('/api/users')\ndef users():\n    return []\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn contracts_join_languages_but_not_the_import_index() {
        let tmp = std::env::temp_dir().join(format!("codearch-xlang-test-{}", std::process::id()));
        let root = xlang_fixture(&tmp);
        let opts = Options {
            root: root.clone(),
            out: Some(tmp.join("CODEBASE.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: false,
            no_git: true,
            ..Options::default()
        };

        let report = run(&opts).unwrap();
        let map = std::fs::read_to_string(report.out_path.as_ref().unwrap()).unwrap();
        let index = std::fs::read_to_string(&report.imports_path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(report.contracts, 1);
        assert!(map.contains("1 API contract"));
        // The index's contract is imports: a contract pair must never read
        // as an importer. Neither file imports the other here, so the index
        // carries no edge lines at all.
        assert!(!index.lines().any(|l| l.contains(" ← ")));
        assert!(!index.contains("server.py"));
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
        let tight_map = std::fs::read_to_string(tight_report.out_path.as_ref().unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(tight_report.flows_dropped, "flows survived budget pressure");
        assert!(!tight_map.contains("Flows:"), "flows still rendered");
        assert!(tight_report.map_tokens <= tight.budget);
    }

    fn split_fixture(tmp: &std::path::Path) -> PathBuf {
        // Two connected groups plus edgeless files: exercises full units,
        // buckets, nesting and routing in one small repo.
        let root = tmp.join("split-repo");
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["shop/cart", "shop/pay", "misc"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("package.json"), r#"{"name": "shop"}"#).unwrap();
        std::fs::write(
            root.join("shop/cart/index.ts"),
            "import { pay } from '../pay/index';\nexport const cart = pay;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("shop/pay/index.ts"),
            "import { cart } from '../cart/index';\nexport const pay = cart;\n",
        )
        .unwrap();
        std::fs::write(root.join("misc/a.ts"), "export const a = 1;\n").unwrap();
        std::fs::write(root.join("misc/b.ts"), "export const b = 2;\n").unwrap();
        root
    }

    #[test]
    fn over_budget_splits_into_root_and_domains() {
        let tmp = std::env::temp_dir().join(format!("codearch-split-{}", std::process::id()));
        let root = split_fixture(&tmp);
        let flat = Options {
            root: root.clone(),
            out: Some(tmp.join("flat.md")),
            codearch_dir: Some(tmp.join("flat-state")),
            write_index: false,
            no_git: true,
            ..Options::default()
        };
        let flat_report = run(&flat).unwrap();
        assert!(!flat_report.split, "fixture should fit flat at full budget");

        // One token under the flat cost forces the split deterministically.
        let opts = Options {
            root: root.clone(),
            out: Some(tmp.join("CODEBASE.md")),
            codearch_dir: Some(tmp.join("state")),
            write_index: true,
            no_git: true,
            budget: flat_report.map_tokens - 1,
            ..Options::default()
        };

        let report = run(&opts).unwrap();
        assert!(report.split, "expected the 700-token budget to force a split");
        let root_md = std::fs::read_to_string(report.out_path.as_ref().unwrap()).unwrap();
        assert!(root_md.contains("## Routing"), "root has no routing table");
        assert!(root_md.contains(".codearch/domains/"), "root points nowhere");
        let domains: Vec<_> = std::fs::read_dir(tmp.join("state").join("domains"))
            .unwrap()
            .collect();
        assert!(!domains.is_empty(), "no domain files written");
        let index = std::fs::read_to_string(report.index_path.as_ref().unwrap()).unwrap();
        assert!(index.contains("\"hierarchy\""), "index.json has no hierarchy");
        assert!(index.contains("\"parent\""), "index.json has no parent map");
        // Flat clusters stay readable for eval tooling.
        assert!(index.contains("\"clusters\""), "index.json lost flat clusters");

        // Cached second run reproduces byte for byte, root and domains.
        // store.json compares semantically: HashMap serialization order is
        // nondeterministic across runs, but the content must agree exactly.
        let before = walk_files(&tmp);
        let report2 = run(&opts).unwrap();
        assert!(report2.split);
        let after = walk_files(&tmp);
        assert_eq!(before.len(), after.len(), "file set diverged");
        for ((pb, cb), (pa, ca)) in before.iter().zip(after.iter()) {
            assert_eq!(pb, pa, "path set diverged");
            if pb.ends_with("store.json") {
                let vb: serde_json::Value = serde_json::from_str(cb).unwrap();
                let va: serde_json::Value = serde_json::from_str(ca).unwrap();
                assert_eq!(vb, va, "cache content diverged");
            } else {
                assert_eq!(cb, ca, "content diverged for {}", pb.display());
            }
        }

        // A content change re-probes (fingerprint mismatch) and still splits:
        // the memo must never freeze a stale verdict.
        std::fs::write(root.join("misc/a.ts"), "export const a = 1; // touched\n").unwrap();
        let report3 = run(&opts).unwrap();
        assert!(report3.split, "changed run should still split");
        let root3 = std::fs::read_to_string(report3.out_path.as_ref().unwrap()).unwrap();
        assert!(root3.contains("## Routing"), "changed run lost routing");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn split_fingerprint_moves_with_inputs() {
        use std::collections::HashMap;
        let mut files = HashMap::new();
        files.insert(
            "a.ts".to_string(),
            cache::StoredFile {
                hash: "h1".to_string(),
                ..Default::default()
            },
        );
        let store = cache::Store {
            version: cache::FORMAT,
            files,
            ..Default::default()
        };
        let opts = Options::default();
        let fp1 = split_fingerprint(&store, &opts, &None);
        // Same inputs → same verdict key.
        assert_eq!(fp1, split_fingerprint(&store, &opts, &None));
        // Any content change → different key.
        let mut store2 = store.clone();
        store2.files.get_mut("a.ts").unwrap().hash = "h2".to_string();
        assert_ne!(fp1, split_fingerprint(&store2, &opts, &None));
        // Budget is part of the key: the verdict is budget-relative.
        let mut opts2 = Options::default();
        opts2.budget += 1;
        assert_ne!(fp1, split_fingerprint(&store, &opts2, &None));
    }

    fn walk_files(dir: &std::path::Path) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|e| e != "tmp").unwrap_or(true) {
                    out.push((p.strip_prefix(dir).unwrap().to_path_buf(), std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }
}
