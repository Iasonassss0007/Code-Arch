//! Stage 9 — Label.
//!
//! Job    Name and describe clusters the graph already found.
//! In     ClusterSummary — deterministic, compact, no raw source
//! Out    { name, summary } per cluster
//! Fails  Generation failure -> derived name and templated summary. A run
//!        never blocks on the model.
//!
//! The model never sees source code and never invents a relationship: every
//! field of `ClusterSummary` is produced by an earlier deterministic stage.
//! M0 ships only `DerivedLabeler`, the declared fallback path. The llama.cpp
//! implementation slots in behind the same trait at M0.5, which is why the
//! trait exists now rather than later.

use crate::cluster::Partition;
use crate::graph::CodeGraph;
use crate::inventory::Inventory;
use crate::parse::FileParse;
use crate::resolve::Resolution;
use crate::types::{FileId, RouteHint};
use std::collections::HashMap;

#[cfg(feature = "llm")]
pub mod llm;
pub mod validate;

/// Directory names that describe layout, not domain.
const GENERIC_SEGMENTS: &[&str] = &[
    "src", "lib", "app", "apps", "packages", "pkg", "modules", "source", "index", "internal",
    "common", "shared",
];

/// Conventional abbreviations, expanded so the map reads as prose.
const NAME_ALIASES: &[(&str, &str)] = &[
    ("auth", "Authentication"),
    ("api", "API"),
    ("db", "Database"),
    ("ui", "UI"),
    ("utils", "Utilities"),
    ("util", "Utilities"),
    ("cfg", "Configuration"),
    ("config", "Configuration"),
    ("i18n", "Internationalization"),
    ("repo", "Repositories"),
    ("repos", "Repositories"),
    ("gql", "GraphQL"),
    ("http", "HTTP"),
];

const MAX_SYMBOLS: usize = 8;
const MAX_ENTRY_POINTS: usize = 4;
const MAX_EXTERNALS: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct ClusterSummary {
    pub id: usize,
    /// Files ranked by importance, most important first.
    pub files: Vec<FileId>,
    pub scores: Vec<f64>,
    /// Directories the cluster occupies, most populated first.
    pub dirs: Vec<String>,
    pub top_symbols: Vec<String>,
    pub entry_points: Vec<String>,
    pub external_deps: Vec<String>,
    pub depends_on: Vec<usize>,
    pub depended_on_by: Vec<usize>,
}

impl ClusterSummary {
    pub fn size(&self) -> usize {
        self.files.len()
    }

    /// How much of the reader's attention this domain deserves. Summing member
    /// importance rather than averaging it is deliberate: a large, central
    /// subsystem matters more than a small peripheral one, and a repository's
    /// benchmark or example directories should not open its map.
    pub fn importance(&self) -> f64 {
        self.scores.iter().sum()
    }
}

#[derive(Debug, Clone)]
pub struct Label {
    pub name: String,
    pub summary: String,
}

/// Stage 9's contract. Anything that can turn a structured summary into a name
/// and one sentence satisfies it.
pub trait Labeler {
    fn label(&self, summary: &ClusterSummary, siblings: &[String]) -> Label;

    /// How many clusters this labeler could not name itself and had to derive.
    /// Zero for labelers that cannot fail, which is why it has a default.
    fn fell_back(&self) -> usize {
        0
    }

    /// Generated names kept with a derived summary because the generated
    /// sentence failed the summary guard. Zero for labelers without a model.
    fn summary_fell_back(&self) -> usize {
        0
    }
}

/// The deterministic fallback: no model, no network, no invention.
pub struct DerivedLabeler;

