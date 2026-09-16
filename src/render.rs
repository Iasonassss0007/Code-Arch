//! Stage 10 — Render.
//!
//! Job    Assemble output within a hard token budget.
//! In     everything above
//! Out    CODEBASE.md, .codearch/index.json, .codearch/imports.md
//! Fails  Cannot fail. Budget overflow truncates by ascending importance.
//!
//! The budget is measured with a real tokenizer rather than estimated from
//! character counts, because a map that quietly doubles its own size defeats
//! the point of the tool.
//!
//! Only the domain names and summaries come from stage 9. Everything else on
//! the page traces to a deterministic stage, and the "Not Analyzed" section is
//! mandatory: the honest boundary of the map is part of the map.

use crate::confidence::{self, Band, ClusterConfidence};
use crate::flows::Flow;
use crate::inventory::Inventory;
use crate::split::{CoarseUnit, UnitKind};
use crate::label::{ClusterSummary, Label};
use crate::parse::FileParse;
use crate::profile::Profile;
use crate::resolve::Resolution;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Most-imported files named in the root map.
pub const IMPORT_HUBS: usize = 8;

/// Where the reverse import index lives, as the root map names it.
pub const IMPORTS_REL: &str = ".codearch/imports.md";

pub struct MapInput<'a> {
    pub inv: &'a Inventory,
    pub profile: &'a Profile,
    pub res: &'a Resolution,
    pub parsed: &'a [FileParse],
    pub summaries: &'a [ClusterSummary],
    pub labels: &'a [Label],
    pub imports: &'a ImportsIndex,
    pub hubs: &'a [(String, usize)],
    /// One entry per summary, in the same order.
    pub confidence: &'a [ClusterConfidence],
    /// Stage-8 flows per summary, same order. Empty where the band or the
    /// entries gave nothing; the ladder may additionally drop them all.
    pub flows: &'a [Vec<Flow>],
    /// Co-change pairs stage 4 contributed. `None` means there was no usable
    /// git history, which the map states rather than hides.
    pub cochange_pairs: Option<usize>,
    /// Cross-language URL-contract pairs fused into the graph (exact
    /// normalized-path matches). Zero leaves every historical sentence
    /// byte-identical; the render tests assert this.
    pub contracts: usize,
    /// Cross-language semantic-contract pairs (shared rare symbol shapes,
    /// `createUser` ↔ `create_user`). Stated separately: the evidence is
    /// weaker and the reader deserves to know which kind was fused.
    pub semantic_contracts: usize,
    /// Whether `.codearch/routes.md` was written (route links exist). The
    /// Imports section names the `callers` query only then.
    pub has_routes: bool,
    pub budget: usize,
}

/// Where the map's relationships came from, stated rather than smoothed over.
/// Zero contracts reproduces the two historical sentences exactly.
fn relations_text(cochange_pairs: Option<usize>, contracts: usize, semantic: usize) -> String {
    let mut parts = vec!["resolved imports".to_string()];
    if let Some(pairs) = cochange_pairs {
        parts.push(format!("{pairs} git co-change pairs"));
    }
    if contracts > 0 {
        parts.push(if contracts == 1 {
            "1 API contract".to_string()
        } else {
            format!("{contracts} API contracts")
        });
    }
    if semantic > 0 {
        parts.push(if semantic == 1 {
            "1 cross-language symbol contract".to_string()
        } else {
            format!("{semantic} cross-language symbol contracts")
        });
    }
    parts.push("directory structure".to_string());
    let mut s = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            s.push_str(if i + 1 == parts.len() { " and " } else { ", " });
        }
        s.push_str(part);
    }
    if cochange_pairs.is_none() && contracts == 0 && semantic == 0 {
        s.push_str(" only");
    }
    format!("relationships come from {s}")
}

pub struct RenderedMap {
    pub markdown: String,
    pub tokens: usize,
    /// True when the budget forced a smaller per-domain file list.
    pub truncated: bool,
    /// True when the budget forced flows off before file lists shrank.
    pub flows_dropped: bool,
}

/// (per-domain file count, flows on). Flows are enhancement: they go before
/// file lists shrink, never after.
const LADDER: &[(usize, bool)] = &[
    (8, true),
    (8, false),
    (6, false),
    (5, false),
    (4, false),
    (3, false),
    (2, false),
    (1, false),
];

pub fn render_map(input: &MapInput) -> RenderedMap {
    let mut last = String::new();
    let mut last_flows = false;
    for (i, &(per_domain, with_flows)) in LADDER.iter().enumerate() {
        let md = build(input, per_domain, with_flows);
        let tokens = count_tokens(&md);
        if tokens <= input.budget || i == LADDER.len() - 1 {
            return RenderedMap {
                markdown: md,
                tokens,
                truncated: i > 0,
                flows_dropped: !with_flows,
            };
        }
        last = md;
        last_flows = !with_flows;
    }
    let tokens = count_tokens(&last);
    RenderedMap {
        markdown: last,
        tokens,
        truncated: true,
        flows_dropped: last_flows,
    }
}