impl Labeler for DerivedLabeler {
    fn label(&self, s: &ClusterSummary, siblings: &[String]) -> Label {
        Label {
            name: derive_name(s, siblings),
            summary: derive_summary(s),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn summarize(
    inv: &Inventory,
    parsed: &[FileParse],
    res: &Resolution,
    g: &CodeGraph,
    part: &Partition,
    scores: &[f64],
    routes: &[RouteHint],
) -> Vec<ClusterSummary> {
    let members = part.members();
    let mut routes_by_file: HashMap<FileId, Vec<&str>> = HashMap::new();
    for r in routes {
        routes_by_file
            .entry(r.file)
            .or_default()
            .push(r.label.as_str());
    }

    let mut out: Vec<ClusterSummary> = Vec::with_capacity(part.count);

    for (id, files) in members.iter().enumerate() {
        let mut ranked: Vec<FileId> = files.clone();
        // Importance first; path breaks ties so the output is reproducible.
        ranked.sort_by(|&a, &b| {
            scores[b]
                .partial_cmp(&scores[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| inv.get(a).rel.cmp(&inv.get(b).rel))
        });

        let dirs = dominant_dirs(inv, &ranked, scores);
        let top_symbols = pick_symbols(parsed, &ranked, scores);

        let mut entry_points: Vec<String> = Vec::new();
        for &f in &ranked {
            if let Some(labels) = routes_by_file.get(&f) {
                for l in labels {
                    if entry_points.len() < MAX_ENTRY_POINTS {
                        entry_points.push((*l).to_string());
                    }
                }
            }
        }

        let mut ext_counts: HashMap<&str, usize> = HashMap::new();
        for &f in &ranked {
            if let Some(list) = res.file_externals.get(f) {
                for pkg in list {
                    *ext_counts.entry(pkg.as_str()).or_insert(0) += 1;
                }
            }
        }
        let mut ext: Vec<(&str, usize)> = ext_counts.into_iter().collect();
        ext.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let external_deps: Vec<String> = ext
            .into_iter()
            .take(MAX_EXTERNALS)
            .map(|(k, _)| k.to_string())
            .collect();

        let scores_ranked = ranked.iter().map(|&f| scores[f]).collect();

        out.push(ClusterSummary {
            id,
            files: ranked,
            scores: scores_ranked,
            dirs,
            top_symbols,
            entry_points,
            external_deps,
            depends_on: Vec::new(),
            depended_on_by: Vec::new(),
        });
    }

    // Cluster-to-cluster edges, derived from resolved imports only.
    for i in 0..out.len() {
        let mut deps: Vec<usize> = Vec::new();
        let mut rdeps: Vec<usize> = Vec::new();
        for &f in &out[i].files {
            for &j in &g.out_edges[f] {
                let c = part.membership[j];
                if c != i && !deps.contains(&c) {
                    deps.push(c);
                }
            }
            for &j in &g.in_edges[f] {
                let c = part.membership[j];
                if c != i && !rdeps.contains(&c) {
                    rdeps.push(c);
                }
            }
        }
        deps.sort_unstable();
        rdeps.sort_unstable();
        out[i].depends_on = deps;
        out[i].depended_on_by = rdeps;
    }

    reorder_by_importance(out)
}

/// Emit domains most-important first, and renumber the cross-references so
/// "Depends on" still points at the right domains after the shuffle.
fn reorder_by_importance(mut summaries: Vec<ClusterSummary>) -> Vec<ClusterSummary> {
    let mut order: Vec<usize> = (0..summaries.len()).collect();
    order.sort_by(|&a, &b| {
        summaries[b]
            .importance()
            .partial_cmp(&summaries[a].importance())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| summaries[b].size().cmp(&summaries[a].size()))
            .then_with(|| summaries[a].dirs.cmp(&summaries[b].dirs))
    });

    let mut old_to_new = vec![0usize; summaries.len()];
    for (new, &old) in order.iter().enumerate() {
        old_to_new[old] = new;
    }

    let mut reordered: Vec<ClusterSummary> = Vec::with_capacity(summaries.len());
    for &old in &order {
        reordered.push(std::mem::take(&mut summaries[old]));
    }

    for (new, s) in reordered.iter_mut().enumerate() {
        s.id = new;
        for d in s.depends_on.iter_mut() {
            *d = old_to_new[*d];
        }
        for d in s.depended_on_by.iter_mut() {
            *d = old_to_new[*d];
        }
        s.depends_on.sort_unstable();
        s.depended_on_by.sort_unstable();
    }

    reordered
}

/// Directories that best characterize a cluster.
///
/// Weighted by importance, not by file count. A domain whose centre is
/// `src/context.ts` and `src/hono.ts` should be named for `src`, even when a
/// populous `src/utils` holds more of its files.
fn dominant_dirs(inv: &Inventory, files: &[FileId], scores: &[f64]) -> Vec<String> {
    let mut weight: HashMap<&str, f64> = HashMap::new();
    for &f in files {
        // The floor keeps a directory of unimportant files from vanishing
        // entirely, so ties still fall back to sheer count.
        let w = scores.get(f).copied().unwrap_or(0.0).max(0.0) + 0.01;
        *weight.entry(inv.get(f).dir()).or_insert(0.0) += w;
    }
    let mut v: Vec<(&str, f64)> = weight.into_iter().collect();
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(b.0))
    });
    v.into_iter().take(3).map(|(d, _)| d.to_string()).collect()
}

fn pick_symbols(parsed: &[FileParse], files: &[FileId], scores: &[f64]) -> Vec<String> {
    let mut scored: Vec<(f64, String)> = Vec::new();
    for &f in files {
        let Some(p) = parsed.get(f) else { continue };
        for sym in &p.symbols {
            let weight = sym.kind.salience() as f64 * 2.0
                + if sym.exported { 3.0 } else { 0.0 }
                + scores.get(f).copied().unwrap_or(0.0) * 5.0;
            scored.push((weight, sym.name.clone()));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    let mut out: Vec<String> = Vec::new();
    for (_, name) in scored {
        if !out.contains(&name) {
            out.push(name);
        }
        if out.len() >= MAX_SYMBOLS {
            break;
        }
    }
    out
}

fn derive_name(s: &ClusterSummary, siblings: &[String]) -> String {
    let candidate = s
        .dirs
        .first()
        .map(|d| name_from_dir(d))
        .unwrap_or_else(|| "Root".to_string());

    if !siblings.contains(&candidate) {
        return candidate;
    }

    // Collision: qualify with the nearest ancestor segment that actually adds
    // information. `benchmarks/jsx/src/jsx` must not become "Jsx Jsx".
    if let Some(dir) = s.dirs.first() {
        let segs: Vec<&str> = dir.split('/').filter(|x| !x.is_empty()).collect();
        for seg in segs.iter().rev().skip(1) {
            let prefix = humanize(seg);
            if prefix == candidate || GENERIC_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str())
            {
                continue;
            }
            let qualified = format!("{prefix} {candidate}");
            if !siblings.contains(&qualified) {
                return qualified;
            }
        }
    }

    let mut n = 2;
    loop {
        let numbered = format!("{candidate} {n}");
        if !siblings.contains(&numbered) {
            return numbered;
        }
        n += 1;
    }
}

/// Deepest non-generic segment of a directory path.
fn name_from_dir(dir: &str) -> String {
    let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in segs.iter().rev() {
        if !GENERIC_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str()) {
            return humanize(seg);
        }
    }
    // Every segment was layout-only ("src", "packages/app/src"). There is no
    // domain name to be had, and "Src" would be worse than admitting that —
    // but a cluster centred on the source root really is the core.
    if segs.is_empty() {
        "Root".to_string()
    } else {
        "Core".to_string()
    }
}