fn build(input: &MapInput, per_domain: usize, with_flows: bool) -> String {
    let inv = input.inv;
    let mut s = String::new();

    let title = map_title(input.profile, inv);
    s.push_str(&format!("# Codebase Map — {title}\n\n"));

    s.push_str(&format!(
        "Generated by Code Arch · {} analyzed files · {} lines · ~{} source tokens (est.)\n",
        inv.len(),
        inv.total_loc(),
        estimate_source_tokens(inv)
    ));
    let sizes: Vec<usize> = input.summaries.iter().map(|c| c.size()).collect();
    let overall = confidence::global(input.confidence, &sizes);
    s.push_str(&format!(
        "Import resolution: {:.0}% · {} domains · confidence: {}\n\n",
        input.res.resolution_rate * 100.0,
        input.summaries.len(),
        Band::of(overall).label()
    ));

    s.push_str("## What This Is\n\n");
    match &input.profile.description {
        Some(d) if !d.trim().is_empty() => s.push_str(&format!("{d}\n\n")),
        _ => s.push_str(&format!(
            "{title} — no description declared in package.json.\n\n"
        )),
    }

    s.push_str("## Stack\n\n");
    if input.profile.frameworks.is_empty() {
        s.push_str("No frameworks identified from the manifest.\n");
    } else {
        s.push_str(&format!("{}\n", input.profile.frameworks.join(" · ")));
    }
    let top = input.res.top_externals(8);
    if !top.is_empty() {
        let names: Vec<String> = top
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect();
        s.push_str(&format!("\nMost-referenced packages: {}\n", names.join(", ")));
    }
    s.push('\n');

    s.push_str("## Domains\n\n");
    for (i, sum) in input.summaries.iter().enumerate() {
        let label = &input.labels[i];
        let band = input
            .confidence
            .get(i)
            .map(|c| c.band())
            .unwrap_or(Band::High);
        s.push_str(&format!(
            "### {} · confidence {}\n\n",
            label.name,
            band.label()
        ));

        // Low confidence means the graph could not establish structure here.
        // Listing the files by directory is honest; a summary and a dependency
        // line would be the tool asserting exactly what it just said it does
        // not know.
        if band == Band::Low {
            s.push_str(
                "Structure could not be reliably determined for this region. \
Files are listed by directory, with no claimed relationships.\n\n",
            );
            for (dir, files) in files_by_directory(inv, &sum.files, per_domain) {
                s.push_str(&format!("- `{dir}` — {}\n", files.join(", ")));
            }
            if sum.files.len() > per_domain {
                s.push_str(&format!(
                    "- …and {} more files in this domain\n",
                    sum.files.len() - per_domain
                ));
            }
            s.push('\n');
            continue;
        }

        s.push_str(&format!("{}\n\n", label.summary));
        if band == Band::Medium {
            s.push_str("Relationships partially inferred.\n\n");
        }

        if !sum.entry_points.is_empty() {
            s.push_str(&format!("Entry points: {}\n\n", sum.entry_points.join(", ")));
        }

        s.push_str("Key files:\n\n");
        for &f in sum.files.iter().take(per_domain) {
            let rel = &inv.get(f).rel;
            match file_symbols(input.parsed, f, 3) {
                Some(syms) => s.push_str(&format!("- `{rel}` — {syms}\n")),
                None => s.push_str(&format!("- `{rel}`\n")),
            }
        }
        if sum.files.len() > per_domain {
            s.push_str(&format!(
                "- …and {} more files in this domain\n",
                sum.files.len() - per_domain
            ));
        }
        s.push('\n');

        if with_flows {
            if let Some(domain_flows) = input.flows.get(i) {
                if !domain_flows.is_empty() {
                    s.push_str("Flows:\n\n");
                    for f in domain_flows {
                        let chain: Vec<String> = f
                            .path
                            .iter()
                            .map(|&id| format!("`{}`", inv.get(id).rel))
                            .collect();
                        let mut line = format!("- {}: {}", f.entry_label, chain.join(" → "));
                        if f.leaves > 0 {
                            line.push_str(&format!(" → … (+{} leaves)", f.leaves));
                        }
                        line.push('\n');
                        s.push_str(&line);
                    }
                    s.push('\n');
                }
            }
        }

        let dep_names = |ids: &[usize]| -> String {
            ids.iter()
                .filter_map(|&c| input.labels.get(c).map(|l| l.name.clone()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let deps = dep_names(&sum.depends_on);
        let rdeps = dep_names(&sum.depended_on_by);
        if !deps.is_empty() || !rdeps.is_empty() {
            let mut line = Vec::new();
            if !deps.is_empty() {
                line.push(format!("Depends on: {deps}"));
            }
            if !rdeps.is_empty() {
                line.push(format!("Depended on by: {rdeps}"));
            }
            s.push_str(&format!("{}\n\n", line.join(" · ")));
        }
    }

    s.push_str(&imports_section(input.hubs, input.imports, input.has_routes));

    s.push_str("## Task Navigation\n\n");
    for (i, sum) in input.summaries.iter().enumerate() {
        let files: Vec<String> = sum
            .files
            .iter()
            .take(3)
            .map(|&f| format!("`{}`", inv.get(f).rel))
            .collect();
        if files.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "- Changes to **{}** — read {}\n",
            input.labels[i].name,
            files.join(", ")
        ));
    }
    s.push('\n');

    s.push_str("## Not Analyzed\n\n");
    s.push_str(&analyzed_boundary(input));
    let flows_shown = with_flows && input.flows.iter().any(|f| !f.is_empty());
    let relations = relations_text(
        input.cochange_pairs,
        input.contracts,
        input.semantic_contracts,
    );
    if flows_shown {
        s.push_str(&format!(
            "- Execution flows below are import-chain traversals from entry points \
             (depth ≤ 4), not model output; {relations}\n"
        ));
    } else {
        s.push_str(&format!("- This map contains no execution flows; {relations}\n"));
    }

    s
}

/// The honest boundary, shared by the flat map, the split root and domain
/// files: what was excluded and what could not be resolved. Extracted so
/// all three state it identically; the caveats below it differ per page.
fn analyzed_boundary(input: &MapInput) -> String {
    let e = &input.inv.excluded;
    let mut s = String::new();
    s.push_str(&format!(
        "- Excluded files: {} generated, {} type declarations, {} config, {} oversized, {} unreadable\n",
        e.generated, e.declarations, e.config, e.too_large, e.unreadable
    ));
    s.push_str(&format!(
        "- Vendor and build directories were not walked; {} files in unsupported languages were ignored\n",
        e.non_supported
    ));
    if !input.res.unresolved.is_empty() {
        s.push_str(&format!(
            "- {} import specifiers could not be resolved to a file and carry no edge in this map\n",
            input.res.unresolved.len()
        ));
    }
    s
}

/// Files grouped under their directory, keeping the importance order the
/// summary already imposed. Used only by the low-confidence band.
fn files_by_directory(inv: &Inventory, files: &[usize], limit: usize) -> Vec<(String, Vec<String>)> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for &f in files.iter().take(limit) {
        let rec = inv.get(f);
        let dir = if rec.dir().is_empty() { "." } else { rec.dir() };
        let base = rec.rel.rsplit('/').next().unwrap_or(&rec.rel);
        if !grouped.contains_key(dir) {
            order.push(dir.to_string());
        }
        grouped
            .entry(dir.to_string())
            .or_default()
            .push(format!("`{base}`"));
    }
    order
        .into_iter()
        .map(|d| {
            let files = grouped.remove(&d).unwrap_or_default();
            (d, files)
        })
        .collect()
}

fn file_symbols(parsed: &[FileParse], f: usize, n: usize) -> Option<String> {
    let p = parsed.get(f)?;
    let mut names: Vec<&str> = Vec::new();
    for sym in &p.symbols {
        if sym.exported && !names.contains(&sym.name.as_str()) {
            names.push(&sym.name);
        }
        if names.len() >= n {
            break;
        }
    }
    if names.is_empty() {
        for sym in &p.symbols {
            if !names.contains(&sym.name.as_str()) {
                names.push(&sym.name);
            }
            if names.len() >= n {
                break;
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn estimate_source_tokens(inv: &Inventory) -> usize {
    // Roughly four bytes per token for source code; stated as an estimate on
    // the page so nobody mistakes it for a measurement.
    let bytes: u64 = inv.files.iter().map(|f| f.bytes).sum();
    (bytes / 4) as usize
}

fn dir_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "repository".to_string())
}

pub fn map_title(profile: &Profile, inv: &Inventory) -> String {
    profile
        .project_name
        .clone()
        .unwrap_or_else(|| dir_name(&inv.root))
}

/// The reverse import index: written beside the root map, read on demand, and
/// deliberately outside the root budget.
pub struct ImportsIndex {
    pub markdown: String,
    /// Import edges listed.
    pub imports: usize,
    /// Files with at least one importer.
    pub imported_files: usize,
    pub tokens: usize,
}

/// Files with the most importers, most first, ties broken by path.
pub fn import_hubs(paths: &[&str], in_edges: &[Vec<usize>], n: usize) -> Vec<(String, usize)> {
    let mut hubs: Vec<(String, usize)> = in_edges
        .iter()
        .enumerate()
        .filter(|(_, importers)| !importers.is_empty())
        .map(|(f, importers)| (paths[f].to_string(), importers.len()))
        .collect();
    hubs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hubs.truncate(n);
    hubs
}

/// One line per imported file, so a text search for a path lands on the whole
/// answer for it. Lines and importers are sorted by path, which keeps the file
/// deterministic and confines a one-file change to the lines naming that file.
pub fn import_index(
    title: &str,
    paths: &[&str],
    in_edges: &[Vec<usize>],
    resolution_rate: f64,
    unresolved: usize,
) -> ImportsIndex {
    let mut lines: Vec<(&str, String)> = Vec::new();
    let mut imports = 0;
    for (f, importers) in in_edges.iter().enumerate() {
        if importers.is_empty() {
            continue;
        }
        let mut names: Vec<&str> = importers.iter().map(|&i| paths[i]).collect();
        names.sort_unstable();
        imports += names.len();
        lines.push((paths[f], format!("{} ← {}", paths[f], names.join(", "))));
    }
    lines.sort_by(|a, b| a.0.cmp(b.0));

    let mut s = format!("# Reverse Import Index — {title}\n\n");
    s.push_str("Generated by Code Arch. For each file, every analyzed file that imports it.\n");
    s.push_str(&format!(
        "{imports} imports into {} files · import resolution {:.0}% · {unresolved} unresolved specifiers carry no edge.\n",
        lines.len(),
        resolution_rate * 100.0
    ));
    s.push_str("External packages are not listed. Files nothing imports are omitted.\n\n");
    for (_, line) in &lines {
        s.push_str(line);
        s.push('\n');
    }

    let tokens = count_tokens(&s);
    ImportsIndex {
        markdown: s,
        imports,
        imported_files: lines.len(),
        tokens,
    }
}

/// The root map's pointer to the index, with the hubs as orientation.
pub fn imports_section(hubs: &[(String, usize)], index: &ImportsIndex, has_routes: bool) -> String {
    let mut s = String::from("## Imports\n\n");
    if index.imports == 0 {
        s.push_str("No internal imports were resolved, so there is no import index.\n\n");
        return s;
    }
    s.push_str("Most-imported files (number of analyzed files importing each):\n\n");
    for (path, count) in hubs {
        s.push_str(&format!("- `{path}` — {count}\n"));
    }
    s.push_str(&format!(
        "\nReverse import index: `{IMPORTS_REL}` lists, for every imported file, each analyzed \
file that imports it ({} imports into {} files, ~{} tokens). Query it with \
`codearch importers <file>`.",
        index.imports, index.imported_files, index.tokens
    ));
    if has_routes {
        s.push_str(" Query route callers with `codearch callers <View>`.");
    }
    s.push_str("\n\n");
    s
}

pub fn index_json(
    inv: &Inventory,
    res: &Resolution,
    summaries: &[ClusterSummary],
    labels: &[Label],
    conf: &[ClusterConfidence],
    flows: &[Vec<Flow>],
    map_tokens: usize,
    hierarchy: Option<serde_json::Value>,
) -> String {
    let clusters: Vec<serde_json::Value> = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let domain_flows: Vec<serde_json::Value> = flows
                .get(i)
                .map(|fs| {
                    fs.iter()
                        .map(|f| {
                            serde_json::json!({
                                "entry": f.entry_label,
                                "path": f.path.iter().map(|&id| inv.get(id).rel.clone()).collect::<Vec<_>>(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "id": s.id,
                "name": labels[i].name,
                "summary": labels[i].summary,
                "files": s.files.iter().map(|&f| inv.get(f).rel.clone()).collect::<Vec<_>>(),
                "depends_on": s.depends_on,
                "depended_on_by": s.depended_on_by,
                "confidence": conf.get(i).map(|c| serde_json::json!({
                    "score": c.score,
                    "band": c.band().label(),
                    "resolution": c.resolution,
                    "stability": c.stability,
                    "agreement": c.agreement,
                })),
                "flows": domain_flows,
            })
        })
        .collect();

    let mut value = serde_json::json!({
        "version": 1,
        "root": inv.root.to_string_lossy(),
        "files": inv.len(),
        "resolution_rate": res.resolution_rate,
        "confidence": confidence::global(
            conf,
            &summaries.iter().map(|c| c.size()).collect::<Vec<_>>(),
        ),
        "map_tokens": map_tokens,
        "clusters": clusters,
    });
    // Present only when split: flat consumers read `clusters` unchanged.
    if let Some(h) = hierarchy {
        if let Some(map) = value.as_object_mut() {
            map.insert("hierarchy".to_string(), h);
        }
    }

    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

static BPE: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();

/// Measured token count, with a character-based fallback if the tokenizer
/// cannot be loaded.
pub fn count_tokens(s: &str) -> usize {
    let bpe = BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok());
    match bpe {
        Some(b) => b.encode_with_special_tokens(s).len(),
        None => s.chars().count() / 4 + 1,
    }
}

/// Split hierarchy rendering (M5). The flat `build()` above is untouched so
/// small-repo maps stay byte-identical; everything here only runs when the
/// flat map exceeds budget.
///
/// [`SplitView`] bundles the coarse-level inputs, all aligned by coarse
/// unit index. Fine-level inputs (summaries, labels, flows, confidence)
/// arrive through the shared [`MapInput`].
pub struct SplitView<'a> {
    pub units: &'a [CoarseUnit],
    /// Coarse summaries, aligned with units. Buckets carry a real summary
    /// too (dirs/top_symbols feed routing terms); only the label differs.
    pub summaries: &'a [ClusterSummary],
    /// Coarse labels, aligned. Bucket labels are directory statements, not
    /// model or derived names — see `bucket_label` in the runner.
    pub labels: &'a [Label],
    /// Routing terms per unit, aligned.
    pub terms: &'a [Vec<String>],
    /// Domain file slugs (`billing`), aligned.
    pub slugs: &'a [String],
    /// Per-unit domain token budgets, aligned.
    pub budgets: &'a [usize],
    /// Fine cluster id → coarse unit index.
    pub parent: &'a [usize],
    /// Display name per fine cluster, aligned by fine id. `None` renders the
    /// cluster's files bare. The runner assigns names to connected clusters
    /// of 2+ files; singletons and edgeless clusters list bare.
    pub fine_labels: &'a [Option<String>],
}

/// Root map: overview, full units, bucket pointers, routing table. Must fit
/// the root budget; routing terms truncate first (they are the flexible
/// part), then bucket lines keep only the pointer.
pub fn render_split_root(input: &MapInput, view: &SplitView) -> String {
    let inv = input.inv;
    let mut s = String::new();

    let title = map_title(input.profile, inv);
    s.push_str(&format!("# Codebase Map — {title}\n\n"));

    s.push_str(&format!(
        "Generated by Code Arch · {} analyzed files · {} lines · ~{} source tokens (est.)\n",
        inv.len(),
        inv.total_loc(),
        estimate_source_tokens(inv)
    ));
    let sizes: Vec<usize> = input.summaries.iter().map(|c| c.size()).collect();
    let overall = confidence::global(input.confidence, &sizes);
    s.push_str(&format!(
        "Import resolution: {:.0}% · {} domains in {} files · confidence: {}\n\n",
        input.res.resolution_rate * 100.0,
        view.units.len(),
        view.slugs.len(),
        Band::of(overall).label()
    ));

    s.push_str("## What This Is\n\n");
    match &input.profile.description {
        Some(d) if !d.trim().is_empty() => s.push_str(&format!("{d}\n\n")),
        _ => s.push_str(&format!(
            "{title} — no description declared in package.json.\n\n"
        )),
    }

    s.push_str("## Stack\n\n");
    if input.profile.frameworks.is_empty() {
        s.push_str("No frameworks identified from the manifest.\n");
    } else {
        s.push_str(&format!("{}\n", input.profile.frameworks.join(" · ")));
    }
    let top = input.res.top_externals(8);
    if !top.is_empty() {
        let names: Vec<String> = top
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect();
        s.push_str(&format!("\nMost-referenced packages: {}\n", names.join(", ")));
    }
    s.push('\n');

    s.push_str("## Domains\n\n");
    for (i, unit) in view.units.iter().enumerate() {
        if unit.kind != UnitKind::Full {
            continue;
        }
        let label = &view.labels[i];
        let sum = &view.summaries[i];
        s.push_str(&format!("### {}\n\n", label.name));
        s.push_str(&format!("{}\n\n", label.summary));
        if !sum.entry_points.is_empty() {
            s.push_str(&format!("Entry points: {}\n\n", sum.entry_points.join(", ")));
        }
        s.push_str("Key files:\n\n");
        for &f in sum.files.iter().take(3) {
            let rel = &inv.get(f).rel;
            match file_symbols(input.parsed, f, 3) {
                Some(syms) => s.push_str(&format!("- `{rel}` — {syms}\n")),
                None => s.push_str(&format!("- `{rel}`\n")),
            }
        }
        s.push_str(&format!(
            "\nDetail: `.codearch/domains/{}.md` ({} files)\n\n",
            view.slugs[i],
            unit.files.len()
        ));
    }

    let buckets: Vec<usize> = view
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == UnitKind::Bucket)
        .map(|(i, _)| i)
        .collect();
    if !buckets.is_empty() {
        s.push_str("### Ungrouped Files\n\n");
        s.push_str(
            "These files share no imports with anything mapped; they are \
             grouped by directory with no claimed relationships.\n\n",
        );
        for &i in &buckets {
            s.push_str(&format!(
                "- `{}` — {} files → `.codearch/domains/{}.md`\n",
                view.units[i].dir_key,
                view.units[i].files.len(),
                view.slugs[i]
            ));
        }
        s.push('\n');
    }

    s.push_str("## Routing\n\n");
    s.push_str("Keywords to domain detail files. Terms are discriminative by construction.\n\n");
    for (i, terms) in view.terms.iter().enumerate() {
        if terms.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "{}   → `.codearch/domains/{}.md`\n",
            terms.join(", "),
            view.slugs[i]
        ));
    }
    s.push('\n');

    s.push_str(&imports_section(input.hubs, input.imports, input.has_routes));

    s.push_str("## Task Navigation\n\n");
    for (i, unit) in view.units.iter().enumerate() {
        if unit.kind != UnitKind::Full {
            continue;
        }
        s.push_str(&format!(
            "- Changes to **{}** — read `.codearch/domains/{}.md`\n",
            view.labels[i].name, view.slugs[i]
        ));
    }
    s.push('\n');

    s.push_str("## Not Analyzed\n\n");
    s.push_str(&analyzed_boundary(input));
    let relations = relations_text(
        input.cochange_pairs,
        input.contracts,
        input.semantic_contracts,
    );
    s.push_str(&format!(
        "- This root map routes to `.codearch/domains/*.md` for detail; execution flows live in domain files. {relations}\n"
    ));

    s
}

/// One domain file. `detail_cap` bounds every file listing on the page
/// (key files, subsection files, bucket directory files); callers ladder it
/// down until the page fits its budget share.
pub fn render_domain(
    input: &MapInput,
    view: &SplitView,
    unit_idx: usize,
    detail_cap: usize,
) -> String {
    let inv = input.inv;
    let unit = &view.units[unit_idx];
    let label = &view.labels[unit_idx];
    let sum = &view.summaries[unit_idx];
    let mut s = String::new();

    let title = map_title(input.profile, inv);
    s.push_str(&format!("# {} — {title}\n\n", label.name));
    s.push_str(&format!("{}\n\n", label.summary));

    if !sum.entry_points.is_empty() {
        s.push_str(&format!("Entry points: {}\n\n", sum.entry_points.join(", ")));
    }

    if unit.kind == UnitKind::Full {
        s.push_str("Key files:\n\n");
        for &f in sum.files.iter().take(detail_cap) {
            let rel = &inv.get(f).rel;
            match file_symbols(input.parsed, f, 3) {
                Some(syms) => s.push_str(&format!("- `{rel}` — {syms}\n")),
                None => s.push_str(&format!("- `{rel}`\n")),
            }
        }
        if sum.files.len() > detail_cap {
            s.push_str(&format!(
                "- …and {} more files in this domain\n",
                sum.files.len() - detail_cap
            ));
        }
        s.push('\n');

        // Aggregated member flows, deduplicated: the same chain reached from
        // two entries renders once.
        let mut seen = std::collections::HashSet::new();
        let mut chains: Vec<String> = Vec::new();
        for &fine_id in &unit.fine {
            if let Some(flows) = input.flows.get(fine_id) {
                for f in flows {
                    let chain: Vec<String> = f
                        .path
                        .iter()
                        .map(|&id| format!("`{}`", inv.get(id).rel))
                        .collect();
                    let mut line = format!("- {}: {}", f.entry_label, chain.join(" → "));
                    if f.leaves > 0 {
                        line.push_str(&format!(" → … (+{} leaves)", f.leaves));
                    }
                    if seen.insert(line.clone()) {
                        chains.push(line);
                    }
                }
            }
        }
        chains.sort();
        if !chains.is_empty() {
            s.push_str("Flows:\n\n");
            for line in chains {
                s.push_str(&line);
                s.push('\n');
            }
            s.push('\n');
        }

        let dep_names = |ids: &[usize]| -> String {
            ids.iter()
                .filter_map(|&c| view.labels.get(c).map(|l| l.name.clone()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let deps = dep_names(&sum.depends_on);
        let rdeps = dep_names(&sum.depended_on_by);
        if !deps.is_empty() || !rdeps.is_empty() {
            let mut line = Vec::new();
            if !deps.is_empty() {
                line.push(format!("Depends on: {deps}"));
            }
            if !rdeps.is_empty() {
                line.push(format!("Depended on by: {rdeps}"));
            }
            s.push_str(&format!("{}\n\n", line.join(" · ")));
        }

        // Fine subsections: named connected clusters; singleton members fold
        // into a by-directory listing instead of hundreds of subsections.
        // Buckets get the same treatment: their connected nested clusters
        // are real substructure even though the bucket claims nothing.
        let (subsections, singletons) = fine_subsections(input, view, unit, detail_cap);
        s.push_str(&subsections);
        if !singletons.is_empty() {
            let mut singletons = singletons;
            singletons.sort_by(|&a, &b| inv.get(a).rel.cmp(&inv.get(b).rel));
            s.push_str("### Ungrouped Files\n\n");
            for (dir, files) in files_by_directory(inv, &singletons, singletons.len()) {
                s.push_str(&format!("- `{dir}` — {}\n", files.join(", ")));
            }
            s.push('\n');
        }
    } else {
        // Buckets claim no unit-level structure: no Key files, no Flows, no
        // Depends. Connected nested clusters still render as subsections;
        // the rest list by directory.
        s.push_str(
            "Files grouped by directory with no claimed relationships. \
             Nothing here imported anything mapped.\n\n",
        );
        let (subsections, singletons) = fine_subsections(input, view, unit, detail_cap);
        s.push_str(&subsections);
        if !singletons.is_empty() {
            let mut singletons = singletons;
            singletons.sort_by(|&a, &b| inv.get(a).rel.cmp(&inv.get(b).rel));
            s.push_str("### Ungrouped Files\n\n");
            for (dir, files) in files_by_directory(inv, &singletons, detail_cap) {
                s.push_str(&format!("- `{dir}` — {}\n", files.join(", ")));
            }
            if singletons.len() > detail_cap {
                s.push_str(&format!(
                    "- …and {} more files in this directory group\n",
                    singletons.len() - detail_cap
                ));
            }
            s.push('\n');
        }
    }

    s.push_str("## Not Analyzed\n\n");
    s.push_str(&analyzed_boundary(input));
    s
}

/// Named fine subsections for one coarse unit plus the leftover singleton
/// files. Shared by full units and buckets: substructure renders wherever
/// the edges support it, unit-level claims only where the band allows.
fn fine_subsections(
    input: &MapInput,
    view: &SplitView,
    unit: &CoarseUnit,
    detail_cap: usize,
) -> (String, Vec<usize>) {
    let inv = input.inv;
    let mut s = String::new();
    let mut singletons: Vec<usize> = Vec::new();
    for &fine_id in &unit.fine {
        let name = view.fine_labels.get(fine_id).and_then(|o| o.as_ref());
        let files: Vec<usize> = input
            .summaries
            .get(fine_id)
            .map(|fs| fs.files.clone())
            .unwrap_or_default();
        match name {
            Some(n) => {
                s.push_str(&format!("### {n}\n\n"));
                for &f in files.iter().take(detail_cap) {
                    let rel = &inv.get(f).rel;
                    match file_symbols(input.parsed, f, 2) {
                        Some(syms) => s.push_str(&format!("- `{rel}` — {syms}\n")),
                        None => s.push_str(&format!("- `{rel}`\n")),
                    }
                }
                if files.len() > detail_cap {
                    s.push_str(&format!("- …and {} more files in this subsystem\n", files.len() - detail_cap));
                }
                s.push('\n');
            }
            None => singletons.extend(files),
        }
    }
    (s, singletons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_tokens_above_zero_for_real_text() {
        let n = count_tokens("export function authenticateUser() {}");
        assert!(n > 3, "got {n}");
    }

    #[test]
    fn token_count_grows_with_length() {
        let short = count_tokens("hello world");
        let long = count_tokens(&"hello world ".repeat(50));
        assert!(long > short * 10);
    }

    /// `(importer, imported)` pairs to the `in_edges` shape `CodeGraph` holds.
    fn in_edges(n: usize, pairs: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut v = vec![Vec::new(); n];
        for &(from, to) in pairs {
            v[to].push(from);
        }
        for row in &mut v {
            row.sort_unstable();
        }
        v
    }

    fn index_lines(md: &str) -> Vec<&str> {
        md.lines().filter(|l| l.contains(" ← ")).collect()
    }

    #[test]
    fn hubs_rank_by_importer_count_then_path() {
        let paths = ["b.ts", "a.ts", "c.ts", "d.ts"];
        let edges = in_edges(4, &[(3, 0), (2, 0), (3, 1), (2, 1), (3, 2)]);
        let hubs = import_hubs(&paths, &edges, 2);
        assert_eq!(hubs, vec![("a.ts".to_string(), 2), ("b.ts".to_string(), 2)]);
    }

    #[test]
    fn hubs_skip_files_nothing_imports() {
        let paths = ["b.ts", "a.ts", "c.ts", "d.ts"];
        let edges = in_edges(4, &[(3, 0), (2, 0), (3, 1), (2, 1), (3, 2)]);
        let hubs = import_hubs(&paths, &edges, 10);
        assert_eq!(hubs.len(), 3);
        assert!(hubs.iter().all(|(p, _)| p != "d.ts"));
    }

    #[test]
    fn index_sorts_lines_by_imported_path_and_importers_by_path() {
        let paths = ["src/z.ts", "src/a.ts", "src/m.ts"];
        let edges = in_edges(3, &[(0, 1), (2, 1), (1, 0)]);
        let idx = import_index("t", &paths, &edges, 1.0, 0);
        assert_eq!(
            index_lines(&idx.markdown),
            vec!["src/a.ts ← src/m.ts, src/z.ts", "src/z.ts ← src/a.ts"]
        );
    }

    #[test]
    fn index_omits_files_nothing_imports() {
        let paths = ["src/z.ts", "src/a.ts", "src/m.ts"];
        let edges = in_edges(3, &[(0, 1), (2, 1), (1, 0)]);
        let idx = import_index("t", &paths, &edges, 1.0, 0);
        assert!(!idx.markdown.lines().any(|l| l.starts_with("src/m.ts ←")));
    }

    #[test]
    fn index_header_states_counts_and_what_is_missing() {
        let paths = ["src/z.ts", "src/a.ts", "src/m.ts"];
        let edges = in_edges(3, &[(0, 1), (2, 1), (1, 0)]);
        let idx = import_index("hono", &paths, &edges, 0.98, 19);
        assert_eq!((idx.imports, idx.imported_files), (3, 2));
        assert!(idx.markdown.starts_with("# Reverse Import Index — hono\n"));
        assert!(idx.markdown.contains("3 imports into 2 files"));
        assert!(idx.markdown.contains("import resolution 98%"));
        assert!(idx.markdown.contains("19 unresolved specifiers carry no edge"));
        assert_eq!(idx.tokens, count_tokens(&idx.markdown));
    }

    #[test]
    fn index_without_edges_is_a_header_only() {
        let paths = ["a.ts", "b.ts"];
        let idx = import_index("t", &paths, &in_edges(2, &[]), 0.0, 0);
        assert_eq!(idx.imports, 0);
        assert!(index_lines(&idx.markdown).is_empty());
        assert!(idx.markdown.contains("0 imports into 0 files"));
    }

    #[test]
    fn section_lists_hubs_and_points_to_the_index() {
        let paths = ["src/z.ts", "src/a.ts", "src/m.ts"];
        let edges = in_edges(3, &[(0, 1), (2, 1), (1, 0)]);
        let idx = import_index("t", &paths, &edges, 1.0, 0);
        let s = imports_section(&import_hubs(&paths, &edges, 8), &idx, false);
        assert!(s.starts_with("## Imports\n"));
        assert!(s.contains("- `src/a.ts` — 2\n"));
        assert!(s.contains("- `src/z.ts` — 1\n"));
        assert!(s.contains("`.codearch/imports.md`"));
        assert!(s.contains(&format!("(3 imports into 2 files, ~{} tokens)", idx.tokens)));
        assert!(s.contains("Query it with `codearch importers <file>`."));
        assert!(!s.contains("callers"));
        let routed = imports_section(&import_hubs(&paths, &edges, 8), &idx, true);
        assert!(routed.contains("Query route callers with `codearch callers <View>`."));
    }

    #[test]
    fn section_without_imports_names_no_file() {
        let paths = ["a.ts", "b.ts"];
        let edges = in_edges(2, &[]);
        let idx = import_index("t", &paths, &edges, 0.0, 0);
        let s = imports_section(&import_hubs(&paths, &edges, 8), &idx, false);
        assert!(s.contains("No internal imports were resolved, so there is no import index."));
        assert!(!s.contains("imports.md"));
    }

    #[test]
    fn relations_text_reproduces_history_without_contracts() {
        // Zero contracts must reproduce the two historical sentences exactly:
        // single-ecosystem maps stay byte-identical.
        assert_eq!(
            relations_text(None, 0, 0),
            "relationships come from resolved imports and directory structure only"
        );
        assert_eq!(
            relations_text(Some(3), 0, 0),
            "relationships come from resolved imports, 3 git co-change pairs and directory structure"
        );
    }

    #[test]
    fn relations_text_names_contracts() {
        assert_eq!(
            relations_text(None, 1, 0),
            "relationships come from resolved imports, 1 API contract and directory structure"
        );
        assert_eq!(
            relations_text(Some(2), 3, 0),
            "relationships come from resolved imports, 2 git co-change pairs, 3 API contracts and directory structure"
        );
    }

    #[test]
    fn relations_text_names_semantic_contracts() {
        assert_eq!(
            relations_text(None, 0, 1),
            "relationships come from resolved imports, 1 cross-language symbol contract and directory structure"
        );
        assert_eq!(
            relations_text(None, 2, 3),
            "relationships come from resolved imports, 2 API contracts, 3 cross-language symbol contracts and directory structure"
        );
    }
}