/// `password_reset` -> `Password Reset`, `userProfile` -> `User Profile`.
pub fn humanize(seg: &str) -> String {
    let lower = seg.to_ascii_lowercase();
    if let Some((_, full)) = NAME_ALIASES.iter().find(|(k, _)| *k == lower) {
        return (*full).to_string();
    }

    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in seg.chars() {
        if c == '-' || c == '_' || c == '.' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
        .iter()
        .map(|w| {
            let lw = w.to_ascii_lowercase();
            match NAME_ALIASES.iter().find(|(k, _)| *k == lw) {
                Some((_, full)) => (*full).to_string(),
                None => {
                    let mut cs = w.chars();
                    match cs.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                        None => String::new(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A grounded, templated sentence. Every clause is a fact from an earlier stage.
fn derive_summary(s: &ClusterSummary) -> String {
    let mut parts: Vec<String> = Vec::new();

    let noun = if s.size() == 1 { "file" } else { "files" };
    let where_ = match s.dirs.first() {
        Some(d) if !d.is_empty() => format!("{} {noun} under `{}`", s.size(), d),
        _ => format!("{} {noun} at the repository root", s.size()),
    };
    parts.push(where_);

    if !s.top_symbols.is_empty() {
        let shown: Vec<&str> = s.top_symbols.iter().take(3).map(|x| x.as_str()).collect();
        parts.push(format!("key symbols {}", shown.join(", ")));
    }

    if !s.external_deps.is_empty() {
        let shown: Vec<&str> = s.external_deps.iter().take(2).map(|x| x.as_str()).collect();
        parts.push(format!("uses {}", shown.join(", ")));
    }

    format!("{}.", parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_separators_and_camel_case() {
        assert_eq!(humanize("password_reset"), "Password Reset");
        assert_eq!(humanize("userProfile"), "User Profile");
        assert_eq!(humanize("billing"), "Billing");
    }

    #[test]
    fn expands_known_abbreviations() {
        assert_eq!(humanize("auth"), "Authentication");
        assert_eq!(humanize("api"), "API");
        assert_eq!(humanize("db"), "Database");
    }

    #[test]
    fn skips_layout_only_directory_segments() {
        assert_eq!(name_from_dir("src/billing"), "Billing");
        assert_eq!(name_from_dir("src/auth/session"), "Session");
        // Nothing but layout segments: name it for what it is, not "Src".
        assert_eq!(name_from_dir("packages/app/src"), "Core");
        assert_eq!(name_from_dir(""), "Root");
    }

    #[test]
    fn disambiguates_colliding_sibling_names() {
        let s = ClusterSummary {
            dirs: vec!["src/billing/api".into()],
            ..Default::default()
        };
        // "API" is taken, so the parent segment qualifies it.
        let name = derive_name(&s, &["API".to_string()]);
        assert_eq!(name, "Billing API");
    }

    #[test]
    fn does_not_repeat_a_segment_when_qualifying() {
        // `benchmarks/jsx/src/jsx` must not become "Jsx Jsx".
        let s = ClusterSummary {
            dirs: vec!["benchmarks/jsx/src/jsx".into()],
            ..Default::default()
        };
        let name = derive_name(&s, &["Jsx".to_string()]);
        assert_eq!(name, "Benchmarks Jsx");
    }

    #[test]
    fn orders_domains_by_importance_and_remaps_references() {
        // Cluster 0 is trivial, cluster 1 is the important one; 0 depends on 1.
        let summaries = vec![
            ClusterSummary {
                id: 0,
                files: vec![0],
                scores: vec![0.01],
                depends_on: vec![1],
                ..Default::default()
            },
            ClusterSummary {
                id: 1,
                files: vec![1, 2],
                scores: vec![0.9, 0.8],
                depended_on_by: vec![0],
                ..Default::default()
            },
        ];
        let out = reorder_by_importance(summaries);
        // The important cluster now leads the map...
        assert_eq!(out[0].files, vec![1, 2]);
        assert_eq!(out[0].id, 0);
        // ...and the cross-references followed the renumbering.
        assert_eq!(out[0].depended_on_by, vec![1]);
        assert_eq!(out[1].depends_on, vec![0]);
    }

    #[test]
    fn pluralizes_a_single_file_domain() {
        let s = ClusterSummary {
            files: vec![0],
            dirs: vec!["src/x".into()],
            ..Default::default()
        };
        assert!(derive_summary(&s).contains("1 file under"));
    }

    #[test]
    fn summary_only_states_known_facts() {
        let s = ClusterSummary {
            files: vec![0, 1, 2],
            dirs: vec!["src/auth".into()],
            top_symbols: vec!["authenticateUser".into(), "createSession".into()],
            external_deps: vec!["jsonwebtoken".into()],
            ..Default::default()
        };
        let out = derive_summary(&s);
        assert!(out.contains("3 files under `src/auth`"));
        assert!(out.contains("authenticateUser"));
        assert!(out.contains("jsonwebtoken"));
    }
}
